use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::user_ns::UserNamespaceConfig;

pub mod main_process;
mod sandbox_process;
mod supervisor_process;
mod zygote_process;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    working: PathBuf,
    namespace: UserNamespaceConfig,
}
