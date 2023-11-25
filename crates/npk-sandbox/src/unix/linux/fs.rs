use std::{
    ffi::CStr,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::{Path, PathBuf},
};

use nix::{
    fcntl::{open, OFlag},
    mount::{mount, umount2, MntFlags, MsFlags},
    sys::stat::{makedev, Mode, SFlag},
    unistd::fchdir,
    NixPath,
};
use npk_util::io::TempDir;
use procfs::process::{MountOptFields, Process};

use super::NIX_NONE;

const BIND: &CStr = unsafe { CStr::from_bytes_with_nul_unchecked(b"bind\0") };
const PROC: &CStr = unsafe { CStr::from_bytes_with_nul_unchecked(b"proc\0") };
const SYSFS: &CStr = unsafe { CStr::from_bytes_with_nul_unchecked(b"sysfs\0") };
const TMPFS: &CStr = unsafe { CStr::from_bytes_with_nul_unchecked(b"tmpfs\0") };
const DEVPTS: &CStr = unsafe { CStr::from_bytes_with_nul_unchecked(b"devpts\0") };

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountType {
    Bind,
    Proc,
    SysFs,
    TmpFs,
    DevPts,
}

impl From<&MountType> for &CStr {
    fn from(value: &MountType) -> Self {
        match value {
            MountType::Bind => BIND,
            MountType::Proc => PROC,
            MountType::SysFs => SYSFS,
            MountType::TmpFs => TMPFS,
            MountType::DevPts => DEVPTS,
        }
    }
}

impl NixPath for MountType {
    fn is_empty(&self) -> bool {
        false
    }

    fn len(&self) -> usize {
        std::convert::Into::<&CStr>::into(self).len()
    }

    fn with_nix_path<T, F>(&self, f: F) -> nix::Result<T>
    where
        F: FnOnce(&std::ffi::CStr) -> T,
    {
        std::convert::Into::<&CStr>::into(self).with_nix_path(f)
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

#[tracing::instrument(level = "trace", skip_all, err(Debug))]
pub fn bind(src: impl AsRef<Path>, dest: impl AsRef<Path>) -> nix::Result<()> {
    let src = src.as_ref();
    let dest = dest.as_ref();

    tracing::trace!(?src, ?dest, "creating bind mount");

    if let Some(parent) = dest.parent() {
        if !parent.exists() {
            tracing::trace!(?parent, "creating parent directory");
            std::fs::create_dir_all(parent).map_err(super::std_error_to_nix)?;
        }
    }

    if src.is_dir() {
        if !dest.is_dir() {
            tracing::trace!(?dest, "creating target directory");
            std::fs::create_dir(dest).map_err(super::std_error_to_nix)?;
        }
    } else if !dest.is_file() {
        tracing::trace!(?dest, "creating target file");
        std::fs::write(dest, b"").map_err(super::std_error_to_nix)?;
    }

    mount(
        Some(src),
        dest,
        Some(&MountType::Bind),
        MsFlags::MS_REC | MsFlags::MS_BIND,
        NIX_NONE,
    )
}

// Unused: this is the "correct" way to do the first bind in the chroot, but that approach works. Should we care?
#[tracing::instrument(level = "trace", skip_all, err(Debug))]
pub fn make_root_private(path: impl AsRef<Path>) -> nix::Result<()> {
    let path = path.as_ref();

    tracing::trace!(?path, "making mount above path private");

    tracing::trace!("finding mount that contains desired chroot");
    let myself = Process::myself().map_err(super::proc::map_proc_err)?;
    let mountinfo = myself.mountinfo().map_err(super::proc::map_proc_err)?;
    let parent = mountinfo
        .into_iter()
        .filter(|mi| path.starts_with(&mi.mount_point))
        .max_by(|a, b| a.mount_point.len().cmp(&b.mount_point.len()))
        .ok_or(nix::errno::Errno::ENOENT)?;

    tracing::trace!(?parent, "found parent mount");
    if parent
        .opt_fields
        .iter()
        .any(|field| matches!(field, MountOptFields::Shared(_)))
    {
        tracing::trace!("making mount point private",);
        mount(
            NIX_NONE,
            &parent.mount_point,
            NIX_NONE,
            MsFlags::MS_PRIVATE | MsFlags::MS_REC,
            NIX_NONE,
        )?;
    } else {
        tracing::trace!("mount point is already private");
    }

    Ok(())
}

#[tracing::instrument(level = "trace", skip_all, err(Debug))]
pub fn pivot(path: impl AsRef<Path>) -> nix::Result<()> {
    let path = path.as_ref();

    tracing::trace!(?path, "pivoting to new root");

    tracing::trace!("creating an explicit private mount at the new root");
    mount(
        Some(path),
        path,
        NIX_NONE,
        MsFlags::MS_PRIVATE | MsFlags::MS_BIND | MsFlags::MS_REC,
        NIX_NONE,
    )?;

    tracing::trace!("opening a fd at the new root");
    let newroot = open(path, OFlag::O_DIRECTORY | OFlag::O_RDONLY, Mode::empty())
        .map(|v| unsafe { OwnedFd::from_raw_fd(v) })?;

    tracing::trace!("pivoting to the new root");
    // pivot root usually changes the root directory to first argument, and then mounts the original root directory at
    // second argument. Giving same path for both stacks mapping of the original root directory above the new directory
    // at the same path, then the call to umount unmounts the original root directory from this path.
    nix::unistd::pivot_root(path, path)?;

    tracing::trace!("making the new root a recursive slave mount");
    mount(
        NIX_NONE,
        "/",
        NIX_NONE,
        MsFlags::MS_SLAVE | MsFlags::MS_REC,
        NIX_NONE,
    )?;

    tracing::trace!("unmounting the old root");
    umount2("/", MntFlags::MNT_DETACH)?;

    tracing::trace!("changing directory to the new root");
    fchdir(newroot.as_raw_fd())?;

    Ok(())
}

#[derive(Debug)]
pub struct TempMount(TempDir);

impl TempMount {
    pub fn forget(mut self) -> PathBuf {
        std::mem::take(&mut self.0).forget()
    }
}

impl TryFrom<TempDir> for TempMount {
    type Error = nix::Error;

    fn try_from(value: TempDir) -> Result<Self, Self::Error> {
        nix::mount::mount(
            NIX_NONE,
            value.as_path(),
            Some(&MountType::TmpFs),
            MsFlags::empty(),
            // MsFlags::MS_NOATIME | MsFlags::MS_NODIRATIME | MsFlags::MS_PRIVATE,
            NIX_NONE,
        )?;
        Ok(TempMount(value))
    }
}

impl Drop for TempMount {
    fn drop(&mut self) {
        let path = std::mem::take(&mut self.0);
        if !path.is_empty() {
            if let Err(error) = nix::mount::umount2(path.as_path(), MntFlags::MNT_DETACH) {
                tracing::error!(?error, ?path, "failed to unmount");
            } else {
                tracing::trace!(?path, "unmounted tmp");
            }
        }
    }
}
