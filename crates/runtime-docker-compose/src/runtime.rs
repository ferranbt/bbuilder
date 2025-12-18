use bollard::Docker;
use bollard::query_parameters::EventsOptionsBuilder;
use futures_util::stream::StreamExt;
use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use std::collections::HashMap;

use runtime_trait::Runtime;
use spec::{File, Manifest};

#[derive(Serialize)]
struct DockerComposeSpec {
    services: HashMap<String, DockerComposeService>,

    #[serde(skip_serializing_if = "HashMap::is_empty")]
    networks: HashMap<String, Option<Network>>,
}

#[derive(Serialize, Default)]
struct DockerComposeService {
    image: String,

    command: Vec<String>,

    #[serde(skip_serializing_if = "Vec::is_empty")]
    entrypoint: Vec<String>,

    #[serde(skip_serializing_if = "HashMap::is_empty")]
    labels: HashMap<String, String>,

    #[serde(skip_serializing_if = "HashMap::is_empty")]
    environment: HashMap<String, String>,

    #[serde(skip_serializing_if = "Vec::is_empty")]
    ports: Vec<Port>,

    #[serde(skip_serializing_if = "Vec::is_empty")]
    networks: Vec<String>,

    #[serde(skip_serializing_if = "Vec::is_empty")]
    volumes: Vec<String>,

    #[serde(skip_serializing_if = "HashMap::is_empty")]
    #[serde(serialize_with = "serialize_depends_on")]
    depends_on: HashMap<String, Option<DependsOnCondition>>,
}

#[derive(Serialize, Default)]
struct Network {}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum DependsOnCondition {
    ServiceCompletedSuccessfully,
}

fn serialize_depends_on<S>(
    map: &HashMap<String, Option<DependsOnCondition>>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::Serialize;

    let mut map_serializer = serializer.serialize_map(Some(map.len()))?;
    for (key, value) in map {
        match value {
            None => {
                // Serialize as empty map for simple dependency
                map_serializer.serialize_entry(key, &())?;
            }
            Some(condition) => {
                #[derive(Serialize)]
                struct WithCondition<'a> {
                    condition: &'a DependsOnCondition,
                }
                map_serializer.serialize_entry(key, &WithCondition { condition })?;
            }
        }
    }
    map_serializer.end()
}

struct Port {
    host: u16,
    container: u16,
}

impl Serialize for Port {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Docker Compose ports format: "host:container" or extended format
        let port_mapping = format!("{}:{}", self.host, self.container);

        // For simple format, just serialize as string
        port_mapping.serialize(serializer)
    }
}

pub struct DockerRuntime {
    dir_path: String,
}

impl DockerRuntime {
    pub fn new(dir_path: String) -> Self {
        tokio::spawn(async move {
            let docker = Docker::connect_with_local_defaults().unwrap();

            // Filter for container events only
            let filters = HashMap::from([
                ("type", vec!["container"]),
                ("label", vec!["bbuilder=true"]),
            ]);
            let options = EventsOptionsBuilder::new().filters(&filters).build();

            let mut events = docker.events(Some(options));
            println!("Listening for container events...");

            while let Some(event_result) = events.next().await {
                match event_result {
                    Ok(event) => {
                        println!("Event: {:?}", event.action);
                        if let Some(actor) = event.actor {
                            println!("  Container ID: {:?}", actor.id);
                            if let Some(attrs) = actor.attributes {
                                if let Some(name) = attrs.get("name") {
                                    println!("  Container Name: {}", name);
                                }
                            }
                        }
                        println!();
                    }
                    Err(e) => eprintln!("Error: {}", e),
                }
            }
        });

        Self { dir_path }
    }

