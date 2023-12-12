#![feature(result_option_inspect)]
#![feature(async_closure)]
#![feature(result_flattening)]

#[cfg(target_os = "linux")]
pub mod linux;

use std::path::PathBuf;

#[cfg(target_os = "linux")]
pub use linux as current;

#[derive(Debug, Clone)]
pub struct Settings {
    pub tmp_directory: PathBuf,
    pub store_directory: PathBuf,

    #[cfg(target_os = "linux")]
    pub linux: linux::Settings,
}
