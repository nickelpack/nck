mod transport;

use bitcode::{Decode, Encode};
use thiserror::Error;

use super::syscall::SyscallError;

pub use transport::*;

#[derive(Debug, Encode, Decode)]
pub enum SandboxMessage {
    IsolateFilesystem,
    BeginWrite(u64, Box<[u8]>),
    Write(u64, Box<[u8]>),
    EndWrite(u64),
}

#[derive(Error, Debug, Encode, Decode)]
pub enum PeerError {
    #[error("an I/O error occurred")]
    IoError,
    #[error("an system error occurred")]
    OsError,
    #[error("{}", _0)]
    Other(String),
}

impl From<std::io::Error> for PeerError {
    fn from(_: std::io::Error) -> Self {
        Self::IoError
    }
}

impl From<nix::Error> for PeerError {
    fn from(_: nix::Error) -> Self {
        Self::IoError
    }
}

impl From<SyscallError> for PeerError {
    fn from(value: SyscallError) -> Self {
        match value {
            SyscallError::OsError(_) => Self::OsError,
            SyscallError::IoError(_) => Self::IoError,
            SyscallError::Other(o) => Self::Other(o),
        }
    }
}
