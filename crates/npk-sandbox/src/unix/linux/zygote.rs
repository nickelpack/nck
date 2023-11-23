mod proto;

use std::{os::unix::net::UnixStream, path::PathBuf};

use nix::sched::{clone, CloneFlags};
use npk_util::io::{timeout, wait_for_file, Buffer, TempDir};
pub use proto::*;

use crate::unix::{linux::proc::ChildProcess, SOCKET_TIMEOUT, ZYGOTE_HEADER_SIZE};

use super::Config;

pub const SOCKET_NAME: &str = "zygote.socket";

#[tracing::instrument(name = "zygote_main", level = "trace", skip_all, err(Debug))]
pub fn main(cfg: super::Config) -> nix::Result<()> {
    if let Err(error) = prctl::set_name("npk-zygote") {
        let error = nix::Error::from_i32(error);
        tracing::warn!(?error, "failed to set zygote process name");
    }

    let socket_path = cfg.working_dir.join(SOCKET_NAME);
    tracing::trace!(
        ?socket_path,
        "waiting for the controller socket to appear on the filesystem"
    );

    timeout(SOCKET_TIMEOUT, || wait_for_file(socket_path.as_path()))
        .map_err(super::std_error_to_nix)?;

    tracing::trace!(?socket_path, "connecting to controller");

    // TODO: This won't actually time out
    let mut socket = timeout(SOCKET_TIMEOUT, || {
        UnixStream::connect(socket_path.as_path())
    })
    .map_err(super::std_error_to_nix)?;

    tracing::info!(?socket_path, "connected to controller");

    let mut read_buffer = Buffer::with_capacity(ZYGOTE_HEADER_SIZE);
    let mut write_buffer = Buffer::with_capacity(ZYGOTE_HEADER_SIZE);
    let mut previous_pid = None::<ChildProcess>;
    loop {
        tracing::trace!("reading next request from controller");
        let request = match read_from_socket(&mut read_buffer, &mut socket) {
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                tracing::info!("controller closed the connection");
                break Ok(());
            }
            o => o,
        }
        .map_err(super::std_error_to_nix)?;

        if let Some(pid) = previous_pid.take() {
            // Closing the child is now the controller's problem.
            pid.into_inner();
        }

        match request {
            Request::Spawn(request) => {
                tracing::debug!(?request, "received spawn request");

                let (pid, sandbox_path, socket_path, rootfs_path) =
                    spawn_sandbox(cfg.clone(), request)?;

                tracing::trace!(
                    ?pid,
                    ?sandbox_path,
                    ?socket_path,
                    ?rootfs_path,
                    "spawned sandbox process"
                );

                let response = SpawnResponse {
                    pid: pid.inner().as_raw(),
                    sandbox_path,
                    socket_path,
                    rootfs_path,
                };

                tracing::trace!("writing response to socket");
                write_to_socket(&mut write_buffer, &mut socket, &response)
                    .map_err(super::std_error_to_nix)?;

                previous_pid = Some(pid);
            }
        }
    }
}

#[tracing::instrument(level = "trace", skip_all, err(Debug))]
fn spawn_sandbox(
    cfg: Config,
    req: SpawnRequest,
) -> nix::Result<(ChildProcess, PathBuf, PathBuf, PathBuf)> {
    tracing::trace!("allocating temporary directory");
    let sandbox_path = TempDir::new_in(cfg.working_dir.as_path())
        .map_err(super::std_error_to_nix)?
        .forget();
    tracing::trace!(?sandbox_path, "temporary directory allocated");

    let mut socket_path = sandbox_path.clone();
    socket_path.set_extension("socket");

    let rootfs_path = sandbox_path.join("rootfs");

    let cloned_sandbox_path = sandbox_path.clone();
    let cloned_socket_path = socket_path.clone();
    let cloned_rootfs_path = rootfs_path.clone();
    let cloned_req = req.clone();
    let cb = Box::new(move || {
        let cloned_sandbox_path = cloned_sandbox_path.clone();
        let cloned_socket_path = cloned_socket_path.clone();
        let cloned_rootfs_path = cloned_rootfs_path.clone();
        let cloned_req = cloned_req.clone();

        // This thread has novel safety requirements, so abandon it as quickly as possible.
        let result = std::thread::spawn(move || {
            super::proc::close_range(3, None).unwrap();
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(super::child::main(
                    cloned_req,
                    cloned_sandbox_path,
                    cloned_socket_path,
                    cloned_rootfs_path,
                ))
        })
        .join();

        if let Err(e) = result {
            std::panic::resume_unwind(e)
        } else {
            0
        }
    });

    let flags = CloneFlags::CLONE_NEWPID
        | CloneFlags::CLONE_NEWUSER
        | CloneFlags::CLONE_NEWNS
        | CloneFlags::CLONE_NEWUTS
        | CloneFlags::CLONE_NEWCGROUP
        | CloneFlags::CLONE_NEWIPC;

    tracing::trace!("cloning current process to sandbox process");

    const STACK_SIZE: usize = 1024 * 1024;
    let mut stack = [0u8; STACK_SIZE];
    let pid = unsafe { clone(cb, &mut stack, flags, None) }.inspect_err(|_| {
        std::fs::remove_dir_all(sandbox_path.as_path()).ok();
    })?;

    tracing::debug!(?pid, "created sandbox process from zygote");

    let pid: ChildProcess = pid.into();

    tracing::debug!(?cfg.mappings, "applying requested user mappings");
    cfg.mappings.apply(Some(pid.inner()))?;

    Ok((pid, sandbox_path, socket_path, rootfs_path))
}
