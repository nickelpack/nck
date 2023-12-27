mod archive;

use clap::{Parser, Subcommand};

trait CommandExec {
    async fn execute(self) -> anyhow::Result<()>;
}

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    #[command(about = "Manage nck archives.")]
    Archive(archive::Cli),
}

#[tokio::main]
pub async fn main() -> anyhow::Result<()> {
    let args = argfile::expand_args_from(
        std::env::args_os(),
        argfile::parse_fromfile,
        argfile::PREFIX,
    )?;

    let cli = Cli::parse_from(args);
    match cli.command {
        Commands::Archive(v) => v.execute().await,
    }
}
