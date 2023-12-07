use std::{
    io::{ErrorKind, Write},
    ops::{Deref, DerefMut},
    os::unix::net::UnixStream,
    sync::{atomic::AtomicU32, Arc},
};

use bytes::{BufMut, BytesMut};
use crc::{Crc, Digest};
use dashmap::DashMap;
use flume::{Receiver, SendError, Sender};
use speedy::{LittleEndian, Readable, Writable};
use tokio::{
    io::AsyncWriteExt,
    net::unix::{OwnedReadHalf, OwnedWriteHalf},
    sync::{oneshot, Mutex},
};
use tracing::Instrument;

use crate::{
    io::{copy_to_buffer, copy_to_buffer_async},
    pool::Pooled,
    BUFFER_POOL,
};

type HeaderCrc = u64;
type HeaderLength = u32;
type HeaderType = u8;
type HeaderId = u32;
type AtomicHeaderId = AtomicU32;

static CRC: Crc<HeaderCrc> = Crc::<HeaderCrc>::new(&crc::CRC_64_REDIS);
const LENGTH_SIZE: usize = std::mem::size_of::<HeaderLength>();
const TYPE_SIZE: usize = std::mem::size_of::<HeaderType>();
const ID_SIZE: usize = std::mem::size_of::<HeaderId>();
const CRC_SIZE: usize = std::mem::size_of::<HeaderCrc>();

const HEADER_SIZE: usize = LENGTH_SIZE + TYPE_SIZE + ID_SIZE + CRC_SIZE;
const CRC_OFFSET: usize = 0;
const LENGTH_OFFSET: usize = CRC_OFFSET + CRC_SIZE;
const TYPE_OFFSET: usize = LENGTH_OFFSET + LENGTH_SIZE;
const ID_OFFSET: usize = TYPE_OFFSET + TYPE_SIZE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PacketType {
    Request,
    Response,
    StreamData,
    StreamEnd,
    StreamError,
}

impl From<PacketType> for HeaderType {
    fn from(value: PacketType) -> Self {
        match value {
            PacketType::Request => 0,
            PacketType::Response => 1,
            PacketType::StreamData => 2,
            PacketType::StreamEnd => 3,
            PacketType::StreamError => 4,
        }
    }
}

impl TryFrom<HeaderType> for PacketType {
    type Error = HeaderType;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Request),
            1 => Ok(Self::Response),
            2 => Ok(Self::StreamData),
            3 => Ok(Self::StreamEnd),
            4 => Ok(Self::StreamError),
            o => Err(o),
        }
    }
}

#[inline]
fn no_write(_: &mut BytesMut) -> std::io::Result<()> {
    Ok(())
}

fn write<F: FnOnce(&mut BytesMut) -> std::io::Result<()>>(
    ty: PacketType,
    id: HeaderId,
    write: F,
    buffer: &mut BytesMut,
    length_hint: Option<usize>,
) -> std::io::Result<()> {
    buffer.reserve(length_hint.unwrap_or_default() + HEADER_SIZE);
    buffer.put_bytes(0u8, HEADER_SIZE);
    write(buffer)?;

    let len = buffer.len() - HEADER_SIZE;
    if len > HeaderLength::MAX as usize {
        return Err(std::io::Error::from(ErrorKind::InvalidInput));
    }

    let len = len as HeaderLength;
    buffer[LENGTH_OFFSET..][..LENGTH_SIZE].copy_from_slice(&len.to_le_bytes());
    buffer[TYPE_OFFSET..][..TYPE_SIZE].copy_from_slice(&Into::<HeaderType>::into(ty).to_le_bytes());
    buffer[ID_OFFSET..][..ID_SIZE].copy_from_slice(&id.to_le_bytes());

    let crc = CRC.checksum(&buffer[LENGTH_OFFSET..]) as HeaderCrc;
    buffer[CRC_OFFSET..][..CRC_SIZE].copy_from_slice(&crc.to_le_bytes());
    Ok(())
}

fn parse_header(buffer: &BytesMut) -> std::io::Result<(OverlapHeader, Digest<'static, u64>)> {
    let length =
        HeaderLength::from_le_bytes(buffer[LENGTH_OFFSET..][..LENGTH_SIZE].try_into().unwrap())
            as usize;
    let ty = HeaderType::from_le_bytes(buffer[TYPE_OFFSET..][..TYPE_SIZE].try_into().unwrap());
    let id = HeaderId::from_le_bytes(buffer[ID_OFFSET..][..ID_SIZE].try_into().unwrap());
    let read_crc = HeaderCrc::from_le_bytes(buffer[CRC_OFFSET..][..CRC_SIZE].try_into().unwrap());

    let ty = ty.try_into().map_err(|e| {
        tracing::error!("invalid packet type {:x}", e);
        std::io::Error::from(ErrorKind::InvalidData)
    })?;

    let mut crc = CRC.digest();
    crc.update(&buffer[LENGTH_OFFSET..HEADER_SIZE]);
    Ok((
        OverlapHeader {
            length,
            ty,
            id,
            crc: read_crc,
        },
        crc,
    ))
}

