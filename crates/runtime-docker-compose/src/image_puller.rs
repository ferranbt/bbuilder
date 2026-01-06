use bollard::Docker;
use bollard::query_parameters::CreateImageOptions;
use futures_util::stream::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};

/// Represents a pending image pull operation
struct PullFuture {
    notify: Arc<Notify>,
    result: Arc<Mutex<Option<Result<(), String>>>>,
}

impl PullFuture {
    fn new() -> Self {
        Self {
            notify: Arc::new(Notify::new()),
            result: Arc::new(Mutex::new(None)),
        }
    }

    /// Wait blocks until the pull completes or context is canceled
    async fn wait(&self) -> Result<(), String> {
        self.notify.notified().await;
        self.result
            .lock()
            .await
            .clone()
            .expect("result should be set when notified")
    }

    /// Complete marks the pull as done and unblocks all waiters
    async fn complete(&self, result: Result<(), String>) {
        *self.result.lock().await = Some(result);
        self.notify.notify_waiters();
    }
}

/// ImagePuller coordinates image pulls to prevent duplicate pulls
pub struct ImagePuller {
    client: Docker,
    pulls: Arc<Mutex<HashMap<String, Arc<PullFuture>>>>,
}

impl ImagePuller {
    pub fn new(client: Docker) -> Self {
        Self {
            client,
            pulls: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// PullImage pulls an image if needed, or waits for an existing pull
    pub async fn pull_image(&self, image_name: &str) -> Result<(), String> {
        // Check if already pulling
        let future = {
            let mut pulls = self.pulls.lock().await;
            if let Some(existing_future) = pulls.get(image_name) {
                existing_future.clone()
            } else {
                // Start a new pull
                let future = Arc::new(PullFuture::new());
                pulls.insert(image_name.to_string(), future.clone());

                // Spawn the actual pull operation
                let client = self.client.clone();
                let image_name_owned = image_name.to_string();
                let future_clone = future.clone();
                let pulls_clone = self.pulls.clone();

                tokio::spawn(async move {
                    let result = Self::pull_image_impl(&client, &image_name_owned).await;
                    future_clone.complete(result).await;

                    // Clean up the future
                    pulls_clone.lock().await.remove(&image_name_owned);
                });

                future
            }
        };

        future.wait().await
    }

    async fn pull_image_impl(client: &Docker, image_name: &str) -> Result<(), String> {
        let options = CreateImageOptions {
            from_image: Some(image_name.to_string()),
            ..Default::default()
        };

        let mut stream = client.create_image(Some(options), None, None);

        // Consume the output to ensure pull completes
        while let Some(result) = stream.next().await {
            if let Err(e) = result {
                return Err(format!("error during image pull {}: {}", image_name, e));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bollard::query_parameters::RemoveImageOptions;
    use futures_util::future::join_all;

    async fn ensure_image_removed(docker: &Docker, image_name: &str) {
        let _ = docker
            .remove_image(
                image_name,
                Some(RemoveImageOptions {
                    force: true,
                    ..Default::default()
                }),
                None,
            )
            .await;
    }

    #[tokio::test]
    async fn test_pull_image() {
        let docker = Docker::connect_with_local_defaults().unwrap();
        let image_name = "alpine:latest";

        // Ensure image doesn't exist
        ensure_image_removed(&docker, image_name).await;

        let puller = ImagePuller::new(docker);

        // Pull a small test image
        let result = puller.pull_image(image_name).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_concurrent_pulls_same_image() {
        let docker = Docker::connect_with_local_defaults().unwrap();
        let image_name = "alpine:latest";

        // Ensure image doesn't exist
        ensure_image_removed(&docker, image_name).await;

        let puller = Arc::new(ImagePuller::new(docker));

        // Spawn multiple concurrent pulls for the same image
        let mut handles = vec![];
        for _ in 0..5 {
            let puller_clone = puller.clone();
            let handle = tokio::spawn(async move { puller_clone.pull_image(image_name).await });
            handles.push(handle);
        }

        // All should complete successfully
        for handle in handles {
            let result = handle.await.unwrap();
            assert!(result.is_ok());
        }
    }

    #[tokio::test]
    async fn test_pull_multiple_images_with_join_all() {
        let docker = Docker::connect_with_local_defaults().unwrap();
        let images = vec!["alpine:3.18", "alpine:3.19", "alpine:3.20"];

        // Ensure images don't exist
        for image in &images {
            ensure_image_removed(&docker, image).await;
        }

        let puller = ImagePuller::new(docker);

        // Pull all images concurrently using join_all
        let futures: Vec<_> = images.iter().map(|img| puller.pull_image(img)).collect();
        let results = join_all(futures).await;

        // All pulls should succeed
        for (i, result) in results.iter().enumerate() {
            assert!(result.is_ok(), "Failed to pull image {}", images[i]);
        }
    }
}
