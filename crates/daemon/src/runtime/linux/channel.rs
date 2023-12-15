use std::{
    io::Read,
    marker::PhantomData,
    mem::ManuallyDrop,
    os::{
        fd::{AsRawFd, FromRawFd, IntoRawFd, RawFd},
        unix::net::UnixStream,
    },
    sync::Arc,
};

use bytes::BytesMut;
use nck_core::pool::Pooled;

pub use async_impl::AsyncPeer;
use parking_lot::Mutex;
pub use sync_impl::Peer;

type PacketLength = u16;
const PACKET_LENGTH_SIZE: usize = std::mem::size_of::<PacketLength>();

#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    #[error("an i/o error occurred")]
    IO(std::io::Error),
    #[error("failed to serialize or deserialize data")]
    Postcard(#[from] postcard::Error),
    #[error("the data is too large to send over the channel")]
    TooLarge,
    #[error("channel connection broken")]
    BrokenChannel,
}

impl From<std::io::Error> for ChannelError {
    fn from(value: std::io::Error) -> Self {
        match value.kind() {
            std::io::ErrorKind::ConnectionReset => Self::BrokenChannel,
            std::io::ErrorKind::BrokenPipe => Self::BrokenChannel,
            _ => Self::IO(value),
        }
    }
}

/// A full duplex channel peer that has not yet been realized.
///
/// This structure is safe to send across forks and clones. Once inside the intended process it can be realized into
/// the actual channel. When this structure is cloned, the original copy becomes invalid.
#[derive(Debug)]
pub struct PendingPeer<T>(Arc<Mutex<Option<UnixStream>>>, PhantomData<T>);

impl<T> Clone for PendingPeer<T> {
    fn clone(&self) -> Self {
        let mut lock = self.0.lock();
        let stream = lock.take();
        PendingPeer(Arc::new(Mutex::new(stream)), PhantomData)
    }
}

/// Creates a full duplex channel in the pending state.
pub fn unix_pair<T>() -> std::io::Result<(PendingPeer<T>, PendingPeer<T>)> {
    let (p1, p2) = UnixStream::pair()?;
    Ok((PendingPeer::new(p1), PendingPeer::new(p2)))
}

impl<T> IntoRawFd for PendingPeer<T> {
    fn into_raw_fd(self) -> RawFd {
        let mut lock = self.0.lock();
        let mut value = lock.take().unwrap();
        value.as_raw_fd()
    }
}

impl<T> FromRawFd for PendingPeer<T> {
    unsafe fn from_raw_fd(fd: RawFd) -> Self {
        let stream = UnixStream::from_raw_fd(fd);
        Self::new(stream)
    }
}

impl<T> PendingPeer<T> {
    fn new(stream: UnixStream) -> Self {
        Self(Arc::new(Mutex::new(Some(stream))), PhantomData)
    }

    /// Realizes an asychronous peer from this pending peer.
    pub async fn realize_async(self) -> std::io::Result<async_impl::AsyncPeer<T>> {
        let mut lock = self.0.lock();
        let inner = lock.take().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::Other,
                "channel embryo has already been realized",
            )
        })?;
        inner.set_nonblocking(true)?;

        let inner = tokio::net::UnixStream::from_std(inner)?;
        Ok(async_impl::AsyncPeer::create_from(inner))
    }

    /// Realizes a synchronous peer from this pending peer.
    pub fn realize(self) -> std::io::Result<sync_impl::Peer<T>> {
        let mut lock = self.0.lock();
        let inner = lock.take().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::Other,
                "channel embryo has already been realized",
            )
        })?;

        sync_impl::Peer::create_from(inner)
    }
}

