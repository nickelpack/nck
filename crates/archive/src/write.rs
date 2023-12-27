use std::{io::Write, ops::Range, os::unix::prelude::*, pin::Pin, task::Poll};

use bytes::BytesMut;
use nck_hashing::{SupportedHash, SupportedHasher};
use nck_io::pool::{Pooled, BUFFER_POOL};
use tokio::io::{AsyncWrite, AsyncWriteExt};

use crate::{
    hash_data, Entry, EntryTarget, ENTRY_DATA, ENTRY_ENTRY, MAX_LENGTH, TYPE_DATA, TYPE_DIR,
    TYPE_LINK,
};

#[derive(Debug)]
pub struct Writer<T> {
    writer: T,
}

impl<T> Writer<T> {
    pub fn into_inner(self) -> T {
        self.writer
    }
}

impl<T: Write> Writer<T> {
    pub fn new(mut writer: T) -> std::io::Result<Self> {
        writer.write_all(b"NCK00")?;
        Ok(Self { writer })
    }

    /// Writes a binary blob to the output stream.
    pub fn write_data(
        mut self,
        hasher: SupportedHasher,
    ) -> std::io::Result<DataWriter<'static, T>> {
        self.writer.write_all(&[ENTRY_DATA])?;
        Ok(DataWriter {
            writer: self,
            hash: hasher,
            buffer: BUFFER_POOL.take(),
            range: 0..0,
        })
    }

    /// Writes an entry to the output stream.
    pub fn write_entry(&mut self, entry: Entry) -> std::io::Result<()> {
        self.writer.write_all(&[ENTRY_ENTRY])?;

        let path = entry.path.as_os_str().as_bytes();
        self.write_length_prefixed(path)?;

        match entry.target {
            EntryTarget::Data(hash, flags) => {
                let (id, bytes) = hash_data(&hash);
                self.writer.write_all(&[TYPE_DATA, id])?;
                self.writer.write_all(bytes)?;
                self.writer.write_all(&flags.bits().to_be_bytes())?;
            }
            EntryTarget::Link(dest, flags) => {
                let path = dest.as_os_str().as_bytes();
                self.writer.write_all(&[TYPE_LINK])?;
                self.write_length_prefixed(path)?;
                self.writer.write_all(&flags.bits().to_be_bytes())?;
            }
            EntryTarget::Directory => {
                self.writer.write_all(&[TYPE_DIR])?;
            }
        }

        Ok(())
    }

    fn write_length_prefixed(&mut self, buf: &[u8]) -> Result<(), std::io::Error> {
        if buf.len() > MAX_LENGTH {
            return Err(std::io::ErrorKind::InvalidInput.into());
        }
        self.writer.write_all(&(buf.len() as u16).to_be_bytes())?;
        self.writer.write_all(buf)?;
        Ok(())
    }
}

impl<T: AsyncWrite + Unpin> Writer<T> {
    pub async fn new_async(mut writer: T) -> std::io::Result<Self> {
        writer.write_all(b"NCK00").await?;
        Ok(Self { writer })
    }

    /// Writes a binary blob to the output stream.
    pub async fn write_data_async(
        mut self,
        hasher: SupportedHasher,
    ) -> std::io::Result<DataWriter<'static, T>> {
        self.writer.write_all(&[ENTRY_DATA]).await?;
        Ok(DataWriter {
            writer: self,
            hash: hasher,
            buffer: BUFFER_POOL.take(),
            range: 0..0,
        })
    }

    /// Writes an entry to the output stream.
    pub async fn write_entry_async(&mut self, entry: Entry) -> std::io::Result<()> {
        self.writer.write_all(&[ENTRY_ENTRY]).await?;

        let path = entry.path.as_os_str().as_bytes();
        self.write_length_prefixed_async(path).await?;

        match entry.target {
            EntryTarget::Data(hash, flags) => {
                let (id, bytes) = hash_data(&hash);
                self.writer.write_all(&[TYPE_DATA, id]).await?;
                self.writer.write_all(bytes).await?;
                self.writer.write_all(&flags.bits().to_be_bytes()).await?;
            }
            EntryTarget::Link(dest, flags) => {
                let path = dest.as_os_str().as_bytes();
                self.writer.write_all(&[TYPE_LINK]).await?;
                self.write_length_prefixed_async(path).await?;
                self.writer.write_all(&flags.bits().to_be_bytes()).await?;
            }
            EntryTarget::Directory => {
                self.writer.write_all(&[TYPE_DIR]).await?;
            }
        }

        Ok(())
    }

    async fn write_length_prefixed_async(&mut self, buf: &[u8]) -> Result<(), std::io::Error> {
        if buf.len() > MAX_LENGTH {
            return Err(std::io::ErrorKind::InvalidInput.into());
        }
        self.writer
            .write_all(&(buf.len() as u16).to_be_bytes())
            .await?;
        self.writer.write_all(buf).await?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct DataWriter<'a, T> {
    writer: Writer<T>,
    hash: SupportedHasher,
    buffer: Pooled<'a, BytesMut>,
    range: Range<usize>,
}

