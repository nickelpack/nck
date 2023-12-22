use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    marker::PhantomData,
    os::{
        fd::{FromRawFd, IntoRawFd},
        unix::net::{SocketAncillary, UnixStream},
    },
    sync::Arc,
};

use bytes::BytesMut;
use nck_core::pool::Pooled;
use parking_lot::Mutex;

pub use async_impl::AsyncChannel;
use serde::{de::Visitor, Deserialize, Serialize};
pub use sync_impl::Channel;

type PacketLength = u16;
const PACKET_LENGTH_SIZE: usize = std::mem::size_of::<PacketLength>();

/// This contains a list of file descriptors for use during serialization and deserialization.
///
/// It basically exists to ensure that fds are cleand up if something fails.
#[derive(Debug)]
struct FdQueue(VecDeque<i32>);

impl FdQueue {
    fn new() -> Self {
        Self(VecDeque::new())
    }

    fn push_fds(&mut self, ancillary: &mut SocketAncillary) -> Result<(), ChannelError> {
        for message in ancillary.messages() {
            match message {
                Ok(std::os::unix::net::AncillaryData::ScmRights(fds)) => {
                    for fd in fds {
                        self.0.push_back(fd)
                    }
                }
                _ => return Err(ChannelError::ChannelTransfer),
            }
        }
        ancillary.clear();
        Ok(())
    }

    fn push(&mut self, stream: UnixStream) {
        self.0.push_back(stream.into_raw_fd())
    }

    fn pop(&mut self) -> Result<UnixStream, ChannelError> {
        self.0
            .pop_front()
            .ok_or(ChannelError::ChannelTransfer)
            .map(|fd| unsafe { UnixStream::from_raw_fd(fd) })
    }

    fn ensure_used(&mut self) -> Result<(), ChannelError> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(ChannelError::ChannelTransfer)
        }
    }

    fn clear(&mut self) {
        self.0.clear()
    }

    fn fds(&mut self) -> &[i32] {
        self.0.make_contiguous()
    }
}

impl Drop for FdQueue {
    fn drop(&mut self) {
        while let Ok(v) = self.pop() {
            drop(v)
        }
    }
}

thread_local! {
    static CHANNEL_FDS: RefCell<Option<FdQueue>> = RefCell::new(None);
}

#[derive(Debug, thiserror::Error)]
pub enum PendingChannelError {
    /// An I/O error occurred.
    #[error("an i/o error occurred")]
    IO(#[from] std::io::Error),
    /// The pending channel has already been consumed, through cloning or acquiring its raw file descriptor.
    #[error("the pending channel has already been consumed")]
    Consumed,
}

/// A full duplex channel peer that has not yet been realized.
///
/// The pending channel can be cloned or turned into an actual channel ([`AsyncChannel`] or [`Channel`]). Only one clone
/// can be made (and subsequently exactly one clone made from descendants). This is because the handle to the unix
/// socket is transferred to the new clone, instead of being cloned itself. This ensures that exactly one process
/// controls (and therefore closes) the unix socket after the channel has been realized.
#[derive(Debug)]
pub struct PendingChannel<S, R>(Arc<Mutex<Option<UnixStream>>>, PhantomData<(S, R)>);

impl<S, R> Serialize for PendingChannel<S, R> {
    fn serialize<Ser>(&self, serializer: Ser) -> Result<Ser::Ok, Ser::Error>
    where
        Ser: serde::Serializer,
    {
        use serde::ser::Error;
        CHANNEL_FDS.with_borrow_mut(|v| {
            if let Some(fds) = v.as_mut() {
                let value = self.take().map_err(Ser::Error::custom)?;
                fds.push(value);
                Ok(())
            } else {
                Err(Ser::Error::custom(ChannelError::ChannelTransfer))
            }
        })?;
        serializer.serialize_bool(true)
    }
}

impl<'de, S, R> Deserialize<'de> for PendingChannel<S, R> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_bool(PendingChannelVisitor(PhantomData))
    }
}

