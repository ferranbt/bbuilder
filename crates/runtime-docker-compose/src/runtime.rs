use bollard::Docker;
use bollard::query_parameters::EventsOptionsBuilder;
use futures_util::stream::StreamExt;
use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use std::collections::HashMap;
use std::collections::HashSet;
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use runtime_trait::Runtime;
use spec::{File, Manifest};

#[derive(Serialize)]
struct DockerComposeSpec {
    services: HashMap<String, DockerComposeService>,

    #[serde(skip_serializing_if = "HashMap::is_empty")]
    networks: HashMap<String, Option<Network>>,

    #[serde(skip_serializing_if = "HashMap::is_empty")]
    volumes: HashMap<String, Option<Volume>>,
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
    #[serde(serialize_with = "serialize_service_volume")]
    volumes: Vec<ServiceVolume>,

    #[serde(skip_serializing_if = "HashMap::is_empty")]
    #[serde(serialize_with = "serialize_depends_on")]
    depends_on: HashMap<String, Option<DependsOnCondition>>,
}

#[derive(Default)]
struct ServiceVolume {
    pub host: String,
    pub target: String,
}

#[derive(Serialize, Default)]
struct Volume {
    #[serde(skip_serializing_if = "Option::is_none")]
    driver: Option<String>,

    #[serde(skip_serializing_if = "HashMap::is_empty")]
    driver_opts: HashMap<String, String>,

    #[serde(skip_serializing_if = "HashMap::is_empty")]
    labels: HashMap<String, String>,
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

fn serialize_service_volume<S>(
    volumes: &Vec<ServiceVolume>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    use serde::ser::SerializeSeq;

    let mut seq = serializer.serialize_seq(Some(volumes.len()))?;
    for volume in volumes {
        let volume_str = format!("{}:{}", volume.host, volume.target);
        seq.serialize_element(&volume_str)?;
    }
    seq.end()
}

#[derive(Debug)]
struct Port {
    host: Option<u16>,
    container: u16,
}

impl Serialize for Port {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let port_mapping = if let Some(host) = self.host {
            // Docker Compose ports format: "host:container" or extended format
            format!("{}:{}", host, self.container)
        } else {
            format!("{}", self.container)
        };

        // For simple format, just serialize as string
        port_mapping.serialize(serializer)
    }
}

#[derive(Clone)]
struct ReservedPorts {
    reserved: Arc<Mutex<HashSet<u16>>>,
}