impl<'a, T> DataWriter<'a, T> {
    fn split(&mut self) -> (&mut Writer<T>, &mut Pooled<'a, BytesMut>, &mut Range<usize>) {
        (&mut self.writer, &mut self.buffer, &mut self.range)
    }
}

impl<'a, T: Write + Unpin> DataWriter<'a, T> {
    pub fn finish(mut self) -> std::io::Result<(Writer<T>, SupportedHash)> {
        self.writer.writer.write_all(&0u16.to_be_bytes())?;

        let hash = self.hash.finalize();
        let (id, bytes) = hash_data(&hash);
        self.writer.writer.write_all(&[id])?;
        self.writer.writer.write_all(bytes)?;
        Ok((self.writer, hash))
    }
}

impl<'a, T: Write> Write for DataWriter<'a, T> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        while !self.range.is_empty() {
            let (writer, buffer, range) = self.split();

            let len = writer.writer.write(&buffer[range.clone()])?;
            range.start += len;
            if Range::is_empty(range) {
                // Subtract the length prefix from the length
                return Ok(self.range.end - 2);
            }
        }

        let to_write = buf.len().min(MAX_LENGTH);
        self.hash.update(&buf[..to_write]);

        self.buffer.clear();
        self.buffer
            .extend_from_slice(&(to_write as u16).to_be_bytes());
        self.buffer.extend_from_slice(&buf[..to_write]);
        self.range = 0..self.buffer.len();

        self.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.writer.writer.flush()
    }
}

impl<'a, T: AsyncWrite + Unpin> DataWriter<'a, T> {
    pub async fn finish_async(mut self) -> std::io::Result<(Writer<T>, SupportedHash)> {
        self.writer.writer.write_all(&0u16.to_be_bytes()).await?;

        let hash = self.hash.finalize();
        let (id, bytes) = hash_data(&hash);
        self.writer.writer.write_all(&[id]).await?;
        self.writer.writer.write_all(bytes).await?;
        Ok((self.writer, hash))
    }
}