struct PendingChannelVisitor<S, R>(PhantomData<(S, R)>);

impl<'de, S, R> Visitor<'de> for PendingChannelVisitor<S, R> {
    type Value = PendingChannel<S, R>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("an i32 PID")
    }

    fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        let fd = CHANNEL_FDS.with_borrow_mut(|v| {
            if let Some(fds) = v.as_mut() {
                fds.pop().map_err(E::custom)
            } else {
                Err(E::custom(ChannelError::ChannelTransfer))
            }
        })?;
        Ok(PendingChannel::new(fd))
    }
}

impl<S, R> Clone for PendingChannel<S, R> {
    fn clone(&self) -> Self {
        self.try_clone().unwrap()
    }
}

impl<S, R> PendingChannel<S, R> {
    fn new(stream: UnixStream) -> Self {
        Self(Arc::new(Mutex::new(Some(stream))), PhantomData)
    }

    fn take(&self) -> Result<UnixStream, PendingChannelError> {
        let mut lock = self.0.lock();
        lock.take().ok_or(PendingChannelError::Consumed)
    }

    /// Determines whether this pending channel controls the unix socket handle.
    pub fn controls_handle(&self) -> bool {
        let lock = self.0.lock();
        lock.is_some()
    }

    /// Creates a new clone of this pending peer.
    ///
    /// The existing pending peer (the value of `self`) will become invalid.
    pub fn try_clone(&self) -> Result<Self, PendingChannelError> {
        let stream = self.take()?;
        Ok(PendingChannel(
            Arc::new(Mutex::new(Some(stream))),
            PhantomData,
        ))
    }

    /// Creates a [`AsyncChannel`] from this pending peer.
    pub async fn into_peer_async(self) -> Result<AsyncChannel<S, R>, PendingChannelError> {
        let inner = self.take()?;
        inner.set_nonblocking(true)?;

        let inner = tokio::net::UnixStream::from_std(inner)?;
        Ok(async_impl::AsyncChannel::create_from(inner))
    }

