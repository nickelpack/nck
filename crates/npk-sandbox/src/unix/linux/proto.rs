use remoc::prelude::*;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum SandboxError {
    #[error("an RPC error occurred {:?}", _0)]
    RpcError(#[from] rtc::CallError),
    #[error("an I/O error occurred {:?}", _0)]
    IoError(#[from] std::io::Error),
}

pub trait SandboxProcess {
    fn isolate_network() -> Result<(), SandboxError>;
    fn isolate_filesystem() -> Result<(), SandboxError>;
}
