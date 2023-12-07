use bytes::BytesMut;
use std::{
    future::Future,
    io::Read,
    ops::Deref,
    path::{Path, PathBuf},
    sync::atomic::AtomicUsize,
    time::{Duration, Instant},
};
use tokio::{
    fs::{File, OpenOptions},
    io::{AsyncRead, AsyncReadExt},
};

use rand::seq::SliceRandom;

pub fn copy_to_buffer(
    reader: &mut impl Read,
    buffer: &mut BytesMut,
    length: usize,
) -> std::io::Result<()> {
    let start = buffer.len();
    buffer.resize(start + length, 0u8);
    let mut target = &mut buffer[start..(start + length)];
    while !target.is_empty() {
        let len = reader.read(target)?;
        if len == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        target = &mut target[len..];
    }
    Ok(())
}

pub async fn copy_to_buffer_async(
    reader: &mut (impl AsyncRead + Unpin),
    buffer: &mut BytesMut,
    length: usize,
) -> std::io::Result<()> {
    let start = buffer.len();
    buffer.resize(start + length, 0u8);
    let mut target = &mut buffer[start..(start + length)];
    while !target.is_empty() {
        let len = reader.read(target).await?;
        if len == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        target = &mut target[len..];
    }
    Ok(())
}

pub trait Timeout {
    fn timeout_async<R>(
        &self,
        f: impl std::future::Future<Output = std::io::Result<R>>,
    ) -> impl Future<Output = std::io::Result<R>>;

    fn timeout<R>(&self, f: impl FnMut() -> std::io::Result<R>) -> Result<R, std::io::Error>;
}

impl Timeout for Duration {
    async fn timeout_async<R>(
        &self,
        f: impl std::future::Future<Output = std::io::Result<R>>,
    ) -> std::io::Result<R> {
        tokio::time::timeout(*self, f)
            .await
            .unwrap_or_else(|_| Err(std::io::Error::from(std::io::ErrorKind::TimedOut)))
    }

    fn timeout<R>(&self, mut f: impl FnMut() -> std::io::Result<R>) -> Result<R, std::io::Error> {
        let timeout = Instant::now() + *self;
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
    pub async fn new() -> std::io::Result<(Self, File)> {
        Self::new_in(std::env::temp_dir()).await
    }

    pub async fn new_in(parent: impl AsRef<Path>) -> std::io::Result<(Self, File)> {
        const MAX_RETRIES: u32 = 1024;
        let parent = parent.as_ref();
        tokio::fs::create_dir_all(parent).await?;

        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .append(false)
            .truncate(false)
            .create_new(true);

        for _ in 0..MAX_RETRIES {
            let date = std::time::SystemTime::now();
            let duration = date
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let suffix = CHARS
                .choose_multiple(&mut rand::thread_rng(), 8)
                .map(|v| *v as char)
                .collect::<String>();
            let name = format!(
                "{:x}-{:x}-{}",
                COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
                duration,
                suffix
            );
            let path = parent.join(name);
            match options.open(path.as_path()).await {
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    continue;
                }
                Err(e) => return Err(e),
                Ok(file) => return Ok((Self { path }, file)),
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

impl Default for TempDir {
    fn default() -> Self {
        Self {
            path: PathBuf::new(),
        }
    }
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
