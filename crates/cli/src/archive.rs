mod add;
mod create;

use clap::{Args, Subcommand};

use crate::CommandExec;

#[derive(Debug, Args)]
#[command(name = "archive", about = "Manipulate nck archives.", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    #[command(about = "Create a new archive.")]
    Create(create::Cli),
}

impl CommandExec for Cli {
    async fn execute(self) -> anyhow::Result<()> {
        match self.command {
            Commands::Create(v) => v.execute().await,
        }
    }
}
