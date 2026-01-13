use bollard::Docker;
use bollard::query_parameters::EventsOptionsBuilder;
use futures_util::stream::StreamExt;
use spec::Manifest;
use std::collections::HashMap;
use std::collections::HashMap as StdHashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::mpsc;

use crate::compose::DockerComposeSpec;

/// Global manager for multiple deployments - routes events to the correct deployment
pub struct DeploymentsWatcher {
    deployments: HashMap<String, Arc<Mutex<DeploymentWatcher>>>,
}

/// Single deployment watcher
pub struct DeploymentWatcher {
    manifest_name: String,
    pub services: Vec<ServiceStatus>,
}

pub struct ServiceStatus {
    pub name: String,
    pub container_ip: Option<String>,
    pub state: ContainerState,
    pub health: Option<HealthStatus>,
    pub artifacts: Vec<ArtifactStatus>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ContainerState {
    Pending,
    PullingImage,
    Running,
    Healthy,
    Unhealthy,
    Completed,
    Failed(String),
}

pub struct HealthStatus {
    pub is_ready: bool,
    pub is_healthy: bool,
    pub last_updated: Instant,
}

pub struct ArtifactStatus {
    pub name: String,
    pub target_path: String,
    pub state: ArtifactState,
    pub download_progress: Option<DownloadProgress>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ArtifactState {
    Pending,
    Downloading,
    Ready,
    Failed(String),
}

pub struct DownloadProgress {
    pub bytes_downloaded: u64,
    pub total_bytes: Option<u64>,
    pub download_speed: Option<u64>,
    pub last_updated: Instant,
}

#[derive(Debug, Clone)]
pub enum ContainerEvent {
    PullingImage,
    Started { container_ip: String },
    Died { exit_code: i64 },
}

#[derive(Debug)]
pub struct DockerEventMessage {
    pub container_name: String,
    pub event: ContainerEvent,
}

pub enum ArtifactEvent {
    DownloadStarted,
    DownloadProgress {
        bytes_downloaded: u64,
        total_bytes: Option<u64>,
        speed: Option<u64>,
    },
    DownloadCompleted,
    DownloadFailed {
        error: String,
    },
}

impl DeploymentWatcher {
    pub fn new(manifest: &Manifest, spec: &DockerComposeSpec) -> Self {
        let mut services = Vec::new();

        // Create a ServiceStatus for each service in the docker-compose spec
        for (service_name, docker_service) in &spec.services {
            let mut artifacts = Vec::new();

            // If this is an init container (contains "-init-"), extract artifact info
            if service_name.contains("-init-") {
                // Extract artifact name from service name
                // Format: "pod-spec-init-artifact-name"
                if let Some(artifact_name) = service_name.split("-init-").nth(1) {
                    // For init containers, the command typically contains [url, target_path]
                    if docker_service.command.len() >= 2 {
                        artifacts.push(ArtifactStatus {
                            name: artifact_name.to_string(),
                            target_path: docker_service.command[1].clone(),
                            state: ArtifactState::Pending,
                            download_progress: None,
                        });
                    }
                }
            }

            services.push(ServiceStatus {
                name: service_name.clone(),
                container_ip: None,
                state: ContainerState::Pending,
                health: None,
                artifacts,
            });
        }

        Self {
            manifest_name: manifest.name.clone(),
            services,
        }
    }

    pub fn handle_container_event(&mut self, container_name: String, event: ContainerEvent) {
        // Find the service by container name
        let service = self.services.iter_mut().find(|s| s.name == container_name);

        if let Some(service) = service {
            match event {
                ContainerEvent::PullingImage => {
                    service.state = ContainerState::PullingImage;
                }
                ContainerEvent::Started { container_ip } => {
                    service.state = ContainerState::Running;
                    service.container_ip = Some(container_ip.clone());

                    // If this is an init container (fetcher), spawn a task to track download progress
                    if container_name.contains("-init-") && !service.artifacts.is_empty() {
                        let service_name = container_name.clone();

                        // Spawn task to poll fetcher-api
                        tokio::spawn(async move {
                            // TODO: Implement fetcher-api polling when fetcher-api crate is available
                            // This will poll http://{container_ip}:{port}/status or similar
                            // and emit ArtifactEvent updates
                            tracing::debug!(
                                "Spawned fetcher tracker for {} at {}",
                                service_name,
                                container_ip
                            );
                        });
                    }
                }
                ContainerEvent::Died { exit_code } => {
                    service.state =
                        ContainerState::Failed(format!("Died with exit code: {}", exit_code));
                    // Mark artifacts as failed
                    for artifact in &mut service.artifacts {
                        if artifact.state != ArtifactState::Ready {
                            artifact.state = ArtifactState::Failed("Container died".to_string());
                        }
                    }
                }
            }
        }
    }

