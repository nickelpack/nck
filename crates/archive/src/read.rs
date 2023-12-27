use std::{
    ffi::OsString,
    io::{ErrorKind, Read},
    ops::Range,
    os::unix::prelude::*,
    task::Poll,
};

use bytes::BytesMut;
use nck_hashing::SupportedHash;
use nck_io::{
    pool::{Pooled, BUFFER_POOL},
    BytesMutExt,
};
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::{
    create_hash, hash_length, Entry, EntryFlags, ENTRY_DATA, ENTRY_ENTRY, TYPE_DATA, TYPE_DIR,
    TYPE_LINK,
};

#[derive(Debug)]
pub struct Reader<T> {
    reader: T,
    valid: bool,
    got_header: bool,
}

impl<T> Reader<T> {
    pub fn new(reader: T) -> Self {
        Self {
            reader,
            got_header: false,
            valid: true,
        }
    }

    pub fn into_inner(self) -> T {
        self.reader
    }

    fn invalidate(&mut self) -> std::io::Result<!> {
        self.valid = false;
        Err(ErrorKind::InvalidData.into())
    }

    fn reader(&mut self) -> std::io::Result<&mut T> {
        if self.valid {
            Ok(&mut self.reader)
        } else {
            Err(ErrorKind::InvalidData.into())
        }
    }
}

impl<T: Read> Reader<T> {
    pub fn next_event(&mut self) -> std::io::Result<ReadEvent<'_, T>> {
        if !self.got_header {
            let mut header = [0u8; 5];
            self.reader()?.read_exact(&mut header)?;
            if &header != b"NCK00" {
                self.invalidate()?;
            }
            self.got_header = true;
        }

        let t = self.read_type()?;
        match t {
            Some(ENTRY_DATA) => Ok(ReadEvent::Data(DataReader {
                reader: Some(self),
                remaining: BUFFER_POOL.take(),
                hash: None,
                range: 0..0,
            })),
            Some(ENTRY_ENTRY) => Ok(self.read_entry()?),
            Some(_) => self.invalidate()?,
            None => Ok(ReadEvent::None),
        }
    }

    fn read_entry(&mut self) -> std::io::Result<ReadEvent<'_, T>> {
        let mut buffer = BUFFER_POOL.take();

        let path = self.read_os_string(&mut buffer)?;

        match self.read_required_type()? {
            TYPE_DATA => {
                let hash = self.read_hash(&mut buffer)?;
                let flags = self.read_u16()?;
                Ok(ReadEvent::Entry(Entry::data(
                    path,
                    hash,
                    Some(EntryFlags::from_bits_truncate(flags)),
                )))
            }
            TYPE_LINK => {
                let source = self.read_os_string(&mut buffer)?;
                let flags = self.read_u16()?;
                Ok(ReadEvent::Entry(Entry::link(
                    path,
                    source,
                    Some(EntryFlags::from_bits_truncate(flags)),
                )))
            }
            TYPE_DIR => Ok(ReadEvent::Entry(Entry::directory(path))),
            _ => self.invalidate()?,
        }
    }

    fn read_type(&mut self) -> std::io::Result<Option<u8>> {
        let mut id_buf = [0u8; 1];
        if self.reader()?.read(&mut id_buf)? == 0 {
            Ok(None)
        } else {
            Ok(Some(id_buf[0]))
        }
    }

    fn read_required_type(&mut self) -> std::io::Result<u8> {
        let mut id_buf = [0u8; 1];
        self.reader()?.read_exact(&mut id_buf[..])?;
        Ok(id_buf[0])
    }

    fn read_hash(&mut self, buf: &mut Pooled<'_, BytesMut>) -> std::io::Result<SupportedHash> {
        let id = self.read_required_type()?;

        let len = hash_length(id).map_err(|_| self.invalidate().unwrap_err())?;

        buf.clear();
        buf.extend_from_reader(self.reader().unwrap(), len)?;
        Ok(create_hash(id, &buf[..len]))
    }

    fn read_os_string(&mut self, buf: &mut Pooled<'_, BytesMut>) -> std::io::Result<OsString> {
        self.read_length_prefixed(buf)?;
        Ok(OsString::from_vec(buf.to_vec()))
    }

    fn read_length_prefixed(&mut self, buf: &mut Pooled<'_, BytesMut>) -> std::io::Result<()> {
        let len = self.read_u16()? as usize;

        buf.clear();
        buf.extend_from_reader(self.reader().unwrap(), len)?;
        Ok(())
    }

    fn read_u16(&mut self) -> std::io::Result<u16> {
        let mut buf = [0u8; 2];
        self.reader()?.read_exact(&mut buf[..])?;
        Ok(u16::from_be_bytes(buf))
    }
}

