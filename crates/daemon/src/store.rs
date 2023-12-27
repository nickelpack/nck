use std::{
    hash::Hash,
    io::ErrorKind,
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
    sync::{atomic::AtomicUsize, Arc, LazyLock},
};

use dashmap::{mapref::entry::Entry, DashMap};
use nck_hashing::{StableHashExt, SupportedHash, SupportedHasher};
use nck_io::fs::TempFile;
use tokio::{
    fs::{File, OpenOptions},
    io::AsyncWriteExt,
};

use crate::{
    build::linux::{Controller, Sandbox},
    settings::{ROOT_DIRECTORY, STORE_DIRECTORY},
    spec::Spec,
};

pub static FILES_DIRECTORY: LazyLock<PathBuf> = LazyLock::new(|| STORE_DIRECTORY.join("files"));
pub static TMP_DIRECTORY: LazyLock<PathBuf> = LazyLock::new(|| ROOT_DIRECTORY.join("tmp"));

#[derive(Debug)]
struct StoreState {
    controller: Controller,
    locks: DashMap<PathBuf, AtomicUsize>,
    builds: DashMap<usize, Build>,
    counter: AtomicUsize,
}

impl StoreState {
    fn increase_lock(&self, path: PathBuf) {
        self.locks
            .entry(path.clone())
            .and_modify(|f| {
                f.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            })
            .or_default();
    }

    fn decrease_lock(&self, path: PathBuf) {
        match self.locks.entry(path) {
            Entry::Occupied(occupied) => {
                let remove = {
                    let val = occupied.get();
                    val.fetch_sub(1, std::sync::atomic::Ordering::SeqCst) == 1
                };
                if remove {
                    occupied.remove_entry();
                }
            }
            Entry::Vacant(vacant) => {
                let path = vacant.into_key();
                tracing::warn!(?path, "excess lock free");
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Store(Arc<StoreState>);

impl Store {
    pub async fn new(controller: Controller) -> anyhow::Result<Self> {
        let results = tokio::join!(
            tokio::fs::create_dir_all(FILES_DIRECTORY.as_path()),
            tokio::fs::create_dir_all(TMP_DIRECTORY.as_path())
        );
        results.0?;
        results.1?;

        Ok(Self(Arc::new(StoreState {
            controller,
            locks: DashMap::new(),
            builds: DashMap::new(),
            counter: AtomicUsize::new(0),
        })))
    }

    pub async fn get_file(&self, hash: &SupportedHash) -> std::io::Result<StoreLock> {
        let path = FILES_DIRECTORY.join(hash.to_string());
        let dec = DecrementLock::new(path.clone(), self.0.clone());

        let file = tokio::fs::OpenOptions::new()
            .read(true)
            .create(false)
            .truncate(false)
            .open(path.as_path())
            .await?;
        Ok(StoreLock { file, dec })
    }

    pub async fn create_file(&self) -> std::io::Result<PendingFile> {
        PendingFile::new(self.0.clone()).await
    }

    pub async fn start(&self, spec: Spec, locks: Vec<StoreLock>) -> anyhow::Result<()> {
        let hash = spec.hash(SupportedHasher::blake3());
        let name = format!("{}-{}", spec.name(), &hash);

        let output_path = STORE_DIRECTORY.join(format!("{}.spec", name));

        match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(output_path.as_path())
            .await
        {
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(other) => Err(other)?,
            Ok(mut file) => {
                let s = rmp_serde::to_vec(&spec)?;
                //let s = toml::to_string_pretty(&spec)?;
                file.write_all(s.as_slice()).await?;
            }
        }
        let sandbox = self.0.controller.spawn_async(output_path.as_path()).await?;
        let build = Build {
            sandbox,
            locks: locks.into_iter().map(|v| v.dec).collect(),
        };

        let id = self
            .0
            .counter
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.0.builds.insert(id, build);

        Ok(())
    }
}

/// Decreases the refcount for a store file lock when dropped.
#[derive(Debug)]
struct DecrementLock {
    path: Option<PathBuf>,
    state: Arc<StoreState>,
}

impl DecrementLock {
    fn new(path: PathBuf, state: Arc<StoreState>) -> Self {
        state.increase_lock(path.clone());
        Self {
            path: Some(path),
            state,
        }
    }
}

impl Drop for DecrementLock {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            self.state.decrease_lock(path);
        }
    }
}

/// A locked store file.
#[derive(Debug)]
pub struct StoreLock {
    file: File,

    // We don't inline these because the lock should be taken before the file is opened to avoid a race condition.
    dec: DecrementLock,
}

impl PartialEq for StoreLock {
    fn eq(&self, other: &Self) -> bool {
        self.dec.path == other.dec.path
    }
}

impl Eq for StoreLock {}

impl Hash for StoreLock {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.dec.path.hash(state)
    }
}

impl StoreLock {
    pub fn as_path(&self) -> &Path {
        self.dec.path.as_ref().unwrap().as_path()
    }
}

/// A file that will be written to the store.
#[derive(Debug)]
pub struct PendingFile {
    _temp: TempFile,
    lock: StoreLock,
    state: Arc<StoreState>,
}

impl PendingFile {
    async fn new(state: Arc<StoreState>) -> std::io::Result<PendingFile> {
        let mut dec = None;
        let (temp, file) = TempFile::new_with_side_effect_in(TMP_DIRECTORY.as_path(), |path| {
            dec = Some(DecrementLock::new(path.to_path_buf(), state.clone()))
        })
        .await?;
        let lock = StoreLock {
            file,
            dec: dec.unwrap(),
        };
        Ok(Self {
            _temp: temp,
            lock,
            state,
        })
    }

    /// Writes the file into the store.
    pub async fn complete(self, hash: &SupportedHash) -> std::io::Result<StoreLock> {
        let path = FILES_DIRECTORY.join(hash.to_string());

        let dec = DecrementLock::new(path.clone(), self.state.clone());
        tokio::fs::copy(self.lock.as_path(), path.as_path()).await?;

        let file = tokio::fs::OpenOptions::new()
            .read(true)
            .create(false)
            .truncate(false)
            .open(path.as_path())
            .await?;

        Ok(StoreLock { file, dec })
    }
}

impl Deref for PendingFile {
    type Target = File;

    fn deref(&self) -> &Self::Target {
        &self.lock.file
    }
}

impl DerefMut for PendingFile {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.lock.file
    }
}

#[derive(Debug)]
pub struct Build {
    sandbox: Sandbox,
    locks: Vec<DecrementLock>,
}
