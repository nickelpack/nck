#![tarpaulin::skip]

use std::{
    cell::RefCell,
    ops::Add,
    time::{Duration, Instant},
};

use nix::{
    sys::{
        signal::Signal,
        wait::{waitpid, WaitPidFlag},
    },
    unistd::{setresgid, setresuid, Gid, Pid, Uid},
};

const CHILD_DROP_WAIT: Duration = Duration::from_secs(5);

/// Kills a child process (first with SIGINT, then with SIGKILL if it takes more than 5 seconds) when this value is
/// dropped.
pub struct ChildProcess(RefCell<Option<Pid>>);

impl std::fmt::Debug for ChildProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.borrow().fmt(f)
    }
}

impl From<Pid> for ChildProcess {
    fn from(value: Pid) -> Self {
        Self::new(value)
    }
}

impl From<i32> for ChildProcess {
    fn from(value: i32) -> Self {
        Self::new(Pid::from_raw(value))
    }
}

impl Drop for ChildProcess {
    fn drop(&mut self) {
        if let Err(error) = self.try_drop_impl() {
            tracing::warn!(?error, "failed to drop child process");
        }
    }
}

impl ChildProcess {
    /// Forgets the child process and returns the pid.
    pub fn forget(self) -> Pid {
        self.take().unwrap()
    }

    pub fn inner(&self) -> Pid {
        self.0.borrow().unwrap()
    }

    pub fn new(pid: Pid) -> Self {
        Self(RefCell::new(Some(pid)))
    }

    pub fn take(&self) -> Option<Pid> {
        self.0.borrow_mut().take()
    }

    fn poll(pid: Pid) -> std::io::Result<()> {
        match waitpid(pid, Some(WaitPidFlag::WNOHANG)) {
            Ok(v) => match v {
                nix::sys::wait::WaitStatus::Exited(_, _) => Ok(()),
                nix::sys::wait::WaitStatus::Signaled(_, _, _) => Ok(()),
                nix::sys::wait::WaitStatus::Stopped(_, _) => Ok(()),
                nix::sys::wait::WaitStatus::PtraceEvent(_, _, _)
                | nix::sys::wait::WaitStatus::Continued(_)
                | nix::sys::wait::WaitStatus::StillAlive
                | nix::sys::wait::WaitStatus::PtraceSyscall(_) => {
                    Err(std::io::ErrorKind::WouldBlock.into())
                }
            },
            Err(e) => match e {
                nix::Error::ECHILD => Ok(()),
                other => Err(std::io::Error::from_raw_os_error(other as i32)),
            },
        }
    }

    fn kill(pid: Pid, signal: Signal) -> nix::Result<bool> {
        match nix::sys::signal::kill(pid, signal) {
            Ok(_) => match Self::poll(pid) {
                Ok(_) => Ok(true),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(false),
                Err(error) => Err(error
                    .raw_os_error()
                    .map(nix::Error::from_i32)
                    .unwrap_or(nix::Error::EFAULT)),
            },
            Err(nix::Error::ESRCH) => Ok(true),
            Err(e) => Err(e),
        }
    }

    fn try_drop_impl(&mut self) -> nix::Result<()> {
        let pid = *if let Some(pid) = self.0.get_mut() {
            pid
        } else {
            return Ok(());
        };

        if Self::kill(pid, Signal::SIGTERM)? {
            return Ok(());
        }

        tracing::trace!("waiting for process to exit");
        let end = Instant::now().add(CHILD_DROP_WAIT);

        loop {
            match Self::poll(pid) {
                Ok(_) => return Ok(()),
                Err(err) => match err.kind() {
                    std::io::ErrorKind::WouldBlock if end > Instant::now() => {
                        std::thread::sleep(Duration::from_millis(15))
                    }
                    std::io::ErrorKind::WouldBlock => break,
                    _ => {
                        return Err(err
                            .raw_os_error()
                            .map(nix::Error::from_i32)
                            .unwrap_or(nix::Error::EFAULT))
                    }
                },
            }
        }

        tracing::warn!("process has taken too long to exit, sending SIGKILL",);
        Self::kill(pid, Signal::SIGKILL)?;
        Ok(())
    }
}

#[tracing::instrument(level = "trace", skip_all)]
pub fn set_id(uid: Uid, gid: Gid, supplementary: impl AsRef<[Gid]>) -> nix::Result<()> {
    let supplementary = supplementary.as_ref();

    if !supplementary.is_empty() {
        tracing::trace!(?supplementary, "setting supplementary groups");
        nix::unistd::setgroups(supplementary)?;
    }
    tracing::trace!(?gid, "setting gids");
    setresgid(gid, gid, gid)?;
    tracing::trace!(?uid, "setting uids");
    setresuid(uid, uid, uid)?;

    Ok(())
}