fn validate_crc(
    read_crc: HeaderCrc,
    mut crc: Digest<'static, HeaderCrc>,
    buffer: &BytesMut,
) -> std::io::Result<()> {
    crc.update(buffer);

    let actual_crc = crc.finalize();
    if actual_crc == read_crc {
        Ok(())
    } else {
        tracing::error!("CRC validation failed");
        Err(std::io::Error::from(ErrorKind::InvalidData))
    }
}

type OverlapBuffer = Pooled<'static, BytesMut>;

#[derive(Debug)]
struct OverlapRequest {
    responder: oneshot::Sender<OverlapBuffer>,
}

#[derive(Debug)]
struct OverlapHeader {
    id: HeaderId,
    ty: PacketType,
    length: usize,
    crc: HeaderCrc,
}

#[derive(Debug, Readable, Writable)]
enum PeerResult<T, E> {
    Ok(T),
    Err(E),
}

#[derive(Debug)]
struct OverlapPeerState {
    writer: Mutex<OwnedWriteHalf>,
    streams: DashMap<HeaderId, Sender<OverlapBuffer>>,
    requests: DashMap<HeaderId, OverlapRequest>,
    value: AtomicHeaderId,
    request_receiver: flume::Receiver<(HeaderId, OverlapBuffer)>,
}

/// A token that can be used to write back a response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OverlapToken(HeaderId);

/// A packet received from `AsyncPeer` or `SyncPeer`.
#[derive(Debug)]
pub struct OverlapPacket<T>(OverlapToken, T);

impl<T> OverlapPacket<T> {
    /// Gets the token of the packet.
    pub fn id(&self) -> OverlapToken {
        self.0
    }

    /// Converts the packet into a token and a value.
    pub fn into_inner(self) -> (OverlapToken, T) {
        (self.0, self.1)
    }
}

impl<T> Deref for OverlapPacket<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.1
    }
}

impl<T> From<OverlapPacket<T>> for OverlapToken {
    #[inline]
    fn from(val: OverlapPacket<T>) -> Self {
        val.0
    }
}

impl<T> From<&OverlapPacket<T>> for OverlapToken {
    #[inline]
    fn from(val: &OverlapPacket<T>) -> Self {
        val.0
    }
}

impl<T> AsRef<T> for OverlapPacket<T> {
    #[inline]
    fn as_ref(&self) -> &T {
        &self.1
    }
}

impl<T> DerefMut for OverlapPacket<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.1
    }
}

impl<T> AsMut<T> for OverlapPacket<T> {
    #[inline]
    fn as_mut(&mut self) -> &mut T {
        &mut self.1
    }
}

/// One side of a connection.
#[derive(Debug, Clone)]
pub struct AsyncPeer(Arc<OverlapPeerState>);

impl AsyncPeer {
    /// Creates a new peer from a Unix socket.
    pub fn new((reader, writer): (OwnedReadHalf, OwnedWriteHalf)) -> Self {
        let (sender, request_receiver) = flume::unbounded();
        let state = Arc::new(OverlapPeerState {
            writer: Mutex::new(writer),
            streams: DashMap::new(),
            requests: DashMap::new(),
            value: AtomicHeaderId::new(0),
            request_receiver,
        });
        tokio::spawn(Self::worker(state.clone(), reader, sender).in_current_span());
        Self(state)
    }

