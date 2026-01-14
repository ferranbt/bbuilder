use clap::{Parser, Subcommand};
use runtime_docker_compose::DockerRuntime;
use spec::Dep;
use std::fs;

#[derive(Parser)]
#[command(name = "bbuilder")]
#[command(about = "A builder application", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
    /// Path to the config folder
    #[arg(long, default_value = "./bbuilder")]
    config_folder: String,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the builder with the specified manifest file
    Run {
        /// Path to the manifest file
        #[arg(value_name = "FILE")]
        filename: String,
        /// Name for the deployment
        #[arg(short, long)]
        name: Option<String>,
        /// Dry run mode - generate files without starting containers
        #[arg(long)]
        dry_run: bool,
    },
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            filename,
            name,
            dry_run,
        } => run_command(filename, name, cli.config_folder, dry_run).await?,
    }

    Ok(())
}

async fn run_command(
    filename: String,
    name: Option<String>,
    config_folder: String,
    dry_run: bool,
) -> eyre::Result<()> {
    let contents = fs::read_to_string(&filename)?;
    let input: Dep = serde_json::from_str(contents.as_str())?;

    println!("input {:?}", input);

    let deployment_name = name
        .or(input.name.clone())
        .ok_or_else(|| eyre::eyre!("No name provided: specify via --name flag or in manifest"))?;

    let mut manifest = catalog::apply(input)?;
    manifest.name = deployment_name.clone();

    // Store manifest in ./bbuilder/manifests/<name>/manifest.json
    let manifest_dir = std::path::Path::new(&config_folder)
        .join("manifests")
        .join(&deployment_name);
    fs::create_dir_all(&manifest_dir)?;
    let manifest_path = manifest_dir.join("manifest.json");
    fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)?;

    // Pass ./bbuilder/docker-runtime to DockerRuntime
    let docker_runtime_path = std::path::Path::new(&config_folder)
        .join("docker-runtime")
        .to_string_lossy()
        .to_string();

    let runtime = DockerRuntime::new(docker_runtime_path);
    runtime.run(manifest, dry_run).await?;

    Ok(())
}
