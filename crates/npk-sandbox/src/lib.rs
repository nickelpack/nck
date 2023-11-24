#![feature(result_option_inspect)]
#[cfg(unix)]
pub mod unix;

#[cfg(unix)]
pub use unix as current;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[cfg(target_os = "linux")]
    pub linux: unix::linux::Config,
}