impl ReservedPorts {
    fn new() -> Self {
        Self {
            reserved: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    fn reserve_port(&self, preferred: u16) -> Option<u16> {
        let mut reserved = self.reserved.lock().unwrap();

        let mut port = preferred;
        loop {
            // Check if port is already reserved
            if reserved.contains(&port) {
                port += 1;
                if port == 0 {
                    // Wrapped around
                    return None;
                }
                continue;
            }

            // Try to bind to the port to check availability
            if TcpListener::bind(format!("127.0.0.1:{}", port)).is_ok() {
                reserved.insert(port);
                return Some(port);
            }

            port += 1;
            if port == 0 {
                // Wrapped around
                return None;
            }
        }
    }
}

pub struct DockerRuntime {
    dir_path: String,
    reserved_ports: ReservedPorts,
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

        Self {
            dir_path,
            reserved_ports: ReservedPorts::new(),
        }
    }

    fn convert_to_docker_compose_spec(
        &self,
        manifest: Manifest,
    ) -> eyre::Result<DockerComposeSpec> {
        let mut services = HashMap::new();
        let compose_dir = std::path::Path::new(&self.dir_path).join(&manifest.name);

        for (pod_name, pod) in &manifest.pods {
            for (spec_name, spec) in &pod.specs {
                let image = format!(
                    "{}:{}",
                    spec.image,
                    spec.tag.clone().unwrap_or("latest".to_string())
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
                    volumes.push(ServiceVolume {
                        host: absolute_data_path.display().to_string(),
                        target: "/data".to_string(),
                    });
                }

                for (key, value) in &spec.env {
                    environment.insert(key.clone(), value.clone());
                }

                for port in &spec.ports {
                    let host_port = self.reserved_ports.reserve_port(port.port);
                    ports.push(Port {
                        host: host_port,
                        container: port.port,
                    });
                }

                for arg in &spec.args {
                    let cleaned_arg = match arg {
                        spec::Arg::Value(value) => Some(value.clone()),
                        spec::Arg::Dir { path, .. } => Some(path.clone()),
                        spec::Arg::Port { preferred, .. } => Some(format!("{}", preferred)),
                        spec::Arg::File(file) => {
                            artifacts_to_process.push(spec::Artifacts::File(file.clone()));
                            None
                        }
                        spec::Arg::Ref { name, port } => {
                            let reference = manifest.resolve_ref(name.clone(), port.clone())?;
                            Some(reference)
                        }
                    };
                    if let Some(cleaned_arg) = cleaned_arg {
                        command.push(cleaned_arg);
                    }
                }

                // Add artifacts from spec.artifacts
                artifacts_to_process.extend(spec.artifacts.clone());

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

                                // Create init container service
                                let init_service = DockerComposeService {
                                    image: "ghcr.io/ferranbt/bbuilder/fetcher:latest".to_string(),
                                    command: vec![content, target_path],
                                    volumes: vec![ServiceVolume {
                                        host: absolute_data_path.display().to_string(),
                                        target: "/data".to_string(),
                                    }],
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

                                volumes.push(ServiceVolume {
                                    host: target_host_path.display().to_string(),
                                    target: target_path,
                                });
                            }
                        }
                    }
                }

                let mut labels = spec.labels.clone();
                labels.insert("bbuilder".to_string(), "true".to_string());

                if let Some(metrics_port) = spec.ports.iter().find(|port| port.name == "metrics") {
                    labels.insert("metrics".to_string(), format!("{}", metrics_port.port));
                }

                let service = DockerComposeService {
                    command,
                    entrypoint: spec.entrypoint.clone(),
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

        let volumes = HashMap::new();

        Ok(DockerComposeSpec {
            services,
            networks,
            volumes,
        })
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
        let compose_file_path = parent_folder.join("docker-compose.yaml");
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

#[cfg(test)]
mod tests {
    use super::*;
    use spec::{Artifacts, File, Manifest, Pod, Spec};

    #[tokio::test]
    async fn test_artifact_files_are_mounted_in_volumes() -> eyre::Result<()> {
        let temp_dir = std::env::temp_dir().join("test-runtime");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let runtime = DockerRuntime::new(temp_dir.to_str().unwrap().to_string());

        let mut manifest = Manifest::new("test-manifest".to_string());

        let file_artifact = File {
            name: "config.json".to_string(),
            target_path: "/app/config.json".to_string(),
            content: r#"{"key": "value"}"#.to_string(),
        };

        let spec = Spec::builder()
            .image("test-image")
            .artifact(Artifacts::File(file_artifact))
            .build();

        let pod = Pod::default().with_spec("test-service", spec);
        manifest.add_spec("test-pod".to_string(), pod);

        let docker_compose = runtime.convert_to_docker_compose_spec(manifest)?;
        let service = docker_compose
            .services
            .get("test-pod-test-service")
            .unwrap();

        let has_config_volume = service
            .volumes
            .iter()
            .any(|v| v.host.contains("config.json") && v.target.contains("/app/config.json"));

        assert!(
            has_config_volume,
            "Artifact file should be mounted in volumes"
        );

        let _ = std::fs::remove_dir_all(&temp_dir);

        Ok(())
    }

    #[tokio::test]
    async fn test_port_arg_uses_preferred_port() {
        let temp_dir = std::env::temp_dir().join("test-runtime-port");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let runtime = DockerRuntime::new(temp_dir.to_str().unwrap().to_string());

        let mut manifest = Manifest::new("port-test".to_string());

        let spec = Spec::builder()
            .image("test-image")
            .arg(spec::Arg::Port {
                name: "rpc-port".to_string(),
                preferred: 8545,
            })
            .build();

        let pod = Pod::default().with_spec("service", spec);
        manifest.add_spec("pod".to_string(), pod);

        let docker_compose = runtime.convert_to_docker_compose_spec(manifest).unwrap();
        let service = docker_compose.services.get("pod-service").unwrap();

        assert_eq!(service.ports.len(), 1, "Port not found");
        assert_eq!(
            service.command,
            ["8545"],
            "Command should contain port arg with preferred port"
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_reserved_ports() {
        let reserved_ports = ReservedPorts::new();

        // Reserve a port
        let port1 = reserved_ports.reserve_port(8545);
        assert_eq!(port1, Some(8545));

        // Try to reserve the same port again, should get 8546
        let port2 = reserved_ports.reserve_port(8545);
        assert_eq!(port2, Some(8546));

        // Reserve another port
        let port3 = reserved_ports.reserve_port(8545);
        assert_eq!(port3, Some(8547));
    }

    #[test]
    fn test_docker_compose_spec_with_volumes() {
        let mut spec = DockerComposeSpec {
            services: HashMap::new(),
            networks: HashMap::new(),
            volumes: HashMap::new(),
        };

        let service = DockerComposeService {
            image: "test-image:latest".to_string(),
            command: vec![],
            volumes: vec![ServiceVolume {
                host: "/host/path".to_string(),
                target: "/container/path".to_string(),
            }],
            ..Default::default()
        };
        spec.services.insert("test-service".to_string(), service);
        spec.networks.insert("test".to_string(), None);

        let mut driver_opts = HashMap::new();
        driver_opts.insert("type".to_string(), "nfs".to_string());

        let mut labels = HashMap::new();
        labels.insert("description".to_string(), "My data volume".to_string());

        let volume = Volume {
            driver: Some("local".to_string()),
            driver_opts,
            labels,
        };
        spec.volumes.insert("mydata".to_string(), Some(volume));

        let yaml = serde_yaml::to_string(&spec).unwrap();
        let expected = include_str!("../fixtures/docker-compose-with-volumes.yaml");
        assert_eq!(yaml.trim(), expected.trim());
    }
}
