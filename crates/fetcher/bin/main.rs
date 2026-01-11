use clap::Parser;
use fetcher::{ConsoleProgressTracker, fetch_with_progress, verify_checksum, MultiProgressTracker};
use std::path::PathBuf;
use std::process;
use std::sync::Arc;

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

    /// Enable WebSocket progress server
    #[arg(long)]
    ws: bool,

    /// Port for WebSocket server (default: 7070)
    #[arg(long, default_value = "7070")]
    ws_port: u16,

    /// Bind address for WebSocket server (default: 127.0.0.1)
    #[arg(long, default_value = "0.0.0.0")]
    ws_bind_address: String,
}

#[tokio::main]
async fn main() {
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

    // Build progress tracker with console, and optionally WebSocket
    let mut progress = MultiProgressTracker::new().add_tracker(ConsoleProgressTracker::new());

    // Start WebSocket server if enabled
    let _ws_server_handle = if args.ws {
        use fetcher::websocket::{start_progress_server, WebSocketProgressTracker};
        use fetcher_api::ProgressMessage;
        use tokio::sync::broadcast;

        let addr = format!("{}:{}", args.ws_bind_address, args.ws_port)
            .parse()
            .expect("Invalid bind address");

        let (tx, _rx) = broadcast::channel::<ProgressMessage>(100);
        let tx = Arc::new(tx);

        let handle = start_progress_server(addr, tx.clone())
            .await
            .expect("Failed to start WebSocket server");

        progress = progress.add_tracker(WebSocketProgressTracker::new(tx));

        Some(handle)
    } else {
        None
    };

    if let Err(e) = fetch_with_progress(
        &args.source,
        &args.destination,
        &mut progress,
        args.checksum,
    )
    .await
    {
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
