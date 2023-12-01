use std::{
    collections::HashMap,
    io::{ErrorKind, Write},
    ops::{Deref, DerefMut},
    sync::{atomic::AtomicU64, Arc},
};

use bitcode::{Decode, Encode};
use bytes::{BufMut, BytesMut};
use lockfree_object_pool::LinearReusable;
use npk_util::io::{copy_to_buffer, copy_to_buffer_async, BUFFER_POOL};

use tokio::{
    io::AsyncWriteExt,
    net::{
        unix::{OwnedReadHalf, OwnedWriteHalf},
        UnixStream,
    },
    sync::Mutex,
};
use tracing::Span;

use crate::{bitcode_decode, bitcode_pull};

use super::{super::CRC, PeerError};

const LENGTH_SIZE: usize = std::mem::size_of::<usize>();
const CRC_SIZE: usize = std::mem::size_of::<u64>();
const PACKET_HEADER_SIZE: usize = LENGTH_SIZE + CRC_SIZE;

const PACKET_REQUEST: u8 = 0;
const PACKET_RESPONSE: u8 = 1;
const TYPE_SIZE: usize = std::mem::size_of::<u8>();
const ID_SIZE: usize = std::mem::size_of::<u64>();
const OVERLAP_HEADER_SIZE: usize = TYPE_SIZE + ID_SIZE;

fn encode(packet: impl AsRef<[u8]>, buffer: &mut BytesMut) {
    let packet = packet.as_ref();
    let crc = CRC.checksum(packet);
    let len = packet.len().to_ne_bytes();

    buffer.reserve(PACKET_HEADER_SIZE + packet.len());
    buffer.put_slice(&len);
    buffer.put_u64_ne(crc);
    buffer.extend_from_slice(packet);
}

#[derive(Debug)]
struct PeerAsyncState {
    reader: Mutex<OwnedReadHalf>,
    writer: Mutex<OwnedWriteHalf>,
}

#[derive(Debug, Clone)]
pub struct PeerAsync(Arc<PeerAsyncState>);

impl PeerAsync {
    pub fn new(socket: UnixStream) -> Self {
        let (reader, writer) = socket.into_split();
        let reader = Mutex::new(reader);
        let writer = Mutex::new(writer);
        Self(Arc::new(PeerAsyncState { reader, writer }))
    }

    pub async fn write<T: Encode>(&self, value: &T) -> std::io::Result<()> {
        let mut bitcode = bitcode_pull();
        let packet = bitcode.encode(value).unwrap();

        let mut writer = self.0.writer.lock().await;
        let mut buffer = BUFFER_POOL.pull();

        encode(packet, buffer.deref_mut());
        writer.write_all_buf(buffer.deref_mut()).await
    }

    pub async fn read<T: Decode>(&self) -> std::io::Result<T> {
        let mut reader = self.0.reader.lock().await;
        let mut buffer = BUFFER_POOL.pull();

        let data = Self::read_impl(buffer.deref_mut(), reader.deref_mut()).await?;

        bitcode_decode(data)
    }

    async fn read_impl<'a>(
        buffer: &'a mut BytesMut,
        mut reader: &'a mut OwnedReadHalf,
    ) -> Result<&'a [u8], std::io::Error> {
        copy_to_buffer_async(reader, buffer, PACKET_HEADER_SIZE).await?;

        let len = usize::from_ne_bytes(buffer[..LENGTH_SIZE].try_into().unwrap());
        let crc = u64::from_ne_bytes(buffer[LENGTH_SIZE..PACKET_HEADER_SIZE].try_into().unwrap());

        buffer.clear();
        copy_to_buffer_async(&mut reader, buffer, len).await?;
        let data = &buffer[..len];

        if crc != CRC.checksum(data) {
            return Err(std::io::ErrorKind::InvalidData.into());
        }

        Ok(data)
    }
}

#[derive(Debug, Encode, Decode)]
enum PeerResult<T> {
    Ok(T),
    Err(PeerError),
}

