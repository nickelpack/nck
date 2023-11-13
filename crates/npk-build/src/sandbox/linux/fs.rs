use std::{
    ffi::{OsStr, OsString},
    ops::Deref,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::{Path, PathBuf},
};

use nix::{
    fcntl::OFlag,
    mount::{MntFlags, MsFlags},
    sys::stat::{makedev, Mode, SFlag},
    unistd::{Gid, Uid},
    NixPath,
};

use anyhow::{anyhow, Context, Result};
use derive_more::Display;
use once_cell::sync::Lazy;
use procfs::process::{MountOptFields, Process};

macro_rules! make_os_str {
    ($val: expr) => {
        Lazy::new(move || {
            static VAL: &'static str = $val;
            let val = Box::new(OsString::from(VAL));
            let val = Box::leak(val);
            let os: &'static OsStr = val.as_os_str();
            os
        })
    };
}

const BIND: Lazy<&'static OsStr> = make_os_str!("bind");
const PROC: Lazy<&'static OsStr> = make_os_str!("proc");
const SYSFS: Lazy<&'static OsStr> = make_os_str!("sysfs");
const TMPFS: Lazy<&'static OsStr> = make_os_str!("tmpfs");
const DEVPTS: Lazy<&'static OsStr> = make_os_str!("devpts");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountType {
    Bind,
    Proc,
    SysFs,
    TmpFs,
    DevPts,
}

impl AsRef<OsStr> for MountType {
    fn as_ref(&self) -> &OsStr {
        match self {
            MountType::Bind => BIND.deref(),
            MountType::Proc => PROC.deref(),
            MountType::SysFs => SYSFS.deref(),
            MountType::TmpFs => TMPFS.deref(),
            MountType::DevPts => DEVPTS.deref(),
        }
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

#[derive(Debug, Display)]
#[display(fmt = "failed to open {:?} with flags {:x} and {:o}", _0, _1, _2)]
pub struct OpenFailure(PathBuf, i32, u32);

pub fn open(path: &Path, flags: OFlag, mode: Mode) -> Result<OwnedFd> {
    nix::fcntl::open(path, flags, mode)
        .map(|fd| unsafe { OwnedFd::from_raw_fd(fd) })
        .with_context(|| OpenFailure(path.to_path_buf(), flags.bits(), mode.bits()))
}

#[derive(Debug, Display)]
#[display(fmt = "failed to chown {:?} to {:?}:{:?}", _0, _1, _2)]
pub struct ChownFailure(PathBuf, Option<Uid>, Option<Gid>);

pub fn chown(path: &Path, uid: Option<Uid>, gid: Option<Gid>) -> Result<()> {
    std::os::unix::fs::chown(path, uid.map(Uid::as_raw), gid.map(Gid::as_raw))
        .with_context(|| ChownFailure(path.to_path_buf(), uid, gid))
}

#[derive(Debug, Display)]
#[display(fmt = "failed to symlink {:?} to {:?}", _0, _1)]
pub struct SymlinkFailure(PathBuf, PathBuf);

pub fn symlink(src: &Path, dest: &Path) -> Result<()> {
    std::os::unix::fs::symlink(src, dest)
        .with_context(|| SymlinkFailure(src.to_path_buf(), dest.to_path_buf()))
}

#[derive(Debug, Display)]
#[display(fmt = "failed to mount {:?}", _0)]
pub struct MountFailure(PathBuf);

pub fn mount<T, O>(
    src: Option<&Path>,
    dest: &Path,
    ty: Option<T>,
    mut flags: MsFlags,
    options: Option<O>,
) -> Result<()>
where
    T: AsRef<OsStr>,
    O: AsRef<OsStr>,
{
    let ty = ty.as_ref().map(AsRef::as_ref);
    let options = options.as_ref().map(AsRef::as_ref);

    if options == Some(&BIND) {
        flags |= MsFlags::MS_BIND;
    }

    nix::mount::mount(src, dest, ty, flags, options)
        .with_context(|| MountFailure(dest.to_path_buf()))
}

#[derive(Debug, Display)]
#[display(fmt = "failed to bind {:?} to {:?}", _0, _1)]
pub struct BindFailure(PathBuf, PathBuf);

pub fn bind(src: &Path, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| BindFailure(src.to_path_buf(), dest.to_path_buf()))?;
    }

    if src.is_dir() {
        std::fs::create_dir(dest)
            .with_context(|| BindFailure(src.to_path_buf(), dest.to_path_buf()))?;
    } else {
        std::fs::write(dest, b"")
            .with_context(|| BindFailure(src.to_path_buf(), dest.to_path_buf()))?;
    }

    mount(
        Some(src),
        dest,
        Some(MountType::Bind),
        MsFlags::MS_REC | MsFlags::MS_BIND,
        None::<&str>,
    )
    .with_context(|| BindFailure(src.to_path_buf(), dest.to_path_buf()))
}

