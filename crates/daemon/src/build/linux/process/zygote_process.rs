use std::{
    collections::VecDeque,
    os::{fd::OwnedFd, unix::net::UnixStream},
};

use anyhow::{anyhow, bail};
use nix::sched::CloneFlags;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::build::linux::{
    fork,
    io::{ChannelError, EmptyFds, MessageChannel},
    proc::ChildProcess,
    process::supervisor_process::{SupervisorError, SupervisorMapped},
};

// The direction here is from the caller's/remote's perspective.

#[derive(Debug, Serialize, Deserialize)]
pub enum InitialRequest {
    Spawn { config: super::SandboxConfig },
}

#[derive(Debug, Serialize, Deserialize, Error)]
#[error("failed to spawn the supervisor process")]
pub struct SpawnError;

pub fn zygote_process(peer: UnixStream) -> anyhow::Result<()> {
    tracing::info!("zygote started");
    let mut fds = VecDeque::new();
    loop {
        let message = match peer.read_message(&mut fds) {
            Err(e) if e.is_closed_channel() => break,
            other => other?,
        };

        let response = match message {
            InitialRequest::Spawn { config } => match spawn(config, fds.pop_front()) {
                Ok(_) => Ok(()),
                Err(error) => {
                    tracing::error!(?error, "failed to spawn the supervisor process");
                    Err(SpawnError)
                }
            },
        };

        fds.clear();
        peer.write_message(response, EmptyFds)?;
    }
    Ok(())
}

fn spawn(config: super::SandboxConfig, mut sandbox_peer: Option<OwnedFd>) -> anyhow::Result<()> {
    let sandbox_peer = sandbox_peer.take().ok_or_else(|| anyhow!("missing fd"))?;
    let (supervisor_peer, local_supervisor_peer) = UnixStream::pair()?;

    let cb = {
        Box::new(move || {
            super::supervisor_process::supervisor_process(
                &supervisor_peer,
                sandbox_peer.try_clone()?,
            )
        })
    };

    let pid: ChildProcess = fork::clone(
        cb,
        CloneFlags::CLONE_NEWPID | CloneFlags::CLONE_NEWNS | CloneFlags::CLONE_NEWUSER,
    )?
    .into();

    if let Err(error) = config.namespace.write_mappings(pid.inner()) {
        if let Err(error) = local_supervisor_peer.write_message(SupervisorMapped::Exit, EmptyFds) {
            tracing::warn!(?error, "failed to inform supervisor to exit");
        }
        return Err(error.into());
    }

    local_supervisor_peer.write_message(SupervisorMapped::Proceed, EmptyFds)?;
    tracing::trace!("waiting for temp request");

    if let Err::<(), SupervisorError>(_) = local_supervisor_peer.read_message(&mut EmptyFds)? {
        bail!("supervisor process failed");
    }
    pid.forget();
    tracing::info!("started supervisor process");

    Ok(())
}
