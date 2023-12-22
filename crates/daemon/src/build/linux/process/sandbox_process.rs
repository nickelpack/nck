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
    tracing::trace!("hello from the sandbox");
    Ok(())
}
