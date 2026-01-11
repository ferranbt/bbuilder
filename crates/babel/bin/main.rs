use babel::{BabelServer, CosmosBabel, EthereumBabel, EthereumBeaconBabel};
use clap::Parser;

#[derive(Parser)]
#[command(name = "babel")]
#[command(about = "Blockchain node health check server", long_about = None)]
struct Cli {
    /// Node type: ethereum, ethereum_beacon, cosmos
    #[arg(long)]
    node_type: String,

    /// RPC/API URL for the node
    #[arg(long)]
    rpc_url: String,

    /// REST server bind address
    #[arg(long, default_value = "127.0.0.1:3000")]
    addr: String,

    /// RPC server bind address
    #[arg(long, default_value = "127.0.0.1:3001")]
    rpc_addr: String,

    /// Node name for metrics labels (defaults to hostname/IP from rpc_url)
    #[arg(long)]
    nodename: Option<String>,
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let cli = Cli::parse();

    // Determine nodename: use provided value or extract from rpc_url
    let nodename = match cli.nodename {
        Some(name) => Some(name),
        None => {
            // Parse the URL and extract hostname
            match url::Url::parse(&cli.rpc_url) {
                Ok(url) => {
                    let host = url.host_str().unwrap_or("unknown");
                    if host == "localhost" || host == "127.0.0.1" {
                        None
                    } else {
                        Some(host.to_string())
                    }
                }
                Err(_) => None,
            }
        }
    };

    tracing::info!(
        "Starting Babel server for {} node at {} with nodename {:?}",
        cli.node_type,
        cli.rpc_url,
        nodename,
    );

    let server = match cli.node_type.as_str() {
        "ethereum" => {
            let babel = EthereumBabel::new(cli.rpc_url).await?;
            BabelServer::new(babel, nodename)
        }
        "ethereum_beacon" => {
            let babel = EthereumBeaconBabel::new(cli.rpc_url);
            BabelServer::new(babel, nodename)
        }
        "cosmos" => {
            let babel = CosmosBabel::new(cli.rpc_url);
            BabelServer::new(babel, nodename)
        }
        _ => {
            return Err(eyre::eyre!(
                "Unknown node type: {}. Supported types: ethereum, ethereum_beacon, cosmos",
                cli.node_type
            ));
        }
    };

    // Start RPC server in background
    let rpc_addr = cli.rpc_addr.parse()?;
    let (_rpc_handle, _rpc_local_addr) = babel::rpc::start_rpc_server(rpc_addr, server.cached_status()).await?;

    // Start REST server
    server.serve(&cli.addr).await?;

    Ok(())
}