    /// Creates a [`Channel`] from this pending peer.
    pub fn into_peer(self) -> Result<Channel<S, R>, PendingChannelError> {
        let inner = self.take()?;
        Ok(sync_impl::Channel::create_from(inner)?)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    // An i/o error occurred sending data over the channel.
    #[error("an i/o error occurred")]
    IO(std::io::Error),
    /// A serialization error occcured when serializing or deserializing data to send over the channel.
    #[error("failed to serialize or deserialize data")]
    Postcard(#[from] postcard::Error),
    /// The packet would be too large to send over the channel.
    #[error("the data is too large to send over the channel")]
    TooLarge,
    /// A channel could not be sent or received.
    #[error("a channel could not be sent or received")]
    ChannelTransfer,
    /// The channel has been closed by the remote peer.
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

fn serialize<T: Serialize>(
    mut buffer: Pooled<'_, BytesMut>,
    data: T,
) -> Result<(Pooled<'_, BytesMut>, FdQueue), ChannelError> {
    buffer.extend_from_slice(&[0u8; PACKET_LENGTH_SIZE]);
    let old = CHANNEL_FDS.with_borrow_mut(|fds| fds.replace(FdQueue::new()));

    let buffer = buffer.apply_result(|v| postcard::to_extend(&data, v));

    let fds = CHANNEL_FDS.with_borrow_mut(|fds| {
        if let Some(old) = old {
            fds.replace(old).unwrap()
        } else {
            fds.take().unwrap()
        }
    });

    let mut buffer = buffer?;
    let len = buffer.len() - PACKET_LENGTH_SIZE;
    if len > PacketLength::MAX as usize {
        return Err(ChannelError::TooLarge);
    }

    buffer[..PACKET_LENGTH_SIZE].copy_from_slice(&(len as PacketLength).to_be_bytes());
    Ok((buffer, fds))
}

fn deserialize<'de, T: Deserialize<'de>>(bytes: &'de [u8], q: FdQueue) -> Result<T, ChannelError> {
    let old = CHANNEL_FDS.with_borrow_mut(|fds| fds.replace(q));

    let result = postcard::from_bytes::<'de>(bytes);

    CHANNEL_FDS
        .with_borrow_mut(|fds| {
            if let Some(old) = old {
                fds.replace(old).unwrap()
            } else {
                fds.take().unwrap()
            }
        })
        .ensure_used()?;

    Ok(result?)
}

mod async_impl {
    use nck_core::BUFFER_POOL;
    use serde::{Deserialize, Serialize};
    use std::{
        io::{IoSlice, IoSliceMut},
        marker::PhantomData,
        mem::ManuallyDrop,
        os::{
            fd::{AsRawFd, FromRawFd, IntoRawFd, RawFd},
            unix::net::{SocketAncillary, UnixStream as StdUnixStream},
        },
        sync::Arc,
    };
    use tokio::{
        io::Interest,
        net::UnixStream,
        sync::{Mutex, MutexGuard},
    };

    use super::{FdQueue, PacketLength, PACKET_LENGTH_SIZE};

    #[derive(Debug)]
    struct State {
        read: Mutex<()>,
        write: Mutex<()>,
        socket: UnixStream,
        fd: RawFd,
    }

    #[derive(Debug)]
    pub struct AsyncChannel<S, R> {
        state: Arc<State>,
        _p: PhantomData<(S, R)>,
    }

    impl<S, R> AsyncChannel<S, R> {
        pub(super) fn create_from(socket: UnixStream) -> Self {
            let fd = socket.as_raw_fd();
            let read = Mutex::new(());
            let write = Mutex::new(());
            Self {
                state: Arc::new(State {
                    read,
                    write,
                    socket,
                    fd,
                }),
                _p: PhantomData,
            }
        }

        async fn send_with_ancillary<'a>(
            &self,
            _mutex: &MutexGuard<'_, ()>,
            io: &[IoSlice<'_>],
            ancillary: &mut SocketAncillary<'_>,
        ) -> std::io::Result<usize> {
            self.state
                .socket
                .async_io(Interest::WRITABLE, || {
                    let socket =
                        ManuallyDrop::new(unsafe { StdUnixStream::from_raw_fd(self.state.fd) });
                    let result = socket.send_vectored_with_ancillary(io, ancillary);
                    ManuallyDrop::into_inner(socket).into_raw_fd();
                    result
                })
                .await
        }

        async fn recv_with_ancillary(
            &self,
            _mutex: &MutexGuard<'_, ()>,
            io: &mut [IoSliceMut<'_>],
            ancillary: &mut SocketAncillary<'_>,
        ) -> std::io::Result<usize> {
            self.state
                .socket
                .async_io(Interest::READABLE, || {
                    let socket =
                        ManuallyDrop::new(unsafe { StdUnixStream::from_raw_fd(self.state.fd) });
                    let result = socket.recv_vectored_with_ancillary(io, ancillary);
                    ManuallyDrop::into_inner(socket).into_raw_fd();
                    result
                })
                .await
        }
    }

    impl<S, R> AsyncChannel<S, R>
    where
        for<'de> R: Deserialize<'de>,
    {
        /// Receives a message over the channel.
        pub async fn recv(&self) -> Result<R, super::ChannelError> {
            let mut buffer = BUFFER_POOL.take();

            if buffer.len() < PACKET_LENGTH_SIZE {
                buffer.resize(PACKET_LENGTH_SIZE, 0u8);
            }

            let reader = self.state.read.lock().await;
            let mut ancillary_buffer = [0; 128];
            let mut ancillary = SocketAncillary::new(&mut ancillary_buffer);
            let mut q = FdQueue::new();

            let mut bytes = &mut buffer[..PACKET_LENGTH_SIZE];
            while !bytes.is_empty() {
                #[cfg(not(test))]
                let send_bytes = &mut bytes;

                #[cfg(test)]
                let send_bytes = {
                    let len = bytes.len();
                    &mut bytes[..len.min(1024)]
                };

                let len = self
                    .recv_with_ancillary(
                        &reader,
                        &mut [IoSliceMut::new(send_bytes)],
                        &mut ancillary,
                    )
                    .await?;
                q.push_fds(&mut ancillary)?;
                bytes = &mut bytes[len..];
            }
            let len = PacketLength::from_be_bytes(buffer[..PACKET_LENGTH_SIZE].try_into().unwrap())
                as usize;

            if buffer.len() < len {
                buffer.resize(len, 0u8);
            }

            let mut bytes = &mut buffer[..len];
            while !bytes.is_empty() {
                #[cfg(not(test))]
                let send_bytes = &mut bytes;

                #[cfg(test)]
                let send_bytes = {
                    let len = bytes.len();
                    &mut bytes[..len.min(1024)]
                };

                let len = self
                    .recv_with_ancillary(
                        &reader,
                        &mut [IoSliceMut::new(send_bytes)],
                        &mut ancillary,
                    )
                    .await?;
                q.push_fds(&mut ancillary)?;
                bytes = &mut bytes[len..];
            }
            drop(reader);

            let result = super::deserialize(&buffer[..len], q)?;
            Ok(result)
        }
    }

    impl<S, R> AsyncChannel<S, R>
    where
        S: Serialize,
    {
        /// Sends a message over the channel.
        pub async fn send(&self, data: S) -> Result<(), super::ChannelError> {
            let writer = self.state.write.lock().await;
            let (buffer, mut fds) = super::serialize(BUFFER_POOL.take(), data)?;

            let mut ancillary_buffer = [0; 128];
            let mut ancillary = SocketAncillary::new(&mut ancillary_buffer);
            ancillary.add_fds(fds.fds());
            let mut bytes = &buffer[..];

            while !bytes.is_empty() {
                #[cfg(not(test))]
                let send_bytes = &bytes;

                #[cfg(test)]
                let send_bytes = &bytes[..bytes.len().min(1024)];

                let len = self
                    .send_with_ancillary(&writer, &[IoSlice::new(send_bytes)], &mut ancillary)
                    .await?;
                ancillary.clear();
                fds.clear();
                bytes = &bytes[len..];
            }

            Ok(())
        }
    }
}

mod sync_impl {
    use nck_core::BUFFER_POOL;
    use parking_lot::Mutex;
    use serde::{Deserialize, Serialize};
    use std::io::{IoSlice, IoSliceMut, Write};
    use std::os::unix::net::{SocketAncillary, UnixStream};
    use std::{marker::PhantomData, sync::Arc};

    use super::{FdQueue, PacketLength, PACKET_LENGTH_SIZE};

    #[derive(Debug)]
    struct State {
        read: Mutex<UnixStream>,
        write: Mutex<UnixStream>,
    }

    #[derive(Debug)]
    pub struct Channel<S, R> {
        state: Arc<State>,
        _p: PhantomData<(S, R)>,
    }

    impl<S, R> Channel<S, R> {
        pub(super) fn create_from(socket: UnixStream) -> std::io::Result<Self> {
            let (read, write) = (socket.try_clone()?, socket);
            let read = Mutex::new(read);
            let write = Mutex::new(write);
            Ok(Self {
                state: Arc::new(State { read, write }),
                _p: PhantomData,
            })
        }
    }

    impl<S, R> Channel<S, R>
    where
        for<'de> R: Deserialize<'de>,
    {
        /// Receives a message over the channel.
        pub fn recv(&self) -> Result<R, super::ChannelError> {
            let mut buffer = BUFFER_POOL.take();

            if buffer.len() < PACKET_LENGTH_SIZE {
                buffer.resize(PACKET_LENGTH_SIZE, 0u8);
            }

            let reader = self.state.read.lock();
            let mut ancillary_buffer = [0; 128];
            let mut ancillary = SocketAncillary::new(&mut ancillary_buffer);
            let mut q = FdQueue::new();

            let mut bytes = &mut buffer[..PACKET_LENGTH_SIZE];
            while !bytes.is_empty() {
                #[cfg(not(test))]
                let send_bytes = &mut bytes;

                #[cfg(test)]
                let send_bytes = {
                    let len = bytes.len();
                    &mut bytes[..len.min(1024)]
                };

                let len = reader.recv_vectored_with_ancillary(
                    &mut [IoSliceMut::new(send_bytes)],
                    &mut ancillary,
                )?;
                q.push_fds(&mut ancillary)?;
                bytes = &mut bytes[len..];
            }
            let len = PacketLength::from_be_bytes(buffer[..PACKET_LENGTH_SIZE].try_into().unwrap())
                as usize;

            if buffer.len() < len {
                buffer.resize(len, 0u8);
            }

            let mut bytes = &mut buffer[..len];
            while !bytes.is_empty() {
                #[cfg(not(test))]
                let send_bytes = &mut bytes;

                #[cfg(test)]
                let send_bytes = {
                    let len = bytes.len();
                    &mut bytes[..len.min(1024)]
                };

                let len = reader.recv_vectored_with_ancillary(
                    &mut [IoSliceMut::new(send_bytes)],
                    &mut ancillary,
                )?;
                q.push_fds(&mut ancillary)?;
                bytes = &mut bytes[len..];
            }

            super::deserialize(&buffer[..len], q)
        }
    }

    impl<S, R> Channel<S, R>
    where
        S: Serialize,
    {
        /// Sends a message over the channel.
        pub fn send(&self, data: S) -> Result<(), super::ChannelError> {
            let (buffer, mut fds) = super::serialize(BUFFER_POOL.take(), data)?;

            let mut ancillary_buffer = [0; 128];
            let mut ancillary = SocketAncillary::new(&mut ancillary_buffer);
            ancillary.add_fds(fds.fds());

            let mut writer = self.state.write.lock();
            let mut bytes = &buffer[..];

            while !bytes.is_empty() {
                #[cfg(not(test))]
                let send_bytes = bytes;
                #[cfg(test)]
                let send_bytes = &bytes[..(bytes.len().min(1024))];

                let len = writer
                    .send_vectored_with_ancillary(&[IoSlice::new(send_bytes)], &mut ancillary)?;
                ancillary.clear();
                fds.clear();
                bytes = &bytes[len..];
            }
            writer.flush()?;
            Ok(())
        }
    }
}

/// Creates a full duplex channel in the pending state.
pub fn unix_pair<A, B>() -> std::io::Result<(PendingChannel<A, B>, PendingChannel<B, A>)> {
    let (p1, p2) = UnixStream::pair()?;
    Ok((PendingChannel::new(p1), PendingChannel::new(p2)))
}

#[cfg(test)]
mod test {
    use std::time::Duration;

    use super::{unix_pair, PendingChannel};
    use nix::sched::CloneFlags;
    use rstest::rstest;
    use serde::{Deserialize, Serialize};

    /// An intentionally large struct to excercise the custom async code.
    #[derive(Serialize, Deserialize)]
    struct BigThing(Vec<u8>, PendingChannel<i32, i32>);

    impl From<PendingChannel<i32, i32>> for BigThing {
        fn from(value: PendingChannel<i32, i32>) -> Self {
            let size = (u16::MAX as usize) - 32;
            Self(vec![0u8; size], value)
        }
    }

    #[rstest]
    #[timeout(Duration::from_secs(2))]
    fn serialize() -> anyhow::Result<()> {
        let (child_channel, server_channel) = unix_pair::<i32, BigThing>()?;

        super::super::fork::clone(
            Box::new(move || -> anyhow::Result<()> {
                let channel = child_channel.clone().into_peer().unwrap();
                let received_channel = channel.recv().unwrap();
                let received_channel = received_channel.1.into_peer().unwrap();
                received_channel.send(123)?;
                Ok(())
            }),
            CloneFlags::empty(),
        )?;

        let server_channel = server_channel.into_peer()?;
        let (sent_channel, kept_channel) = unix_pair::<i32, i32>()?;
        server_channel.send(sent_channel.into())?;
        let value = kept_channel.into_peer()?.recv()?;
        assert_eq!(value, 123);

        Ok(())
    }

    #[rstest]
    #[timeout(Duration::from_secs(2))]
    fn serialize_async_child() -> anyhow::Result<()> {
        let (child_channel, server_channel) = unix_pair::<i32, BigThing>()?;

        super::super::fork::clone(
            Box::new(move || -> anyhow::Result<()> {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(async {
                        let channel = child_channel.clone().into_peer_async().await.unwrap();
                        let received_channel = channel.recv().await.unwrap();
                        let received_channel = received_channel.1.into_peer_async().await.unwrap();
                        received_channel.send(123).await?;
                        Ok(())
                    })
            }),
            CloneFlags::empty(),
        )?;

        let server_channel = server_channel.into_peer()?;
        let (sent_channel, kept_channel) = unix_pair::<i32, i32>()?;
        server_channel.send(sent_channel.into())?;
        let value = kept_channel.into_peer()?.recv()?;
        assert_eq!(value, 123);

        Ok(())
    }

    #[rstest]
    #[timeout(Duration::from_secs(2))]
    fn serialize_async_parent() -> anyhow::Result<()> {
        let (child_channel, server_channel) = unix_pair::<i32, BigThing>()?;

        super::super::fork::clone(
            Box::new(move || -> anyhow::Result<()> {
                let channel = child_channel.clone().into_peer().unwrap();
                let received_channel = channel.recv().unwrap();
                let received_channel = received_channel.1.into_peer().unwrap();
                received_channel.send(123)?;
                Ok(())
            }),
            CloneFlags::empty(),
        )?;

        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                async fn imp(server_channel: PendingChannel<BigThing, i32>) -> anyhow::Result<()> {
                    let server_channel = server_channel.into_peer_async().await?;
                    let (sent_channel, kept_channel) = unix_pair::<i32, i32>()?;
                    server_channel.send(sent_channel.into()).await?;
                    let value = kept_channel.into_peer_async().await?.recv().await?;
                    assert_eq!(value, 123);
                    Ok(())
                }
                imp(server_channel).await
            })
    }

    #[rstest]
    #[timeout(Duration::from_secs(2))]
    fn serialize_async_both() -> anyhow::Result<()> {
        let (child_channel, server_channel) = unix_pair::<i32, BigThing>()?;

        super::super::fork::clone(
            Box::new(move || -> anyhow::Result<()> {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(async {
                        let channel = child_channel.clone().into_peer_async().await.unwrap();
                        let received_channel = channel.recv().await.unwrap();
                        let received_channel = received_channel.1.into_peer_async().await.unwrap();
                        received_channel.send(123).await?;
                        Ok(())
                    })
            }),
            CloneFlags::empty(),
        )?;

        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                async fn imp(server_channel: PendingChannel<BigThing, i32>) -> anyhow::Result<()> {
                    let server_channel = server_channel.into_peer_async().await?;
                    let (sent_channel, kept_channel) = unix_pair::<i32, i32>()?;
                    server_channel.send(sent_channel.into()).await?;
                    let value = kept_channel.into_peer_async().await?.recv().await?;
                    assert_eq!(value, 123);
                    Ok(())
                }
                imp(server_channel).await
            })
    }
}
