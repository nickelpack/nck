#![feature(never_type)]
#![feature(core_io_borrowed_buf)]
#![feature(read_buf)]

mod read;
mod write;

use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use nck_hashing::SupportedHash;
pub use read::*;
pub use write::*;

const ENTRY_DATA: u8 = 1;
const ENTRY_ENTRY: u8 = 2;

const TYPE_DATA: u8 = 1;
const TYPE_LINK: u8 = 2;
const TYPE_DIR: u8 = 3;

#[cfg(not(test))]
const MAX_LENGTH: usize = u16::MAX as usize;

#[cfg(test)]
const MAX_LENGTH: usize = 32;

bitflags::bitflags! {
    #[derive(Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
    pub struct EntryFlags: u16 {
        const EXECUTABLE = 0b0000_0000_0000_0001;
    }
}

/// The target for a file entry.
#[derive(Debug, PartialEq, Eq)]
pub enum EntryTarget {
    /// The entry contains data, which is referred to by hash.
    Data(SupportedHash, EntryFlags),
    /// The entry contains a link.
    Link(PathBuf, EntryFlags),
    /// The entry is a directory.
    Directory,
}

/// A single entry.
#[derive(Debug, PartialEq, Eq)]
pub struct Entry {
    path: PathBuf,
    target: EntryTarget,
}

impl Entry {
    pub fn new(path: impl AsRef<Path>, target: EntryTarget) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            target,
        }
    }

    pub fn data(path: impl AsRef<Path>, hash: SupportedHash, flags: Option<EntryFlags>) -> Self {
        Self::new(path, EntryTarget::Data(hash, flags.unwrap_or_default()))
    }

    pub fn link(
        path: impl AsRef<Path>,
        source: impl AsRef<Path>,
        flags: Option<EntryFlags>,
    ) -> Self {
        Self::new(
            path,
            EntryTarget::Link(source.as_ref().to_path_buf(), flags.unwrap_or_default()),
        )
    }

    pub fn directory(path: impl AsRef<Path>) -> Self {
        Self::new(path, EntryTarget::Directory)
    }
}

fn hash_data(hash: &SupportedHash) -> (u8, &[u8]) {
    match hash {
        SupportedHash::Blake3(h) => (1, &h[..]),
    }
}

fn hash_length(hash: u8) -> std::io::Result<usize> {
    match hash {
        1 => Ok(32),
        _ => Err(std::io::Error::new(
            ErrorKind::InvalidData,
            format!("unknown hash type '{hash:x}'"),
        )),
    }
}

fn create_hash(hash: u8, data: &[u8]) -> SupportedHash {
    match hash {
        1 => SupportedHash::Blake3(data.try_into().unwrap()),
        // Validated by calling hash_length first
        _ => unreachable!(),
    }
}
