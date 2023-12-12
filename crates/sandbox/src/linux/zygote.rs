use std::{
    ffi::{OsStr, OsString},
    io::{BufRead, BufReader, BufWriter, Error, ErrorKind, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

use nck_core::io::{wait_for_file, TempDir, Timeout};
use nix::{
    sched::CloneFlags,
    sys::stat::Mode,
    unistd::{ForkResult, Gid, Uid},
};

use super::{
    sandbox::SandboxArgs,
    syscall::{ChildProcess, Syscall, SyscallError, TempMount},
    SOCKET_TIMEOUT,
};

pub const SOCKET_NAME: &str = "zygote.socket";
const STACK_SIZE: usize = 1024 * 1024;

#[tracing::instrument(name = "zygote_main", level = "trace", skip_all)]
pub fn main<SC: Syscall + 'static>(cfg: crate::Settings) -> std::io::Result<()> {
    if let Err(error) = prctl::set_name("nck-zygote") {
        let error = nix::Error::from_i32(error);
        tracing::warn!(?error, "failed to set zygote process name");
    }

    let socket_path = cfg.tmp_directory.join(SOCKET_NAME);
    tracing::trace!(
        ?socket_path,
        "waiting for the controller socket to appear on the filesystem"
    );

    SOCKET_TIMEOUT.timeout(|| wait_for_file(socket_path.as_path()))?;

    tracing::trace!(?socket_path, "connecting to controller");

    // TODO: This won't actually time out
    let socket = SOCKET_TIMEOUT.timeout(|| UnixStream::connect(socket_path.as_path()))?;

    tracing::info!("connected to controller");

    let mut reader = BufReader::new(socket.try_clone().unwrap());
    let mut writer = BufWriter::new(socket);
    loop {
        let request = read_number(&mut reader, u16::from_be_bytes)?;
        match request {
            0 => {
                let uid = Uid::from_raw(read_number(&mut reader, u32::from_be_bytes)?);
                let gid = Gid::from_raw(read_number(&mut reader, u32::from_be_bytes)?);
                let formula_path = read_path(&mut reader)?;
                let result = spawn_sandbox::<SC>(cfg.clone(), formula_path, uid, gid);

                match result {
                    Ok((pid, work_dir, socket_path)) => {
                        writer.write_all(&0u16.to_be_bytes())?;
                        write_os_string(&mut writer, work_dir.forget().as_os_str())?;
                        write_os_string(&mut writer, socket_path.as_os_str())?;
                        writer.flush()?;
                        pid.into_inner();
                    }
                    Err(error) => {
                        tracing::error!(?error, "failed to created a new sandbox");
                        writer.write_all(&1u16.to_be_bytes())?;
                    }
                }
            }
            _ => {
                break Err(Error::from(ErrorKind::InvalidData));
            }
        }
    }
}

#[tracing::instrument(level = "trace", skip_all, fields(?path))]
fn spawn_sandbox<SC: Syscall + 'static>(
    cfg: crate::Settings,
    path: PathBuf,
    uid: Uid,
    gid: Gid,
) -> Result<(ChildProcess<SC>, TempDir, PathBuf), SyscallError> {
    tracing::trace!("allocating temporary directory");
    let sandbox_dir = TempDir::new_in(cfg.tmp_directory.as_path())?;
    let sandbox_path = sandbox_dir.as_path().to_path_buf();

    SC::chmod(sandbox_path.as_path(), Mode::from_bits_truncate(0o772)).inspect_err(|_| {
        SC::remove_dir_all(sandbox_path.as_path()).ok();
    })?;
    tracing::trace!(?sandbox_path, "temporary directory allocated");

    let mut socket_path = sandbox_path.clone();
    socket_path.set_extension("socket");

    let cloned_sandbox_path = sandbox_path.clone();
    let cloned_socket_path = socket_path.clone();
    let cloned_store_path = cfg.store_directory.clone();
    let cb = Box::new(move || {
        let sandbox_path = cloned_sandbox_path.clone();
        let socket_path = cloned_socket_path.clone();
        let store_path = cloned_store_path.clone();
        let formula_path = path.clone();

        // This thread has novel safety requirements, so abandon it as quickly as possible.
        let result = std::thread::spawn(move || {
            inner_main::<SC>(InnerArgs {
                formula_path,
                sandbox_path,
                socket_path,
                store_path,
            })
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
        .push_uid(Uid::from_raw(0), uid)
        .push_gid(Gid::from_raw(0), gid);

    tracing::trace!(?mappings, "applying requested user mappings");
    mappings.apply(Some(pid.inner()))?;

    Ok((pid, sandbox_dir, socket_path))
}

#[derive(Debug, Clone)]
pub struct InnerArgs {
    formula_path: PathBuf,
    sandbox_path: PathBuf,
    socket_path: PathBuf,
    store_path: PathBuf,
}

#[inline(always)]
pub fn inner_main<SC: Syscall + 'static>(args: InnerArgs) -> isize {
    let rootfs_dir =
        match wait_for_controller::<SC>(args.sandbox_path.as_path(), args.socket_path.as_path()) {
            Err(error) => {
                tracing::error!(?error, "failed to initialize child process");
                return -1;
            }
            Ok(result) => result,
        };

    match SC::fork() {
        Ok(ForkResult::Parent { child }) => {
            super::supervisor::main(args.formula_path, child, rootfs_dir)
        }
        Ok(ForkResult::Child) => super::result_to_isize_runtime(
            "sandbox",
            super::sandbox::sandbox_main::<SC>(SandboxArgs {
                formula_path: args.formula_path,
                socket_path: args.socket_path,
                rootfs_path: rootfs_dir.forget(),
                store_directory: args.store_path,
            }),
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
) -> Result<TempMount<SC>, SyscallError> {
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
    Duration::from_secs(5).timeout(|| wait_for_file(socket_path))?;

    // The zugote is in charge of newuidmap/newgidmap, so if the controller socket has appeared on the filesystem
    // it means that the mapping has occurred and the result has been received by the controller.
    tracing::trace!("becoming root");
    SC::set_id(Uid::from_raw(0), Gid::from_raw(0), Vec::default())?;

    tracing::trace!("creating rootfs directory");
    let temp = TempDir::new_in(sandbox_path)?;

    tracing::trace!("mounting rootfs directory as tmpfs");
    temp.try_into()
}

fn write_os_string(writer: &mut impl Write, os_str: &OsStr) -> std::io::Result<()> {
    let bytes = os_str.as_encoded_bytes();
    if bytes.len() > u16::MAX as usize {
        return Err(Error::from(ErrorKind::InvalidInput));
    }
    writer.write_all(&bytes.len().to_be_bytes())?;
    writer.write_all(bytes)?;
    Ok(())
}

fn read_number<T, const SIZE: usize>(
    reader: &mut impl BufRead,
    convert: impl FnOnce([u8; SIZE]) -> T,
) -> std::io::Result<T> {
    let mut buffer = [0u8; SIZE];
    reader.read_exact(&mut buffer)?;
    let value = convert(buffer);
    Ok(value)
}

fn read_path(reader: &mut impl BufRead) -> std::io::Result<PathBuf> {
    let len = read_number(reader, u16::from_be_bytes)? as usize;
    let mut buffer = vec![0u8; len];
    reader.read_exact(&mut buffer[..len])?;
    Ok(PathBuf::from(unsafe {
        OsString::from_encoded_bytes_unchecked(buffer)
    }))
}
