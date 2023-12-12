use std::path::PathBuf;

use nix::{
    sys::wait::{waitpid, WaitPidFlag},
    unistd::Pid,
};
use signal_hook::{
    consts::{SIGCHLD, SIGHUP, SIGINT, SIGQUIT, SIGTERM},
    iterator::Signals,
};

use super::syscall::{ChildProcess, Result, Syscall, TempMount};

pub fn main<SC: Syscall>(path: PathBuf, child: Pid, rootfs_dir: TempMount<SC>) -> isize {
    let child_proc: ChildProcess<SC> = child.into();
    let path = path.join("supervisor");
    if let Err(error) = supervisor_main(path, child_proc, rootfs_dir) {
        tracing::error!(?error, "supervisor failed");
        -1
    } else {
        0
    }
}

#[tracing::instrument(level = "trace", skip_all)]
fn supervisor_main<SC: Syscall>(
    path: PathBuf,
    child: ChildProcess<SC>,
    rootfs_dir: TempMount<SC>,
) -> Result<()> {
    if let Err(error) = prctl::set_name(&path.to_string_lossy()) {
        let error = nix::Error::from_i32(error);
        tracing::warn!(?error, "failed to set supervisor process name");
    }

    tracing::trace!("waiting for a signal");
    let mut signals = Signals::new([SIGINT, SIGTERM, SIGQUIT, SIGHUP, SIGCHLD])?;

    let mut child = Some(child);
    match signals.forever().next() {
        Some(SIGINT) => tracing::trace!("got SIGINT"),
        Some(SIGTERM) => tracing::trace!("got SIGTERM"),
        Some(SIGQUIT) => tracing::trace!("got SIGQUIT"),
        Some(SIGCHLD) => {
            tracing::trace!("child process exited");
            if waitpid(Pid::from_raw(-1), Some(WaitPidFlag::WNOHANG)).is_ok() {
                // Child has already exited, so don't try to kill it
                child.take().unwrap().into_inner();
            }
        }
        Some(other) => tracing::trace!(other, "got unknown signal"),
        None => {}
    }

    tracing::trace!("cleaning up filesystem");
    // Ensure that NLL doesn't drop these early
    drop(rootfs_dir);
    drop(child);
    Ok(())
}