    fn convert_to_docker_compose_spec(
        &self,
        manifest: Manifest,
    ) -> eyre::Result<DockerComposeSpec> {
        let mut services = HashMap::new();
        let compose_dir = std::path::Path::new(&self.dir_path).join(&manifest.name);

        for (pod_name, pod) in manifest.pods {
            for (spec_name, spec) in pod.specs {
                let image = format!(
                    "{}:{}",
                    spec.image,
                    spec.tag.unwrap_or("latest".to_string())
                );

                let mut ports = vec![];
                let mut command = vec![];
                let mut volumes = vec![];
                let mut init_services = HashMap::new();
                let mut artifacts_to_process = vec![];
                let mut environment = HashMap::new();

                // Track volume mounts by target directory to reuse volumes
                // let mut volume_mounts: HashMap<String, String> = HashMap::new();
                let data_path = compose_dir.join("data");
                std::fs::create_dir_all(&data_path)?;
                let absolute_data_path = data_path.canonicalize()?;

                {
                    let volume_mapping = format!("{}:{}", absolute_data_path.display(), "/data");
                    volumes.push(volume_mapping);
                }

                for (key, value) in spec.env {
                    environment.insert(key, value);
                }

                for arg in spec.args {
                    let cleaned_arg = match arg {
                        spec::Arg::Value(value) => Some(value),
                        spec::Arg::Dir { path, .. } => Some(path),
                        spec::Arg::Port { name, preferred } => {
                            ports.push(Port {
                                host: preferred,
                                container: preferred,
                            });
                            Some(format!("--{}={}", name, preferred))
                        }
                        spec::Arg::File(file) => {
                            artifacts_to_process.push(spec::Artifacts::File(file));
                            None
                        }
                        spec::Arg::Ref { .. } => format!("").into(),
                    };
                    if let Some(cleaned_arg) = cleaned_arg {
                        command.push(cleaned_arg);
                    }
                }

                // Add artifacts from spec.artifacts
                artifacts_to_process.extend(spec.artifacts);

                let config_path = compose_dir.join("_config");
                std::fs::create_dir_all(&config_path)?;
                let absolute_config_path = config_path.canonicalize()?;

                // Process all artifacts after args have been hydrated
                for artifact in artifacts_to_process {
                    match artifact {
                        spec::Artifacts::File(File {
                            name,
                            target_path,
                            content,
                        }) => {
                            // Check if the file is a URL
                            if content.starts_with("https://") {
                                // For URLs, create an init container to download the file
                                let init_service_name =
                                    format!("{}-{}-init-{}", pod_name, spec_name, name);

                                // Resolve the target path within the mounted volume
                                // target_path might be something like "/data/heimdall/genesis.json"
                                // We need to strip the volume mount prefix and get the path relative to /data
                                let container_target = std::path::Path::new(&target_path);
                                let volume_mount = std::path::Path::new("/data");

                                let relative_target = container_target
                                    .strip_prefix(volume_mount)
                                    .unwrap_or(container_target);

                                // The path inside the container after mounting absolute_data_path to /data
                                let download_path = format!("/data/{}", relative_target.display());

                                // Create init container service
                                let init_service = DockerComposeService {
                                    image: "curlimages/curl:latest".to_string(),
                                    command: vec![
                                        "sh".to_string(),
                                        "-c".to_string(),
                                        format!(
                                            "mkdir -p $(dirname {}) && curl -L -o {} {}",
                                            download_path, download_path, content
                                        ),
                                    ],

                                    volumes: vec![format!(
                                        "{}:{}",
                                        absolute_data_path.display(),
                                        "/data"
                                    )],
                                    ..Default::default()
                                };

                                services.insert(init_service_name.clone(), init_service);
                                init_services.insert(
                                    init_service_name,
                                    Some(DependsOnCondition::ServiceCompletedSuccessfully),
                                );
                            } else {
                                let target_host_path = absolute_config_path.join(name);
                                if let Some(parent) = target_host_path.parent() {
                                    std::fs::create_dir_all(parent)?;
                                }
                                std::fs::write(&target_host_path, content)?;

                                volumes.push(format!(
                                    "{}:{}",
                                    target_host_path.display(),
                                    target_path
                                ));
                            }
                        }
                    }
                }

                let mut labels = spec.labels;
                labels.insert("bbuilder".to_string(), "true".to_string());

                let service = DockerComposeService {
                    command,
                    entrypoint: spec.entrypoint,
                    environment,
                    image,
                    labels,
                    ports,
                    volumes,
                    networks: vec!["test".to_string()],
                    depends_on: init_services,
                };

                let service_name = format!("{}-{}", pod_name, spec_name);
                services.insert(service_name.to_string(), service);
            }
        }

        let mut networks = HashMap::new();
        networks.insert("test".to_string(), None);

        Ok(DockerComposeSpec { services, networks })
    }
}

#[async_trait::async_trait]
impl Runtime for DockerRuntime {
    async fn run(&self, manifest: Manifest) -> eyre::Result<()> {
        let name = manifest.name.clone();

        // Create the parent folder path
        let parent_folder = std::path::Path::new(&self.dir_path).join(&name);
        std::fs::create_dir_all(&parent_folder)?;

        let docker_compose_spec = self.convert_to_docker_compose_spec(manifest)?;

        // Write the compose file in the parent folder
        let compose_file_path = parent_folder.join("docker_compose.yaml");
        std::fs::write(
            compose_file_path.clone(),
            serde_yaml::to_string(&docker_compose_spec)?,
        )?;

        /*
        // Run docker-compose up in detached mode
        Command::new("docker-compose")
            .arg("-f")
            .arg(&compose_file_path)
            .arg("up")
            .arg("-d")
            .status()?;
        */

        Ok(())
    }
}
