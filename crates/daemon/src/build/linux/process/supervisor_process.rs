use std::{
    fs::File,
    os::{fd::OwnedFd, unix::net::UnixStream},
    path::PathBuf,
};

use anyhow::Context;
use nck_io::fs::TempDir;
use nix::{
    libc::{SIGCHLD, SIGHUP, SIGINT, SIGQUIT, SIGTERM},
    mount::{MntFlags, MsFlags},
    sched::CloneFlags,
    sys::wait::{waitpid, WaitPidFlag},
    unistd::{Gid, Pid, Uid},
};
use serde::{Deserialize, Serialize};
use signal_hook::iterator::Signals;

use crate::{
    build::linux::{
        fork,
        fs::{mount, unmount, MountType, SYS_NONE},
        io::{EmptyFds, MessageChannel},
    },
    settings::StoreSettings,
};

use super::{set_id, ChildProcess};

#[derive(Debug, Serialize, Deserialize)]
pub enum SupervisorRequest {
    UserMapped,
    Exit,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SupervisorResponse {
    Started,
    Failed,
}

pub fn supervisor_process(
    config: StoreSettings,
    supervisor_peer: &UnixStream,
    sandbox_peer: OwnedFd,
    spec_path: PathBuf,
) -> anyhow::Result<()> {
    match fallible_supervisor_process(config, supervisor_peer, sandbox_peer, spec_path) {
        Ok(Some((child, _tmp))) => {
            supervisor_peer.write_message(SupervisorResponse::Started, EmptyFds)?;
            wait(child).context("while waiting for child to exit")?;
        }
        Ok(None) => {
            tracing::trace!("exit requested");
        }
        Err(error) => {
            tracing::error!(?error, "supervisor failed");
            supervisor_peer.write_message(SupervisorResponse::Failed, EmptyFds)?;
            Err(error)?;
        }
    }
    Ok(())
}

fn fallible_supervisor_process(
    config: StoreSettings,
    supervisor_peer: &UnixStream,
    sandbox_peer: OwnedFd,
    spec_path: PathBuf,
) -> anyhow::Result<Option<(ChildProcess, Unmount)>> {
    if let Err(error) = prctl::set_name("nck-supervisor") {
        tracing::warn!(?error, "failed to set supervisor name");
    }

    match supervisor_peer.read_message(&mut EmptyFds)? {
        SupervisorRequest::UserMapped => {}
        SupervisorRequest::Exit => {
            return Ok(None);
        }
    }

    set_id(Uid::from_raw(0), Gid::from_raw(0), [])?;

    let tmp = TempDir::new_in(config.temp.as_path())?;
    mount(
        SYS_NONE,
        tmp.as_path(),
        Some(&MountType::TmpFs),
        MsFlags::empty(),
        SYS_NONE,
    )?;

    let f = std::fs::OpenOptions::new()
        .read(false)
        .write(true)
        .create(true)
        .truncate(false)
        .open(tmp.as_path().join(".keep"))?;

    // File will keep mount alive
    unmount(
        tmp.as_path(),
        MntFlags::MNT_DETACH | MntFlags::UMOUNT_NOFOLLOW,
    )?;

    let tmp = Unmount(tmp, f);
    let cb = {
        let spec_path = spec_path.clone();
        let sandbox_path = tmp.0.as_path().to_path_buf();
        let config = config.clone();
        Box::new(move || {
            super::sandbox_process::sandbox_process(
                config.clone(),
                sandbox_path.clone(),
                sandbox_peer.try_clone().unwrap(),
                spec_path.clone(),
            )
        })
    };

    let child = fork::clone(cb, CloneFlags::empty())?;
    Ok(Some((child.into(), tmp)))
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

#[derive(Debug)]
#[allow(dead_code)]
struct Unmount(TempDir, File);
