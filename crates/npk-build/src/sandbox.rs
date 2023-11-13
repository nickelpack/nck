#[cfg(target_os = "linux")]
mod linux;

use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
use linux as imp;

pub use imp::start;

pub struct SandboxOptions {
    pub(crate) path: PathBuf,
    pub(crate) network_access: bool,
    pub(crate) uid: Option<u32>,
    pub(crate) gid: Option<u32>,
}

impl SandboxOptions {
    pub fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            network_access: false,
            uid: None,
            gid: None,
        }
    }
}
