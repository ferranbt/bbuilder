use eyre::{Context, Result};
use flate2::read::GzDecoder;
use futures_util::StreamExt;
use pin_project_lite::pin_project;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};
use tar::Archive;
use tokio::io::{AsyncRead, ReadBuf};
use url::Url;
use uuid::Uuid;

pub mod websocket;

#[derive(Debug, Clone, Copy, PartialEq)]
enum ArchiveFormat {
    TarGz,
    None,
}

impl ArchiveFormat {
    fn detect(url: &Url) -> Self {
        let path = url.path();
        if path.ends_with(".tar.gz") || path.ends_with(".tgz") {
            ArchiveFormat::TarGz
        } else {
            ArchiveFormat::None
        }
    }
}

/// Trait for tracking download progress
pub trait ProgressTracker {
    /// Called when the total size is known
    fn set_total(&mut self, total: u64);

    /// Called when bytes are downloaded
    fn update(&mut self, downloaded: u64);

    /// Called when download is complete
    fn finish(&mut self);
}

/// A no-op progress tracker that does nothing
pub struct NoOpProgressTracker;

impl ProgressTracker for NoOpProgressTracker {
    fn set_total(&mut self, _total: u64) {}
    fn update(&mut self, _downloaded: u64) {}
    fn finish(&mut self) {}
}

/// A simple console progress tracker that prints to stdout
pub struct ConsoleProgressTracker {
    total: Option<u64>,
    downloaded: u64,
}

impl ConsoleProgressTracker {
    pub fn new() -> Self {
        Self {
            total: None,
            downloaded: 0,
        }
    }
}

impl ProgressTracker for ConsoleProgressTracker {
    fn set_total(&mut self, total: u64) {
        self.total = Some(total);
        tracing::info!(
            "Total size: {} bytes ({:.2} MB)",
            total,
            total as f64 / 1024.0 / 1024.0
        );
    }

    fn update(&mut self, downloaded: u64) {
        self.downloaded = downloaded;
        if let Some(total) = self.total {
            let percentage = (downloaded as f64 / total as f64) * 100.0;
            tracing::info!(
                "Downloaded: {} / {} bytes ({:.2}%)",
                downloaded,
                total,
                percentage
            );
        } else {
            tracing::info!("Downloaded: {} bytes", downloaded);
        }
    }

    fn finish(&mut self) {
        tracing::info!("Download complete!");
    }
}

/// A progress tracker that forwards updates to multiple trackers
pub struct MultiProgressTracker {
    trackers: Vec<Box<dyn ProgressTracker>>,
}

impl MultiProgressTracker {
    pub fn new() -> Self {
        Self {
            trackers: Vec::new(),
        }
    }

    pub fn add_tracker<T: ProgressTracker + 'static>(mut self, tracker: T) -> Self {
        self.trackers.push(Box::new(tracker));
        self
    }
}

impl ProgressTracker for MultiProgressTracker {
    fn set_total(&mut self, total: u64) {
        for tracker in &mut self.trackers {
            tracker.set_total(total);
        }
    }

    fn update(&mut self, downloaded: u64) {
        for tracker in &mut self.trackers {
            tracker.update(downloaded);
        }
    }

    fn finish(&mut self) {
        for tracker in &mut self.trackers {
            tracker.finish();
        }
    }
}

pub async fn fetch(source: &str, destination: &PathBuf, checksum: Option<String>) -> Result<()> {
    fetch_with_progress(source, destination, &mut NoOpProgressTracker, checksum).await
}

pub async fn fetch_with_progress<T: ProgressTracker>(
    source: &str,
    destination: &PathBuf,
    progress: &mut T,
    checksum: Option<String>,
) -> Result<()> {
    // Parse the source as a URL
    let url =
        Url::parse(source).with_context(|| format!("Failed to parse source as URL: {}", source))?;

    match url.scheme() {
        "http" | "https" => fetch_http(&url, destination, progress, checksum).await,
        scheme => eyre::bail!("Unsupported URL scheme: {}", scheme),
    }?;

    Ok(())
}

