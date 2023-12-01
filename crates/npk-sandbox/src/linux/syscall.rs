use std::{
    ffi::OsStr,
    fs::Permissions,
    io::{ErrorKind, Write},
    marker::PhantomData,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{fs::symlink, prelude::PermissionsExt},
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use nix::{
    errno::Errno,
    fcntl::{open, OFlag},
    mount::{MntFlags, MsFlags},
    sched::CloneFlags,
    sys::{
        personality::{self, Persona},
        signal::Signal,
        stat::{makedev, Mode, SFlag},
    },
    unistd::{fchdir, sethostname, setresgid, setresuid, ForkResult, Gid, Pid, Uid},
};
use npk_util::io::TempDir;
use thiserror::Error;

pub const SYS_NONE: Option<&Path> = None::<&Path>;

const BIND: &[u8] = b"bind";
const PROC: &[u8] = b"proc";
const SYSFS: &[u8] = b"sysfs";
const TMPFS: &[u8] = b"tmpfs";
const DEVPTS: &[u8] = b"devpts";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountType {
    Bind,
    Proc,
    SysFs,
    TmpFs,
    DevPts,
}

impl AsRef<OsStr> for MountType {
    #[inline]
    fn as_ref(&self) -> &OsStr {
        let cstr = match self {
            MountType::Bind => BIND,
            MountType::Proc => PROC,
            MountType::SysFs => SYSFS,
            MountType::TmpFs => TMPFS,
            MountType::DevPts => DEVPTS,
        };
        unsafe { OsStr::from_encoded_bytes_unchecked(cstr) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceType {
    Null,
    Zero,
    Full,
    Random,
    URandom,
    Tty,
    Ptmx,
}

impl From<DeviceType> for SFlag {
    #[inline]
    fn from(value: DeviceType) -> Self {
        match value {
            DeviceType::Null
            | DeviceType::Zero
            | DeviceType::Full
            | DeviceType::Random
            | DeviceType::URandom
            | DeviceType::Tty
            | DeviceType::Ptmx => SFlag::S_IFCHR,
        }
    }
}

impl From<DeviceType> for u64 {
    #[inline]
    fn from(value: DeviceType) -> Self {
        match value {
            DeviceType::Null => makedev(1, 3),
            DeviceType::Zero => makedev(1, 5),
            DeviceType::Full => makedev(1, 7),
            DeviceType::Random => makedev(1, 8),
            DeviceType::URandom => makedev(1, 9),
            DeviceType::Tty => makedev(5, 0),
            DeviceType::Ptmx => makedev(5, 2),
        }
    }
}

#[derive(Debug, Error)]
pub enum SyscallError {
    #[error("an OS error occurred: {:?}", _0)]
    OsError(nix::Error),
    #[error("an IO error occurred: {:?}", _0)]
    IoError(#[from] std::io::Error),
    #[error("an unknown error occurred: {}", _0)]
    Other(String),
}

impl SyscallError {
    pub fn errno(&self) -> Option<Errno> {
        match self {
            SyscallError::OsError(e) => Some(*e),
            SyscallError::IoError(e) => e.raw_os_error().map(Errno::from_i32),
            SyscallError::Other(_) => None,
        }
    }
}

impl From<nix::Error> for SyscallError {
    fn from(value: nix::Error) -> Self {
        let error = std::io::Error::from_raw_os_error(value as i32);
        match error.kind() {
            std::io::ErrorKind::Other => Self::OsError(value),
            _ => Self::IoError(error),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    Running,
    Stopped,
    NotFound,
}

impl ProcessState {
    #[inline]
    pub fn is_stopped(&self) -> bool {
        matches!(self, ProcessState::Stopped | ProcessState::NotFound)
    }
}

pub type Result<T> = std::result::Result<T, SyscallError>;

pub trait Syscall: Send + Sync {
    fn bind(src: impl AsRef<Path>, dest: impl AsRef<Path>) -> Result<()>;
    fn pivot(new_root: impl AsRef<Path>) -> Result<()>;
    fn mount<P1: AsRef<Path>, P2: AsRef<OsStr>, P3: AsRef<OsStr>>(
        source: Option<P1>,
        target: impl AsRef<Path>,
        fstype: Option<P2>,
        flags: MsFlags,
        data: Option<P3>,
    ) -> Result<()>;
    fn unmount(path: impl AsRef<Path>, flags: MntFlags) -> Result<()>;
    fn change_personality(f: impl FnOnce(Persona) -> Persona) -> Result<()>;
    fn kill(pid: Pid, signal: Signal) -> Result<ProcessState>;
    fn poll(pid: Pid) -> Result<ProcessState>;
    fn kill_wait(pid: Pid, timeout: Duration) -> Result<()>;
    fn close_range(min: i32, max: Option<i32>) -> Result<()>;
    fn set_id(uid: Uid, gid: Gid, supplementary: impl AsRef<[Gid]>) -> Result<()>;
    fn unshare(flags: CloneFlags) -> Result<()>;
    fn fork() -> Result<ForkResult>;
    fn clone<const STACK_SIZE: usize>(
        cb: impl FnMut() -> isize,
        flags: CloneFlags,
        signal: Option<i32>,
    ) -> Result<Pid>;
    fn chmod(path: impl AsRef<Path>, mode: Mode) -> Result<()>;
    fn remove_dir_all(path: impl AsRef<Path>) -> Result<()>;
    fn create_dir_all(path: impl AsRef<Path>) -> Result<()>;
    fn create_dir(path: impl AsRef<Path>) -> Result<()>;
    fn touch(path: impl AsRef<Path>) -> Result<()>;
    fn append(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> Result<()>;
    fn overwrite(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> Result<()>;
    fn set_keep_capabilities(keep: bool) -> Result<()>;
    fn remove_file(path: impl AsRef<Path>) -> Result<()>;
    fn set_hostname(hostname: impl AsRef<OsStr>) -> Result<()>;
    fn symlink(src: impl AsRef<Path>, dest: impl AsRef<Path>) -> Result<()>;
}

#[derive(Debug, Clone, Copy)]
pub struct NixSysCall;

impl Syscall for NixSysCall {
    #[tracing::instrument(level = "trace", skip_all, fields(src = ?src.as_ref(), dest = ?dest.as_ref()))]
    fn bind(src: impl AsRef<Path>, dest: impl AsRef<Path>) -> Result<()> {
        let src = src.as_ref();
        let dest = dest.as_ref();

        tracing::trace!(?src, ?dest, "creating bind mount");

        if let Some(parent) = dest.parent() {
            if !parent.exists() {
                tracing::trace!(?parent, "creating parent directory");
                Self::create_dir_all(parent)?;
            }
        }

        if src.is_dir() {
            if !dest.is_dir() {
                tracing::trace!(?dest, "creating target directory");
                Self::create_dir(dest)?;
            }
        } else if !dest.is_file() {
            tracing::trace!(?dest, "creating target file");
            Self::touch(dest)?;
        }

        Self::mount(
            Some(src),
            dest,
            Some(MountType::Bind),
            MsFlags::MS_REC | MsFlags::MS_BIND,
            SYS_NONE,
        )
    }

    #[tracing::instrument(level = "trace", skip_all, fields(new_root = ?new_root.as_ref()))]
    fn pivot(new_root: impl AsRef<Path>) -> Result<()> {
        let new_root = new_root.as_ref();

        tracing::trace!(?new_root, "pivoting to new root");

        tracing::trace!("creating an explicit private mount at the new root");
        Self::mount(
            Some(new_root),
            new_root,
            SYS_NONE,
            MsFlags::MS_PRIVATE | MsFlags::MS_BIND | MsFlags::MS_REC,
            SYS_NONE,
        )?;

        tracing::trace!("opening a fd at the new root");
        let newroot = open(
            new_root,
            OFlag::O_DIRECTORY | OFlag::O_RDONLY,
            Mode::empty(),
        )
        .map(|v| unsafe { OwnedFd::from_raw_fd(v) })?;

        tracing::trace!("pivoting to the new root");
        // pivot root usually changes the root directory to first argument, and then mounts the original root directory at
        // second argument. Giving same path for both stacks mapping of the original root directory above the new directory
        // at the same path, then the call to umount unmounts the original root directory from this path.
        nix::unistd::pivot_root(new_root, new_root)?;

        tracing::trace!("making the new root a recursive slave mount");
        Self::mount(
            SYS_NONE,
            "/",
            SYS_NONE,
            MsFlags::MS_SLAVE | MsFlags::MS_REC,
            SYS_NONE,
        )?;

        tracing::trace!("unmounting the old root");
        Self::unmount("/", MntFlags::MNT_DETACH)?;

        tracing::trace!("changing directory to the new root");
        fchdir(newroot.as_raw_fd())?;
        Ok(())
    }

    #[inline]
    #[tracing::instrument(level = "trace", skip_all, fields(target = ?target.as_ref()))]
    fn mount<P1: AsRef<Path>, P2: AsRef<OsStr>, P3: AsRef<OsStr>>(
        source: Option<P1>,
        target: impl AsRef<Path>,
        fstype: Option<P2>,
        flags: MsFlags,
        data: Option<P3>,
    ) -> Result<()> {
        let source = source.as_ref().map(|f| f.as_ref());
        let target = target.as_ref();
        let fstype = fstype.as_ref().map(|f| f.as_ref());
        let data = data.as_ref().map(|f| f.as_ref());
        nix::mount::mount(source, target, fstype, flags, data)?;
        Ok(())
    }

    #[inline]
    #[tracing::instrument(level = "trace", skip_all, fields(path = ?path.as_ref(), ?flags))]
    fn unmount(path: impl AsRef<Path>, flags: MntFlags) -> Result<()> {
        let path = path.as_ref();
        nix::mount::umount2(path, flags)?;
        Ok(())
    }

    #[inline]
    #[tracing::instrument(level = "trace", skip_all)]
    fn change_personality(f: impl FnOnce(Persona) -> Persona) -> Result<()> {
        let mut persona = personality::get()?;
        tracing::trace!(?persona, "got existing persona");
        persona = f(persona);
        tracing::trace!(?persona, "setting persona");
        personality::set(persona)?;
        Ok(())
    }

    #[inline]
    #[tracing::instrument(level = "trace", fields(?pid, ?signal))]
    fn kill(pid: Pid, signal: Signal) -> Result<ProcessState> {
        match nix::sys::signal::kill(pid, signal) {
            Ok(_) => Self::poll(pid),
            Err(Errno::ESRCH) => Ok(ProcessState::NotFound),
            Err(e) => Err(e.into()),
        }
    }

    #[tracing::instrument(level = "trace", fields(?pid))]
    fn kill_wait(pid: Pid, timeout: Duration) -> Result<()> {
        if Self::kill(pid, Signal::SIGTERM)?.is_stopped() {
            return Ok(());
        }

        tracing::trace!("waiting for process to exit");
        let end = Instant::now() + timeout;
        while Instant::now() < end {
            std::thread::sleep(Duration::from_millis(25));
            if Self::poll(pid)?.is_stopped() {
                return Ok(());
            }
        }

        tracing::warn!("process has taken too long to exit, sending SIGKILL",);
        if Self::kill(pid, Signal::SIGKILL)?.is_stopped() {
            return Ok(());
        }

        tracing::trace!("waiting for process to exit");
        let end = Instant::now() + Duration::from_secs(1);
        while Instant::now() < end {
            std::thread::sleep(Duration::from_millis(25));
            if Self::poll(pid)?.is_stopped() {
                return Ok(());
            }
        }

        tracing::error!("process has leaked");
        Err(SyscallError::Other("process has leaked".into()))
    }

    #[tracing::instrument(level = "trace", fields(?pid))]
    fn poll(pid: Pid) -> Result<ProcessState> {
        let result = procfs::process::Process::new(pid.as_raw());
        for _ in 0..5 {
            match result {
                Ok(v) if v.is_alive() => return Ok(ProcessState::Running),
                Ok(_) => return Ok(ProcessState::Stopped),
                Err(procfs::ProcError::NotFound(_)) => return Ok(ProcessState::NotFound),
                Err(procfs::ProcError::PermissionDenied(_)) => {
                    return Err(SyscallError::IoError(ErrorKind::PermissionDenied.into()))
                }
                Err(procfs::ProcError::Incomplete(_)) => {}
                Err(procfs::ProcError::Io(error, _)) => return Err(SyscallError::IoError(error)),
                Err(procfs::ProcError::Other(error)) => return Err(SyscallError::Other(error)),
                Err(procfs::ProcError::InternalError(error)) => {
                    tracing::error!(?error, "an internal procfs error occurred, please report it at https://github.com/eminence/procfs");
                    return Err(SyscallError::Other(error.msg));
                }
            }
        }
        Err(SyscallError::IoError(
            std::io::ErrorKind::InvalidData.into(),
        ))
    }

    #[tracing::instrument(level = "trace")]
    fn close_range(min: i32, max: Option<i32>) -> Result<()> {
        use nix::libc;
        match unsafe {
            libc::syscall(
                libc::SYS_close_range,
                min,
                max.unwrap_or(libc::c_int::MAX),
                0,
            )
        } {
            0 => Ok(()),
            -1 => Err(nix::Error::last().into()),
            _ => Err(nix::Error::UnknownErrno.into()),
        }
    }

    #[tracing::instrument(level = "trace", skip(supplementary), fields(supplementary = ?supplementary.as_ref()))]
    fn set_id(uid: Uid, gid: Gid, supplementary: impl AsRef<[Gid]>) -> Result<()> {
        let supplementary = supplementary.as_ref();

        if !supplementary.is_empty() {
            tracing::trace!("setting supplementary groups");
            nix::unistd::setgroups(supplementary)?;
        }
        tracing::trace!("setting gids");
        setresgid(gid, gid, gid)?;
        tracing::trace!("setting uids");
        setresuid(uid, uid, uid)?;

        Ok(())
    }

    #[inline]
    #[tracing::instrument(level = "trace", skip_all, fields(?flags))]
    fn unshare(flags: CloneFlags) -> Result<()> {
        nix::sched::unshare(flags)?;
        Ok(())
    }

    #[inline]
    #[tracing::instrument(level = "trace")]
    fn fork() -> Result<ForkResult> {
        let result = unsafe { nix::unistd::fork() }?;
        Ok(result)
    }

    #[inline]
    #[tracing::instrument(level = "trace", skip_all)]
    fn clone<const STACK_SIZE: usize>(
        cb: impl FnMut() -> isize,
        flags: CloneFlags,
        signal: Option<i32>,
    ) -> Result<Pid> {
        let mut stack = [0u8; STACK_SIZE];
        let pid = unsafe { nix::sched::clone(Box::new(cb), &mut stack, flags, signal) }?;
        Ok(pid)
    }

    #[inline]
    #[tracing::instrument(level = "trace", skip_all, fields(path = ?path.as_ref(), ?mode))]
    fn chmod(path: impl AsRef<Path>, mode: Mode) -> Result<()> {
        std::fs::set_permissions(path, Permissions::from_mode(mode.bits()))?;
        Ok(())
    }

    #[inline]
    #[tracing::instrument(level = "trace", skip_all, fields(path = ?path.as_ref()))]
    fn create_dir(path: impl AsRef<Path>) -> Result<()> {
        std::fs::create_dir(path)?;
        Ok(())
    }

    #[inline]
    #[tracing::instrument(level = "trace", skip_all, fields(path = ?path.as_ref()))]
    fn create_dir_all(path: impl AsRef<Path>) -> Result<()> {
        std::fs::create_dir_all(path)?;
        Ok(())
    }

    #[inline]
    #[tracing::instrument(level = "trace", skip_all, fields(path = ?path.as_ref()))]
    fn remove_dir_all(path: impl AsRef<Path>) -> Result<()> {
        std::fs::remove_dir_all(path)?;
        Ok(())
    }

    #[inline]
    #[tracing::instrument(level = "trace", skip_all, fields(path = ?path.as_ref()))]
    fn remove_file(path: impl AsRef<Path>) -> Result<()> {
        std::fs::remove_file(path)?;
        Ok(())
    }

    #[inline]
    #[tracing::instrument(level = "trace", skip_all, fields(path = ?path.as_ref()))]
    fn touch(path: impl AsRef<Path>) -> Result<()> {
        std::fs::OpenOptions::new()
            .create(true)
            .read(false)
            .write(true)
            .open(path.as_ref())?;
        Ok(())
    }

    #[inline]
    #[tracing::instrument(level = "trace", skip_all, fields(path = ?path.as_ref()))]
    fn append(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .append(true)
            .open(path.as_ref())?;
        file.write_all(contents.as_ref())?;
        Ok(())
    }

    #[inline]
    #[tracing::instrument(level = "trace", skip_all, fields(path = ?path.as_ref()))]
    fn overwrite(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .append(false)
            .open(path.as_ref())?;
        file.write_all(contents.as_ref())?;
        Ok(())
    }

    #[inline]
    #[tracing::instrument(level = "trace")]
    fn set_keep_capabilities(keep: bool) -> Result<()> {
        prctl::set_keep_capabilities(keep).map_err(Errno::from_i32)?;
        Ok(())
    }

    #[inline]
    #[tracing::instrument(level = "trace", skip_all, fields(hostname = ?hostname.as_ref()))]
    fn set_hostname(hostname: impl AsRef<OsStr>) -> Result<()> {
        sethostname(hostname.as_ref())?;
        Ok(())
    }

    #[inline]
    #[tracing::instrument(level = "trace", skip_all, fields(src = ?src.as_ref(), dest = ?dest.as_ref()))]
    fn symlink(src: impl AsRef<Path>, dest: impl AsRef<Path>) -> Result<()> {
        symlink(src.as_ref(), dest.as_ref())?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct TempMount<SC: Syscall> {
    dir: TempDir,
    _phantom: PhantomData<SC>,
}

impl<SC: Syscall> TempMount<SC> {
    #[inline]
    pub fn forget(mut self) -> PathBuf {
        std::mem::take(&mut self.dir).forget()
    }
}

impl<SC: Syscall> TryFrom<TempDir> for TempMount<SC> {
    type Error = SyscallError;

    #[inline]
    fn try_from(value: TempDir) -> Result<Self> {
        SC::mount(
            SYS_NONE,
            value.as_path(),
            Some(MountType::TmpFs),
            MsFlags::empty(),
            SYS_NONE,
        )?;
        Ok(TempMount {
            dir: value,
            _phantom: PhantomData,
        })
    }
}

impl<SC: Syscall> Drop for TempMount<SC> {
    fn drop(&mut self) {
        let path = std::mem::take(&mut self.dir);
        if !path.as_path().as_os_str().is_empty() && path.exists() {
            match SC::unmount(path.as_path(), MntFlags::MNT_DETACH) {
                Err(error) if error.errno() == Some(Errno::ENOENT) => {
                    tracing::trace!(?path, "temporary directory not mounted")
                }
                Err(error) => tracing::error!(?error, ?path, "failed to unmount"),
                Ok(_) => tracing::trace!(?path, "unmounted tmp"),
            }
            drop(path)
        }
    }
}

pub struct ChildProcess<SC: Syscall> {
    pid: Option<Pid>,
    timeout: Duration,
    _phantom: PhantomData<SC>,
}

impl<SC: Syscall> std::fmt::Debug for ChildProcess<SC> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(pid) = self.pid {
            pid.fmt(f)
        } else {
            f.debug_struct("Pid").finish()
        }
    }
}

impl<SC: Syscall> ChildProcess<SC> {
    #[inline]
    pub fn inner(&self) -> Pid {
        self.pid.unwrap()
    }

    #[inline]
    pub fn into_inner(mut self) -> Option<Pid> {
        self.pid.take()
    }
}

impl<SC: Syscall> From<Pid> for ChildProcess<SC> {
    #[inline]
    fn from(value: Pid) -> Self {
        Self {
            pid: Some(value),
            timeout: Duration::from_secs(5),
            _phantom: PhantomData,
        }
    }
}

impl<SC: Syscall> From<ChildProcess<SC>> for Pid {
    #[inline]
    fn from(value: ChildProcess<SC>) -> Self {
        value.inner()
    }
}

impl<SC: Syscall> Drop for ChildProcess<SC> {
    #[inline]
    fn drop(&mut self) {
        if let Some(pid) = self.pid.take() {
            SC::kill_wait(pid, self.timeout).ok();
        }
    }
}

#[derive(Default, Debug, Clone)]
pub struct Mappings {
    gids: Vec<(u32, u32, u32)>,
    uids: Vec<(u32, u32, u32)>,
}

#[derive(Error, Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidMapping {
    #[error("the parent range exceeds the maximum uid/gid range")]
    InvalidParentRange,
    #[error("the child range exceeds the maximum uid/gid range")]
    InvalidSubRange,
}

impl Mappings {
    #[inline]
    pub fn push_uid(&mut self, namespace: Uid, parent: Uid) -> &mut Self {
        self.uids.push((namespace.as_raw(), parent.as_raw(), 1));
        self
    }

    #[inline]
    pub fn push_gid(&mut self, namespace: Gid, parent: Gid) -> &mut Self {
        self.gids.push((namespace.as_raw(), parent.as_raw(), 1));
        self
    }

    #[inline]
    pub fn push_uid_range(
        &mut self,
        namespace: Uid,
        parent: Uid,
        length: u32,
    ) -> std::result::Result<&mut Self, InvalidMapping> {
        self.uids.push(Self::validate((
            namespace.as_raw(),
            parent.as_raw(),
            length,
        ))?);
        Ok(self)
    }

    #[inline]
    pub fn push_gid_range(
        &mut self,
        namespace: Gid,
        parent: Gid,
        length: u32,
    ) -> std::result::Result<&mut Self, InvalidMapping> {
        self.gids.push(Self::validate((
            namespace.as_raw(),
            parent.as_raw(),
            length,
        ))?);
        Ok(self)
    }

    #[inline]
    fn validate(value: (u32, u32, u32)) -> std::result::Result<(u32, u32, u32), InvalidMapping> {
        if value.0.overflowing_add(value.2).1 {
            Err(InvalidMapping::InvalidSubRange)
        } else if value.1.overflowing_add(value.2).1 {
            Err(InvalidMapping::InvalidParentRange)
        } else {
            Ok(value)
        }
    }

    #[tracing::instrument(level = "trace", skip(self))]
    pub(crate) fn apply(&self, pid: Option<Pid>) -> Result<()> {
        Self::exec("newuidmap", pid, &self.uids)?;
        Self::exec("newgidmap", pid, &self.gids)?;
        Ok(())
    }

    #[tracing::instrument(level = "trace", skip(map))]
    fn exec(app: &str, pid: Option<Pid>, map: &Vec<(u32, u32, u32)>) -> Result<()> {
        static DEFAULT_MAP: [(u32, u32, u32); 1] = [(0, 0, u32::MAX)];
        let map = if map.is_empty() {
            // We always write this so that someone else doesn't do it for us
            // This is the kernel default
            &DEFAULT_MAP[..]
        } else {
            &map[..]
        };

        let pid = pid.unwrap_or_else(nix::unistd::getpid);
        let mut args = Vec::new();
        args.push(pid.to_string());
        for item in map {
            args.push(item.0.to_string());
            args.push(item.1.to_string());
            args.push(item.2.to_string());
        }

        tracing::trace!(?map, "applying map");
        let proc = std::process::Command::new(app).args(args).output()?;

        if proc.status.success() {
            Ok(())
        } else {
            let stdout = unsafe { OsStr::from_encoded_bytes_unchecked(&proc.stdout) };
            let stderr = unsafe { OsStr::from_encoded_bytes_unchecked(&proc.stderr) };
            tracing::error!(?stdout, ?stderr, "process failed");
            Err(SyscallError::Other(
                "failed to set subuid/subgid".to_string(),
            ))
        }
    }
}
