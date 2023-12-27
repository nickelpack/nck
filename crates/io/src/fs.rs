use std::{
    io::{ErrorKind, Result},
    ops::Deref,
    path::{Path, PathBuf},
    sync::atomic::AtomicUsize,
};

use rand::seq::SliceRandom;
use tokio::fs::{File, OpenOptions};

const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz234567";
static COUNTER: AtomicUsize = AtomicUsize::new(0);

pub struct TempFile {
    path: PathBuf,
}

impl TempFile {
    /// Create a new temporary file in the system temporary directory.
    pub async fn new() -> Result<(Self, File)> {
        Self::new_in(std::env::temp_dir()).await
    }

    /// Create a new temporary file in the specified directory.
    pub async fn new_in(parent: impl AsRef<Path>) -> Result<(Self, File)> {
        Self::new_with_side_effect_in(parent, |_| {}).await
    }

    /// Create a new temporary file in the specified directory.
    ///
    /// This includes the ability to have a side-effect run *prior* to the temporary file being checked and created.
    pub async fn new_with_side_effect_in(
        parent: impl AsRef<Path>,
        mut f: impl FnMut(&Path),
    ) -> Result<(Self, File)> {
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

        let pid = std::process::id();
        for _ in 0..MAX_RETRIES {
            let suffix = CHARS
                .choose_multiple(&mut rand::thread_rng(), 8)
                .map(|v| *v as char)
                .collect::<String>();
            let name = format!(
                "{:x}-{:x}-{}",
                COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
                pid,
                suffix
            );
            let path = parent.join(name);
            f(path.as_path());
            match options.open(path.as_path()).await {
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                    continue;
                }
                Err(e) => return Err(e),
                Ok(file) => return Ok((Self { path }, file)),
            }
        }

        Err(ErrorKind::AlreadyExists.into())
    }

    /// The path to the temporary file.
    #[inline(always)]
    pub fn as_path(&self) -> &Path {
        self.path.as_path()
    }

    /// Forget the temporary file so that it is not cleaned up.
    #[inline(always)]
    pub fn forget(mut self) -> PathBuf {
        std::mem::replace(&mut self.path, PathBuf::new())
    }

    /// Immediately delete the temporary file.
    pub fn delete(&mut self) -> Result<()> {
        self.delete_impl()
    }

    fn delete_impl(&mut self) -> Result<()> {
        let path = std::mem::replace(&mut self.path, PathBuf::new());
        if !path.as_os_str().is_empty() {
            std::fs::remove_file(path)
        } else {
            Ok(())
        }
    }
}

impl std::fmt::Debug for TempFile {
    #[inline(always)]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.path.fmt(f)
    }
}

impl<T: AsRef<Path>> From<T> for TempFile {
    #[inline(always)]
    fn from(value: T) -> Self {
        Self {
            path: value.as_ref().to_path_buf(),
        }
    }
}

impl Deref for TempFile {
    type Target = Path;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        self.path.as_path()
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        self.delete_impl().ok();
    }
}

/// A temporary directory.
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

impl TempDir {
    /// Creates a new temporary directory under the system temporary directory.
    pub fn new() -> Result<TempDir> {
        Self::new_in(std::env::temp_dir())
    }

    /// Creates a new temporary directory under the specified directory.
    pub fn new_in(parent: impl AsRef<Path>) -> Result<TempDir> {
        const MAX_RETRIES: u32 = 1024;
        let parent = parent.as_ref();
        std::fs::create_dir_all(parent)?;

        let pid = std::process::id();
        let mut rng = rand::thread_rng();
        for _ in 0..MAX_RETRIES {
            let suffix = CHARS
                .choose_multiple(&mut rng, 8)
                .map(|v| *v as char)
                .collect::<String>();
            let name = format!(
                "{:x}-{:x}-{}",
                COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
                pid,
                suffix
            );
            let path = parent.join(name);
            match std::fs::create_dir(path.as_path()) {
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                    continue;
                }
                Err(e) => return Err(e),
                Ok(_) => return Ok(TempDir { path }),
            }
        }

        Err(ErrorKind::AlreadyExists.into())
    }

    /// The path to the temporary directory.
    #[inline(always)]
    pub fn as_path(&self) -> &Path {
        self.path.as_path()
    }

    /// Forget the temporary directory so that it is not cleaned up.
    #[inline(always)]
    pub fn forget(mut self) -> PathBuf {
        std::mem::replace(&mut self.path, PathBuf::new())
    }

    /// Immediately delete the temporary directory.
    #[inline(always)]
    pub fn delete(mut self) -> Result<()> {
        self.delete_impl()
    }

    fn delete_impl(&mut self) -> Result<()> {
        let path = std::mem::replace(&mut self.path, PathBuf::new());
        if !path.as_os_str().is_empty() {
            std::fs::remove_dir_all(path)
        } else {
            Ok(())
        }
    }
}

impl std::fmt::Debug for TempDir {
    #[inline(always)]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.path.fmt(f)
    }
}

impl<T: AsRef<Path>> From<T> for TempDir {
    #[inline(always)]
    fn from(value: T) -> Self {
        Self {
            path: value.as_ref().to_path_buf(),
        }
    }
}

impl Deref for TempDir {
    type Target = Path;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        self.path.as_path()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        self.delete_impl().ok();
    }
}
