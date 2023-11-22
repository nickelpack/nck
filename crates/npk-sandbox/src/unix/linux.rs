mod child;
mod controller;
mod fs;
mod proc;
pub mod proto;
mod user;
mod zygote;

use std::{path::PathBuf, process::ExitCode};

use anyhow::{Context, Result};

pub use controller::*;
pub use user::Mappings;

#[derive(Debug, Clone)]
pub struct Config {
    pub working_dir: PathBuf,
    pub mappings: Mappings,
}

fn errno_to_stdio_err(err: nix::errno::Errno) -> std::io::Error {
    std::io::Error::from_raw_os_error(err as i32)
}

pub fn main<F>(cfg: Config, f: impl FnOnce(controller::Controller) -> F) -> ExitCode
where
    F: std::future::Future<Output = Result<()>>,
{
    pub fn imp<FF>(cfg: Config, f: impl FnOnce(controller::Controller) -> FF) -> Result<()>
    where
        FF: std::future::Future<Output = Result<()>>,
    {
        std::fs::create_dir_all(cfg.working_dir.as_path()).with_context(|| {
            format!("while creating the working directory {:?}", cfg.working_dir)
        })?;

        proc::set_keep_capabilities(true).with_context(|| "while retaining capabilities")?;

        match proc::fork().with_context(|| "while forking zygote")? {
            nix::unistd::ForkResult::Parent { child } => controller::main(cfg, child.into(), f),
            nix::unistd::ForkResult::Child => zygote::main(cfg),
        }
    }

    if let Err(e) = imp(cfg, f) {
        eprintln!("failed: {:?}", e);
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
