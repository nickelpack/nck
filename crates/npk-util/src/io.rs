use std::{
    io::{Read, Write},
    ops::{Deref, Range},
    path::{Path, PathBuf},
    sync::atomic::AtomicUsize,
    time::{Duration, Instant},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use rand::seq::SliceRandom;

pub async fn timeout_async<R>(
    duration: Duration,
    f: impl std::future::Future<Output = std::io::Result<R>>,
) -> std::io::Result<R> {
    tokio::time::timeout(duration, f)
        .await
        .unwrap_or_else(|_| Err(std::io::Error::from(std::io::ErrorKind::TimedOut)))
}

pub async fn wait_for_file_async(path: impl AsRef<Path>) -> std::io::Result<()> {
    let mut duration = 1;
    let path = path.as_ref();
    while !path.try_exists()? {
        tokio::time::sleep(Duration::from_millis(duration)).await;
        if duration < 64 {
            duration *= 2;
        }
    }
    Ok(())
}

pub fn timeout<R>(
    duration: Duration,
    mut f: impl FnMut() -> std::io::Result<R>,
) -> Result<R, std::io::Error> {
    let timeout = Instant::now() + duration;
    let mut duration = 1;
    loop {
        match f() {
            Ok(r) => return Ok(r),
            Err(e) => match e.kind() {
                std::io::ErrorKind::WouldBlock => {}
                _ => return Err(e),
            },
        }

        if Instant::now() > timeout {
            return Err(std::io::Error::from(std::io::ErrorKind::TimedOut));
        }

        std::thread::sleep(Duration::from_millis(duration));
        if duration < 64 {
            duration *= 2;
        }
    }
}

pub fn wait_for_file(path: impl AsRef<Path>) -> std::io::Result<()> {
    let path = path.as_ref();
    if path.try_exists()? {
        Ok(())
    } else {
        Err(std::io::ErrorKind::WouldBlock.into())
    }
}

pub struct TempFile {
    path: PathBuf,
}

impl TempFile {
    pub fn forget(mut self) -> PathBuf {
        std::mem::replace(&mut self.path, PathBuf::new())
    }

    pub fn delete(mut self) -> std::io::Result<()> {
        self.delete_impl()
    }

    pub fn delete_impl(&mut self) -> std::io::Result<()> {
        let path = std::mem::replace(&mut self.path, PathBuf::new());
        if !path.as_os_str().is_empty() {
            std::fs::remove_file(path)
        } else {
            Ok(())
        }
    }
}

impl std::fmt::Debug for TempFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.path.fmt(f)
    }
}

impl<T: AsRef<Path>> From<T> for TempFile {
    fn from(value: T) -> Self {
        Self {
            path: value.as_ref().to_path_buf(),
        }
    }
}

impl Deref for TempFile {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        self.path.as_path()
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        if let Err(error) = self.delete_impl() {
            tracing::error!(?error, "failed to clean up {:?}", self.path);
        }
    }
}

pub struct TempDir {
    path: PathBuf,
}

const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
static COUNTER: AtomicUsize = AtomicUsize::new(0);

impl TempDir {
    pub fn new() -> std::io::Result<TempDir> {
        Self::new_in(std::env::temp_dir())
    }

    pub fn new_in(parent: impl AsRef<Path>) -> std::io::Result<TempDir> {
        const MAX_RETRIES: u32 = 1024;
        let parent = parent.as_ref();
        std::fs::create_dir_all(parent)?;

        let mut rng = rand::thread_rng();
        for _ in 0..MAX_RETRIES {
            let date = std::time::SystemTime::now();
            let duration = date
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let suffix = CHARS
                .choose_multiple(&mut rng, 8)
                .map(|v| *v as char)
                .collect::<String>();
            let name = format!(
                "{:x}-{:x}-{}",
                COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
                duration,
                suffix
            );
            let path = parent.join(name);
            match std::fs::create_dir(path.as_path()) {
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    continue;
                }
                Err(e) => return Err(e),
                Ok(_) => return Ok(TempDir { path }),
            }
        }

        Err(std::io::ErrorKind::AlreadyExists.into())
    }

    pub fn as_path(&self) -> &Path {
        self.path.as_path()
    }

    pub fn forget(mut self) -> PathBuf {
        std::mem::replace(&mut self.path, PathBuf::new())
    }

    pub fn delete(mut self) -> std::io::Result<()> {
        self.delete_impl()
    }

    fn delete_impl(&mut self) -> std::io::Result<()> {
        let path = std::mem::replace(&mut self.path, PathBuf::new());
        if !path.as_os_str().is_empty() {
            std::fs::remove_dir_all(path)
        } else {
            Ok(())
        }
    }
}

impl std::fmt::Debug for TempDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.path.fmt(f)
    }
}

impl<T: AsRef<Path>> From<T> for TempDir {
    fn from(value: T) -> Self {
        Self {
            path: value.as_ref().to_path_buf(),
        }
    }
}

impl Deref for TempDir {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        self.path.as_path()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        self.delete_impl().ok();
    }
}

#[derive(Debug)]
pub struct Buffer {
    buf: Vec<u8>,
    range: Range<usize>,
}

impl Buffer {
    #[inline]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            buf: Vec::with_capacity(capacity),
            range: 0..0,
        }
    }

    #[inline]
    fn ensure_buf(&mut self, length: usize) {
        if self.buf.len() < length + self.range.start {
            self.buf.copy_within(self.range.clone(), 0);
            self.range = 0..(self.range.len());
            if self.buf.len() < length {
                self.buf.resize(length, 0);
            }
        }
    }

    #[inline]
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.buf[self.range.clone()]
    }

    #[inline]
    pub fn read_buf(&mut self, reader: &mut impl Read, length: usize) -> std::io::Result<&[u8]> {
        self.ensure_buf(length);
        while self.range.len() < length {
            let len = reader.read(&mut self.buf[self.range.end..])?;
            if len == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            self.range.end += len;
        }
        let result = &self.buf[self.range.start..(self.range.start + length)];
        self.range.start += length;
        Ok(result)
    }

    #[inline]
    pub async fn read_buf_async<R: AsyncRead + Unpin>(
        &mut self,
        reader: &mut R,
        length: usize,
    ) -> std::io::Result<&[u8]> {
        self.ensure_buf(length);
        while self.range.len() < length {
            let len = reader.read(&mut self.buf[self.range.end..]).await?;
            if len == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            self.range.end += len;
        }
        let result = &self.buf[self.range.start..(self.range.start + length)];
        self.range.start += length;
        Ok(result)
    }

    #[inline]
    pub fn clear(&mut self) {
        self.range = 0..0
    }

    #[inline]
    pub fn flush_to(&mut self, writer: &mut impl Write) -> std::io::Result<()> {
        writer.write_all(&self.buf[self.range.clone()])?;
        self.range = 0..0;
        self.buf.clear();
        writer.flush()?;
        Ok(())
    }

    #[inline]
    pub async fn flush_to_async<W: AsyncWrite + Unpin>(
        &mut self,
        writer: &mut W,
    ) -> std::io::Result<()> {
        writer.write_all(&self.buf[self.range.clone()]).await?;
        self.range = 0..0;
        self.buf.clear();
        writer.flush().await?;
        Ok(())
    }
}

impl Write for Buffer {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let len = Write::write(&mut self.buf, buf)?;
        self.range.end += len;
        Ok(len)
    }

    #[inline]
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
