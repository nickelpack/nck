#![tarpaulin::skip]

use std::{
    ffi::{OsStr, OsString},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::Path,
};

use nix::{
    fcntl::{open, OFlag},
    mount::{MntFlags, MsFlags},
    sys::stat::{makedev, Mode, SFlag},
    unistd::fchdir,
};

pub const SYS_NONE: Option<&Path> = None::<&Path>;

const BIND: &[u8] = b"bind";
const PROC: &[u8] = b"proc";
const SYSFS: &[u8] = b"sysfs";
const TMPFS: &[u8] = b"tmpfs";
const DEVPTS: &[u8] = b"devpts";
const OVERLAY: &[u8] = b"fuse-overlayfs";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountType {
    Bind,
    Proc,
    SysFs,
    TmpFs,
    DevPts,
    Overlay,
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
            MountType::Overlay => OVERLAY,
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

#[tracing::instrument(level = "trace", skip_all)]
pub fn mount<P1: AsRef<Path>, P2: AsRef<OsStr>, P3: AsRef<OsStr>>(
    source: Option<P1>,
    target: impl AsRef<Path>,
    fstype: Option<P2>,
    flags: MsFlags,
    data: Option<P3>,
) -> nix::Result<()> {
    let source = source.as_ref().map(|f| f.as_ref());
    let target = target.as_ref();
    let fstype = fstype.as_ref().map(|f| f.as_ref());
    let data = data.as_ref().map(|f| f.as_ref());

    tracing::trace!(?target, "mounting");
    nix::mount::mount(source, target, fstype, flags, data)?;
    Ok(())
}

#[tracing::instrument(level = "trace", skip_all)]
pub fn overlay(
    lower: impl AsRef<Path>,
    work: impl AsRef<Path>,
    upper: impl AsRef<Path>,
    target: impl AsRef<Path>,
    flags: MsFlags,
) -> nix::Result<()> {
    let lower = lower.as_ref();
    let work = work.as_ref();
    let upper = upper.as_ref();

    let mut opts = OsString::new();
    opts.push("lowerdir=");
    opts.push(lower.as_os_str());
    opts.push(",upperdir=");
    opts.push(upper.as_os_str());
    opts.push(",workdir=");
    opts.push(work.as_os_str());

    mount(
        SYS_NONE,
        target,
        Some(MountType::Overlay),
        flags,
        Some(opts),
    )
}

#[tracing::instrument(level = "trace", skip_all)]
pub fn unmount(path: impl AsRef<Path>, flags: MntFlags) -> nix::Result<()> {
    let path = path.as_ref();
    tracing::trace!(?path, ?flags, "unmounting");
    nix::mount::umount2(path, flags)?;
    Ok(())
}

#[tracing::instrument(level = "trace", skip_all)]
pub fn bind(
    src: impl AsRef<Path>,
    dest: impl AsRef<Path>,
    additional_flags: Option<MsFlags>,
) -> nix::Result<()> {
    let src = src.as_ref();
    let dest = dest.as_ref();

    tracing::trace!(?src, ?dest, "creating bind mount");

    if let Some(parent) = dest.parent() {
        tracing::trace!(?parent, "creating parent directory");
        std::fs::create_dir_all(parent)
            .map_err(|e| nix::Error::from_i32(e.raw_os_error().unwrap_or(0)))?;
    }

    if src.is_dir() {
        if !dest.is_dir() {
            tracing::trace!(?dest, "creating target directory");
            std::fs::create_dir(dest)
                .map_err(|e| nix::Error::from_i32(e.raw_os_error().unwrap_or(0)))?;
        }
    } else if !dest.is_file() {
        tracing::trace!(?dest, "creating target file");
        std::fs::write(dest, "")
            .map_err(|e| nix::Error::from_i32(e.raw_os_error().unwrap_or(0)))?;
    }

    mount(
        Some(src),
        dest,
        Some(MountType::Bind),
        MsFlags::MS_REC | MsFlags::MS_BIND | additional_flags.unwrap_or_else(MsFlags::empty),
        SYS_NONE,
    )
}

#[tracing::instrument(level = "trace", skip_all, fields(new_root = ?new_root.as_ref()))]
pub fn pivot(new_root: impl AsRef<Path>) -> nix::Result<()> {
    let new_root = new_root.as_ref();

    tracing::trace!(?new_root, "pivoting to new root");

    tracing::trace!("creating an explicit private mount at the new root");
    mount(
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
    mount(
        SYS_NONE,
        "/",
        SYS_NONE,
        MsFlags::MS_SLAVE | MsFlags::MS_REC,
        SYS_NONE,
    )?;

    tracing::trace!("unmounting the old root");
    unmount("/", MntFlags::MNT_DETACH)?;

    tracing::trace!("changing directory to the new root");
    fchdir(newroot.as_raw_fd())?;
    Ok(())
}
