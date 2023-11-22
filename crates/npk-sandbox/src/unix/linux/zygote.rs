mod proto;

use std::{
    io::Write,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use nix::{
    sched::CloneFlags,
    unistd::{Gid, Uid},
};
use npk_util::io::{timeout, wait_for_file, Buffer, TempDir};
pub use proto::*;

use crate::unix::{linux::proc::ChildProcess, SOCKET_TIMEOUT, ZYGOTE_HEADER_SIZE};

use super::Config;

pub const SOCKET_NAME: &str = "zygote.socket";

pub fn main(cfg: super::Config) -> Result<()> {
    let uid = nix::unistd::getuid();
    let gid = nix::unistd::getgid();

    // Required to clone NEWPID.
    // super::proc::unshare(CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNS)
    //     .with_context(|| "while initializing zygote")?;

    let socket_path = cfg.working_dir.join(SOCKET_NAME);
    timeout(SOCKET_TIMEOUT, || wait_for_file(socket_path.as_path()))
        .with_context(|| "while waiting for the controller socket to appear on the filesystem")?;

    tracing::info!("connecting to controller at {:?}", socket_path.as_path());

    // TODO: This won't actually time out
    let mut socket = timeout(SOCKET_TIMEOUT, || {
        UnixStream::connect(socket_path.as_path())
    })
    .with_context(|| "while connecting to the controller socket")?;

    tracing::info!("connected to controller");

    // super::user::map_direct(uid, gid, false)?;

    // super::user::set_id(uid, gid, Vec::default())
    //     .with_context(|| "while updating the user and group ids")?;

    let mut read_buffer = Buffer::with_capacity(ZYGOTE_HEADER_SIZE);
    let mut write_buffer = Buffer::with_capacity(ZYGOTE_HEADER_SIZE);
    let mut previous_pid = None::<ChildProcess>;
    loop {
        let request = read_from_socket(&mut read_buffer, &mut socket)?;

        if let Some(pid) = previous_pid.take() {
            // Closing the child is now the controller's problem.
            pid.into_inner();
        }

        match request {
            Request::Spawn(req) => {
                tracing::debug!("received spawn request");

                let (pid, working_path, socket_path, rootfs_path) =
                    spawn_sandbox(cfg.clone(), req)?;

                let response = SpawnResponse {
                    pid: pid.inner().as_raw(),
                    sandbox_path: working_path,
                    socket_path,
                    rootfs_path,
                };

                tracing::debug!("writing response to socket");
                write_to_socket(&mut write_buffer, &mut socket, &response)?;

                previous_pid = Some(pid);
            }
        }
    }
}

fn spawn_sandbox(
    cfg: Config,
    req: SpawnRequest,
) -> Result<(ChildProcess, PathBuf, PathBuf, PathBuf)> {
    let sandbox_path = TempDir::new_in(cfg.working_dir.as_path())
        .with_context(|| "while creating a temporary directory for the sandbox infrastructure")?
        .forget();
    let mut socket_path = sandbox_path.clone();
    socket_path.set_extension("socket");

    let rootfs_path = sandbox_path.join("rootfs");

    let cloned_sandbox_path = sandbox_path.clone();
    let cloned_socket_path = socket_path.clone();
    let cloned_rootfs_path = rootfs_path.clone();
    let cloned_req = req.clone();
    let pid = super::proc::clone(
        move || {
            let cloned_sandbox_path = cloned_sandbox_path.clone();
            let cloned_socket_path = cloned_socket_path.clone();
            let cloned_rootfs_path = cloned_rootfs_path.clone();
            let cloned_req = cloned_req.clone();

            // super::proc::unshare(
            //     CloneFlags::CLONE_NEWUSER
            //         | CloneFlags::CLONE_NEWNS
            //         | CloneFlags::CLONE_NEWCGROUP
            //         | CloneFlags::CLONE_NEWUTS
            //         | CloneFlags::CLONE_NEWIPC,
            // )
            // .with_context(|| "while unsharing sandbox")
            // .unwrap();

            // This thread has novel safety requirements, so abandon it as quickly as possible.
            let result = std::thread::spawn(move || {
                super::proc::close_fds().unwrap();
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
        },
        CloneFlags::CLONE_NEWPID
            | CloneFlags::CLONE_NEWUSER
            | CloneFlags::CLONE_NEWNS
            | CloneFlags::CLONE_NEWUTS
            | CloneFlags::CLONE_NEWCGROUP
            | CloneFlags::CLONE_NEWIPC,
    )
    .inspect_err(|_| {
        std::fs::remove_dir_all(sandbox_path.as_path()).ok();
    })
    .with_context(|| "while cloning the zygote into a sandbox process")?;

    tracing::trace!("created sandbox process from zygote {:?}", pid);

    let pid: ChildProcess = pid.into();

    // Required to clone NEWUSER.
    super::user::Mappings::default()
        .push_uid_range(0, 165537..=165538)?
        .push_gid_range(0, 165537..=165538)?
        .push_uid_range(1000, 165538..=166539)?
        .push_gid_range(1000, 165538..=165539)?
        .apply(Some(pid.inner()))
        .with_context(|| "while mapping zygote subuids and subgids")?;

    Ok((pid, sandbox_path, socket_path, rootfs_path))
}
