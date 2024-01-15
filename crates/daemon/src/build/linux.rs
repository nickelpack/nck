mod channel;
mod fork;
mod fs;
mod process;
mod user_ns;

pub use process::main_process::{Controller, PendingController, Sandbox};

use crate::settings::Settings;

pub fn create_controller(config: Settings) -> anyhow::Result<PendingController> {
    process::main_process::main_process(config.store, config.daemon)
}
