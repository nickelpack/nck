use std::{
    ffi::{OsStr, OsString},
    ops::Deref,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::Path,
};

use anyhow::{Context, Result};
use nix::{
    errno::Errno,
    fcntl::OFlag,
    mount::{MntFlags, MsFlags},
    sys::stat::{makedev, Mode, SFlag},
    NixPath,
};
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

static BIND: Lazy<&'static OsStr> = make_os_str!("bind");
static PROC: Lazy<&'static OsStr> = make_os_str!("proc");
static SYSFS: Lazy<&'static OsStr> = make_os_str!("sysfs");
static TMPFS: Lazy<&'static OsStr> = make_os_str!("tmpfs");
static DEVPTS: Lazy<&'static OsStr> = make_os_str!("devpts");

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

pub fn open(path: &Path, flags: OFlag, mode: Mode) -> Result<OwnedFd> {
    nix::fcntl::open(path, flags, mode)
        .map(|fd| unsafe { OwnedFd::from_raw_fd(fd) })
        .with_context(|| format!("while opening {:?}", path))
}

pub fn mount<S: AsRef<Path>, D: AsRef<Path>, T, O>(
    src: Option<S>,
    dest: D,
    ty: Option<T>,
    mut flags: MsFlags,
    options: Option<O>,
) -> Result<()>
where
    T: AsRef<OsStr>,
    O: AsRef<OsStr>,
{
    let src = src.as_ref().map(AsRef::as_ref);
    let dest = dest.as_ref();
    let ty = ty.as_ref().map(AsRef::as_ref);
    let options = options.as_ref().map(AsRef::as_ref);

    if options == Some(&BIND) {
        flags |= MsFlags::MS_BIND;
    }

    nix::mount::mount(src, dest, ty, flags, options)
        .with_context(|| format!("while mounting {:?}", dest))
}

pub fn bind(src: impl AsRef<Path>, dest: impl AsRef<Path>) -> Result<()> {
    let src = src.as_ref();
    let dest = dest.as_ref();

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "while creating the containing directory to bind {:?} from {:?}",
                dest, src
            )
        })?;
    }

    if src.is_dir() {
        if !dest.is_dir() {
            std::fs::create_dir(dest)
                .map_err(|e| Errno::from_i32(e.raw_os_error().unwrap_or_default()))
                .with_context(|| {
                    format!(
                        "while creating the target directory {:?} to bind from {:?}",
                        dest, src
                    )
                })?;
        }
    } else {
        std::fs::write(dest, b"")
            .map_err(|e| Errno::from_i32(e.raw_os_error().unwrap_or_default()))
            .with_context(|| {
                format!(
                    "while creating the target file {:?} to bind from {:?}",
                    dest, src
                )
            })?;
    }

    mount(
        Some(src),
        dest,
        Some(MountType::Bind),
        MsFlags::MS_REC | MsFlags::MS_BIND,
        None::<&str>,
    )
    .with_context(|| format!("while creating a bind from {:?} to {:?}", src, dest))
}

pub fn umount(path: &Path, flags: MntFlags) -> Result<()> {
    nix::mount::umount2(path, flags).with_context(|| format!("while unmounting {:?}", path))
}

pub fn fchdir(fd: &impl AsRawFd) -> Result<()> {
    let path = fd.as_raw_fd();
    nix::unistd::fchdir(path)
        .with_context(|| format!("while changing directory to fd {:?}", fd.as_raw_fd()))
}

pub fn chdir(path: &Path) -> Result<()> {
    nix::unistd::chdir(path).with_context(|| format!("while changing directory to {:?}", path))
}

pub fn pivot_root(new_root: &Path, put_old: &Path) -> Result<()> {
    nix::unistd::pivot_root(new_root, put_old).with_context(|| {
        format!(
            "while pivoting the root from {:?} to {:?}",
            put_old, new_root
        )
    })
}

pub fn make_root_private(path: &Path) -> Result<()> {
    let myself = Process::myself()
        .with_context(|| format!("while getting information about the current process into order to make the mount that contains {:?} private", path))?;

    let mountinfo = myself.mountinfo().with_context(|| {
        format!(
            "while listing the mounts for the current process in order to make the mount that contains {:?} private",
            path
        )
    })?;

    let parent = mountinfo
        .into_iter()
        .filter(|mi| path.starts_with(&mi.mount_point))
        .max_by(|a, b| a.mount_point.len().cmp(&b.mount_point.len()))
        .ok_or(anyhow::anyhow!(
            "the mount that contains {:?} could not be determined",
            path
        ))
        .with_context(|| {
            format!(
                "while finding the mount that contains {:?} in order to make it private",
                path
            )
        })?;

    if parent
        .opt_fields
        .iter()
        .any(|field| matches!(field, MountOptFields::Shared(_)))
    {
        mount(
            None::<&str>,
            &parent.mount_point,
            None::<&str>,
            MsFlags::MS_PRIVATE | MsFlags::MS_REC,
            None::<&str>,
        )
        .with_context(|| format!("while making {:?} a private mount", parent.mount_point))?;
    }

    Ok(())
}

pub fn chroot(path: &Path) -> Result<()> {
    mount(
        Some(path),
        path,
        None::<&str>,
        MsFlags::MS_BIND | MsFlags::MS_REC,
        None::<&str>,
    )
    .with_context(|| format!("while binding {:?} to itself in order to pivot", path))?;

    let newroot =
        open(path, OFlag::O_DIRECTORY | OFlag::O_RDONLY, Mode::empty()).with_context(|| {
            format!(
                "while opening the source directory {:?} in order to pivot to it",
                path
            )
        })?;

    // pivot root usually changes the root directory to first argument, and then mounts the original root directory at
    // second argument. Giving same path for both stacks mapping of the original root directory above the new directory
    // at the same path, then the call to umount unmounts the original root directory from this path.
    pivot_root(path, path).with_context(|| format!("while pivoting to {:?}", path))?;

    mount(
        None::<&str>,
        "/",
        None::<&str>,
        MsFlags::MS_SLAVE | MsFlags::MS_REC,
        None::<&str>,
    )
    .with_context(|| {
        format!(
            "while marking the new root as a slave in order to complete pivot to {:?}",
            path
        )
    })?;

    umount(Path::new("/"), MntFlags::MNT_DETACH).with_context(|| {
        format!(
            "while unmounting the old root in order to complete the pivot to {:?}",
            path
        )
    })?;

    fchdir(&newroot)
        .with_context(|| format!("while switching to the newly pivoted root at {:?}", path))?;

    Ok(())
}
