use std::time::{Duration, Instant};

use nix::{
    errno::Errno,
    sys::{
        personality::{self, Persona},
        signal::Signal,
    },
    unistd::Pid,
};

#[tracing::instrument(level = "trace", skip_all, err(Debug))]
pub fn change_personality(f: impl FnOnce(Persona) -> Persona) -> nix::Result<()> {
    let mut persona = personality::get()?;
    tracing::trace!(?persona, "got existing persona");
    persona = f(persona);
    tracing::trace!(?persona, "setting persona");
    personality::set(persona)?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    Running,
    Stopped,
}

impl ProcessState {
    pub fn is_stopped(&self) -> bool {
        *self == ProcessState::Stopped
    }
}

#[tracing::instrument(level = "trace", fields(?pid, ?signal), err(Debug))]
pub fn kill(pid: Pid, signal: Signal) -> nix::Result<ProcessState> {
    match nix::sys::signal::kill(pid, signal) {
        Ok(_) => poll(pid),
        Err(Errno::ESRCH) => Ok(ProcessState::Stopped),
        Err(e) => Err(e),
    }
}

#[tracing::instrument(level = "trace", fields(?pid), err(Debug))]
pub fn poll(pid: Pid) -> nix::Result<ProcessState> {
    let result = procfs::process::Process::new(pid.as_raw()).map_err(map_proc_err);
    match result {
        Ok(v) if v.is_alive() => Ok(ProcessState::Running),
        Ok(_) => Ok(ProcessState::Stopped),
        Err(nix::Error::ENOENT) => Ok(ProcessState::Stopped),
        Err(e) => Err(e),
    }
}

#[tracing::instrument(level = "trace", fields(?pid), err(Debug))]
pub fn kill_wait(pid: Pid) -> nix::Result<()> {
    if kill(pid, Signal::SIGTERM)?.is_stopped() {
        return Ok(());
    }

    tracing::trace!("waiting for process to exit");
    let end = Instant::now() + Duration::from_secs(2);
    while Instant::now() < end {
        std::thread::sleep(Duration::from_millis(25));
        if poll(pid)?.is_stopped() {
            return Ok(());
        }
    }

    tracing::warn!("process has taken too long to exit, sending SIGKILL",);
    if kill(pid, Signal::SIGKILL)?.is_stopped() {
        return Ok(());
    }

    tracing::trace!("waiting for process to exit");
    let end = Instant::now() + Duration::from_secs(1);
    while Instant::now() < end {
        std::thread::sleep(Duration::from_millis(25));
        if poll(pid)?.is_stopped() {
            return Ok(());
        }
    }

    tracing::error!("process has leaked");
    Err(nix::Error::UnknownErrno)
}

#[tracing::instrument(level = "trace", err(Debug))]
pub fn close_range(min: i32, max: Option<i32>) -> nix::Result<()> {
    use nix::libc;
    match unsafe {
        libc::syscall(
            libc::SYS_close_range,
            min,
            max.unwrap_or(libc::c_int::MAX),
            libc::CLOSE_RANGE_CLOEXEC,
        )
    } {
        0 => Ok(()),
        -1 => Err(nix::Error::last()),
        _ => Err(nix::Error::UnknownErrno),
    }
}

pub struct ChildProcess(Option<Pid>);

impl std::fmt::Debug for ChildProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(pid) = self.0 {
            pid.fmt(f)
        } else {
            f.debug_struct("Pid").finish()
        }
    }
}

impl ChildProcess {
    pub fn inner(&self) -> Pid {
        self.0.unwrap()
    }

    pub fn into_inner(mut self) -> Option<Pid> {
        self.0.take()
    }
}

impl From<Pid> for ChildProcess {
    fn from(value: Pid) -> Self {
        Self(Some(value))
    }
}

impl From<ChildProcess> for Pid {
    fn from(value: ChildProcess) -> Self {
        value.inner()
    }
}

impl Drop for ChildProcess {
    fn drop(&mut self) {
        if let Some(pid) = self.0.take() {
            kill_wait(pid).ok();
        }
    }
}

pub fn map_proc_err(e: procfs::ProcError) -> nix::Error {
    match e {
        procfs::ProcError::PermissionDenied(_) => nix::Error::EPERM,
        procfs::ProcError::NotFound(_) => nix::Error::ENOENT,
        procfs::ProcError::Incomplete(_) => nix::Error::EBADF,
        procfs::ProcError::Io(e, _) => nix::Error::from_i32(e.raw_os_error().unwrap_or_default()),
        procfs::ProcError::InternalError(procfs_error) => {
            tracing::error!(?procfs_error, "an internal procfs error occurred, please report it at https://github.com/eminence/procfs");
            nix::Error::UnknownErrno
        }
        procfs::ProcError::Other(other_error) => {
            tracing::error!(other_error, "an unspecified error occurred in procfs");
            nix::Error::UnknownErrno
        }
    }
}