async fn fetch_http<T: ProgressTracker>(
    url: &Url,
    destination: &PathBuf,
    progress: &mut T,
    checksum: Option<String>,
) -> Result<()> {
    // Detect if the URL points to an archive
    let archive_format = ArchiveFormat::detect(url);

    // Create the parent directory if it doesn't exist
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create parent directory: {}", parent.display()))?;
    }

    // Download the file
    let response = reqwest::get(url.as_str())
        .await
        .with_context(|| format!("Failed to download from: {}", url))?;

    if !response.status().is_success() {
        eyre::bail!("HTTP request failed with status: {}", response.status());
    }

    // Get the content length if available
    if let Some(total) = response.content_length() {
        progress.set_total(total);
    }

    // Determine download path: use temp file for tar.gz, direct destination otherwise
    let download_path = match archive_format {
        ArchiveFormat::TarGz => {
            let temp_dir = std::env::temp_dir();
            temp_dir.join(format!("fetcher_{}.tar.gz", Uuid::new_v4()))
        }
        ArchiveFormat::None => destination.clone(),
    };

    // Create a progress reader wrapper and download to file
    let mut progress_reader = AsyncProgressReader::new(response.bytes_stream(), progress);
    let mut file = tokio::fs::File::create(&download_path)
        .await
        .with_context(|| format!("Failed to create file: {}", download_path.display()))?;

    tokio::io::copy(&mut progress_reader, &mut file)
        .await
        .context("Failed to write file")?;

    progress_reader.finish();

    // Verify checksum if provided
    if let Some(expected_checksum) = checksum {
        verify_checksum(&download_path, &expected_checksum)?;
    }

    // Extract if tar.gz
    if archive_format == ArchiveFormat::TarGz {
        tracing::info!("Extracting tar.gz archive...");
        extract_tar_gz_from_file(&download_path, destination)?;
        // Clean up temp file
        tokio::fs::remove_file(&download_path)
            .await
            .with_context(|| format!("Failed to remove temp file: {}", download_path.display()))?;
    }

    Ok(())
}

/// Verify the SHA-256 checksum of a file
pub fn verify_checksum(file_path: &Path, expected_checksum: &str) -> Result<()> {
    tracing::info!("Verifying checksum");

    let mut file = File::open(file_path).with_context(|| {
        format!(
            "Failed to open file for checksum verification: {}",
            file_path.display()
        )
    })?;

    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)
        .context("Failed to read file for checksum computation")?;

    let computed_hash = format!("{:x}", hasher.finalize());

    if computed_hash != expected_checksum {
        eyre::bail!(
            "Checksum verification failed!\nExpected: {}\nComputed: {}",
            expected_checksum,
            computed_hash
        );
    }

    tracing::info!("✓ Checksum verified: {}", computed_hash);
    Ok(())
}

// An async reader wrapper that tracks progress
pin_project! {
    struct AsyncProgressReader<'a, S, T: ProgressTracker> {
        #[pin]
        stream: S,
        progress: &'a mut T,
        downloaded: u64,
        buffer: Vec<u8>,
        buffer_pos: usize,
    }
}

impl<'a, S, T: ProgressTracker> AsyncProgressReader<'a, S, T>
where
    S: futures_util::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin,
{
    fn new(stream: S, progress: &'a mut T) -> Self {
        Self {
            stream,
            progress,
            downloaded: 0,
            buffer: Vec::new(),
            buffer_pos: 0,
        }
    }

    fn finish(&mut self) {
        self.progress.finish();
    }
}

impl<'a, S, T: ProgressTracker> AsyncRead for AsyncProgressReader<'a, S, T>
where
    S: futures_util::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let mut this = self.project();

        // If we have buffered data, use it first
        if *this.buffer_pos < this.buffer.len() {
            let remaining = &this.buffer[*this.buffer_pos..];
            let to_copy = std::cmp::min(remaining.len(), buf.remaining());
            buf.put_slice(&remaining[..to_copy]);
            *this.buffer_pos += to_copy;

            *this.downloaded += to_copy as u64;
            this.progress.update(*this.downloaded);

            return Poll::Ready(Ok(()));
        }

        // Clear the buffer if we've consumed it all
        this.buffer.clear();
        *this.buffer_pos = 0;

        // Poll for next chunk from stream
        match this.stream.poll_next_unpin(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                let chunk_len = chunk.len();
                if chunk_len == 0 {
                    return Poll::Ready(Ok(()));
                }

                // Copy what we can directly into the output buffer
                let to_copy = std::cmp::min(chunk_len, buf.remaining());
                buf.put_slice(&chunk[..to_copy]);

                *this.downloaded += to_copy as u64;
                this.progress.update(*this.downloaded);

                // Store any remaining data in our buffer
                if to_copy < chunk_len {
                    this.buffer.extend_from_slice(&chunk[to_copy..]);
                }

                Poll::Ready(Ok(()))
            }
            Poll::Ready(Some(Err(e))) => {
                Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::Other, e)))
            }
            Poll::Ready(None) => Poll::Ready(Ok(())),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Extract a tar.gz archive from a file to a destination directory
