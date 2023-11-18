use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use nix::{
    errno::Errno,
    sched::CloneFlags,
    sys::{
        personality::{self, Persona},
        signal::Signal,
    },
    unistd::Pid,
};

pub fn set_hostname(hostname: &str) -> Result<()> {
    nix::unistd::sethostname(hostname)
        .with_context(|| format!("while setting the hostname to {}", hostname))
}

pub fn get_personality() -> Result<Persona> {
    personality::get().with_context(|| "while getting the current personality")
}

pub fn set_personality(persona: Persona) -> Result<Persona> {
    personality::set(persona)
        .with_context(|| format!("while setting the personality to {:x}", persona.bits()))
}

pub fn disable_aslr() -> Result<()> {
    let mut persona = get_personality().with_context(|| "while disabling ASLR")?;
    persona |= Persona::ADDR_NO_RANDOMIZE;
    set_personality(persona).with_context(|| "while disabling ASLR")?;
    Ok(())
}

pub fn set_keep_capabilities(keep: bool) -> Result<()> {
    prctl::set_keep_capabilities(keep)
        .map_err(nix::errno::from_i32)
        .with_context(|| {
            format!(
                "while {} keep capabilities",
                if keep { "enabling" } else { "disabling" }
            )
        })
}

pub fn clone(mut child_fun: impl FnMut() -> isize, flags: CloneFlags) -> Result<Pid> {
    const STACK_SIZE: usize = 1 * 1024 * 1024;
    let mut stack = [0u8; STACK_SIZE];

    let result = unsafe { nix::sched::clone(Box::new(child_fun), &mut stack, flags, None) }
        .with_context(|| format!("while cloning the current process with {:x}", flags.bits()))?;
    Ok(result)
}

pub fn fork() -> Result<nix::unistd::ForkResult> {
    unsafe { nix::unistd::fork() }.with_context(|| "while forking the current process")
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

pub fn kill(pid: Pid, signal: Signal) -> Result<ProcessState> {
    match nix::sys::signal::kill(pid, signal) {
        Ok(_) => poll(pid),
        Err(Errno::ESRCH) => Ok(ProcessState::Stopped),
        Err(e) => Err(e.into()),
    }
    .with_context(|| format!("while sending {} to process {}", signal, pid))
}

pub fn poll(pid: Pid) -> Result<ProcessState> {
    let result = match procfs::process::Process::new(pid.as_raw()) {
        Ok(d) => Ok(d),
        Err(e) => match e {
            procfs::ProcError::PermissionDenied(_) => Err(Errno::EPERM),
            procfs::ProcError::NotFound(_) => return Ok(ProcessState::Stopped),
            procfs::ProcError::Incomplete(_) => Err(Errno::EBADF),
            procfs::ProcError::Io(e, _) => {
                Err(Errno::from_i32(e.raw_os_error().unwrap_or_default()))
            }
            _ => Err(Errno::UnknownErrno),
        },
    }
    .with_context(|| format!("while polling the state of process {}", pid))?;
    if result.is_alive() {
        Ok(ProcessState::Running)
    } else {
        Ok(ProcessState::Stopped)
    }
}

pub fn kill_wait(pid: Pid) -> Result<()> {
    if kill(pid, Signal::SIGTERM)
        .with_context(|| format!("while gracefully killing {}", pid))?
        .is_stopped()
    {
        return Ok(());
    }

    tracing::info!("waiting for process {:?} to exit", pid);
    let end = Instant::now() + Duration::from_secs(2);
    while Instant::now() < end {
        std::thread::sleep(Duration::from_millis(25));
        if poll(pid)?.is_stopped() {
            return Ok(());
        }
    }

    tracing::warn!(
        "process {:?} has taken too long to exit, sending SIGKILL",
        pid
    );

    if kill(pid, Signal::SIGKILL)
        .with_context(|| format!("while gracefully killing {}", pid))?
        .is_stopped()
    {
        return Ok(());
    }

    tracing::info!("waiting for process {:?} to exit", pid);
    let end = Instant::now() + Duration::from_secs(1);
    while Instant::now() < end {
        std::thread::sleep(Duration::from_millis(25));
        if poll(pid)?.is_stopped() {
            return Ok(());
        }
    }

    Err(anyhow::anyhow!("process {} has leaked", pid))
}

pub fn close_fds() -> Result<()> {
    use nix::libc;
    match unsafe {
        libc::syscall(
            libc::SYS_close_range,
            3,
            libc::c_int::MAX,
            libc::CLOSE_RANGE_CLOEXEC,
        )
    } {
        0 => Ok(()),
        -1 => Err(nix::errno::Errno::last()),
        _ => Err(nix::errno::Errno::UnknownErrno),
    }
    .with_context(|| "while closing all file descriptor in the current process")?;

    Ok(())
}
