use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{build::linux::channel::PendingChannel, spec::Spec};

#[derive(Debug, Serialize, Deserialize)]
pub enum SandboxRequest {}

#[derive(Debug, Serialize, Deserialize)]
pub enum SandboxResponse {}

pub fn sandbox_process(
    sandbox_peer: PendingChannel<SandboxResponse, SandboxRequest>,
    spec_path: PathBuf,
) -> anyhow::Result<()> {
    let sandbox_peer = sandbox_peer.into_peer()?;
    let spec = std::fs::read(spec_path.as_path())?;
    println!("aaa {}", String::from_utf8_lossy(spec.as_slice()));
    let spec = rmp_serde::from_slice::<Spec>(spec.as_slice()).unwrap();

    println!("--");
    println!("-- {:?}", spec);

    Ok(())
}