impl<T: AsyncRead + Unpin + Send> Reader<T> {
    pub async fn next_event_async(&mut self) -> std::io::Result<ReadEvent<'_, T>> {
        if !self.got_header {
            let mut header = [0u8; 5];
            self.reader()?.read_exact(&mut header).await?;
            if &header != b"NCK00" {
                self.invalidate()?;
            }
            self.got_header = true;
        }

        let t = self.read_type_async().await?;
        match t {
            Some(ENTRY_DATA) => Ok(ReadEvent::Data(DataReader {
                reader: Some(self),
                remaining: BUFFER_POOL.take(),
                hash: None,
                range: 0..0,
            })),
            Some(ENTRY_ENTRY) => Ok(self.read_entry_async().await?),
            Some(_) => self.invalidate()?,
            None => Ok(ReadEvent::None),
        }
    }

    async fn read_entry_async(&mut self) -> std::io::Result<ReadEvent<'_, T>> {
        let mut buffer = BUFFER_POOL.take();

        let path = self.read_os_string_async(&mut buffer).await?;

        match self.read_required_type_async().await? {
            TYPE_DATA => {
                let hash = self.read_hash_async(&mut buffer).await?;
                let flags = self.read_u16_async().await?;
                Ok(ReadEvent::Entry(Entry::data(
                    path,
                    hash,
                    Some(EntryFlags::from_bits_truncate(flags)),
                )))
            }
            TYPE_LINK => {
                let source = self.read_os_string_async(&mut buffer).await?;
                let flags = self.read_u16_async().await?;
                Ok(ReadEvent::Entry(Entry::link(
                    path,
                    source,
                    Some(EntryFlags::from_bits_truncate(flags)),
                )))
            }
            TYPE_DIR => Ok(ReadEvent::Entry(Entry::directory(path))),
            _ => self.invalidate()?,
        }
    }

    async fn read_type_async(&mut self) -> std::io::Result<Option<u8>> {
        let mut id_buf = [0u8; 1];
        if self.reader()?.read(&mut id_buf).await? == 0 {
            Ok(None)
        } else {
            Ok(Some(id_buf[0]))
        }
    }

    async fn read_required_type_async(&mut self) -> std::io::Result<u8> {
        let mut id_buf = [0u8; 1];
        self.reader()?.read_exact(&mut id_buf[..]).await?;
        Ok(id_buf[0])
    }

    async fn read_hash_async(
        &mut self,
        buf: &mut Pooled<'_, BytesMut>,
    ) -> std::io::Result<SupportedHash> {
        let id = self.read_required_type_async().await?;

        let len = hash_length(id).map_err(|_| self.invalidate().unwrap_err())?;

        buf.clear();
        buf.extend_from_reader_async(self.reader().unwrap(), len)
            .await?;
        Ok(create_hash(id, &buf[..len]))
    }

    async fn read_os_string_async(
        &mut self,
        buf: &mut Pooled<'_, BytesMut>,
    ) -> std::io::Result<OsString> {
        self.read_length_prefixed_async(buf).await?;
        Ok(OsString::from_vec(buf.to_vec()))
    }

    async fn read_length_prefixed_async(
        &mut self,
        buf: &mut Pooled<'_, BytesMut>,
    ) -> std::io::Result<()> {
        let len = self.read_u16_async().await? as usize;

        buf.clear();
        buf.extend_from_reader_async(self.reader().unwrap(), len)
            .await?;
        Ok(())
    }

    async fn read_u16_async(&mut self) -> std::io::Result<u16> {
        let mut buf = [0u8; 2];
        self.reader()?.read_exact(&mut buf[..]).await?;
        Ok(u16::from_be_bytes(buf))
    }
}

