use std::{
    io::{BorrowedBuf, Error, ErrorKind, Read, Result},
    pin::Pin,
    task::Poll,
};

use bytes::BytesMut;
use tokio::io::{AsyncRead, ReadBuf};

/// Extension trait for [`BytesMut`].
pub trait BytesMutExt: crate::sealed::Sealed {
    /// Extends the buffer to a specific total length.
    fn extend_from_reader(&mut self, reader: &mut impl Read, total_length: usize) -> Result<()>;

    /// Extends the buffer to a specific total length.
    fn extend_from_reader_async(
        &mut self,
        reader: &mut (impl AsyncRead + Unpin + Send),
        len: usize,
    ) -> impl std::future::Future<Output = Result<()>> + Send;

    /// Extends the buffer to a specific total length.
    fn poll_extend_from_reader(
        &mut self,
        reader: &mut (impl AsyncRead + Unpin),
        len: usize,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<()>>;
}

impl crate::sealed::Sealed for BytesMut {}

impl BytesMutExt for BytesMut {
    fn extend_from_reader(&mut self, reader: &mut impl Read, len: usize) -> Result<()> {
        let len = len.saturating_sub(self.len());
        if len == 0 {
            return Ok(());
        }

        self.reserve(len);

        let mut buffer = BorrowedBuf::from(&mut self.spare_capacity_mut()[..len]);
        reader.read_buf_exact(buffer.unfilled())?;
        let len = self.len() + len;
        unsafe { self.set_len(len) };
        Ok(())
    }

    async fn extend_from_reader_async(
        &mut self,
        reader: &mut (impl AsyncRead + Unpin + Send),
        len: usize,
    ) -> Result<()> {
        use tokio::io::AsyncReadExt;

        let remaining = len.saturating_sub(self.len());
        if remaining == 0 {
            return Ok(());
        }

        self.reserve(remaining);

        let mut spare = &mut self.spare_capacity_mut()[..remaining];
        while !spare.is_empty() {
            let mut buf = ReadBuf::uninit(spare);
            reader.read_buf(&mut buf).await?;
            let read = buf.filled().len();
            if read == 0 {
                break;
            }
            spare = &mut spare[read..];
        }

        let spare_len = spare.len();
        let len = len - spare_len;
        unsafe { self.set_len(len) };
        if spare_len == 0 {
            Ok(())
        } else {
            Err(Error::from(ErrorKind::UnexpectedEof))
        }
    }

    fn poll_extend_from_reader(
        &mut self,
        reader: &mut (impl AsyncRead + Unpin),
        len: usize,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<()>> {
        let start = self.len();
        let remaining = len.saturating_sub(start);
        if remaining == 0 {
            return Poll::Ready(Ok(()));
        }

        self.reserve(remaining);
        let mut buf = ReadBuf::uninit(&mut self.spare_capacity_mut()[..remaining]);

        if AsyncRead::poll_read(Pin::new(reader), cx, &mut buf).is_pending() {
            return Poll::Pending;
        }

        let len = buf.filled_mut().len();
        if len == 0 {
            Poll::Ready(Err(ErrorKind::UnexpectedEof.into()))
        } else {
            unsafe {
                self.set_len(start + len);
            }
            Poll::Ready(Ok(()))
        }
    }
}
