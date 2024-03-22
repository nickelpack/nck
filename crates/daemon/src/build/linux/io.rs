use std::{
    io::{Error, ErrorKind},
    mem::size_of,
    os::{
        fd::{AsRawFd as _, FromRawFd as _, OwnedFd},
        unix::net::UnixStream,
    },
};

use nck_io::pool::BUFFER_POOL;
use serde::{Deserialize, Serialize};
use tokio::io::unix::AsyncFd;
use uds::UnixStreamExt as _;

pub struct EmptyFds;

impl Iterator for EmptyFds {
    type Item = &'static OwnedFd;

    #[inline(always)]
    fn next(&mut self) -> Option<Self::Item> {
        None
    }
}

impl Extend<OwnedFd> for EmptyFds {
    #[inline(always)]
    fn extend<T: IntoIterator<Item = OwnedFd>>(&mut self, iter: T) {
        let mut iter = iter.into_iter();
        while iter.next().is_some() {}
    }
}

pub trait AsyncMessageChannel {
    fn write_message<'a, T, I>(
        &self,
        message: T,
        fds: I,
    ) -> impl std::future::Future<Output = std::io::Result<()>> + Send
    where
        T: Serialize + Send,
        I: IntoIterator<Item = &'a OwnedFd> + Send;

    fn read_message<T, E>(
        &self,
        fds: &mut E,
    ) -> impl std::future::Future<Output = std::io::Result<T>> + Send
    where
        for<'de> T: Deserialize<'de> + Send,
        E: Extend<OwnedFd> + Send;
}

impl AsyncMessageChannel for AsyncFd<UnixStream> {
    async fn write_message<'a, T, I>(&self, message: T, fds: I) -> std::io::Result<()>
    where
        T: Serialize + Send,
        I: IntoIterator<Item = &'a OwnedFd> + Send,
    {
        async fn write_impl(
            socket: &AsyncFd<UnixStream>,
            mut message: &[u8],
            mut fds: &[i32],
        ) -> std::io::Result<()> {
            while !message.is_empty() {
                let mut socket = socket.writable().await?;
                match socket.try_io(|fd| fd.get_ref().send_fds(message, fds)) {
                    Ok(result) => {
                        let size = result?;
                        message = &message[size..];
                        fds = &[];
                    }
                    Err(_would_block) => {}
                }
            }
            Ok(())
        }

        let fds = fds.into_iter().map(|v| v.as_raw_fd()).collect::<Vec<_>>();

        let message = postcard::to_stdvec(&message).unwrap();
        let len_bytes = message.len().to_ne_bytes();

        // Send the fds with the len because a zero-size write cannot carry a fd.
        tracing::trace!(
            len = message.len(),
            message_type = std::any::type_name::<T>(),
            ?fds,
            "sending message"
        );
        write_impl(self, &len_bytes, &fds).await?;
        write_impl(self, &message, &[]).await?;
        tracing::trace!(
            len = message.len(),
            message_type = std::any::type_name::<T>(),
            "sent message"
        );

        Ok(())
    }

    async fn read_message<T, E>(&self, fds: &mut E) -> std::io::Result<T>
    where
        for<'de> T: Deserialize<'de> + Send,
        E: Extend<OwnedFd> + Send,
    {
        async fn read_impl<EE>(
            socket: &AsyncFd<UnixStream>,
            mut message: &mut [u8],
            fds: &mut EE,
        ) -> std::io::Result<()>
        where
            EE: Extend<OwnedFd> + Send,
        {
            while !message.is_empty() {
                let mut socket = socket.readable().await?;
                let mut fds_buf = [0; 16];
                match socket.try_io(|fd| fd.get_ref().recv_fds(message, &mut fds_buf)) {
                    Ok(Ok((size, fd_size))) => {
                        message = &mut message[size..];
                        fds.extend(
                            fds_buf[..fd_size]
                                .iter()
                                .map(|v| unsafe { OwnedFd::from_raw_fd(*v) }),
                        );
                    }
                    Ok(Err(e)) => return Err(e),
                    Err(_would_block) => {}
                }
            }
            Ok(())
        }

        let mut len = [0; size_of::<usize>()];
        read_impl(self, &mut len, fds).await?;
        let len = usize::from_ne_bytes(len);

        let mut buf = BUFFER_POOL.take();
        buf.resize(len, 0u8);
        read_impl(self, &mut buf[..len], fds).await?;

        postcard::from_bytes(&buf[..len]).map_err(|_| Error::from(ErrorKind::InvalidData))
    }
}

pub trait MessageChannel {
    fn write_message<'a, T, I>(&self, message: T, fds: I) -> std::io::Result<()>
    where
        T: Serialize,
        I: IntoIterator<Item = &'a OwnedFd> + Send;

    fn read_message<T, E>(&self, fds: &mut E) -> std::io::Result<T>
    where
        for<'de> T: Deserialize<'de>,
        E: Extend<OwnedFd>;
}

impl MessageChannel for UnixStream {
    fn write_message<'a, T, I>(&self, message: T, fds: I) -> std::io::Result<()>
    where
        T: Serialize,
        I: IntoIterator<Item = &'a OwnedFd>,
    {
        fn write_impl(
            socket: &UnixStream,
            mut message: &[u8],
            mut fds: &[i32],
        ) -> std::io::Result<()> {
            while !message.is_empty() {
                let size = socket.send_fds(message, fds)?;
                message = &message[size..];
                fds = &[];
            }
            Ok(())
        }

        let fds = fds.into_iter().map(|v| v.as_raw_fd()).collect::<Vec<_>>();

        let message = postcard::to_stdvec(&message).unwrap();
        let len_bytes = message.len().to_ne_bytes();

        // Send the fds with the len because a zero-size write cannot carry a fd.
        write_impl(self, &len_bytes, &fds)?;
        write_impl(self, &message, &[])?;

        Ok(())
    }

    fn read_message<T, E>(&self, fds: &mut E) -> std::io::Result<T>
    where
        for<'de> T: Deserialize<'de>,
        E: Extend<OwnedFd>,
    {
        fn read_impl<EE>(
            socket: &UnixStream,
            mut message: &mut [u8],
            fds: &mut EE,
        ) -> std::io::Result<()>
        where
            EE: Extend<OwnedFd>,
        {
            while !message.is_empty() {
                let mut fds_buf = [0; 16];
                let (size, fd_size) = socket.recv_fds(message, &mut fds_buf)?;
                message = &mut message[size..];
                fds.extend(
                    fds_buf[..fd_size]
                        .iter()
                        .map(|v| unsafe { OwnedFd::from_raw_fd(*v) }),
                );
            }
            Ok(())
        }

        let mut len = [0; size_of::<usize>()];
        read_impl(self, &mut len, fds)?;
        let len = usize::from_ne_bytes(len);

        let mut buf = BUFFER_POOL.take();
        buf.resize(len, 0u8);
        read_impl(self, &mut buf[..len], fds)?;

        postcard::from_bytes(&buf[..len]).map_err(|_| Error::from(ErrorKind::InvalidData))
    }
}

pub trait ChannelError {
    fn is_closed_channel(&self) -> bool;
}

impl ChannelError for Error {
    fn is_closed_channel(&self) -> bool {
        matches!(
            self.kind(),
            ErrorKind::ConnectionReset
                | ErrorKind::ConnectionAborted
                | ErrorKind::NotConnected
                | ErrorKind::BrokenPipe
        )
    }
}
