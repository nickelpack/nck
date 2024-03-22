use std::os::{fd::OwnedFd, unix::net::UnixStream};

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub enum SandboxRequest {}

#[tracing::instrument(level = "trace", skip_all)]
pub fn sandbox_process(sandbox_peer: OwnedFd) -> anyhow::Result<()> {
    let sandbox_peer: UnixStream = sandbox_peer.into();

    Ok(())
}