    pub fn handle_artifact_event(
        &mut self,
        service_name: String,
        artifact_name: String,
        event: ArtifactEvent,
    ) {
        // Find the service
        let service = self.services.iter_mut().find(|s| s.name == service_name);

        if let Some(service) = service {
            // Find the artifact within the service
            let artifact = service
                .artifacts
                .iter_mut()
                .find(|a| a.name == artifact_name);

            if let Some(artifact) = artifact {
                match event {
                    ArtifactEvent::DownloadStarted => {
                        artifact.state = ArtifactState::Downloading;
                    }
                    ArtifactEvent::DownloadProgress {
                        bytes_downloaded,
                        total_bytes,
                        speed,
                    } => {
                        artifact.state = ArtifactState::Downloading;
                        artifact.download_progress = Some(DownloadProgress {
                            bytes_downloaded,
                            total_bytes,
                            download_speed: speed,
                            last_updated: Instant::now(),
                        });
                    }
                    ArtifactEvent::DownloadCompleted => {
                        artifact.state = ArtifactState::Ready;
                        artifact.download_progress = None;
                    }
                    ArtifactEvent::DownloadFailed { error } => {
                        artifact.state = ArtifactState::Failed(error);
                        artifact.download_progress = None;
                    }
                }
            }
        }
    }

    pub async fn update_health(&mut self, service_name: String, health: HealthStatus) {
        // Find the service
        let service = self.services.iter_mut().find(|s| s.name == service_name);

        if let Some(service) = service {
            // Update container state based on health status
            if service.state == ContainerState::Running {
                if health.is_healthy {
                    service.state = ContainerState::Healthy;
                } else {
                    service.state = ContainerState::Unhealthy;
                }
            }

            // Update the health info
            service.health = Some(health);
        }
    }
}

impl DeploymentsWatcher {
    pub fn new() -> Self {
        Self {
            deployments: HashMap::new(),
        }
    }

    /// Start listening to Docker events and return a receiver for events
    pub fn start_event_listener() -> mpsc::UnboundedReceiver<DockerEventMessage> {
        let (tx, rx) = mpsc::unbounded_channel();

        //tokio::spawn(listen_docker_events(tx));

        rx
    }

    /// Process events from the receiver
    pub async fn process_events(
        watcher: Arc<Mutex<Self>>,
        mut event_rx: mpsc::UnboundedReceiver<DockerEventMessage>,
    ) {
        while let Some(msg) = event_rx.recv().await {
            if let Ok(mut w) = watcher.lock() {
                w.handle_container_event(msg.container_name, msg.event);
            }
        }
    }

    /// Register a deployment watcher
    pub fn register(&mut self, manifest_name: String, watcher: Arc<Mutex<DeploymentWatcher>>) {
        self.deployments.insert(manifest_name, watcher);
    }

    /// Route container event to the appropriate deployment
    /// Container name format: "manifest-name-pod-spec" or "manifest-name-pod-spec-init-artifact"
    pub fn handle_container_event(&mut self, container_name: String, event: ContainerEvent) {
        // Extract manifest name from container name (first segment before first hyphen)
        // This assumes container names follow the pattern: manifest-pod-spec...
        for (manifest_name, watcher) in &self.deployments {
            if container_name.starts_with(manifest_name) {
                if let Ok(mut w) = watcher.lock() {
                    w.handle_container_event(container_name, event);
                }
                return;
            }
        }
    }

    /// Route artifact event to the appropriate deployment
    pub fn handle_artifact_event(
        &mut self,
        service_name: String,
        artifact_name: String,
        event: ArtifactEvent,
    ) {
        for (manifest_name, watcher) in &self.deployments {
            if service_name.starts_with(manifest_name) {
                if let Ok(mut w) = watcher.lock() {
                    w.handle_artifact_event(service_name, artifact_name, event);
                }
                return;
            }
        }
    }

