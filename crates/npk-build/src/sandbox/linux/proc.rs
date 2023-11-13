use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use derive_more::Display;
use nix::{
    errno::Errno,
    sched::CloneFlags,
    sys::{
        personality::{self, Persona},
        signal::Signal,
    },
    unistd::Pid,
};

#[derive(Debug, Clone, Display)]
#[display(fmt = "failed to set the hostname to {}", _0)]
pub struct SetHostnameFailure(String);

pub fn set_hostname(hostname: &str) -> Result<()> {
    nix::unistd::sethostname(hostname).with_context(|| SetHostnameFailure(hostname.to_string()))
}

#[derive(Debug, Clone, Display)]
#[display(fmt = "failed to get the current personality")]
pub struct GetPersonalityFailure;

pub fn get_personality() -> Result<Persona> {
    personality::get().with_context(|| GetPersonalityFailure)
}

#[derive(Debug, Clone, Display)]
#[display(fmt = "failed to set the current personality to 0x{:x}", _0)]
pub struct SetPersonalityFailure(i32);

pub fn set_personality(persona: Persona) -> Result<Persona> {
    personality::set(persona).with_context(|| SetPersonalityFailure(persona.bits()))
}

#[derive(Debug, Clone, Display)]
#[display(fmt = "failed to disable ASLR")]
pub struct DisableAslrFailure;

pub fn disable_aslr() -> Result<()> {
    let mut persona = get_personality().with_context(|| DisableAslrFailure)?;
    persona |= Persona::ADDR_NO_RANDOMIZE;
    set_personality(persona).with_context(|| DisableAslrFailure)?;
    Ok(())
}

#[derive(Debug, Clone, Display)]
#[display(fmt = "failed to set keep capabilities to {}", _0)]
pub struct SetKeepCapabilities(bool);

pub fn set_keep_capabilities(keep: bool) -> Result<()> {
    prctl::set_keep_capabilities(keep)
        .map_err(nix::errno::from_i32)
        .with_context(|| SetKeepCapabilities(keep))
}

#[derive(Debug, Clone, Display)]
#[display(fmt = "failed to clone the process")]
pub struct CloneFailure;

pub fn clone(clone_flags: CloneFlags, child_fun: impl FnMut() -> isize) -> Result<Pid> {
    const STACK_SIZE: usize = 4 * 1024 * 1024; // 4 MB
    let stack = vec![0u8; STACK_SIZE].into_boxed_slice();
    let stack = Box::leak(stack);

    unsafe { nix::sched::clone(Box::new(child_fun), stack, clone_flags, None) }
        .with_context(|| CloneFailure)
}

#[derive(Debug, Clone, Display)]
#[display(fmt = "failed to send {} to {}", _1, _0)]
pub struct KillFailure(Pid, Signal);

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
        Err(e) => Err(e).with_context(|| KillFailure(pid, signal)),
    }
}

pub fn poll(pid: Pid) -> Result<ProcessState> {
    if procfs::process::Process::new(pid.as_raw())?.is_alive() {
        Ok(ProcessState::Running)
    } else {
        Ok(ProcessState::Stopped)
    }
}

#[derive(Debug, Clone, Display)]
#[display(fmt = "failed to kill {}", _0)]
pub struct KillWaitFailure(Pid);

pub async fn kill_wait(pid: Pid) -> Result<()> {
    if kill(pid, Signal::SIGTERM)
        .with_context(|| KillWaitFailure(pid))?
        .is_stopped()
    {
        return Ok(());
    }

    tracing::info!("waiting for process {:?} to exit", pid);
    let end = Instant::now() + Duration::from_secs(2);
    while Instant::now() < end {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if poll(pid)
            .with_context(|| KillWaitFailure(pid))?
            .is_stopped()
        {
            return Ok(());
        }
    }

    tracing::warn!(
        "process {:?} has taken too long to exit, sending SIGKILL",
        pid
    );

    if kill(pid, Signal::SIGKILL)
        .with_context(|| KillWaitFailure(pid))?
        .is_stopped()
    {
        return Ok(());
    }

    tracing::info!("waiting for process {:?} to exit", pid);
    let end = Instant::now() + Duration::from_secs(1);
    while Instant::now() < end {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if poll(pid)
            .with_context(|| KillWaitFailure(pid))?
            .is_stopped()
        {
            return Ok(());
        }
    }

    Err(anyhow!("process has leaked").context(KillWaitFailure(pid)))
}
