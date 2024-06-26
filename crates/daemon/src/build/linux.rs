mod fork;
mod fs;
mod io;
mod proc;
mod process;
mod rootfs;
mod shiftfs;
mod user_ns;

use crate::settings::Settings;
pub use process::main_process::{Controller, PendingController, Sandbox};

pub fn create_controller(config: Settings) -> anyhow::Result<PendingController> {
    process::main_process::main_process(config.daemon)
}
