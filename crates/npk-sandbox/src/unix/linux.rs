mod child;
mod controller;
mod fs;
mod main_proc;
mod parent;
mod proc;
pub mod proto;
mod user;
mod zygote;

use std::time::Duration;

use anyhow::{Context, Result};
use nix::{errno::Errno, sched::CloneFlags, unistd::Pid};
use tokio::{net::UnixListener, time::timeout};

pub use controller::*;
pub use main_proc::*;
pub use zygote::*;

use super::SandboxBuilder;

pub use parent::Sandbox;

fn errno_to_io(err: Errno) -> std::io::Error {
    std::io::Error::from_raw_os_error(err as i32)
}

pub(crate) fn apply_assumptions(builder: &mut SandboxBuilder) {
    if builder.hostname().is_some() {
        builder.set_networking_isolated(true);
    }
    if builder.uid().is_some()
        || builder.gid().is_some()
        || !builder.supplementary_gids().is_empty()
    {
        builder.set_users_isolated(true);
    }
    if builder.root().is_some() {
        builder.set_filesystem_isolated(true);
    }
}

pub(crate) async fn start(options: SandboxBuilder) -> Result<Sandbox> {
    let mut flags = CloneFlags::empty();

    if options.shared_filesystem {
        flags |= CloneFlags::CLONE_FS;
    }
    if !options.shared_resources {
        flags |= CloneFlags::CLONE_NEWCGROUP;
    }
    if !options.shared_ipc {
        flags |= CloneFlags::CLONE_NEWIPC;
    }
    if !options.shared_networking {
        flags |= CloneFlags::CLONE_NEWNET;
    }
    if !options.shared_users {
        flags |= CloneFlags::CLONE_NEWUSER;
    }
    if !options.shared_processes {
        flags |= CloneFlags::CLONE_NEWPID;
    }
    if options.propagate_signals {
        flags |= CloneFlags::CLONE_SIGHAND;
    }
    if options.propagate_file_descriptors {
        flags |= CloneFlags::CLONE_FILES;
    }
    if options.propagate_debugger {
        flags |= CloneFlags::CLONE_PTRACE;
    }
    if options.root().is_some() {
        flags |= CloneFlags::CLONE_NEWNS;
    }
    if options.hostname().is_some() {
        flags |= CloneFlags::CLONE_NEWUTS;
    }

    let working_dir = tempfile::tempdir()
        .with_context(|| "while creating a temporary directory for the sandbox infrastructure")?;
    let working_path = working_dir.path().to_path_buf();

    let socket_path = working_path.join("connect.socket");
    let listener = UnixListener::bind(socket_path.as_path()).with_context(|| {
        format!(
            "while binding to {:?} in order to accept the child connection",
            socket_path
        )
    })?;

    let (send_opts, receive_opts) = flume::bounded(1);
    let handle = tokio::runtime::Handle::current();
    send_opts
        .send((
            options,
            working_path.clone(),
            socket_path.clone(),
            handle.clone(),
        ))
        .unwrap();

    // We need to spawn it in a blocking thread because the thread will block forever in the child process.
    let pid = handle
        .spawn_blocking(move || {
            proc::clone(
                move || {
                    if let Ok((options, working_path, socket_path, handle)) = receive_opts.recv() {
                        // This thread has novel safety requirements, so abandon it as quickly as possible.
                        let result = std::thread::spawn(move || {
                            tokio::runtime::Builder::new_multi_thread()
                                .enable_all()
                                .build()
                                .unwrap()
                                .block_on(child::start(options, working_path, socket_path))
                        })
                        .join();
                        match result {
                            Ok(r) => r,
                            Err(e) => std::panic::resume_unwind(e),
                        }
                    } else {
                        -1
                    }
                },
                flags,
            )
            .with_context(|| "while cloning the current process in order to start the sandbox")
        })
        .await
        .with_context(|| "while executing cloning the child process")??;

    // Make sure to kill the child process if the socket doesn't connect.
    let pid = PidContainer(Some(pid));

    tracing::info!("waiting for child at {:?}", socket_path);
    let child_socket = timeout(Duration::from_secs(2), listener.accept())
        .await
        .with_context(|| "while accepting the child connection")?
        .with_context(|| "while accepting the child connection")?;

    // Make sure that send_opts is not dropped before the child starts

    tracing::info!("child connected to {:?}", socket_path);
    parent::start(pid, working_dir, child_socket.0, listener).await
}

struct PidContainer(Option<Pid>);

impl std::fmt::Debug for PidContainer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl Drop for PidContainer {
    fn drop(&mut self) {
        if let Some(pid) = self.0.take() {
            proc::kill_wait(pid).ok();
        }
    }
}

pub struct SandboxController {
    socket: UnixListener,
}

impl SandboxController {}