    fn next_id(&self) -> HeaderId {
        self.0
            .value
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    async fn worker(
        state: Arc<OverlapPeerState>,
        reader: OwnedReadHalf,
        sender: flume::Sender<(HeaderId, OverlapBuffer)>,
    ) {
        match Self::worker_impl(state, reader, sender)
            .in_current_span()
            .await
        {
            Err(error) if error.kind() != ErrorKind::ConnectionAborted => {
                tracing::error!(?error, "worker failed");
            }
            _ => {}
        }
    }

    #[tracing::instrument(name = "network", skip_all)]
    async fn worker_impl(
        state: Arc<OverlapPeerState>,
        mut reader: OwnedReadHalf,
        sender: flume::Sender<(HeaderId, OverlapBuffer)>,
    ) -> Result<(), std::io::Error> {
        loop {
            let mut buffer = BUFFER_POOL.take();
            copy_to_buffer_async(&mut reader, buffer.deref_mut(), HEADER_SIZE)
                .in_current_span()
                .await
                .map_err(|e| match e.kind() {
                    // If the buffer is empty then it means the connection terminated cleanly
                    ErrorKind::UnexpectedEof => std::io::Error::from(ErrorKind::ConnectionAborted),
                    _ => e,
                })?;

            let (header, crc) = parse_header(&buffer)?;

            tracing::trace!(?header, "received header");

            buffer.clear();
            copy_to_buffer_async(&mut reader, buffer.deref_mut(), header.length)
                .in_current_span()
                .await?;

            // CRC errors are fatal
            validate_crc(header.crc, crc, &buffer)?;

            Self::handle_packet(&state, header, buffer, &sender).await?;
        }
    }

    #[tracing::instrument(level = "trace", skip_all, fields(r#type = ?header.ty, packet_id = header.id))]
    async fn handle_packet(
        state: &OverlapPeerState,
        header: OverlapHeader,
        buffer: OverlapBuffer,
        sender: &flume::Sender<(HeaderId, OverlapBuffer)>,
    ) -> std::io::Result<()> {
        match header.ty {
            PacketType::Request => {
                if sender.send_async((header.id, buffer)).await.is_err() {
                    tracing::trace!("remote peer disconnected");
                    return Ok(());
                }
            }
            PacketType::Response => {
                if let Some(OverlapRequest { responder, .. }) =
                    state.requests.remove(&header.id).map(|(_, v)| v)
                {
                    if responder.send(buffer).is_err() {
                        tracing::warn!("nothing to handle the response");
                    }
                } else {
                    tracing::warn!("unknown response");
                }
            }
            PacketType::StreamData => {
                if let Some(sender) = state.streams.get(&header.id) {
                    if let Err(SendError(mut buffer)) = sender.send_async(buffer).await {
                        tracing::trace!("handler for stream data disconnected");
                        state.streams.remove(&header.id);

                        buffer.clear();
                        write(
                            PacketType::StreamError,
                            header.id,
                            no_write,
                            &mut buffer,
                            None,
                        )?;

                        let mut writer = state.writer.lock().await;
                        writer.write_all_buf(buffer.deref_mut()).await?;
                    }
                }
            }
            PacketType::StreamEnd => {
                state.streams.remove(&header.id);
                tracing::trace!("stream disconnected");
            }
            PacketType::StreamError => {
                if state.streams.remove(&header.id).is_some() {
                    tracing::trace!("stream failed");
                }
            }
        }
        Ok(())
    }

    /// Gets the next request from the remove peer.
    pub async fn next<'a, T: Readable<'a, LittleEndian>>(
        &self,
    ) -> std::io::Result<OverlapPacket<T>> {
        let (id, data) = self
            .0
            .request_receiver
            .recv_async()
            .await
            .map_err(|_| std::io::Error::from(ErrorKind::ConnectionAborted))?;

        T::read_from_buffer_copying_data(data.get())
            .map(|value| OverlapPacket(OverlapToken(id), value))
            .map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))
    }

    /// Gets a reader for a stream ID.
    pub async fn read_stream(&self, id: HeaderId) -> Receiver<OverlapBuffer> {
        let (sender, receiver) = flume::bounded(4);
        self.0.streams.insert(id, sender);
        receiver
    }

    /// Gets a writer for a stream ID.
    #[tracing::instrument(level = "trace", skip_all)]
    pub async fn write_stream(&self, id: HeaderId) -> Sender<OverlapBuffer> {
        let (sender, receiver) = flume::bounded::<OverlapBuffer>(4);

        let state = self.0.clone();
        tokio::spawn(
            async move {
                while let Ok(v) = receiver.recv_async().await {
                    let mut buffer = BUFFER_POOL.take();
                    let read_buffer = |buffer: &mut BytesMut| {
                        buffer.put_slice(v.as_ref());
                        Ok(())
                    };

                    if write(
                        PacketType::StreamData,
                        id,
                        read_buffer,
                        &mut buffer,
                        Some(v.len()),
                    )
                    .is_err()
                    {
                        return;
                    }

                    let mut writer = state.writer.lock().await;
                    if writer.write_all_buf(buffer.deref_mut()).await.is_err() {
                        return;
                    }
                }

                let mut buffer = BUFFER_POOL.take();
                if write(PacketType::StreamEnd, id, no_write, &mut buffer, None).is_err() {
                    return;
                }

                let mut writer = state.writer.lock().await;
                writer.write_all_buf(buffer.deref_mut()).await.ok();
            }
            .in_current_span(),
        );

        sender
    }

    /// Responds to a request with a `Result`.
    pub async fn respond_result<T: Writable<LittleEndian>, E: Writable<LittleEndian>>(
        &self,
        id: impl Into<OverlapToken>,
        value: Result<T, E>,
    ) -> std::io::Result<()> {
        match value {
            Ok(v) => {
                self.respond(id, &PeerResult::<T, E>::Ok(v))
                    .in_current_span()
                    .await
            }
            Err(v) => {
                self.respond(id, &PeerResult::<T, E>::Err(v))
                    .in_current_span()
                    .await
            }
        }
    }

    /// Responds to a request.
    #[tracing::instrument(level = "TRACE", skip_all)]
    pub async fn respond<T: Writable<LittleEndian>>(
        &self,
        id: impl Into<OverlapToken>,
        value: &T,
    ) -> std::io::Result<()> {
        let id = Into::<OverlapToken>::into(id).0;
        let mut buffer = BUFFER_POOL.take();
        write(
            PacketType::Response,
            id,
            |buffer| {
                value
                    .write_to_stream(buffer.writer())
                    .map_err(|e| std::io::Error::new(ErrorKind::Other, e))
            },
            &mut buffer,
            None,
        )?;

        tracing::trace!(?id, "responding with {} bytes", buffer.len());
        let mut writer = self.0.writer.lock().await;
        writer.write_all_buf(buffer.deref_mut()).await
    }

    /// Performs a request that will complete with a `Result`.
    pub async fn request_result<
        'a,
        O: Readable<'a, LittleEndian>,
        E: Readable<'a, LittleEndian>,
        R: Writable<LittleEndian>,
    >(
        &self,
        value: &R,
    ) -> std::io::Result<Result<O, E>> {
        match self
            .request::<PeerResult<O, E>, R>(value)
            .in_current_span()
            .await?
        {
            PeerResult::Ok(v) => Ok(Ok(v)),
            PeerResult::Err(v) => Ok(Err(v)),
        }
    }

    /// Performs a request.
    #[tracing::instrument(level = "trace", skip_all)]
    pub async fn request<'a, R: Readable<'a, LittleEndian>, S: Writable<LittleEndian>>(
        &self,
        value: &S,
    ) -> std::io::Result<R> {
        let key = self.next_id();
        let (responder, receive) = oneshot::channel();

        self.0.requests.insert(key, OverlapRequest { responder });

        let mut buffer = BUFFER_POOL.take();
        write(
            PacketType::Request,
            key,
            |buffer| {
                value
                    .write_to_stream(buffer.writer())
                    .map_err(|e| std::io::Error::new(ErrorKind::Other, e))
            },
            &mut buffer,
            None,
        )?;

        let mut writer = self.0.writer.lock().await;
        writer.write_all_buf(buffer.deref_mut()).await?;
        drop(writer);
        drop(buffer);

        tracing::trace!(key, "waiting for response");
        let data = receive
            .await
            .map_err(|_| std::io::Error::from(ErrorKind::ConnectionAborted))?;

        R::read_from_buffer_copying_data(data.get())
            .map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))
    }
}

