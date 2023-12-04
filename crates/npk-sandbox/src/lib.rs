#![feature(result_option_inspect)]
#![feature(async_closure)]
#![feature(result_flattening)]
#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "linux")]
pub use linux as current;

use serde::Deserialize;
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[cfg(target_os = "linux")]
    pub linux: linux::Config,
}