impl<'a, T: AsyncWrite + Unpin> AsyncWrite for DataWriter<'a, T> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }

        while !self.range.is_empty() {
            let (writer, buffer, range) = self.split();

            let len = match Pin::new(&mut writer.writer).poll_write(cx, &buffer[range.clone()]) {
                Poll::Ready(Ok(len)) => len,
                other => return other,
            };

            range.start += len;
            if Range::is_empty(range) {
                // Subtract the length prefix from the length
                return Poll::Ready(Ok(self.range.end - 2));
            }
        }

        let to_write = buf.len().min(MAX_LENGTH);
        self.hash.update(&buf[..to_write]);

        self.buffer.clear();
        self.buffer
            .extend_from_slice(&(to_write as u16).to_be_bytes());
        self.buffer.extend_from_slice(&buf[..to_write]);
        self.range = 0..self.buffer.len();

        self.poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.writer.writer).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.writer.writer).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod test {
    use nck_hashing::{SupportedHash, SupportedHasher};
    use nck_io::PrintableBuffer;
    use pretty_assertions::assert_eq;

    use crate::{Entry, EntryFlags, Writer};

    type Result = anyhow::Result<()>;

    fn make_blake(data: impl AsRef<[u8]>) -> SupportedHash {
        let mut result = SupportedHasher::blake3();
        result.update(data);
        result.finalize()
    }

    #[test]
    pub fn write_entry() -> Result {
        let dest = Vec::new();
        let mut writer = Writer::new(dest)?;
        writer.write_entry(Entry::data(
            "/tmp/test",
            make_blake("test1"),
            Some(EntryFlags::EXECUTABLE),
        ))?;
        writer.write_entry(Entry::link("/tmp/test", "../../test", None))?;
        writer.write_entry(Entry::directory("/tmp/test"))?;

        let mut expected = Vec::new();
        expected.extend_from_slice(b"NCK00");

        expected.extend_from_slice(b"\x02");
        expected.extend_from_slice(b"\x00\x09/tmp/test");
        expected.extend_from_slice(b"\x01");
        expected.extend_from_slice(b"\x015\x99\xed\xef(\xaf\xa6{\x9b\xec\x98=WAm\x9a,\xc3:\x16e'\xc3\xf6\xce*\xab\xef\x96\xf6lR");
        expected.extend_from_slice(b"\x00\x01");

        expected.extend_from_slice(b"\x02");
        expected.extend_from_slice(b"\x00\x09/tmp/test");
        expected.extend_from_slice(b"\x02");
        expected.extend_from_slice(b"\x00\x0A../../test");
        expected.extend_from_slice(b"\x00\x00");

        expected.extend_from_slice(b"\x02");
        expected.extend_from_slice(b"\x00\x09/tmp/test");
        expected.extend_from_slice(b"\x03");

        assert_eq!(
            PrintableBuffer(&expected[..]),
            PrintableBuffer(&writer.into_inner()[..])
        );
        Ok(())
    }

    #[tokio::test]
    pub async fn write_entry_async() -> Result {
        let dest = Vec::new();
        let mut writer = Writer::new_async(dest).await?;
        writer
            .write_entry_async(Entry::data(
                "/tmp/test",
                make_blake("test1"),
                Some(EntryFlags::EXECUTABLE),
            ))
            .await?;
        writer
            .write_entry_async(Entry::link("/tmp/test", "../../test", None))
            .await?;
        writer
            .write_entry_async(Entry::directory("/tmp/test"))
            .await?;

        let mut expected = Vec::new();
        expected.extend_from_slice(b"NCK00");

        expected.extend_from_slice(b"\x02");
        expected.extend_from_slice(b"\x00\x09/tmp/test");
        expected.extend_from_slice(b"\x01");
        expected.extend_from_slice(b"\x015\x99\xed\xef(\xaf\xa6{\x9b\xec\x98=WAm\x9a,\xc3:\x16e'\xc3\xf6\xce*\xab\xef\x96\xf6lR");
        expected.extend_from_slice(b"\x00\x01");

        expected.extend_from_slice(b"\x02");
        expected.extend_from_slice(b"\x00\x09/tmp/test");
        expected.extend_from_slice(b"\x02");
        expected.extend_from_slice(b"\x00\x0A../../test");
        expected.extend_from_slice(b"\x00\x00");

        expected.extend_from_slice(b"\x02");
        expected.extend_from_slice(b"\x00\x09/tmp/test");
        expected.extend_from_slice(b"\x03");

        assert_eq!(
            PrintableBuffer(&expected[..]),
            PrintableBuffer(&writer.into_inner()[..])
        );
        Ok(())
    }

    #[test]
    pub fn write_data() -> Result {
        use std::io::Write;
        let data = (0..=48u8).collect::<Vec<_>>();

        let dest = Vec::new();
        let writer = Writer::new(dest)?;
        let mut d = writer.write_data(SupportedHasher::blake3())?;
        d.write_all(data.as_slice())?;
        let (writer, hash) = d.finish()?;

        assert_eq!(SupportedHash::Blake3(*b"\xb7\x83\t\xb3\xd5\xfcWe\xeaO\x02\xa6\xdc\x1d\xfbc7\x01\x90G\0\x11\xf1\x02Sb\xdci\x1e\x17\x88\x95"), hash);

        let mut expected = Vec::new();
        expected.extend_from_slice(b"NCK00");

        expected.extend_from_slice(b"\x01");
        expected.extend_from_slice(b"\x00\x20\x00\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0A\x0B\x0C\x0D\x0E\x0F\x10\x11\x12\x13\x14\x15\x16\x17\x18\x19\x1A\x1B\x1C\x1D\x1E\x1F");
        expected.extend_from_slice(
            b"\x00\x11\x20\x21\x22\x23\x24\x25\x26\x27\x28\x29\x2A\x2B\x2C\x2D\x2E\x2F\x30",
        );
        expected.extend_from_slice(b"\x00\x00");
        expected.extend_from_slice(b"\x01\xb7\x83\t\xb3\xd5\xfcWe\xeaO\x02\xa6\xdc\x1d\xfbc7\x01\x90G\0\x11\xf1\x02Sb\xdci\x1e\x17\x88\x95");

        assert_eq!(
            PrintableBuffer(&expected[..]),
            PrintableBuffer(&writer.into_inner()[..])
        );
        Ok(())
    }

    #[tokio::test]
    pub async fn write_data_async() -> Result {
        use tokio::io::AsyncWriteExt;

        let data = (0..=48u8).collect::<Vec<_>>();

        let dest = Vec::new();
        let writer = Writer::new_async(dest).await?;
        let mut d = writer.write_data_async(SupportedHasher::blake3()).await?;
        d.write_all(data.as_slice()).await?;
        let (writer, hash) = d.finish_async().await?;

        assert_eq!(SupportedHash::Blake3(*b"\xb7\x83\t\xb3\xd5\xfcWe\xeaO\x02\xa6\xdc\x1d\xfbc7\x01\x90G\0\x11\xf1\x02Sb\xdci\x1e\x17\x88\x95"), hash);

        let mut expected = Vec::new();
        expected.extend_from_slice(b"NCK00");

        expected.extend_from_slice(b"\x01");
        expected.extend_from_slice(b"\x00\x20\x00\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0A\x0B\x0C\x0D\x0E\x0F\x10\x11\x12\x13\x14\x15\x16\x17\x18\x19\x1A\x1B\x1C\x1D\x1E\x1F");
        expected.extend_from_slice(
            b"\x00\x11\x20\x21\x22\x23\x24\x25\x26\x27\x28\x29\x2A\x2B\x2C\x2D\x2E\x2F\x30",
        );
        expected.extend_from_slice(b"\x00\x00");
        expected.extend_from_slice(b"\x01\xb7\x83\t\xb3\xd5\xfcWe\xeaO\x02\xa6\xdc\x1d\xfbc7\x01\x90G\0\x11\xf1\x02Sb\xdci\x1e\x17\x88\x95");

        assert_eq!(
            PrintableBuffer(&expected[..]),
            PrintableBuffer(&writer.into_inner()[..])
        );
        Ok(())
    }
}
