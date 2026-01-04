use clap::Parser;
use fetcher::{ConsoleProgressTracker, fetch_with_progress, verify_checksum};
use std::path::PathBuf;
use std::process;

#[derive(Parser, Debug)]
#[command(name = "fetcher")]
#[command(about = "A tool to fetch files from various sources")]
struct Args {
    /// Source URL to fetch from
    source: String,

    /// Destination path to save the file
    destination: PathBuf,

    /// Expected SHA-256 checksum (hex string) to verify the downloaded file
    #[arg(long)]
    checksum: Option<String>,

    /// Skip download if file exists and checksum matches (requires --checksum)
    #[arg(long, requires = "checksum")]
    skip_if_valid_checksum: bool,
}

fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let args = Args::parse();

    tracing::info!(
        "Fetching from {} to {}",
        args.source,
        args.destination.display()
    );

    if let Some(ref checksum) = args.checksum {
        tracing::info!("Checksum verification enabled: {}", checksum);

        // Check if we should skip downloading
        if args.skip_if_valid_checksum && args.destination.exists() {
            tracing::info!("File exists, verifying checksum before download...");
            match verify_checksum(&args.destination, checksum) {
                Ok(_) => {
                    tracing::info!("✓ File already exists with valid checksum, skipping download");
                    return;
                }
                Err(e) => {
                    tracing::info!("Checksum mismatch or verification failed: {}", e);
                    tracing::info!("Proceeding with download...");
                }
            }
        }
    }

    let mut progress = ConsoleProgressTracker::new();

    if let Err(e) = fetch_with_progress(
        &args.source,
        &args.destination,
        &mut progress,
        args.checksum,
    ) {
        tracing::error!("Failed to fetch: {}", e);

        // Print the error chain
        let mut source = e.source();
        while let Some(err) = source {
            tracing::error!("  Caused by: {}", err);
            source = err.source();
        }

        process::exit(1);
    }

    tracing::info!("Successfully downloaded to: {}", args.destination.display());
}
