mod archive;
mod hash;

use clap::{Parser, Subcommand};

// TODO: Having enums derive this for all variants would be ideal, but I can't find a crate that does that - and such a
// crate is extremely out of scope for Nickelpack.
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
    #[command(about = "Hash files.")]
    Hash(hash::Hash),
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
        Commands::Hash(v) => v.execute().await,
    }
}
