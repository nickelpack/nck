mod child;
mod config;
mod controller;
mod fs;
mod proc;
pub mod proto;
mod user;
mod zygote;

use std::{fs::Permissions, os::unix::prelude::PermissionsExt};

pub use config::*;
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

pub fn main<F, R>(cfg: crate::Config, f: impl FnOnce(controller::Controller) -> F) -> Option<R>
where
    F: std::future::Future<Output = R>,
{
    #[tracing::instrument(name = "linux_main", level = "trace", skip_all, err(Debug))]
    pub fn imp<FF, RR>(
        cfg: Config,
        f: impl FnOnce(controller::Controller) -> FF,
    ) -> nix::Result<Option<RR>>
    where
        FF: std::future::Future<Output = RR>,
    {
        tracing::trace!(working_dir = ?cfg.runtime_dir, "creating working directory");
        std::fs::create_dir_all(cfg.runtime_dir.as_path()).map_err(std_error_to_nix)?;

        tracing::trace!("ensuring that child processes retain capabilities");
        prctl::set_keep_capabilities(true).map_err(nix::Error::from_i32)?;

        tracing::trace!("forking to controller and zygote");
        match unsafe { fork() }? {
            nix::unistd::ForkResult::Parent { child } => {
                controller::main(cfg, child.into(), f).map(Some)
            }
            nix::unistd::ForkResult::Child => {
                zygote::main(cfg)?;
                Ok(None)
            }
        }
    }

    imp(cfg.linux, f).unwrap()
}
