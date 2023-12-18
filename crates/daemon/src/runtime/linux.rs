mod channel;
mod fork;
mod process;
mod user_ns;

pub use process::main_process::{Controller, PendingController, Sandbox};

pub fn create_controller() -> anyhow::Result<PendingController> {
    process::main_process::main_process()
}