/// A pared-down version of the peer that can only be used to synchronously service requests.
#[derive(Debug)]
pub struct SyncServer(UnixStream);

impl SyncServer {
    pub fn new(socket: UnixStream) -> Self {
        Self(socket)
    }

    /// Gets the next incoming request.
    pub fn next<'a, R: Readable<'a, LittleEndian>>(&mut self) -> std::io::Result<OverlapPacket<R>> {
        let mut buffer = BUFFER_POOL.take();

        copy_to_buffer(&mut self.0, &mut buffer, HEADER_SIZE)?;
        let (header, crc) = parse_header(&buffer)?;

        assert_eq!(header.ty, PacketType::Request);

        buffer.clear();
        copy_to_buffer(&mut self.0, &mut buffer, header.length)?;
        validate_crc(header.crc, crc, &buffer)?;

        let v = R::read_from_buffer_copying_data(&buffer)
            .map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))?;
        Ok(OverlapPacket(OverlapToken(header.id), v))
    }

    /// Responds to a request with a `Result`.
    pub fn respond_result<T: Writable<LittleEndian>, E: Writable<LittleEndian>>(
        &mut self,
        id: impl Into<OverlapToken>,
        value: Result<T, E>,
    ) -> std::io::Result<()> {
        match value {
            Ok(v) => self.respond(id, &PeerResult::<T, E>::Ok(v)),
            Err(v) => self.respond(id, &PeerResult::<T, E>::Err(v)),
        }
    }

    /// Responds to a request.
    pub fn respond<W: Writable<LittleEndian>>(
        &mut self,
        id: impl Into<OverlapToken>,
        value: &W,
    ) -> std::io::Result<()> {
        let mut buffer = BUFFER_POOL.take();

        write(
            PacketType::Response,
            Into::<OverlapToken>::into(id).0,
            |buffer| {
                value
                    .write_to_stream(buffer.writer())
                    .map_err(|e| std::io::Error::new(ErrorKind::Other, e))
            },
            &mut buffer,
            None,
        )?;

        self.0.write_all(&buffer)
    }
}

#[cfg(test)]
mod test {}
