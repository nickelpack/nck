use std::{io::ErrorKind, marker::PhantomData, sync::Arc};

use async_lock::Mutex;
use bitcode::{Decode, Encode};
use futures::{
    io::{ReadHalf, WriteHalf},
    AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt,
};

pub mod proto;

const CRC: crc::Crc<u64> = crc::Crc::<u64>::new(&crc::CRC_64_REDIS);

pub fn create<I, O, S>(s: S) -> IpcConnection<I, O, S>
where
    S: AsyncRead + AsyncWrite + Unpin,
    I: Decode,
    O: Encode,
{
    IpcConnection {
        s: Mutex::new(s),
        p: PhantomData::default(),
    }
}

pub fn create_split<R: AsyncRead + Unpin, W: AsyncWrite + Unpin, I: Decode, O: Encode>(
    r: R,
    w: W,
) -> (IpcSender<O, W>, IpcReceiver<I, R>) {
    (
        IpcSender {
            s: Arc::new(Mutex::new(w)),
            p: Default::default(),
        },
        IpcReceiver {
            s: Mutex::new(r),
            p: Default::default(),
        },
    )
}

pub struct IpcConnection<I, O, S>
where
    S: AsyncRead + AsyncWrite + Unpin,
    I: Decode,
    O: Encode,
{
    s: Mutex<S>,
    p: PhantomData<(I, O)>,
}

impl<I, O, S> IpcConnection<I, O, S>
where
    S: AsyncRead + AsyncWrite + Unpin,
    I: Decode,
    O: Encode,
{
    pub async fn send(&self, m: O) -> Result<(), std::io::Error> {
        send(&self.s, m).await
    }

    pub async fn receive(&self) -> Result<I, std::io::Error> {
        receive(&self.s).await
    }

    pub fn split(self) -> (IpcSender<O, WriteHalf<S>>, IpcReceiver<I, ReadHalf<S>>) {
        let s = self.s.into_inner();
        let (r, w) = s.split();
        create_split(r, w)
    }
}

#[derive(Clone)]
pub struct IpcSender<T, S>
where
    S: AsyncWrite + Unpin,
    T: Encode,
{
    s: Arc<Mutex<S>>,
    p: PhantomData<T>,
}

impl<T, S> IpcSender<T, S>
where
    S: AsyncWrite + Unpin,
    T: Encode,
{
    pub async fn send(&mut self, m: T) -> Result<(), std::io::Error> {
        send(&self.s, m).await
    }
}

async fn send<T, S>(s: &Mutex<S>, m: T) -> Result<(), std::io::Error>
where
    T: Encode,
    S: AsyncWrite + Unpin,
{
    let encoded = bitcode::encode(&m).unwrap();
    let checksum = CRC.checksum(&encoded).to_le_bytes();
    let len = (encoded.len() as u64).to_le_bytes();
    let mut s = s.lock().await;

    s.write_all(&len).await?;
    s.write_all(&checksum).await?;
    s.write(&encoded).await?;

    Ok(())
}

pub struct IpcReceiver<T, S>
where
    S: AsyncRead + Unpin,
    T: Decode,
{
    s: Mutex<S>,
    p: PhantomData<T>,
}

impl<T, S> IpcReceiver<T, S>
where
    S: AsyncRead + Unpin,
    T: Decode,
{
    pub async fn receive(&mut self) -> Result<T, std::io::Error> {
        receive(&self.s).await
    }
}

async fn receive<T, S>(s: &Mutex<S>) -> Result<T, std::io::Error>
where
    T: Decode,
    S: AsyncRead + Unpin,
{
    let mut buffer = [0u8; std::mem::size_of::<u64>() * 2];
    let mut s = s.lock().await;

    s.read_exact(&mut buffer).await?;

    let (len, checksum) = buffer.split_at(std::mem::size_of::<u64>());
    let len = u64::from_le_bytes(len.try_into().expect("provable"));
    let checksum = u64::from_le_bytes(checksum.try_into().expect("provable"));

    if len > 65536 {
        return Err(std::io::Error::from(ErrorKind::OutOfMemory));
    }

    let mut buffer = vec![0u8; len as usize];
    s.read_exact(&mut buffer).await?;
    drop(s);

    let actual_checksum = CRC.checksum(&buffer);

    if checksum != actual_checksum {
        return Err(std::io::Error::from(ErrorKind::InvalidData));
    }

    bitcode::decode(&buffer).map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))
}
