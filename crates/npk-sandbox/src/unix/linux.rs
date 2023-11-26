mod config;
mod controller;
pub mod proto;
mod sandbox;
mod supervisor;
mod zygote;

mod syscall;

use std::future::Future;

pub use config::*;
pub use controller::Sandbox;

use syscall::{Result, Syscall};

use self::syscall::NixSysCall;

pub type Controller = controller::Controller<NixSysCall>;

fn result_to_isize<R, E: std::fmt::Debug>(name: &str, result: std::result::Result<R, E>) -> isize {
    match result {
        Ok(_) => 0,
        Err(error) => {
            tracing::error!(?error, "{} failed", name);
            -1
        }
    }
}

async fn result_to_isize_async<F, R, E>(name: &str, result: F) -> isize
where
    F: Future<Output = std::result::Result<R, E>>,
    E: std::fmt::Debug,
{
    result_to_isize(name, result.await)
}

fn in_runtime<F>(future: F) -> std::io::Result<F::Output>
where
    F: Future,
{
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map(|runtime| runtime.block_on(future))
}

fn result_to_isize_runtime<F, R, E>(name: &str, result: F) -> isize
where
    F: Future<Output = std::result::Result<R, E>>,
    E: std::fmt::Debug,
{
    match in_runtime(result) {
        Ok(r) => result_to_isize(name, r),
        Err(error) => {
            tracing::error!(?error, "failed to spawn a runtime for {}", name);
            -1
        }
    }
}

#[tracing::instrument(name = "main", level = "trace", skip_all)]
pub fn main<F, R>(cfg: crate::Config, f: impl FnOnce(Controller) -> F) -> Option<R>
where
    F: std::future::Future<Output = R>,
{
    main_impl::<NixSysCall, _, _>(cfg.linux, f)
        .inspect_err(|error| tracing::error!(?error, "linux runtime failed"))
        .unwrap_or(None)
}

#[inline(always)]
fn main_impl<SC: Syscall + 'static, F, R>(
    cfg: Config,
    f: impl FnOnce(controller::Controller<SC>) -> F,
) -> Result<Option<R>>
where
    F: std::future::Future<Output = R>,
{
    tracing::trace!(working_dir = ?cfg.runtime_dir, "creating working directory");
    SC::create_dir_all(cfg.runtime_dir.as_path())?;

    tracing::trace!("ensuring that child processes retain capabilities");
    SC::set_keep_capabilities(true)?;

    tracing::trace!("forking to controller and zygote");
    match SC::fork()? {
        nix::unistd::ForkResult::Parent { child } => {
            in_runtime(controller::main(cfg, child.into(), f))?.map(Some)
        }
        nix::unistd::ForkResult::Child => {
            zygote::main::<SC>(cfg)?;
            Ok(None)
        }
    }
}
