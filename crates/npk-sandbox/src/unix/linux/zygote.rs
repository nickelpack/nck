mod proto;

use std::{
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

use nix::{
    sched::CloneFlags,
    sys::stat::Mode,
    unistd::{ForkResult, Gid, Uid},
};
use npk_util::io::{timeout, wait_for_file, Buffer, TempDir};
pub use proto::*;

use crate::unix::{SOCKET_TIMEOUT, ZYGOTE_HEADER_SIZE};

use super::{
    syscall::{ChildProcess, Result, Syscall, TempMount},
    Config,
};

pub const SOCKET_NAME: &str = "zygote.socket";
const STACK_SIZE: usize = 1024 * 1024;

#[tracing::instrument(name = "zygote_main", level = "trace", skip_all)]
pub fn main<SC: Syscall + 'static>(cfg: super::Config) -> Result<()> {
    if let Err(error) = prctl::set_name("npk-zygote") {
        let error = nix::Error::from_i32(error);
        tracing::warn!(?error, "failed to set zygote process name");
    }

    let socket_path = cfg.runtime_dir.join(SOCKET_NAME);
    tracing::trace!(
        ?socket_path,
        "waiting for the controller socket to appear on the filesystem"
    );

    timeout(SOCKET_TIMEOUT, || wait_for_file(socket_path.as_path()))?;

    tracing::trace!(?socket_path, "connecting to controller");

    // TODO: This won't actually time out
    let mut socket = timeout(SOCKET_TIMEOUT, || {
        UnixStream::connect(socket_path.as_path())
    })?;

    tracing::info!("connected to controller");

    let mut read_buffer = Buffer::with_capacity(ZYGOTE_HEADER_SIZE);
    let mut write_buffer = Buffer::with_capacity(ZYGOTE_HEADER_SIZE);
    let mut bitcode_buffer = bitcode::Buffer::with_capacity(1024);
    let mut previous_pid = None::<ChildProcess<SC>>;
    loop {
        tracing::trace!("reading next request from controller");
        let request = match read_from_socket(&mut read_buffer, &mut bitcode_buffer, &mut socket) {
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                tracing::info!("controller closed the connection");
                break Ok(());
            }
            o => o,
        }?;

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

                let response = SpawnResponse::new(pid.inner(), sandbox_path, socket_path);

                tracing::trace!("writing response to socket");
                write_to_socket(
                    &mut write_buffer,
                    &mut bitcode_buffer,
                    &mut socket,
                    &response,
                )?;
                sandbox_dir.forget();

                previous_pid = Some(pid);
            }
        }
    }
}

#[tracing::instrument(level = "trace", skip_all, fields(name = req.name()))]
fn spawn_sandbox<SC: Syscall + 'static>(
    cfg: Config,
    req: SpawnRequest,
) -> Result<(ChildProcess<SC>, TempDir, PathBuf)> {
    tracing::trace!("allocating temporary directory");
    let sandbox_dir = TempDir::new_in(cfg.runtime_dir.as_path())?;
    let sandbox_path = sandbox_dir.as_path().to_path_buf();

    SC::chmod(sandbox_path.as_path(), Mode::from_bits_truncate(0o772)).inspect_err(|_| {
        SC::remove_dir_all(sandbox_path.as_path()).ok();
    })?;
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
            inner_main::<SC>(cloned_req, cloned_sandbox_path, cloned_socket_path)
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

    let pid = SC::clone::<STACK_SIZE>(cb, flags, None).inspect_err(|_| {
        SC::remove_dir_all(sandbox_path.as_path()).ok();
    })?;

    tracing::trace!(?pid, "created sandbox process from zygote");

    let pid: ChildProcess<SC> = pid.into();

    let mut mappings = super::syscall::Mappings::default();
    mappings
        .push_uid(Uid::from_raw(0), req.root_uid())
        .push_gid(Gid::from_raw(0), req.root_gid())
        .push_uid(Uid::from_raw(1000), req.user_uid())
        .push_gid(Gid::from_raw(1000), req.user_gid());

    tracing::trace!(?mappings, "applying requested user mappings");
    mappings.apply(Some(pid.inner()))?;

    Ok((pid, sandbox_dir, socket_path))
}

#[inline(always)]
pub fn inner_main<SC: Syscall + 'static>(
    req: SpawnRequest,
    sandbox_path: PathBuf,
    socket_path: PathBuf,
) -> isize {
    let rootfs_dir = match wait_for_controller::<SC>(sandbox_path.as_path(), socket_path.as_path())
    {
        Err(error) => {
            tracing::error!(?error, "failed to initialize child process");
            return -1;
        }
        Ok(result) => result,
    };

    match SC::fork() {
        Ok(ForkResult::Parent { child }) => super::supervisor::main(req.name(), child, rootfs_dir),
        Ok(ForkResult::Child) => super::result_to_isize_runtime(
            "sandbox",
            super::sandbox::main::<SC>(req.name(), socket_path, rootfs_dir.forget()),
        ),
        Err(error) => {
            tracing::error!(
                ?error,
                "failed to fork child process to supervisor and sandbox"
            );
            -1
        }
    }
}

#[inline(always)]
fn wait_for_controller<SC: Syscall + 'static>(
    sandbox_path: &Path,
    socket_path: &Path,
) -> Result<TempMount<SC>> {
    tracing::trace!("entering remaining namespaces");
    let flags = CloneFlags::CLONE_NEWPID
        | CloneFlags::CLONE_NEWUTS
        | CloneFlags::CLONE_NEWCGROUP
        | CloneFlags::CLONE_NEWIPC;
    SC::unshare(flags)?;

    tracing::trace!(
        ?socket_path,
        "waiting for the controller socket to appear on the filesystem"
    );
    timeout(Duration::from_secs(5), || wait_for_file(socket_path))?;

    // The zugote is in charge of newuidmap/newgidmap, so if the controller socket has appeared on the filesystem
    // it means that the mapping has occurred and the result has been received by the controller.
    tracing::trace!("becoming root");
    SC::set_id(Uid::from_raw(0), Gid::from_raw(0), Vec::default())?;

    tracing::trace!("creating rootfs directory");
    let temp = TempDir::new_in(sandbox_path)?;

    tracing::trace!("mounting rootfs directory as tmpfs");
    temp.try_into()
}