#[derive(Debug)]
pub enum ReadEvent<'a, T> {
    None,
    Data(DataReader<'a, T>),
    Entry(Entry),
}

impl<'a, T> ReadEvent<'a, T> {
    pub fn is_some(&self) -> bool {
        !matches!(self, ReadEvent::None)
    }

    pub fn is_none(&self) -> bool {
        !self.is_some()
    }
}

#[derive(Debug)]
pub struct DataReader<'a, T> {
    reader: Option<&'a mut Reader<T>>,
    remaining: Pooled<'a, BytesMut>,
    range: Range<usize>,
    hash: Option<SupportedHash>,
}

impl<'a, T> DataReader<'a, T> {
    pub fn hash(&self) -> Option<SupportedHash> {
        self.hash
    }

    #[inline(always)]
    fn split_borrow(&mut self) -> (&mut Option<&'a mut Reader<T>>, &mut Pooled<'a, BytesMut>) {
        (&mut self.reader, &mut self.remaining)
    }
}

impl<'a, T: Read> DataReader<'a, T> {
    fn read_exact_internal(&mut self, len: usize) -> std::io::Result<()> {
        let (reader, buffer) = self.split_borrow();

        let start = buffer.len();
        let remaining = len.saturating_sub(start);
        if remaining == 0 {
            return Ok(());
        }

        let reader = if let Some(reader) = reader.as_mut() {
            reader
        } else {
            return Ok(());
        };

        buffer.extend_from_reader(reader.reader()?, len)
    }
}

impl<'a, T: Read> Read for DataReader<'a, T> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        // Mimics the async implementation for consistency.

        if self.reader.is_none() || buf.is_empty() {
            return Ok(0);
        }

        if !self.range.is_empty() {
            let to_copy = buf.len().min(self.range.len());
            buf[..to_copy]
                .copy_from_slice(&self.remaining[self.range.start..(self.range.start + to_copy)]);
            self.range.start += to_copy;
            if self.range.is_empty() {
                self.remaining.clear();
            }

            return Ok(to_copy);
        }

        self.read_exact_internal(2)?;

        let len = u16::from_be_bytes(self.remaining[0..2].try_into().unwrap()) as usize;
        if len == 0 {
            self.read_exact_internal(3)?;

            let len = hash_length(self.remaining[2])
                .map_err(|_| self.reader.as_mut().unwrap().invalidate().unwrap_err())?;

            self.read_exact_internal(3 + len)?;

            self.hash = Some(create_hash(
                self.remaining[2],
                &self.remaining[3..(3 + len)],
            ));

            return Ok(0);
        }

        self.read_exact_internal(2 + len)?;
        self.range = 2..(2 + len);
        self.read(buf)
    }
}

impl<'a, T: AsyncRead + Unpin> DataReader<'a, T> {
    fn poll_read_exact(
        self: &mut std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        len: usize,
    ) -> std::task::Poll<std::io::Result<()>> {
        let (reader, buffer) = self.split_borrow();

        let start = buffer.len();
        let remaining = len.saturating_sub(start);
        if remaining == 0 {
            return Poll::Ready(Ok(()));
        }

        let reader = if let Some(reader) = reader.as_mut() {
            reader
        } else {
            return Poll::Ready(Ok(()));
        };

        buffer.poll_extend_from_reader(reader.reader()?, len, cx)
    }
}

impl<'a, T: AsyncRead + Unpin> AsyncRead for DataReader<'a, T> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        if self.reader.is_none() || buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }

        if !self.range.is_empty() {
            let to_copy = buf.remaining().min(self.range.len());
            buf.put_slice(&self.remaining[self.range.start..(self.range.start + to_copy)]);

            self.range.start += to_copy;
            if self.range.is_empty() {
                self.remaining.clear();
            }

            return Poll::Ready(Ok(()));
        }

        if self.poll_read_exact(cx, 2).is_pending() {
            return Poll::Pending;
        }

        // We need to retain the length at the start so that it's available during the next poll after a partial read.
        let len = u16::from_be_bytes(self.remaining[0..2].try_into().unwrap()) as usize;
        if len == 0 {
            if self.poll_read_exact(cx, 3).is_pending() {
                return Poll::Pending;
            }

            let len = match hash_length(self.remaining[2]) {
                Ok(v) => v,
                Err(_) => {
                    return Poll::Ready(self.reader.as_mut().unwrap().invalidate().map(|_| ()));
                }
            };

            if self.poll_read_exact(cx, 3 + len).is_pending() {
                return Poll::Pending;
            }

            self.hash = Some(create_hash(
                self.remaining[2],
                &self.remaining[3..(3 + len)],
            ));
            return Poll::Ready(Ok(()));
        }

        if self.poll_read_exact(cx, 2 + len).is_pending() {
            return Poll::Pending;
        }

        self.range = 2..(2 + len);

        self.poll_read(cx, buf)
    }
}

