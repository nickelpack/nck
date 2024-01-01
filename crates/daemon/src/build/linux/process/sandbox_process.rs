use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::build::linux::channel::PendingChannel;

#[derive(Debug, Serialize, Deserialize)]
pub enum SandboxRequest {}

#[derive(Debug, Serialize, Deserialize)]
pub enum SandboxResponse {}

pub fn sandbox_process(
    sandbox_peer: PendingChannel<SandboxResponse, SandboxRequest>,
    spec_path: PathBuf,
) -> anyhow::Result<()> {
    let sandbox_peer = sandbox_peer.into_peer()?;
    let spec = std::fs::read_to_string(spec_path.as_path())?;
    println!("aaa {}", spec);
    let spec = toml::from_str(&spec)?;

    println!("--");
    println!("-- {:?}", spec);

    Ok(())
}