    /// Route health update to the appropriate deployment
    pub async fn update_health(&mut self, service_name: String, health: HealthStatus) {
        for (manifest_name, watcher) in &self.deployments {
            if service_name.starts_with(manifest_name) {
                if let Ok(mut w) = watcher.lock() {
                    w.update_health(service_name, health).await;
                }
                return;
            }
        }
    }
}

/// Listen to Docker events and send them through the channel
async fn listen_docker_events(
    tx: mpsc::UnboundedSender<DockerEventMessage>,
    filters: HashMap<&str, Vec<&str>>,
) {
    let docker = Docker::connect_with_local_defaults().unwrap();

    // Filter for container events only
    let options = EventsOptionsBuilder::new().filters(&filters).build();

    let mut events = docker.events(Some(options));
    tracing::debug!("Listening for container events...");

    while let Some(event_result) = events.next().await {
        match event_result {
            Ok(event) => {
                tracing::debug!("Event: {:?}", event.action);
                if let Some(actor) = event.actor {
                    let container_id = actor.id.clone();
                    tracing::debug!("  Container ID: {:?}", container_id);
                    if let Some(attrs) = actor.attributes {
                        if let Some(name) = attrs.get("name") {
                            tracing::debug!("  Container Name: {}", name);

                            // Map Docker event to ContainerEvent
                            let container_event = match event.action.as_deref() {
                                Some("start") => {
                                    // Extract container IP by inspecting the container
                                    let container_ip = if let Some(id) = container_id {
                                        extract_container_ip(&docker, &id).await
                                    } else {
                                        String::new()
                                    };

                                    Some(ContainerEvent::Started { container_ip })
                                }
                                Some("die") => {
                                    let exit_code = attrs
                                        .get("exitCode")
                                        .and_then(|s| s.parse::<i64>().ok())
                                        .unwrap_or(1);
                                    Some(ContainerEvent::Died { exit_code })
                                }
                                _ => None,
                            };

                            if let Some(event) = container_event {
                                let _ = tx.send(DockerEventMessage {
                                    container_name: name.clone(),
                                    event,
                                });
                            }
                        }
                    }
                }
            }
            Err(e) => tracing::error!("Error: {}", e),
        }
    }
}

/// Extract the container IP address from Docker inspect
async fn extract_container_ip(docker: &Docker, container_id: &str) -> String {
    match docker
        .inspect_container(
            container_id,
            None::<bollard::query_parameters::InspectContainerOptions>,
        )
        .await
    {
        Ok(info) => {
            if let Some(network_settings) = info.network_settings {
                if let Some(networks) = network_settings.networks {
                    // Try to get IP from the first available network
                    for (_, endpoint) in networks {
                        if let Some(ip) = endpoint.ip_address {
                            if !ip.is_empty() {
                                return ip;
                            }
                        }
                    }
                }
            }
            tracing::warn!("Could not extract IP for container {}", container_id);
            String::new()
        }
        Err(e) => {
            tracing::error!("Failed to inspect container {}: {}", container_id, e);
            String::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use bollard::models::ContainerCreateBody;
    use bollard::query_parameters::{
        CreateContainerOptions, RemoveContainerOptions, StartContainerOptions,
    };
    use futures_util::future::BoxFuture;

    async fn cleanup_container(docker: &Docker, name: &str) {
        let _ = docker
            .remove_container(
                name,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await;
    }

    async fn run_test_container(
        container_name: &str,
        args: Vec<&str>,
    ) -> impl FnOnce() -> BoxFuture<'static, ()> {
        let docker = Docker::connect_with_local_defaults().unwrap();

        let image = "alpine:latest".to_string();
        cleanup_container(&docker, container_name).await;

        let mut labels = StdHashMap::new();
        labels.insert("bbuilder".to_string(), "true".to_string());

        let config = ContainerCreateBody {
            image: Some(image),
            cmd: Some(args.into_iter().map(String::from).collect()),
            labels: Some(labels),
            ..Default::default()
        };

        let create_options = CreateContainerOptions {
            name: Some(container_name.to_string()),
            ..Default::default()
        };

        docker
            .create_container(Some(create_options), config)
            .await
            .unwrap();

        // Start the container
        docker
            .start_container(container_name, None::<StartContainerOptions>)
            .await
            .unwrap();

        let docker = docker.clone();
        let container_name = container_name.to_string();

        move || {
            Box::pin(async move {
                cleanup_container(&docker, &container_name).await;
            })
        }
    }

    #[tokio::test]
    async fn test_container_start_and_dies_naturally() {
        let container_name = "test-container-start-and-dies";
        let filters = StdHashMap::from([("container", vec![container_name])]);

        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<DockerEventMessage>();
        tokio::spawn(listen_docker_events(event_tx, filters));

        let args = vec!["sh", "-c", "sleep 1"];
        let cleanup = run_test_container(container_name, args).await;

        // wait enough time to finish
        tokio::time::sleep(Duration::from_secs(2)).await;

        let start_event = event_rx.recv().await.expect("start event");
        assert!(matches!(start_event.event, ContainerEvent::Started { .. }));

        let die_event = event_rx.recv().await.expect("start event");
        assert!(matches!(
            die_event.event,
            ContainerEvent::Died { exit_code: 0 }
        ));

        cleanup().await;
    }

    #[tokio::test]
    async fn test_container_forcefully_stops() {
        let container_name = "test-container-forcefully-stops";
        let filters = StdHashMap::from([("container", vec![container_name])]);

        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<DockerEventMessage>();
        tokio::spawn(listen_docker_events(event_tx, filters));

        let args = vec!["sh", "-c", "sleep 1"];
        let cleanup = run_test_container(container_name, args).await;

        cleanup().await;

        let start_event = event_rx.recv().await.expect("start event");
        assert!(matches!(start_event.event, ContainerEvent::Started { .. }));

        let die_event = event_rx.recv().await.expect("start event");
        assert!(matches!(
            die_event.event,
            ContainerEvent::Died { exit_code: 137 }
        ));
    }

    #[tokio::test]
    async fn test_container_error_args() {
        let container_name = "test-container-error-args";
        let filters = StdHashMap::from([("container", vec![container_name])]);

        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<DockerEventMessage>();
        tokio::spawn(listen_docker_events(event_tx, filters));

        let args = vec!["sh", "-c", "xxxx"];
        let cleanup = run_test_container(container_name, args).await;

        let start_event = event_rx.recv().await.expect("start event");
        assert!(matches!(start_event.event, ContainerEvent::Started { .. }));

        let die_event = event_rx.recv().await.expect("start event");
        assert!(matches!(
            die_event.event,
            ContainerEvent::Died { exit_code: 127 }
        ));

        cleanup().await;
    }
}
