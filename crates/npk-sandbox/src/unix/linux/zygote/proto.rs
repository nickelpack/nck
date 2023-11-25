use std::{ffi::OsStr, io::Write, path::Path};

pub use asy::*;
use bitcode::{Decode, Encode};
use nix::unistd::{Gid, Pid, Uid};
use npk_util::io::Buffer;
pub use sync::*;

use crate::unix::{CRC, USIZE_SIZE, ZYGOTE_HEADER_SIZE};

// This protocol is intentionally extremely simple. Something like remoc would need an entire tokio runtime, which
// defeats the purpose of a zygote: a process that is as small as possible, which can be quickly forked/cloned.

#[derive(Debug, Clone, Encode, Decode)]
pub enum Request {
    Spawn(SpawnRequest),
}

impl From<SpawnRequest> for Request {
    fn from(value: SpawnRequest) -> Self {
        Self::Spawn(value)
    }
}

#[derive(Debug, Clone, Encode, Decode)]
pub struct SpawnRequest {
    name: String,
    root_uid: u32,
    root_gid: u32,
    user_uid: u32,
    user_gid: u32,
}

impl SpawnRequest {
    pub fn new(name: &str, root_uid: Uid, root_gid: Gid, user_uid: Uid, user_gid: Gid) -> Self {
        Self {
            name: name.to_string(),
            root_uid: root_uid.as_raw(),
            root_gid: root_gid.as_raw(),
            user_uid: user_uid.as_raw(),
            user_gid: user_gid.as_raw(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn root_uid(&self) -> Uid {
        Uid::from_raw(self.root_uid)
    }

    pub fn root_gid(&self) -> Gid {
        Gid::from_raw(self.root_gid)
    }

    pub fn user_uid(&self) -> Uid {
        Uid::from_raw(self.user_uid)
    }

    pub fn user_gid(&self) -> Gid {
        Gid::from_raw(self.user_gid)
    }
}

#[derive(Debug, Clone, Encode, Decode)]
pub struct SpawnResponse {
    pid: i32,
    sandbox_path: Box<[u8]>,
    socket_path: Box<[u8]>,
}

impl SpawnResponse {
    pub fn new(pid: Pid, sandbox_path: impl AsRef<Path>, socket_path: impl AsRef<Path>) -> Self {
        Self {
            pid: pid.as_raw(),
            sandbox_path: sandbox_path.as_ref().as_os_str().as_encoded_bytes().into(),
            socket_path: socket_path.as_ref().as_os_str().as_encoded_bytes().into(),
        }
    }

    pub fn pid(&self) -> Pid {
        Pid::from_raw(self.pid)
    }

    pub fn sandbox_path(&self) -> &Path {
        Path::new(unsafe { OsStr::from_encoded_bytes_unchecked(&self.sandbox_path) })
    }

    pub fn socket_path(&self) -> &Path {
        Path::new(unsafe { OsStr::from_encoded_bytes_unchecked(&self.socket_path) })
    }
}

fn write_to_buffer<T: Encode>(
    buffer: &mut Buffer,
    bitcode_buffer: &mut bitcode::Buffer,
    value: &T,
) -> std::io::Result<()> {
    buffer.clear();
    buffer.write_all(&[0u8; ZYGOTE_HEADER_SIZE])?;
    let data = bitcode_buffer.encode(value).map_err(|error| {
        tracing::error!(?error, "failed to serialize request");
        std::io::Error::from(std::io::ErrorKind::Other)
    })?;
    buffer.write_all(data)?;
    let data = buffer.data_mut();
    let len = data.len() - ZYGOTE_HEADER_SIZE;
    let crc = CRC.checksum(&data[ZYGOTE_HEADER_SIZE..]);
    data[..USIZE_SIZE].copy_from_slice(&len.to_ne_bytes());
    data[USIZE_SIZE..ZYGOTE_HEADER_SIZE].copy_from_slice(&crc.to_ne_bytes());
    Ok(())
}

pub fn read_from_buffer<T: Decode>(
    buffer: &[u8],
    bitcode_buffer: &mut bitcode::Buffer,
) -> std::io::Result<T> {
    bitcode_buffer.decode(buffer).map_err(|error| {
        tracing::error!(?error, "failed to deserialize request");
        std::io::Error::from(std::io::ErrorKind::InvalidData)
    })
}

mod sync {
    use std::os::unix::net::UnixStream;

    use bitcode::{Decode, Encode};
    use npk_util::io::Buffer;

    use crate::unix::{CRC, USIZE_SIZE, ZYGOTE_HEADER_SIZE};

    use super::{read_from_buffer, write_to_buffer};

    pub fn read_from_socket<T: Decode>(
        buffer: &mut Buffer,
        bitcode_buffer: &mut bitcode::Buffer,
        socket: &mut UnixStream,
    ) -> std::io::Result<T> {
        let b = buffer.read_buf(socket, ZYGOTE_HEADER_SIZE)?;
        let len = usize::from_ne_bytes(b[..USIZE_SIZE].try_into().unwrap());
        let crc = u64::from_ne_bytes(b[USIZE_SIZE..].try_into().unwrap());

        let b = buffer.read_buf(socket, len)?;
        if crc != CRC.checksum(b) {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        read_from_buffer(b, bitcode_buffer)
    }

    pub fn write_to_socket<T: Encode>(
        buffer: &mut Buffer,
        bitcode_buffer: &mut bitcode::Buffer,
        socket: &mut UnixStream,
        value: &T,
    ) -> std::io::Result<()> {
        write_to_buffer(buffer, bitcode_buffer, value)?;
        buffer.flush_to(socket)?;
        Ok(())
    }
}

mod asy {
    use bitcode::{Decode, Encode};
    use npk_util::io::Buffer;
    use tokio::net::UnixStream;

    use crate::unix::{CRC, USIZE_SIZE, ZYGOTE_HEADER_SIZE};

    use super::{read_from_buffer, write_to_buffer};

    pub async fn read_from_socket_async<'a, T: Decode>(
        buffer: &'a mut Buffer,
        bitcode_buffer: &mut bitcode::Buffer,
        socket: &mut UnixStream,
    ) -> std::io::Result<T> {
        let b = buffer.read_buf_async(socket, ZYGOTE_HEADER_SIZE).await?;
        let len = usize::from_ne_bytes(b[..USIZE_SIZE].try_into().unwrap());
        let crc = u64::from_ne_bytes(b[USIZE_SIZE..].try_into().unwrap());

        let b = buffer.read_buf_async(socket, len).await?;
        if crc != CRC.checksum(b) {
            return Err(std::io::Error::from(std::io::ErrorKind::InvalidData));
        }
        read_from_buffer(b, bitcode_buffer)
    }

    pub async fn write_to_socket_async<T: Encode>(
        buffer: &mut Buffer,
        bitcode_buffer: &mut bitcode::Buffer,
        socket: &mut UnixStream,
        value: &T,
    ) -> std::io::Result<()> {
        write_to_buffer(buffer, bitcode_buffer, value)?;
        buffer.flush_to_async(socket).await?;
        Ok(())
    }
}
