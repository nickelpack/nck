use std::path::PathBuf;

use nix::sched::CloneFlags;
use serde::{Deserialize, Serialize};

use crate::build::linux::{
    channel::{Channel, PendingChannel},
    fork,
};

use super::sandbox_process::{SandboxRequest, SandboxResponse};

#[derive(Debug, Serialize, Deserialize)]
pub enum SupervisorRequest {
    UserMapped,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SupervisorResponse {
    Started,
    Failed,
}

pub fn supervisor_process(
    supervisor_peer: PendingChannel<SupervisorResponse, SupervisorRequest>,
    sandbox_peer: PendingChannel<SandboxResponse, SandboxRequest>,
    spec_path: PathBuf,
) -> anyhow::Result<()> {
    let supervisor_peer = supervisor_peer.into_peer()?;
    if let Err(error) = fallible_supervisor_process(&supervisor_peer, sandbox_peer, spec_path) {
        tracing::error!(?error, "supervisor failed");
        supervisor_peer.send(SupervisorResponse::Failed)?;
        Err(error)?;
    }
    supervisor_peer.send(SupervisorResponse::Started)?;
    Ok(())
}

fn fallible_supervisor_process(
    supervisor_peer: &Channel<SupervisorResponse, SupervisorRequest>,
    sandbox_peer: PendingChannel<SandboxResponse, SandboxRequest>,
    spec_path: PathBuf,
) -> anyhow::Result<()> {
    if let Err(error) = prctl::set_name("nck-supervisor") {
        tracing::warn!(?error, "failed to set supervisor name");
    }

    match supervisor_peer.recv()? {
        SupervisorRequest::UserMapped => (),
    };

    let cb = {
        let sandbox_peer = sandbox_peer.clone();
        let spec_path = spec_path.clone();
        Box::new(move || {
            super::sandbox_process::sandbox_process(sandbox_peer.clone(), spec_path.clone())
        })
    };

    fork::clone(cb, CloneFlags::empty())?;

    Ok(())
}
