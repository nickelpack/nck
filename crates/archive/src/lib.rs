use std::{
    fs::File,
    io::{Read, Seek, Write},
    path::PathBuf,
};

use nck_core::{
    hashing::{SupportedHash, SupportedHasher},
    BUFFER_POOL,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const ENTRY_DATA: u8 = 1;
const ENTRY_ENTRY: u8 = 2;

const TYPE_DATA: u8 = 1;
const TYPE_LINK: u8 = 2;
const TYPE_DIR: u8 = 3;

const MAX_LENGTH: usize = u16::MAX as usize;

bitflags::bitflags! {
    pub struct EntryFlags: u16 {
        const EXECUTABLE = 0b0000_0000_0000_0001;
    }
}

/// The target for a file entry.
pub enum EntryTarget {
    /// The entry contains data, which is referred to by hash.
    Data(SupportedHash, EntryFlags),
    /// The entry contains a link.
    Link(PathBuf, EntryFlags),
    /// The entry is a directory.
    Directory,
}

/// A single entry.
pub struct Entry {
    path: PathBuf,
    target: EntryTarget,
}

pub fn write_data(
    reader: &mut impl Read,
    writer: &mut impl Write,
    mut hasher: SupportedHasher,
) -> std::io::Result<SupportedHash> {
    let mut buffer = BUFFER_POOL.take();

    writer.write_all(&[ENTRY_DATA])?;
    loop {
        let len = reader.read(&mut buffer[..])?;
        if len == 0 {
            break;
        }

        let mut bytes = &buffer[..len];
        hasher.update(bytes);

        while !bytes.is_empty() {
            let to_write = MAX_LENGTH.min(bytes.len());

            writer.write_all(&(to_write as u16).to_be_bytes())?;
            writer.write_all(&bytes[..to_write])?;
            bytes = &bytes[to_write..];
        }
    }

    // Length of zero terminates the data.
    writer.write_all(&0u16.to_be_bytes())?;

    let hash = hasher.finalize();
    let (id, bytes) = hash.as_id_and_bytes();
    writer.write_all(&[id])?;
    writer.write_all(bytes)?;

    Ok(hash)
}

pub async fn write_data_async(
    reader: &mut (impl AsyncRead + Unpin),
    writer: &mut (impl AsyncWrite + Unpin),
    mut hasher: SupportedHasher,
) -> std::io::Result<SupportedHash> {
    let mut buffer = BUFFER_POOL.take();

    writer.write_all(&[ENTRY_DATA]).await?;
    loop {
        let len = reader.read(&mut buffer[..]).await?;
        if len == 0 {
            break;
        }

        let mut bytes = &buffer[..len];
        hasher.update(bytes);

        while !bytes.is_empty() {
            let to_write = MAX_LENGTH.min(bytes.len());

            writer.write_all(&(to_write as u16).to_be_bytes()).await?;
            writer.write_all(&bytes[..to_write]).await?;
            bytes = &bytes[to_write..];
        }
    }

    // Length of zero terminates the data.
    writer.write_all(&0u16.to_be_bytes()).await?;

    let hash = hasher.finalize();
    let (id, bytes) = hash.as_id_and_bytes();
    writer.write_all(&[id]).await?;
    writer.write_all(bytes).await?;

    Ok(hash)
}

pub fn write_entry(entry: Entry, writer: &mut impl Write) -> std::io::Result<()> {
    writer.write_all(&[ENTRY_ENTRY])?;

    let path = entry.path.as_os_str().as_encoded_bytes();
    write_length_prefixed(path, writer)?;

    match entry.target {
        EntryTarget::Data(hash, flags) => {
            let (id, bytes) = hash.as_id_and_bytes();
            writer.write_all(&[TYPE_DATA, id])?;
            writer.write_all(bytes)?;
            writer.write_all(&flags.bits().to_be_bytes())?;
        }
        EntryTarget::Link(dest, flags) => {
            let path = dest.as_os_str().as_encoded_bytes();
            writer.write_all(&[TYPE_LINK])?;
            write_length_prefixed(path, writer)?;
            writer.write_all(&flags.bits().to_be_bytes())?;
        }
        EntryTarget::Directory => {
            writer.write_all(&[TYPE_DIR])?;
        }
    }

    Ok(())
}

fn write_length_prefixed(buf: &[u8], writer: &mut impl Write) -> Result<(), std::io::Error> {
    if buf.len() > MAX_LENGTH {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    writer.write_all(&(buf.len() as u16).to_be_bytes())?;
    writer.write_all(buf)?;
    Ok(())
}
