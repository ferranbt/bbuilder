use bollard::Docker;
use bollard::query_parameters::CreateImageOptions;
use futures_util::future::join_all;
use futures_util::stream::StreamExt;
use spec::{File, Manifest, Platform};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use crate::compose::Volume;
use crate::compose::{
    DependsOn, DependsOnCondition, DockerComposeService, DockerComposeSpec, Port, ServiceVolume,
};
use crate::deployment_watcher::{DeploymentState, DockerEventMessage, listen_docker_events};

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

fn load_reserved_ports(dir_path: &str, reserved_ports: &ReservedPorts) -> eyre::Result<()> {
    // Read existing docker-compose.yaml files and reserve their ports
    if let Ok(entries) = std::fs::read_dir(dir_path) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let compose_file = path.join("docker-compose.yaml");
                if compose_file.exists() {
                    if let Ok(content) = std::fs::read_to_string(&compose_file) {
                        let spec = serde_yaml::from_str::<DockerComposeSpec>(&content)?;
                        // Extract all host ports from the spec
                        for service in spec.services.values() {
                            for port in &service.ports {
                                if let Some(host_port) = port.host {
                                    reserved_ports.reserve_port(host_port);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

pub struct DockerRuntime {
    dir_path: String,
    reserved_ports: ReservedPorts,
    deployments: Arc<Mutex<HashMap<String, Arc<Mutex<DeploymentState>>>>>,
}

impl DockerRuntime {
    pub fn new(dir_path: String) -> Self {
        let reserved_ports = ReservedPorts::new();
        load_reserved_ports(&dir_path, &reserved_ports).unwrap();

        let deployments: Arc<Mutex<HashMap<String, Arc<Mutex<DeploymentState>>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<DockerEventMessage>();

        let filters = HashMap::from([("label", vec!["bbuilder=true"])]);
        tokio::spawn(listen_docker_events(event_tx, filters));

        let deployments_clone = Arc::clone(&deployments);
        tokio::spawn(async move {
            while let Some(event_msg) = event_rx.recv().await {
                if let Ok(deps) = deployments_clone.lock() {
                    for (manifest_name, state) in deps.iter() {
                        if event_msg.container_name.starts_with(manifest_name) {
                            if let Ok(mut w) = state.lock() {
                                w.handle_container_event(
                                    event_msg.container_name.clone(),
                                    event_msg.event,
                                );
                            }
                            break;
                        }
                    }
                }
            }
        });

        Self {
            dir_path,
            reserved_ports,
            deployments,
        }
    }

    async fn pull_images(&self, spec: &DockerComposeSpec) -> eyre::Result<()> {
        let docker = Docker::connect_with_local_defaults()?;

        // Collect all unique images from the spec
        let mut images = HashSet::new();
        for service in spec.services.values() {
            images.insert(service.image.clone());
        }

        // Check which images are not available locally
        let mut images_to_pull = Vec::new();
        for image in images {
            match docker.inspect_image(&image).await {
                Ok(_) => {
                    // Image exists locally
                    tracing::debug!("Image {} already exists locally", image);
                }
                Err(bollard::errors::Error::DockerResponseServerError {
                    status_code: 404, ..
                }) => {
                    tracing::debug!("Image {} not found locally, will pull", image);
                    images_to_pull.push(image);
                }
                Err(_) => {
                    // For other errors (like JSON parse errors), assume image exists
                }
            }
        }

        if images_to_pull.is_empty() {
            tracing::debug!("All images are already available locally");
            return Ok(());
        }

        // Pull all missing images concurrently using join_all
        tracing::info!("Pulling {} images in parallel...", images_to_pull.len());
        let docker_clone = docker.clone();
        let futures: Vec<_> = images_to_pull
            .iter()
            .map(|image| {
                let docker = docker_clone.clone();
                let image = image.clone();
                async move {
                    let options = CreateImageOptions {
                        from_image: Some(image.clone()),
                        ..Default::default()
                    };

                    let mut stream = docker.create_image(Some(options), None, None);

                    // Consume the output to ensure pull completes
                    while let Some(result) = stream.next().await {
                        if let Err(e) = result {
                            return Err(format!("error during image pull {}: {}", image, e));
                        }
                    }

                    Ok(())
                }
            })
            .collect();

        let results = join_all(futures).await;

        // Check if all pulls succeeded
        for (i, result) in results.iter().enumerate() {
            if let Err(e) = result {
                return Err(eyre::eyre!(
                    "Failed to pull image {}: {}",
                    images_to_pull[i],
                    e
                ));
            }
        }

        tracing::info!("Successfully pulled all images");
        Ok(())
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
                let mut init_services = BTreeMap::new();
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
                                    DependsOn {
                                        condition: Some(
                                            DependsOnCondition::ServiceCompletedSuccessfully,
                                        ),
                                    },
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

                let platform = if let Some(platform) = spec.platform {
                    let str = match platform {
                        Platform::LinuxAmd64 => "linux/amd64".to_string(),
                    };
                    Some(str)
                } else {
                    None
                };

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
                    platform,
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

    pub async fn run(&self, manifest: Manifest, dry_run: bool) -> eyre::Result<()> {
        let name = manifest.name.clone();

        // Create the parent folder path
        let parent_folder = std::path::Path::new(&self.dir_path).join(&name);
        std::fs::create_dir_all(&parent_folder)?;

        let docker_compose_spec = self.convert_to_docker_compose_spec(manifest)?;

        // Initialize deployment state
        let deployment_state = Arc::new(Mutex::new(DeploymentState::new(&docker_compose_spec)));

        // Register deployment in the watcher
        if let Ok(mut deployments) = self.deployments.lock() {
            deployments.insert(name.clone(), deployment_state);
        }

        // Write the compose file in the parent folder
        let compose_file_path = parent_folder.join("docker-compose.yaml");
        std::fs::write(
            compose_file_path.clone(),
            serde_yaml::to_string(&docker_compose_spec)?,
        )?;

        // Run docker-compose up in detached mode
        if !dry_run {
            // Pull images before running docker-compose
            self.pull_images(&docker_compose_spec).await?;

            Command::new("docker-compose")
                .arg("-f")
                .arg(&compose_file_path)
                .arg("up")
                .arg("-d")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?;
        }

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

    #[test]
    fn test_load_reserved_ports() -> eyre::Result<()> {
        let fixtures_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("test-load-reserved-ports");

        let reserved_ports = ReservedPorts::new();
        load_reserved_ports(fixtures_dir.to_str().unwrap(), &reserved_ports)?;

        // Verify that ports from the fixture files are reserved
        // service1 has ports 8080 and 9000
        // service2 has port 3000
        let port1 = reserved_ports.reserve_port(8080);
        assert_eq!(port1, Some(8081), "Port 8080 should already be reserved");

        let port2 = reserved_ports.reserve_port(9000);
        assert_eq!(port2, Some(9001), "Port 9000 should already be reserved");

        let port3 = reserved_ports.reserve_port(3000);
        assert_eq!(port3, Some(3001), "Port 3000 should already be reserved");

        Ok(())
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

    #[tokio::test]
    async fn test_pull_multiple_images() -> eyre::Result<()> {
        use bollard::query_parameters::RemoveImageOptions;

        let docker = Docker::connect_with_local_defaults()?;
        let images = vec!["alpine:3.18", "alpine:3.19", "alpine:3.20"];

        // Ensure images don't exist
        for image in &images {
            let _ = docker
                .remove_image(
                    image,
                    Some(RemoveImageOptions {
                        force: true,
                        ..Default::default()
                    }),
                    None,
                )
                .await;
        }

        // Create a docker compose spec with multiple services using different images
        let mut services = HashMap::new();
        for (i, image) in images.iter().enumerate() {
            let service = DockerComposeService {
                image: image.to_string(),
                ..Default::default()
            };
            services.insert(format!("service-{}", i), service);
        }

        let docker_compose_spec = DockerComposeSpec {
            services,
            networks: HashMap::new(),
            volumes: HashMap::new(),
        };

        // Test pull_images
        let temp_dir = std::env::temp_dir().join("test-runtime-pull");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir)?;
        let runtime = DockerRuntime::new(temp_dir.to_str().unwrap().to_string());

        let result = runtime.pull_images(&docker_compose_spec).await;
        assert!(result.is_ok(), "Failed to pull images: {:?}", result);

        // Verify all images were pulled
        for image in &images {
            let inspect_result = docker.inspect_image(image).await;
            assert!(
                inspect_result.is_ok(),
                "Image {} should exist after pull",
                image
            );
        }

        let _ = std::fs::remove_dir_all(&temp_dir);
        Ok(())
    }
}