#[cfg(test)]
mod test {

    use nck_hashing::SupportedHash;
    use nck_io::PrintableBuffer;
    use pretty_assertions::assert_eq;

    use crate::{Entry, EntryFlags, ReadEvent, Reader};

    type Result = anyhow::Result<()>;

    fn get_entry<T>(evt: ReadEvent<'_, T>) -> Entry {
        match evt {
            ReadEvent::Entry(v) => v,
            _ => panic!("expected an entry"),
        }
    }

    #[test]
    fn read_entries() -> Result {
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

        let mut reader = Reader::new(expected.as_slice());

        assert_eq!(
            Entry::data(
                "/tmp/test",
                SupportedHash::Blake3(*b"5\x99\xed\xef(\xaf\xa6{\x9b\xec\x98=WAm\x9a,\xc3:\x16e'\xc3\xf6\xce*\xab\xef\x96\xf6lR"),
                Some(EntryFlags::EXECUTABLE)
            ),
            get_entry(reader.next_event()?));
        assert_eq!(
            Entry::link("/tmp/test", "../../test", None),
            get_entry(reader.next_event()?)
        );
        assert_eq!(
            Entry::directory("/tmp/test"),
            get_entry(reader.next_event()?)
        );
        assert!(reader.next_event()?.is_none());
        assert!(reader.next_event()?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn read_entries_async() -> Result {
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

        let mut reader = Reader::new(expected.as_slice());

        assert_eq!(
            Entry::data(
                "/tmp/test",
                SupportedHash::Blake3(*b"5\x99\xed\xef(\xaf\xa6{\x9b\xec\x98=WAm\x9a,\xc3:\x16e'\xc3\xf6\xce*\xab\xef\x96\xf6lR"),
                Some(EntryFlags::EXECUTABLE)
            ),
            get_entry(reader.next_event_async().await?));
        assert_eq!(
            Entry::link("/tmp/test", "../../test", None),
            get_entry(reader.next_event_async().await?)
        );
        assert_eq!(
            Entry::directory("/tmp/test"),
            get_entry(reader.next_event_async().await?)
        );
        assert!(reader.next_event_async().await?.is_none());
        assert!(reader.next_event_async().await?.is_none());
        Ok(())
    }

    #[test]
    fn read_data() -> Result {
        use std::io::Read;
        let mut expected = Vec::new();
        expected.extend_from_slice(b"NCK00");

        expected.extend_from_slice(b"\x01");
        expected.extend_from_slice(b"\x00\x20\x00\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0A\x0B\x0C\x0D\x0E\x0F\x10\x11\x12\x13\x14\x15\x16\x17\x18\x19\x1A\x1B\x1C\x1D\x1E\x1F");
        expected.extend_from_slice(
            b"\x00\x11\x20\x21\x22\x23\x24\x25\x26\x27\x28\x29\x2A\x2B\x2C\x2D\x2E\x2F\x30",
        );
        expected.extend_from_slice(b"\x00\x00");
        expected.extend_from_slice(b"\x01\xb7\x83\t\xb3\xd5\xfcWe\xeaO\x02\xa6\xdc\x1d\xfbc7\x01\x90G\0\x11\xf1\x02Sb\xdci\x1e\x17\x88\x95");

        expected.extend_from_slice(b"\x01");
        expected.extend_from_slice(b"\x00\x20\x00\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0A\x0B\x0C\x0D\x0E\x0F\x10\x11\x12\x13\x14\x15\x16\x17\x18\x19\x1A\x1B\x1C\x1D\x1E\x1F");
        expected.extend_from_slice(
            b"\x00\x12\x20\x21\x22\x23\x24\x25\x26\x27\x28\x29\x2A\x2B\x2C\x2D\x2E\x2F\x30\x31",
        );
        expected.extend_from_slice(b"\x00\x00");
        expected.extend_from_slice(b"\x01\xb7\x83\t\xb3\xd5\xfcWe\xeaO\x02\xa6\xdc\x1d\xfbc7\x01\x90G\0\x11\xf1\x02Sb\xdci\x1e\x17\x88\x95");

        let mut reader = Reader::new(expected.as_slice());

        match reader.next_event()? {
            ReadEvent::Data(mut reader) => {
                let mut buf = Vec::new();
                reader.read_to_end(&mut buf)?;

                let data = (0..=48u8).collect::<Vec<_>>();
                assert_eq!(PrintableBuffer(&data[..]), PrintableBuffer(&buf[..]));
            }
            _ => panic!("expected data"),
        }

        match reader.next_event()? {
            ReadEvent::Data(mut reader) => {
                let mut buf = Vec::new();
                reader.read_to_end(&mut buf)?;

                let data = (0..=49u8).collect::<Vec<_>>();
                assert_eq!(PrintableBuffer(&data[..]), PrintableBuffer(&buf[..]));
            }
            _ => panic!("expected data"),
        }

        assert!(reader.next_event()?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn read_data_async() -> Result {
        use tokio::io::AsyncReadExt;

        let mut expected = Vec::new();
        expected.extend_from_slice(b"NCK00");

        expected.extend_from_slice(b"\x01");
        expected.extend_from_slice(b"\x00\x20\x00\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0A\x0B\x0C\x0D\x0E\x0F\x10\x11\x12\x13\x14\x15\x16\x17\x18\x19\x1A\x1B\x1C\x1D\x1E\x1F");
        expected.extend_from_slice(
            b"\x00\x11\x20\x21\x22\x23\x24\x25\x26\x27\x28\x29\x2A\x2B\x2C\x2D\x2E\x2F\x30",
        );
        expected.extend_from_slice(b"\x00\x00");
        expected.extend_from_slice(b"\x01\xb7\x83\t\xb3\xd5\xfcWe\xeaO\x02\xa6\xdc\x1d\xfbc7\x01\x90G\0\x11\xf1\x02Sb\xdci\x1e\x17\x88\x95");

        expected.extend_from_slice(b"\x01");
        expected.extend_from_slice(b"\x00\x20\x00\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0A\x0B\x0C\x0D\x0E\x0F\x10\x11\x12\x13\x14\x15\x16\x17\x18\x19\x1A\x1B\x1C\x1D\x1E\x1F");
        expected.extend_from_slice(
            b"\x00\x12\x20\x21\x22\x23\x24\x25\x26\x27\x28\x29\x2A\x2B\x2C\x2D\x2E\x2F\x30\x31",
        );
        expected.extend_from_slice(b"\x00\x00");
        expected.extend_from_slice(b"\x01\xb7\x83\t\xb3\xd5\xfcWe\xeaO\x02\xa6\xdc\x1d\xfbc7\x01\x90G\0\x11\xf1\x02Sb\xdci\x1e\x17\x88\x95");

        let mut reader = Reader::new(expected.as_slice());

        match reader.next_event_async().await? {
            ReadEvent::Data(mut reader) => {
                let mut buf = Vec::new();
                reader.read_to_end(&mut buf).await?;

                let data = (0..=48u8).collect::<Vec<_>>();
                assert_eq!(PrintableBuffer(&data[..]), PrintableBuffer(&buf[..]));
            }
            _ => panic!("expected data"),
        }

        match reader.next_event_async().await? {
            ReadEvent::Data(mut reader) => {
                let mut buf = Vec::new();
                reader.read_to_end(&mut buf).await?;

                let data = (0..=49u8).collect::<Vec<_>>();
                assert_eq!(PrintableBuffer(&data[..]), PrintableBuffer(&buf[..]));
            }
            _ => panic!("expected data"),
        }

        assert!(reader.next_event()?.is_none());
        Ok(())
    }
}
