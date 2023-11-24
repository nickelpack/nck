use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use asy::*;
pub use sync::*;

// This protocol is intentionally extremely simple. Something like remoc would need an entire tokio runtime, which
// defeats the purpose of a zygote: a process that is as small as possible, which can be quickly forked/cloned.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    Spawn(SpawnRequest),
}

impl From<SpawnRequest> for Request {
    fn from(value: SpawnRequest) -> Self {
        Self::Spawn(value)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnRequest {
    pub name: String,
    pub root_uid: u32,
    pub root_gid: u32,
    pub user_uid: u32,
    pub user_gid: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnResponse {
    pub pid: i32,
    pub sandbox_path: PathBuf,
    pub socket_path: PathBuf,
}

mod sync {
    use std::{io::Write, os::unix::net::UnixStream};

    use npk_util::io::Buffer;
    use serde::{Deserialize, Serialize};

    use crate::unix::{CRC, USIZE_SIZE, ZYGOTE_HEADER_SIZE};

    pub fn read_from_socket<'a, T: Deserialize<'a>>(
        buffer: &'a mut Buffer,
        socket: &mut UnixStream,
    ) -> std::io::Result<T> {
        let b = buffer.read_buf(socket, ZYGOTE_HEADER_SIZE)?;
        let len = usize::from_ne_bytes(b[..USIZE_SIZE].try_into().unwrap());
        let crc = u64::from_ne_bytes(b[USIZE_SIZE..].try_into().unwrap());

        let b = buffer.read_buf(socket, len)?;
        if crc != crate::unix::CRC.checksum(b) {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        let request = bincode::deserialize::<T>(b).map_err(|error| {
            tracing::error!(?error, "failed to deserialize request");
            std::io::Error::from(std::io::ErrorKind::InvalidData)
        })?;
        Ok(request)
    }

    pub fn write_to_socket<T: Serialize>(
        mut buffer: &mut Buffer,
        socket: &mut UnixStream,
        value: &T,
    ) -> std::io::Result<()> {
        buffer.clear();
        buffer.write_all(&[0u8; ZYGOTE_HEADER_SIZE])?;
        bincode::serialize_into(&mut buffer, value).map_err(|error| {
            tracing::error!(?error, "failed to serialized request");
            std::io::Error::from(std::io::ErrorKind::Other)
        })?;
        let data = buffer.data_mut();
        let len = data.len() - ZYGOTE_HEADER_SIZE;
        let crc = CRC.checksum(&data[ZYGOTE_HEADER_SIZE..]);
        data[..USIZE_SIZE].copy_from_slice(&len.to_ne_bytes());
        data[USIZE_SIZE..ZYGOTE_HEADER_SIZE].copy_from_slice(&crc.to_ne_bytes());
        buffer.flush_to(socket)?;
        Ok(())
    }
}

mod asy {
    use npk_util::io::Buffer;
    use serde::{Deserialize, Serialize};
    use tokio::net::UnixStream;

    use crate::unix::{CRC, USIZE_SIZE, ZYGOTE_HEADER_SIZE};

    pub async fn read_from_socket_async<'a, T: Deserialize<'a>>(
        buffer: &'a mut Buffer,
        socket: &mut UnixStream,
    ) -> std::io::Result<T> {
        let b = buffer.read_buf_async(socket, ZYGOTE_HEADER_SIZE).await?;
        let len = usize::from_ne_bytes(b[..USIZE_SIZE].try_into().unwrap());
        let crc = u64::from_ne_bytes(b[USIZE_SIZE..].try_into().unwrap());

        let b = buffer.read_buf_async(socket, len).await?;
        if crc != crate::unix::CRC.checksum(b) {
            return Err(std::io::Error::from(std::io::ErrorKind::InvalidData));
        }
        let request = bincode::deserialize::<T>(b).map_err(|error| {
            tracing::error!(?error, "failed to deserialize request");
            std::io::Error::from(std::io::ErrorKind::InvalidData)
        })?;
        Ok(request)
    }

    pub async fn write_to_socket_async<T: Serialize>(
        mut buffer: &mut Buffer,
        socket: &mut UnixStream,
        value: &T,
    ) -> std::io::Result<()> {
        use std::io::Write;
        buffer.clear();
        buffer.write_all(&[0u8; ZYGOTE_HEADER_SIZE])?;
        bincode::serialize_into(&mut buffer, value).map_err(|error| {
            tracing::error!(?error, "failed to serialized request");
            std::io::Error::from(std::io::ErrorKind::Other)
        })?;
        let data = buffer.data_mut();
        let len = data.len() - ZYGOTE_HEADER_SIZE;
        let crc = CRC.checksum(&data[ZYGOTE_HEADER_SIZE..]);
        data[..USIZE_SIZE].copy_from_slice(&len.to_ne_bytes());
        data[USIZE_SIZE..ZYGOTE_HEADER_SIZE].copy_from_slice(&crc.to_ne_bytes());
        buffer.flush_to_async(socket).await?;
        Ok(())
    }
}