fn extract_tar_gz_from_file(file_path: &Path, destination: &Path) -> Result<()> {
    let file = File::open(file_path)
        .with_context(|| format!("Failed to open tar.gz file: {}", file_path.display()))?;
    let gz = GzDecoder::new(file);
    let mut archive = Archive::new(gz);

    // Extract to the destination directory
    archive
        .unpack(destination)
        .with_context(|| format!("Failed to extract tar.gz to: {}", destination.display()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::{Arc, Mutex};

    struct TestProgressTracker {
        set_total_called: Arc<Mutex<bool>>,
        update_called: Arc<Mutex<bool>>,
        finish_called: Arc<Mutex<bool>>,
    }

    impl TestProgressTracker {
        fn new() -> Self {
            Self {
                set_total_called: Arc::new(Mutex::new(false)),
                update_called: Arc::new(Mutex::new(false)),
                finish_called: Arc::new(Mutex::new(false)),
            }
        }

        fn assert_ok(&self) {
            assert!(
                *self.set_total_called.lock().unwrap(),
                "set_total was not called",
            );
            assert!(*self.update_called.lock().unwrap(), "update was not called",);
            assert!(*self.finish_called.lock().unwrap(), "finish was not called",);
        }
    }

    impl ProgressTracker for TestProgressTracker {
        fn set_total(&mut self, _total: u64) {
            *self.set_total_called.lock().unwrap() = true;
        }

        fn update(&mut self, _downloaded: u64) {
            *self.update_called.lock().unwrap() = true;
        }

        fn finish(&mut self) {
            *self.finish_called.lock().unwrap() = true;
        }
    }

    fn get_fixture_path(filename: &str) -> String {
        format!(
            "https://raw.githubusercontent.com/ferranbt/bbuilder/refs/heads/main/crates/fetcher/fixtures/{}",
            filename
        )
    }

    fn ensure_fixture_file(path: &PathBuf) {
        const CONTENT_TXT: &[u8] = include_bytes!("../fixtures/content.txt");
        let actual_content = fs::read(path).expect(&format!("Failed to read file at {:?}", path));
        assert_eq!(actual_content, CONTENT_TXT);
    }

    #[tokio::test]
    async fn test_download_content_txt() {
        let filename = "content.txt";
        let checksum = "3dc7bc0209231cc61cb7d09c2efdfdf7aacb1f0b098db150780e980fa10d6b7a";

        let source = get_fixture_path(filename);
        let destination = PathBuf::from(format!("/tmp/fetcher_test_{}", filename));

        let _ = fs::remove_file(&destination);

        let mut progress = TestProgressTracker::new();

        let result = fetch_with_progress(
            &source,
            &destination,
            &mut progress,
            Some(checksum.to_string()),
        )
        .await;
        assert!(
            result.is_ok(),
            "Failed to download {} or verify checksum: {:?}",
            filename,
            result.err()
        );

        ensure_fixture_file(&destination);
        progress.assert_ok();

        let _ = fs::remove_file(&destination);
    }

    #[tokio::test]
    async fn test_download_content_tar_gz() {
        let filename = "content.tar.gz";
        let checksum = "aa7d1aae79175b06c5529409d65f4794479c9b060381e059a8b6d1510fa2ae48";

        let source = get_fixture_path(filename);
        let destination = PathBuf::from(format!("/tmp/fetcher_test_{}", filename));

        let _ = fs::remove_file(&destination);

        let mut progress = TestProgressTracker::new();

        let result = fetch_with_progress(
            &source,
            &destination,
            &mut progress,
            Some(checksum.to_string()),
        )
        .await;
        assert!(
            result.is_ok(),
            "Failed to download {} or verify checksum: {:?}",
            filename,
            result.err()
        );

        ensure_fixture_file(&destination.join("content.txt"));
        progress.assert_ok();

        let _ = fs::remove_file(&destination);
    }
}
