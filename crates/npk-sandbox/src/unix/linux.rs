mod child;
mod controller;
mod fs;
mod proc;
pub mod proto;
mod user;
mod zygote;

use std::path::PathBuf;

pub use controller::*;
use nix::unistd::fork;
pub use user::Mappings;

const NIX_NONE: Option<&[u8]> = None::<&[u8]>;

pub fn std_error_to_nix(error: std::io::Error) -> nix::Error {
    nix::Error::from_i32(error.raw_os_error().unwrap_or_else(|| {
        tracing::error!(?error, "unknown IO error");
        0
    }))
}

#[derive(Debug, Clone)]
pub struct Config {
    pub working_dir: PathBuf,
    pub mappings: Mappings,
}

pub fn main<F, R>(cfg: Config, default_result: R, f: impl FnOnce(controller::Controller) -> F) -> R
where
    F: std::future::Future<Output = R>,
{
    #[tracing::instrument(name = "linux_main", level = "trace", skip_all, err(Debug))]
    pub fn imp<FF, RR>(
        cfg: Config,
        default_result: RR,
        f: impl FnOnce(controller::Controller) -> FF,
    ) -> nix::Result<RR>
    where
        FF: std::future::Future<Output = RR>,
    {
        std::fs::create_dir_all(cfg.working_dir.as_path()).map_err(std_error_to_nix)?;

        prctl::set_keep_capabilities(true).map_err(nix::Error::from_i32)?;

        match unsafe { fork() }? {
            nix::unistd::ForkResult::Parent { child } => controller::main(cfg, child.into(), f),
            nix::unistd::ForkResult::Child => {
                zygote::main(cfg)?;
                Ok(default_result)
            }
        }
    }

    imp(cfg, default_result, f).unwrap()
}
