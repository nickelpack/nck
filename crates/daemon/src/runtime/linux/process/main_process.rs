use nix::sched::CloneFlags;

use crate::runtime::linux::fork;

pub fn main_process() -> anyhow::Result<()> {
    let (parent, child) = super::channel::unix_pair()?;
    let cb = {
        let child = child.clone();
        Box::new(move || super::zygote_process::zygote_process(child))
    };
    fork::container_clone(cb, CloneFlags::CLONE_NEWNS)?;
    Ok(())
}
