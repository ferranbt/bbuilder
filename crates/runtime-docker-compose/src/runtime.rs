use bollard::Docker;
use bollard::query_parameters::EventsOptionsBuilder;
use futures_util::stream::StreamExt;
use std::collections::HashMap;
use std::collections::HashSet;
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use runtime_trait::Runtime;
use spec::{File, Manifest};

use crate::compose::Volume;
use crate::compose::{
    DependsOnCondition, DockerComposeService, DockerComposeSpec, Port, ServiceVolume,
};

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
        let mut volumes = HashMap::new();
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
                let mut service_volumes = vec![];
                let mut init_services = HashMap::new();
                let mut artifacts_to_process = vec![];
                let mut environment = HashMap::new();

                for vol in &spec.volumes {
                    let mut volume_labels = HashMap::new();
                    volume_labels.insert("bbuilder".to_string(), "true".to_string());

                    let vol_name = format!("{}-{}", pod_name, vol.name.clone());
                    volumes.insert(
                        vol_name.clone(),
                        Some(Volume {
                            labels: volume_labels,
                            ..Default::default()
                        }),
                    );

                    service_volumes.push(ServiceVolume {
                        host: vol_name.clone(),
                        target: vol.path.clone(),
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

                                // Figure out which volume is this artifact refering to
                                let matching_vol = spec
                                    .volumes
                                    .iter()
                                    .find(|vol| target_path.starts_with(&vol.path))
                                    .ok_or_else(|| {
                                        eyre::eyre!(
                                            "No matching volume found for path: {}",
                                            target_path
                                        )
                                    })?;

                                let matching_vol_name =
                                    format!("{}-{}", pod_name, matching_vol.name.clone());

                                // Create init container service
                                let init_service = DockerComposeService {
                                    image: "ghcr.io/ferranbt/bbuilder/fetcher:latest".to_string(),
                                    command: vec![content, target_path],
                                    volumes: vec![ServiceVolume {
                                        host: matching_vol_name.clone(),
                                        target: matching_vol.path.clone(),
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

                                service_volumes.push(ServiceVolume {
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
                    volumes: service_volumes,
                    networks: vec!["test".to_string()],
                    depends_on: init_services,
                };

                let service_name = format!("{}-{}", pod_name, spec_name);
                services.insert(service_name.to_string(), service);
            }
        }

        let mut networks = HashMap::new();
        networks.insert("test".to_string(), None);

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

    fn generate_docker_compose(manifest: Manifest) -> eyre::Result<DockerComposeSpec> {
        let temp_dir = std::env::temp_dir().join("test-runtime");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let runtime = DockerRuntime::new(temp_dir.to_str().unwrap().to_string());

        let docker_compose_spec = runtime.convert_to_docker_compose_spec(manifest)?;
        let _ = std::fs::remove_dir_all(&temp_dir);

        Ok(docker_compose_spec)
    }

    #[tokio::test]
    async fn test_artifact_files_are_mounted_in_volumes() -> eyre::Result<()> {
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

        let docker_compose = generate_docker_compose(manifest)?;
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

        Ok(())
    }

    #[tokio::test]
    async fn test_port_arg_uses_preferred_port() -> eyre::Result<()> {
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

        let docker_compose = generate_docker_compose(manifest)?;
        let service = docker_compose.services.get("pod-service").unwrap();

        assert_eq!(service.ports.len(), 1, "Port not found");
        assert_eq!(
            service.command,
            ["8545"],
            "Command should contain port arg with preferred port"
        );

        Ok(())
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

    #[tokio::test]
    async fn test_volumes_are_created_with_bbuilder_label() -> eyre::Result<()> {
        let mut manifest = Manifest::new("volume-test".to_string());

        let spec = Spec::builder()
            .image("test-image")
            .volume(spec::Volume {
                name: "data".to_string(),
                path: "/data".to_string(),
            })
            .build();

        let pod = Pod::default().with_spec("service", spec);
        manifest.add_spec("pod".to_string(), pod);

        let docker_compose = generate_docker_compose(manifest)?;

        // Verify the volume exists in the docker-compose spec
        assert!(
            docker_compose.volumes.contains_key("pod-data"),
            "Volume 'pod-data' should exist in docker-compose volumes"
        );

        // Verify the volume has the bbuilder=true label
        let volume = docker_compose.volumes.get("pod-data").unwrap();
        assert!(volume.is_some(), "Volume should have configuration");
        let volume_config = volume.as_ref().unwrap();
        assert_eq!(
            volume_config.labels.get("bbuilder"),
            Some(&"true".to_string()),
            "Volume should have bbuilder=true label"
        );

        // Verify the service has the volume mounted
        let service = docker_compose.services.get("pod-service").unwrap();
        let has_volume_mount = service
            .volumes
            .iter()
            .any(|v| v.host == "pod-data" && v.target == "/data");

        assert!(
            has_volume_mount,
            "Service should have volume mounted at /data"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_image_tag_defaults_to_latest() -> eyre::Result<()> {
        let mut manifest = Manifest::new("tag-test".to_string());
        let spec = Spec::builder().image("test-image").build();
        let pod = Pod::default().with_spec("service", spec);
        manifest.add_spec("pod".to_string(), pod);

        let docker_compose = generate_docker_compose(manifest)?;
        let service = docker_compose.services.get("pod-service").unwrap();

        assert_eq!(service.image, "test-image:latest");
        Ok(())
    }

    #[tokio::test]
    async fn test_service_includes_spec_labels_and_bbuilder_label() -> eyre::Result<()> {
        let mut manifest = Manifest::new("label-test".to_string());
        let spec = Spec::builder()
            .image("test-image")
            .label("app", "myapp")
            .label("env", "production")
            .build();
        let pod = Pod::default().with_spec("service", spec);
        manifest.add_spec("pod".to_string(), pod);

        let docker_compose = generate_docker_compose(manifest)?;
        let service = docker_compose.services.get("pod-service").unwrap();

        assert_eq!(service.labels.get("app"), Some(&"myapp".to_string()));
        assert_eq!(service.labels.get("env"), Some(&"production".to_string()));
        assert_eq!(service.labels.get("bbuilder"), Some(&"true".to_string()));
        Ok(())
    }
}
