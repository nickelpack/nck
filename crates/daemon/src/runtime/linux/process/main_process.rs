use nix::sched::CloneFlags;

use crate::runtime::linux::fork;

pub enum ZygoteToMainMessage {}

pub fn main_process() -> anyhow::Result<()> {
    let (parent, child) = super::channel::unix_pair()?;
    let cb = Box::new(move || super::zygote_process::zygote_process(child.clone()));
    fork::clone(cb, CloneFlags::CLONE_NEWNS)?;
    Ok(())
}
