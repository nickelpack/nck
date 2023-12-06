use std::{
    ffi::{OsStr, OsString},
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
};

use speedy::{Context, Readable, Writable};
use thiserror::Error;

use super::syscall::SyscallError;

#[derive(Error, Debug, Readable, Writable)]
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

#[derive(Default, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SerOsString(OsString);

impl From<OsString> for SerOsString {
    #[inline]
    fn from(value: OsString) -> Self {
        Self(value)
    }
}

impl From<&OsStr> for SerOsString {
    #[inline]
    fn from(value: &OsStr) -> Self {
        Self(value.to_os_string())
    }
}

impl From<&Path> for SerOsString {
    fn from(value: &Path) -> Self {
        value.as_os_str().into()
    }
}

impl From<SerOsString> for OsString {
    #[inline]
    fn from(value: SerOsString) -> Self {
        value.0
    }
}

impl From<SerOsString> for PathBuf {
    #[inline]
    fn from(value: SerOsString) -> Self {
        PathBuf::from(value.0)
    }
}

impl Deref for SerOsString {
    type Target = OsString;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for SerOsString {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl AsRef<OsStr> for SerOsString {
    #[inline]
    fn as_ref(&self) -> &OsStr {
        self.0.as_os_str()
    }
}

impl AsRef<Path> for SerOsString {
    #[inline]
    fn as_ref(&self) -> &Path {
        Path::new(self.0.as_os_str())
    }
}

impl std::fmt::Debug for SerOsString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl<C: Context> Writable<C> for SerOsString {
    fn write_to<T: ?Sized + speedy::Writer<C>>(&self, writer: &mut T) -> Result<(), C::Error> {
        let bytes = self.0.as_encoded_bytes();
        writer.write_u32(bytes.len() as u32)?;
        writer.write_slice(bytes)
    }
}

impl<'a, C: Context> Readable<'a, C> for SerOsString {
    fn read_from<R: speedy::Reader<'a, C>>(reader: &mut R) -> Result<Self, <C as Context>::Error> {
        let len = reader.read_u32()? as usize;
        let mut buf = vec![0u8; len];
        reader.read_bytes(&mut buf)?;
        Ok(Self(unsafe { OsString::from_encoded_bytes_unchecked(buf) }))
    }
}
