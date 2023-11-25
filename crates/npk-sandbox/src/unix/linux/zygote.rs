mod proto;

use std::{
    fs::Permissions,
    os::unix::{net::UnixStream, prelude::PermissionsExt},
    path::PathBuf,
};

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

    let socket_path = cfg.runtime_dir.join(SOCKET_NAME);
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

                let (pid, sandbox_dir, socket_path) = spawn_sandbox(cfg.clone(), request)?;
                let sandbox_path = sandbox_dir.as_path().to_path_buf();

                tracing::trace!(?pid, ?sandbox_path, ?socket_path, "spawned sandbox process");

                let response = SpawnResponse {
                    pid: pid.inner().as_raw(),
                    sandbox_path,
                    socket_path,
                };

                tracing::trace!("writing response to socket");
                write_to_socket(&mut write_buffer, &mut socket, &response)
                    .map_err(super::std_error_to_nix)?;
                sandbox_dir.forget();

                previous_pid = Some(pid);
            }
        }
    }
}

#[tracing::instrument(level = "trace", skip_all, fields(name = req.name), err(Debug))]
fn spawn_sandbox(cfg: Config, req: SpawnRequest) -> nix::Result<(ChildProcess, TempDir, PathBuf)> {
    tracing::trace!("allocating temporary directory");
    let sandbox_dir =
        TempDir::new_in(cfg.runtime_dir.as_path()).map_err(super::std_error_to_nix)?;
    let sandbox_path = sandbox_dir.as_path().to_path_buf();

    std::fs::set_permissions(sandbox_path.as_path(), Permissions::from_mode(0o772))
        .inspect_err(|_| {
            std::fs::remove_dir_all(sandbox_path.as_path()).ok();
        })
        .map_err(super::std_error_to_nix)?;
    tracing::trace!(?sandbox_path, "temporary directory allocated");

    let mut socket_path = sandbox_path.clone();
    socket_path.set_extension("socket");

    let cloned_sandbox_path = sandbox_path.clone();
    let cloned_socket_path = socket_path.clone();
    let cloned_req = req.clone();
    let cb = Box::new(move || {
        let cloned_sandbox_path = cloned_sandbox_path.clone();
        let cloned_socket_path = cloned_socket_path.clone();
        let cloned_req = cloned_req.clone();

        // This thread has novel safety requirements, so abandon it as quickly as possible.
        let result = std::thread::spawn(move || {
            super::child::main(cloned_req, cloned_sandbox_path, cloned_socket_path)
        })
        .join();

        if let Err(e) = result {
            std::panic::resume_unwind(e)
        } else {
            0
        }
    });

    let flags = CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNS;

    tracing::trace!("cloning current process to sandbox process");

    const STACK_SIZE: usize = 1024 * 1024;
    let mut stack = [0u8; STACK_SIZE];
    let pid = unsafe { clone(cb, &mut stack, flags, None) }.inspect_err(|_| {
        std::fs::remove_dir_all(sandbox_path.as_path()).ok();
    })?;

    tracing::trace!(?pid, "created sandbox process from zygote");

    let pid: ChildProcess = pid.into();

    let mut mappings = super::Mappings::default();
    mappings
        .push_uid_range(0, req.root_uid..=(req.root_uid + 1))
        .unwrap();
    mappings
        .push_gid_range(0, req.root_gid..=(req.root_gid + 1))
        .unwrap();
    mappings
        .push_uid_range(1000, req.user_uid..=(req.user_uid + 1))
        .unwrap();
    mappings
        .push_gid_range(1000, req.user_gid..=(req.user_gid + 1))
        .unwrap();

    tracing::trace!(?mappings, "applying requested user mappings");
    mappings.apply(Some(pid.inner()))?;

    Ok((pid, sandbox_dir, socket_path))
}
