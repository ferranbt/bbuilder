use bollard::Docker;
use bollard::query_parameters::EventsOptionsBuilder;
use futures_util::stream::StreamExt;
use std::collections::HashMap;
use tokio::sync::mpsc;

use crate::compose::DockerComposeSpec;

/// Single deployment watcher
pub struct DeploymentState {
    pub services: Vec<ServiceStatus>,
}

pub struct ServiceStatus {
    pub name: String,
    pub container_ip: Option<String>,
    pub state: ContainerState,
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

#[derive(Debug, Default, Clone)]
pub enum ContainerEvent {
    #[default]
    Pending,
    PullingImage,
    Started {
        container_ip: String,
    },
    Died {
        exit_code: i64,
    },
}

#[derive(Debug)]
pub struct DockerEventMessage {
    pub container_name: String,
    pub event: ContainerEvent,
}

impl DeploymentState {
    pub fn new(spec: &DockerComposeSpec) -> Self {
        let mut services = Vec::new();

        for (service_name, _) in &spec.services {
            services.push(ServiceStatus {
                name: service_name.clone(),
                container_ip: None,
                state: ContainerState::Pending,
            });
        }

        Self { services }
    }

    pub fn handle_container_event(&mut self, container_name: String, event: ContainerEvent) {
        let service = self.services.iter_mut().find(|s| s.name == container_name);

        if let Some(service) = service {
            match event {
                ContainerEvent::Pending => {}
                ContainerEvent::PullingImage => {
                    service.state = ContainerState::PullingImage;
                }
                ContainerEvent::Started { container_ip } => {
                    service.state = ContainerState::Running;
                    service.container_ip = Some(container_ip.clone());
                }
                ContainerEvent::Died { exit_code } => {
                    service.state =
                        ContainerState::Failed(format!("Died with exit code: {}", exit_code));
                }
            }
        }
    }
}

/// Listen to Docker events and send them through the channel
pub async fn listen_docker_events(
    tx: mpsc::UnboundedSender<DockerEventMessage>,
    filters: HashMap<&str, Vec<&str>>,
) {
    let docker = Docker::connect_with_local_defaults().unwrap();
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
    use std::collections::HashMap;

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

        let mut labels = HashMap::new();
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
        let filters = HashMap::from([("container", vec![container_name])]);

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
        let filters = HashMap::from([("container", vec![container_name])]);

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
        let filters = HashMap::from([("container", vec![container_name])]);

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