mod async_impl {
    use nck_core::BUFFER_POOL;
    use serde::{Deserialize, Serialize};
    use std::{marker::PhantomData, sync::Arc};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter},
        net::{
            unix::{OwnedReadHalf, OwnedWriteHalf},
            UnixStream,
        },
        sync::Mutex,
    };

    use super::{PacketLength, PACKET_LENGTH_SIZE};

    struct State {
        read: Mutex<BufReader<OwnedReadHalf>>,
        write: Mutex<BufWriter<OwnedWriteHalf>>,
    }

    pub struct AsyncPeer<T> {
        state: Arc<State>,
        _p: PhantomData<T>,
    }

    impl<T> AsyncPeer<T> {
        pub(super) fn create_from(socket: UnixStream) -> Self {
            let (read, write) = socket.into_split();
            let read = Mutex::new(BufReader::new(read));
            let write = Mutex::new(BufWriter::new(write));
            Self {
                state: Arc::new(State { read, write }),
                _p: PhantomData,
            }
        }
    }

    impl<T> AsyncPeer<T>
    where
        for<'de> T: Deserialize<'de>,
    {
        /// Receives a message over the channel.
        pub async fn recv_async(&self) -> Result<T, super::ChannelError> {
            let mut buffer = BUFFER_POOL.take();

            if buffer.len() < PACKET_LENGTH_SIZE {
                buffer.resize(PACKET_LENGTH_SIZE, 0u8);
            }

            let mut reader = self.state.read.lock().await;
            reader.read_exact(&mut buffer[..PACKET_LENGTH_SIZE]).await?;
            let len = PacketLength::from_be_bytes(buffer[..PACKET_LENGTH_SIZE].try_into().unwrap())
                as usize;

            if buffer.len() < len {
                buffer.resize(len, 0u8);
            }
            reader.read_exact(&mut buffer[..len]).await?;

            let result = postcard::from_bytes(&buffer[..len])?;
            Ok(result)
        }
    }

    impl<T> AsyncPeer<T>
    where
        T: Serialize,
    {
        /// Sends a message over the channel.
        pub async fn send_async(&self, data: &T) -> Result<(), super::ChannelError> {
            let mut buffer = BUFFER_POOL.take();

            // Make room for the length header
            buffer.extend_from_slice(&[0u8; PACKET_LENGTH_SIZE]);
            let mut buffer = buffer.apply_result(|v| postcard::to_extend(data, v))?;

            let len = buffer.len() - PACKET_LENGTH_SIZE;
            if len > PacketLength::MAX as usize {
                return Err(super::ChannelError::TooLarge);
            }

            buffer[..PACKET_LENGTH_SIZE].copy_from_slice(&(len as PacketLength).to_be_bytes());

            let mut writer = self.state.write.lock().await;
            writer.write_all(&buffer[..]).await?;
            Ok(())
        }
    }
}

mod sync_impl {
    use nck_core::BUFFER_POOL;
    use parking_lot::Mutex;
    use serde::{Deserialize, Serialize};
    use std::io::{BufReader, BufWriter, Read, Write};
    use std::os::unix::net::UnixStream;
    use std::{marker::PhantomData, sync::Arc};

    use super::{PacketLength, PACKET_LENGTH_SIZE};

    struct State {
        read: Mutex<BufReader<UnixStream>>,
        write: Mutex<BufWriter<UnixStream>>,
    }

    pub struct Peer<T> {
        state: Arc<State>,
        _p: PhantomData<T>,
    }

    impl<T> Peer<T> {
        pub(super) fn create_from(socket: UnixStream) -> std::io::Result<Self> {
            let (read, write) = (socket.try_clone()?, socket);
            let read = Mutex::new(BufReader::new(read));
            let write = Mutex::new(BufWriter::new(write));
            Ok(Self {
                state: Arc::new(State { read, write }),
                _p: PhantomData,
            })
        }
    }

    impl<T> Peer<T>
    where
        for<'de> T: Deserialize<'de>,
    {
        /// Receives a message over the channel.
        pub fn recv(&self) -> Result<T, super::ChannelError> {
            let mut buffer = BUFFER_POOL.take();

            if buffer.len() < PACKET_LENGTH_SIZE {
                buffer.resize(PACKET_LENGTH_SIZE, 0u8);
            }

            let mut reader = self.state.read.lock();
            reader.read_exact(&mut buffer[..PACKET_LENGTH_SIZE])?;
            let len = PacketLength::from_be_bytes(buffer[..PACKET_LENGTH_SIZE].try_into().unwrap())
                as usize;

            if buffer.len() < len {
                buffer.resize(len, 0u8);
            }
            reader.read_exact(&mut buffer[..len])?;

            let result = postcard::from_bytes(&buffer[..len])?;
            Ok(result)
        }
    }

    impl<T> Peer<T>
    where
        T: Serialize,
    {
        /// Sends a message over the channel.
        pub fn send(&self, data: &T) -> Result<(), super::ChannelError> {
            let mut buffer = BUFFER_POOL.take();

            // Make room for the length header
            buffer.extend_from_slice(&[0u8; PACKET_LENGTH_SIZE]);
            let mut buffer = buffer.apply_result(|v| postcard::to_extend(data, v))?;

            let len = buffer.len() - PACKET_LENGTH_SIZE;
            if len > PacketLength::MAX as usize {
                return Err(super::ChannelError::TooLarge);
            }

            buffer[..PACKET_LENGTH_SIZE].copy_from_slice(&(len as PacketLength).to_be_bytes());

            let mut writer = self.state.write.lock();
            writer.write_all(&buffer[..])?;
            Ok(())
        }
    }
}
