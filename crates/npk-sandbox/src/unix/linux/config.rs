use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub runtime_dir: PathBuf,
    pub id_map: IdMapConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IdMapConfig {
    pub uid_min: u32,
    pub uid_max: u32,
    pub gid_min: u32,
    pub gid_max: u32,
}
