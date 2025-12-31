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

    /// Server bind address
    #[arg(long, default_value = "127.0.0.1:3000")]
    addr: String,
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    tracing::info!(
        "Starting Babel server for {} node at {}",
        cli.node_type,
        cli.rpc_url
    );

    match cli.node_type.as_str() {
        "ethereum" => {
            let babel = EthereumBabel::new(cli.rpc_url);
            let server = BabelServer::new(babel);
            server.serve(&cli.addr).await?;
        }
        "ethereum_beacon" => {
            let babel = EthereumBeaconBabel::new(cli.rpc_url);
            let server = BabelServer::new(babel);
            server.serve(&cli.addr).await?;
        }
        "cosmos" => {
            let babel = CosmosBabel::new(cli.rpc_url);
            let server = BabelServer::new(babel);
            server.serve(&cli.addr).await?;
        }
        _ => {
            return Err(eyre::eyre!(
                "Unknown node type: {}. Supported types: ethereum, ethereum_beacon, cosmos",
                cli.node_type
            ));
        }
    }

    Ok(())
}
