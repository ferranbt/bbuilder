use eyre::{Context, Result};
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use tar::Archive;
use url::Url;

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

    #[test]
    fn test_download_content_txt() {
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
        );
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

    #[test]
    fn test_download_content_tar_gz() {
        let filename = "content.tar.gz";

        let source = get_fixture_path(filename);
        let destination = PathBuf::from(format!("/tmp/fetcher_test_{}", filename));

        let _ = fs::remove_file(&destination);

        let mut progress = TestProgressTracker::new();

        let result = fetch_with_progress(&source, &destination, &mut progress, None);
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