#[derive(Debug, Display)]
#[display(fmt = "failed to unmount {:?}", _0)]
pub struct UMountFailure(PathBuf);

pub fn umount(path: &Path, flags: MntFlags) -> Result<()> {
    nix::mount::umount2(path, flags).with_context(|| UMountFailure(path.to_path_buf()))
}

#[derive(Debug, Display)]
#[display(fmt = "failed to chdir to fd {:x}", _0)]
pub struct FChDirFailure(i32);

pub fn fchdir(path: &impl AsRawFd) -> Result<()> {
    let path = path.as_raw_fd();
    nix::unistd::fchdir(path).with_context(|| FChDirFailure(path))
}

#[derive(Debug, Display)]
#[display(fmt = "failed to chdir to {:?}", _0)]
pub struct ChDirFailure(PathBuf);

pub fn chdir(path: &Path) -> Result<()> {
    nix::unistd::chdir(path).with_context(|| ChDirFailure(path.to_path_buf()))
}

#[derive(Debug, Display)]
#[display(fmt = "failed to pivot to {:?} from {:?}", _0, _1)]
pub struct PivotRootFailure(PathBuf, PathBuf);

pub fn pivot_root(new_root: &Path, put_old: &Path) -> Result<()> {
    nix::unistd::pivot_root(new_root, put_old)
        .with_context(|| PivotRootFailure(new_root.to_path_buf(), put_old.to_path_buf()))
}

#[derive(Debug, Display)]
pub enum MakeRootPrivateFailure {
    #[display(fmt = "failed to make the process root private: could not list roots")]
    ListRoots(PathBuf),
    #[display(
        fmt = "failed to make the process root private: could not find parent of {:?}",
        _0
    )]
    FindParent(PathBuf),
    #[display(fmt = "failed to make the process root private: could rebind {:?}", _0)]
    Rebind(PathBuf),
}

pub fn make_root_private(path: &Path) -> Result<()> {
    use MakeRootPrivateFailure::*;

    let myself = Process::myself().with_context(|| ListRoots(path.to_path_buf()))?;

    let mountinfo = myself
        .mountinfo()
        .with_context(|| ListRoots(path.to_path_buf()))?;

    let parent = mountinfo
        .into_iter()
        .filter(|mi| path.starts_with(&mi.mount_point))
        .max_by(|a, b| a.mount_point.len().cmp(&b.mount_point.len()))
        .ok_or_else(|| anyhow!("no apparent parent mount"))
        .with_context(|| FindParent(path.to_path_buf()))?;

    if parent
        .opt_fields
        .iter()
        .any(|field| matches!(field, MountOptFields::Shared(_)))
    {
        mount(
            None,
            &parent.mount_point,
            None::<&str>,
            MsFlags::MS_PRIVATE | MsFlags::MS_REC,
            None::<&str>,
        )
        .with_context(|| Rebind(parent.mount_point.clone()))?;
    }

    Ok(())
}

#[derive(Debug, Display)]
#[display(fmt = "failed to chroot to {:?}", _0)]
pub struct ChrootFailure(PathBuf);

pub fn chroot(path: &Path) -> Result<()> {
    mount(
        Some(path),
        path,
        None::<&str>,
        MsFlags::MS_BIND | MsFlags::MS_REC,
        None::<&str>,
    )
    .with_context(|| ChrootFailure(path.to_path_buf()))?;

    let newroot = open(path, OFlag::O_DIRECTORY | OFlag::O_RDONLY, Mode::empty())
        .with_context(|| ChrootFailure(path.to_path_buf()))?;

    // pivot root usually changes the root directory to first argument, and then mounts the original root directory at
    // second argument. Giving same path for both stacks mapping of the original root directory above the new directory
    // at the same path, then the call to umount unmounts the original root directory from this path.
    pivot_root(path, path).with_context(|| ChrootFailure(path.to_path_buf()))?;

    mount(
        None,
        Path::new("/"),
        None::<&str>,
        MsFlags::MS_SLAVE | MsFlags::MS_REC,
        None::<&str>,
    )
    .with_context(|| ChrootFailure(path.to_path_buf()))?;

    umount(Path::new("/"), MntFlags::MNT_DETACH)
        .with_context(|| ChrootFailure(path.to_path_buf()))?;

    fchdir(&newroot).with_context(|| ChrootFailure(path.to_path_buf()))?;

    Ok(())
}

#[derive(Debug, Display)]
#[display(fmt = "failed to chmod {:?} to {:o}", _0, _1)]
pub struct ChmodFailure(PathBuf, u32);

pub fn chmod(path: &Path, mode: Mode) -> Result<()> {
    nix::sys::stat::fchmodat(
        None,
        path,
        mode,
        nix::sys::stat::FchmodatFlags::NoFollowSymlink,
    )
    .with_context(|| ChmodFailure(path.to_path_buf(), mode.bits()))
}
