use eyre::{Context, Result};
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use tar::Archive;
use url::Url;

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

pub fn fetch(source: &str, destination: &PathBuf, checksum: Option<String>) -> Result<()> {
    fetch_with_progress(source, destination, &mut NoOpProgressTracker, checksum)
}

pub fn fetch_with_progress<T: ProgressTracker>(
    source: &str,
    destination: &PathBuf,
    progress: &mut T,
    checksum: Option<String>,
) -> Result<()> {
    // Parse the source as a URL
    let url =
        Url::parse(source).with_context(|| format!("Failed to parse source as URL: {}", source))?;

    match url.scheme() {
        "http" | "https" => fetch_http(&url, destination, progress),
        scheme => eyre::bail!("Unsupported URL scheme: {}", scheme),
    }?;

    if let Some(checksum) = checksum {
        verify_checksum(destination, &checksum)?
    }

    Ok(())
}

fn fetch_http<T: ProgressTracker>(
    url: &Url,
    destination: &PathBuf,
    progress: &mut T,
) -> Result<()> {
    // Detect if the URL points to an archive
    let archive_format = ArchiveFormat::detect(url);

    // Create the parent directory if it doesn't exist
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create parent directory: {}", parent.display()))?;
    }

    // Download the file
    let response = reqwest::blocking::get(url.as_str())
        .with_context(|| format!("Failed to download from: {}", url))?;

    if !response.status().is_success() {
        eyre::bail!("HTTP request failed with status: {}", response.status());
    }

    // Get the content length if available
    if let Some(total) = response.content_length() {
        progress.set_total(total);
    }

    // Create a progress reader wrapper
    let mut progress_reader = ProgressReader::new(response, progress);

    match archive_format {
        ArchiveFormat::TarGz => {
            tracing::info!("Detected tar.gz archive, streaming decompression...");
            extract_tar_gz(&mut progress_reader, destination)?;
        }
        ArchiveFormat::None => {
            // Standard file download
            let mut file = File::create(destination)
                .with_context(|| format!("Failed to create file: {}", destination.display()))?;

            std::io::copy(&mut progress_reader, &mut file).context("Failed to write file")?;
        }
    }

    progress_reader.finish();

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

/// A reader wrapper that tracks progress
struct ProgressReader<'a, R: Read, T: ProgressTracker> {
    inner: R,
    progress: &'a mut T,
    downloaded: u64,
}

impl<'a, R: Read, T: ProgressTracker> ProgressReader<'a, R, T> {
    fn new(inner: R, progress: &'a mut T) -> Self {
        Self {
            inner,
            progress,
            downloaded: 0,
        }
    }

    fn finish(&mut self) {
        self.progress.finish();
    }
}

impl<'a, R: Read, T: ProgressTracker> Read for ProgressReader<'a, R, T> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let bytes_read = self.inner.read(buf)?;
        self.downloaded += bytes_read as u64;
        self.progress.update(self.downloaded);
        Ok(bytes_read)
    }
}

/// Extract a tar.gz archive from a reader to a destination directory
fn extract_tar_gz<R: Read>(reader: R, destination: &Path) -> Result<()> {
    let gz = GzDecoder::new(reader);
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

    #[test]
    fn test_download_readme() {
        let source =
            "https://raw.githubusercontent.com/ferranbt/bbuilder/refs/heads/main/README.md";
        let destination = PathBuf::from("/tmp/fetcher_test_readme.md");

        // Clean up any existing file
        let _ = fs::remove_file(&destination);

        // Download the file with checksum verification
        // Expected SHA-256 checksum for the README.md file
        let checksum = "79dcadb1ef725cbc30bb3a9a877cff3702e9d3bb28baff74265a9d70a2e8874b";
        let result = fetch(source, &destination, Some(checksum.to_string()));
        assert!(
            result.is_ok(),
            "Failed to download file or verify checksum: {:?}",
            result.err()
        );

        // Verify the file exists
        assert!(destination.exists(), "Downloaded file does not exist");

        // Verify the file has content
        let content = fs::read_to_string(&destination).expect("Failed to read downloaded file");
        assert!(!content.is_empty(), "Downloaded file is empty");

        // Clean up
        let _ = fs::remove_file(&destination);
    }
}
