use std::os::{fd::OwnedFd, unix::net::UnixStream};

use anyhow::Context;
use nck_io::fs::clone_mount;
use nix::{
    libc::{SIGCHLD, SIGHUP, SIGINT, SIGQUIT, SIGTERM},
    mount::{MntFlags, MsFlags},
    sched::CloneFlags,
    sys::wait::{waitpid, WaitPidFlag},
    unistd::{Gid, Pid, Uid},
};
use serde::{Deserialize, Serialize};
use signal_hook::iterator::Signals;
use thiserror::Error;

use crate::build::linux::{
    fork,
    fs::{mount, unmount, MountType, SYS_NONE},
    io::{EmptyFds, MessageChannel},
    proc::{set_id, ChildProcess},
};

#[derive(Debug, Serialize, Deserialize)]
pub enum SupervisorMapped {
    Proceed,
    Exit,
}

#[derive(Debug, Serialize, Deserialize, Error)]
#[error("failed to start the sandbox process")]
pub struct SupervisorError;

pub fn supervisor_process(
    supervisor_peer: &UnixStream,
    sandbox_peer: OwnedFd,
) -> anyhow::Result<()> {
    match fallible_supervisor_process(supervisor_peer, sandbox_peer) {
        Ok(Some(child)) => {
            supervisor_peer.write_message(Ok::<(), SupervisorError>(()), EmptyFds)?;
            wait(child).context("while waiting for child to exit")?;
        }
        Ok(None) => {
            tracing::trace!("exit requested");
        }
        Err(error) => {
            tracing::error!(?error, "failed to start the sandbox process");
            supervisor_peer.write_message(Err::<(), _>(SupervisorError), EmptyFds)?;
            Err(error)?;
        }
    }
    Ok(())
}

fn fallible_supervisor_process(
    supervisor_peer: &UnixStream,
    sandbox_peer: OwnedFd,
) -> anyhow::Result<Option<ChildProcess>> {
    if let Err(error) = prctl::set_name("nck-supervisor") {
        tracing::warn!(?error, "failed to set supervisor name");
    }

    match supervisor_peer.read_message(&mut EmptyFds)? {
        SupervisorMapped::Proceed => {}
        SupervisorMapped::Exit => {
            return Ok(None);
        }
    }

    set_id(Uid::from_raw(0), Gid::from_raw(0), [])?;

    mount(
        SYS_NONE,
        "/tmp",
        Some(MountType::TmpFs),
        MsFlags::MS_SHARED,
        SYS_NONE,
    )
    .context("when mounting temporary fs")?;
    let tmp = clone_mount(None, "/tmp").context("when cloning /tmp")?;
    unmount("/tmp", MntFlags::MNT_DETACH).context("when unmounting /tmp")?;

    let cb = {
        Box::new(move || {
            super::sandbox_process::sandbox_process(
                sandbox_peer.try_clone().unwrap(),
                tmp.try_clone().unwrap(),
            )
        })
    };

    let child = fork::clone(cb, CloneFlags::empty())?;
    Ok(Some(child.into()))
}

fn wait(child: ChildProcess) -> anyhow::Result<()> {
    tracing::trace!(?child, "waiting for a signal or child to exit");
    let mut signals = Signals::new([SIGINT, SIGTERM, SIGQUIT, SIGHUP, SIGCHLD])?;

    match signals.forever().next() {
        Some(SIGINT) => tracing::trace!("got SIGINT"),
        Some(SIGTERM) => tracing::trace!("got SIGTERM"),
        Some(SIGQUIT) => tracing::trace!("got SIGQUIT"),
        Some(SIGCHLD) => {
            tracing::trace!("child process exited");
            if waitpid(Pid::from_raw(-1), Some(WaitPidFlag::WNOHANG)).is_ok() {
                // Child has already exited, so don't try to kill it
                child.take();
            }
        }
        Some(other) => tracing::trace!(other, "got unknown signal"),
        None => {}
    }

    drop(child);
    Ok(())
}