type OverlapPacket = LinearReusable<'static, BytesMut>;
type OverlapRequest = (u64, Span, flume::Sender<OverlapPacket>);

#[derive(Debug)]
struct OverlapPeerState {
    writer: Mutex<OwnedWriteHalf>,
    value: AtomicU64,
    response_sender: flume::Sender<OverlapRequest>,
    request_receiver: flume::Receiver<(u64, OverlapPacket)>,
}

#[derive(Debug, Clone)]
pub struct OverlapPeer(Arc<OverlapPeerState>);

impl OverlapPeer {
    pub fn new(socket: UnixStream) -> Self {
        let (reader, writer) = socket.into_split();
        let (response_sender, receiver) = flume::bounded(2);
        let (sender, request_receiver) = flume::bounded(2);
        tokio::spawn(Self::worker(reader, receiver, sender));
        Self(Arc::new(OverlapPeerState {
            writer: Mutex::new(writer),
            value: AtomicU64::new(0),
            response_sender,
            request_receiver,
        }))
    }

    fn next_id(&self) -> u64 {
        self.0
            .value
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    async fn worker(
        mut reader: OwnedReadHalf,
        receiver: flume::Receiver<OverlapRequest>,
        sender: flume::Sender<(u64, OverlapPacket)>,
    ) -> Result<(), ()> {
        let mut map = HashMap::<u64, (Span, flume::Sender<OverlapPacket>)>::new();

        loop {
            let mut buffer = BUFFER_POOL.pull();

            tokio::select! {
                Ok((key, span, responder)) = receiver.recv_async() => {
                    let _ = span.enter();
                    tracing::trace!(key, "waiting for response");
                    map.insert(key, (span, responder));
                    continue;
                }
                Ok(_) = reader.readable() => {},
                else => break Ok(())
            }

            copy_to_buffer_async(
                &mut reader,
                buffer.deref_mut(),
                OVERLAP_HEADER_SIZE + PACKET_HEADER_SIZE,
            )
            .await
            .map_err(|_| ())?;

            loop {
                match receiver.try_recv() {
                    Ok((key, span, responder)) => {
                        let _ = span.enter();
                        tracing::trace!(key, "waiting for response");
                        map.insert(key, (span, responder));
                    }
                    Err(flume::TryRecvError::Empty) => break,
                    Err(flume::TryRecvError::Disconnected) => return Ok(()),
                }
            }

            let ty = buffer[0];
            let key = u64::from_ne_bytes(buffer[1..OVERLAP_HEADER_SIZE].try_into().unwrap());
            let b = &buffer[OVERLAP_HEADER_SIZE..];
            let len = usize::from_ne_bytes(b[..LENGTH_SIZE].try_into().unwrap());
            let crc = u64::from_ne_bytes(b[LENGTH_SIZE..PACKET_HEADER_SIZE].try_into().unwrap());

            buffer.clear();
            copy_to_buffer_async(&mut reader, buffer.deref_mut(), len)
                .await
                .map_err(|_| ())?;

            if crc != CRC.checksum(&buffer) {
                return Ok(());
            }

            if ty == PACKET_RESPONSE {
                tracing::trace!(key, "got response");
                if let Some((_, (span, responder))) = map.remove_entry(&key) {
                    span.entered();
                    if responder.send_async(buffer).await.is_err() {
                        tracing::trace!(key, "could notify thrad of response");
                        return Ok(());
                    }
                } else {
                    tracing::trace!(key, "unknown response");
                }
            } else {
                tracing::trace!(key, "got request");
                if sender.send_async((key, buffer)).await.is_err() {
                    tracing::trace!(key, "could notify thrad of request");
                    return Ok(());
                }
            }
        }
    }

    pub async fn next<T: Decode>(&self) -> std::io::Result<(u64, T)> {
        let (id, data) = self
            .0
            .request_receiver
            .recv_async()
            .await
            .map_err(|_| std::io::Error::from(ErrorKind::ConnectionAborted))?;

        bitcode_decode(data.deref()).map(|v| (id, v))
    }

    pub async fn respond_result<T: Encode>(
        &self,
        id: u64,
        value: Result<T, PeerError>,
    ) -> std::io::Result<()> {
        match value {
            Ok(v) => self.respond(id, &PeerResult::Ok(v)).await,
            Err(v) => self.respond(id, &PeerResult::<T>::Err(v)).await,
        }
    }

    pub async fn respond<T: Encode>(&self, id: u64, value: &T) -> std::io::Result<()> {
        let mut bitcode = bitcode_pull();
        let packet = bitcode.encode(value).unwrap();

        let mut writer = self.0.writer.lock().await;
        let mut buffer = BUFFER_POOL.pull();
        buffer.reserve(OVERLAP_HEADER_SIZE + PACKET_HEADER_SIZE + packet.len());
        buffer.put_u8(PACKET_RESPONSE);
        buffer.put_u64_ne(id);
        encode(packet, buffer.deref_mut());
        writer.write_all_buf(buffer.deref_mut()).await
    }

    pub async fn request_result<R: Decode, W: Encode>(
        &self,
        value: &W,
    ) -> std::io::Result<Result<R, PeerError>> {
        match self.request::<PeerResult<R>, W>(value).await? {
            PeerResult::Ok(v) => Ok(Ok(v)),
            PeerResult::Err(v) => Ok(Err(v)),
        }
    }

    pub async fn request<R: Decode, W: Encode>(&self, value: &W) -> std::io::Result<R> {
        let id = self.next_id();
        let (send, receive) = flume::bounded(1);
        self.0
            .response_sender
            .send_async((id, Span::current(), send))
            .await
            .map_err(|_| std::io::Error::from(ErrorKind::ConnectionAborted))?;

        {
            let mut bitcode = bitcode_pull();
            let packet = bitcode.encode(value).unwrap();
            tracing::trace!("sending {} bytes", packet.len());

            let mut writer = self.0.writer.lock().await;
            let mut buffer = BUFFER_POOL.pull();
            buffer.reserve(OVERLAP_HEADER_SIZE + PACKET_HEADER_SIZE + packet.len());
            buffer.put_u8(PACKET_REQUEST);
            buffer.put_u64_ne(id);
            encode(packet, buffer.deref_mut());
            writer.write_all_buf(buffer.deref_mut()).await?;
        }

        let data = receive
            .recv_async()
            .await
            .map_err(|_| std::io::Error::from(ErrorKind::ConnectionAborted))?;

        bitcode_decode(data.deref())
    }
}

#[derive(Debug)]
pub struct Peer {
    socket: std::os::unix::net::UnixStream,
}

impl Peer {
    pub fn new(socket: std::os::unix::net::UnixStream) -> Self {
        Self { socket }
    }

    pub fn write<T: Encode>(&mut self, value: &T) -> std::io::Result<()> {
        let mut bitcode = bitcode_pull();
        let packet = bitcode.encode(value).unwrap();

        let mut buffer = BUFFER_POOL.pull();
        encode(packet, &mut buffer);

        self.socket.write_all(&buffer)?;

        Ok(())
    }

    pub fn read<T: Decode>(&mut self) -> std::io::Result<T> {
        let mut buffer = BUFFER_POOL.pull();
        copy_to_buffer(&mut self.socket, &mut buffer, PACKET_HEADER_SIZE)?;

        let len = usize::from_ne_bytes(buffer[..LENGTH_SIZE].try_into().unwrap());
        let crc = u64::from_ne_bytes(buffer[LENGTH_SIZE..PACKET_HEADER_SIZE].try_into().unwrap());

        buffer.clear();
        copy_to_buffer(&mut self.socket, &mut buffer, len)?;
        let data = &buffer[..len];

        if crc != CRC.checksum(data) {
            return Err(std::io::ErrorKind::InvalidData.into());
        }

        bitcode_decode(data)
    }
}
