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
    pub rootfs_path: PathBuf,
}

mod sync {
    use std::{io::Write, os::unix::net::UnixStream};

    use anyhow::{Context, Result};
    use npk_util::io::Buffer;
    use serde::{Deserialize, Serialize};

    use crate::unix::{CRC, USIZE_SIZE, ZYGOTE_HEADER_SIZE};

    pub fn read_from_socket<'a, T: Deserialize<'a>>(
        buffer: &'a mut Buffer,
        socket: &mut UnixStream,
    ) -> Result<T> {
        let b = buffer
            .read_buf(socket, ZYGOTE_HEADER_SIZE)
            .with_context(|| "while reading packet header")?;
        let len = usize::from_ne_bytes(b[..USIZE_SIZE].try_into().unwrap());
        let crc = u64::from_ne_bytes(b[USIZE_SIZE..].try_into().unwrap());

        let b = buffer
            .read_buf(socket, len)
            .with_context(|| "while reading request data")?;
        if crc != crate::unix::CRC.checksum(b) {
            return Err(std::io::Error::from(std::io::ErrorKind::InvalidData))
                .with_context(|| "while reading request data");
        }
        let request =
            bincode::deserialize::<T>(b).with_context(|| "while deserializing request")?;
        Ok(request)
    }

    pub fn write_to_socket<T: Serialize>(
        mut buffer: &mut Buffer,
        socket: &mut UnixStream,
        value: &T,
    ) -> Result<()> {
        buffer.clear();
        buffer
            .write_all(&[0u8; ZYGOTE_HEADER_SIZE])
            .with_context(|| "while writing the response header")?;
        bincode::serialize_into(&mut buffer, value)
            .with_context(|| "while writing the response data")?;
        let data = buffer.data_mut();
        let len = data.len() - ZYGOTE_HEADER_SIZE;
        let crc = CRC.checksum(&data[ZYGOTE_HEADER_SIZE..]);
        data[..USIZE_SIZE].copy_from_slice(&len.to_ne_bytes());
        data[USIZE_SIZE..ZYGOTE_HEADER_SIZE].copy_from_slice(&crc.to_ne_bytes());
        buffer
            .flush_to(socket)
            .with_context(|| "while writing the packet to the socket")?;
        Ok(())
    }
}

mod asy {
    use anyhow::{Context, Result};
    use npk_util::io::Buffer;
    use serde::{Deserialize, Serialize};
    use tokio::net::UnixStream;

    use crate::unix::{CRC, USIZE_SIZE, ZYGOTE_HEADER_SIZE};

    pub async fn read_from_socket_async<'a, T: Deserialize<'a>>(
        buffer: &'a mut Buffer,
        socket: &mut UnixStream,
    ) -> Result<T> {
        let b = buffer
            .read_buf_async(socket, ZYGOTE_HEADER_SIZE)
            .await
            .with_context(|| "while reading packet header")?;
        let len = usize::from_ne_bytes(b[..USIZE_SIZE].try_into().unwrap());
        let crc = u64::from_ne_bytes(b[USIZE_SIZE..].try_into().unwrap());

        let b = buffer
            .read_buf_async(socket, len)
            .await
            .with_context(|| "while reading request data")?;
        if crc != crate::unix::CRC.checksum(b) {
            return Err(std::io::Error::from(std::io::ErrorKind::InvalidData))
                .with_context(|| "while reading request data")?;
        }
        let request =
            bincode::deserialize::<T>(b).with_context(|| "while deserializing request")?;
        Ok(request)
    }

    pub async fn write_to_socket_async<T: Serialize>(
        mut buffer: &mut Buffer,
        socket: &mut UnixStream,
        value: &T,
    ) -> Result<()> {
        use std::io::Write;
        buffer.clear();
        buffer
            .write_all(&[0u8; ZYGOTE_HEADER_SIZE])
            .with_context(|| "while writing the response header")?;
        bincode::serialize_into(&mut buffer, value)
            .with_context(|| "while writing the response data")?;
        let data = buffer.data_mut();
        let len = data.len() - ZYGOTE_HEADER_SIZE;
        let crc = CRC.checksum(&data[ZYGOTE_HEADER_SIZE..]);
        data[..USIZE_SIZE].copy_from_slice(&len.to_ne_bytes());
        data[USIZE_SIZE..ZYGOTE_HEADER_SIZE].copy_from_slice(&crc.to_ne_bytes());
        buffer
            .flush_to_async(socket)
            .await
            .with_context(|| "while writing the packet to the socket")?;
        Ok(())
    }
}
