use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    path::PathBuf,
};

use serde::{Deserialize, Serialize};

pub trait DeterministicHasher: Sized {
    type Result;
    fn update(&mut self, bytes: &[u8]);
    fn finalize(self) -> Self::Result;

    fn update_hash<H: DeterministicHash>(&mut self, v: &H) -> &mut Self {
        v.update(self);
        self
    }

    fn update_iter<H: DeterministicHash>(
        &mut self,
        v: impl ExactSizeIterator<Item = H>,
    ) -> &mut Self {
        self.update_usize(v.len());
        for v in v {
            v.update(self);
        }
        self
    }

    fn update_bool(&mut self, v: bool) -> &mut Self {
        self.update_u8(if v { 0xFF } else { 0x00 });
        self
    }

    fn update_u8(&mut self, v: u8) -> &mut Self {
        self.update(&[v]);
        self
    }

    fn update_u16(&mut self, v: u16) -> &mut Self {
        self.update(&v.to_be_bytes());
        self
    }

    fn update_u32(&mut self, v: u32) -> &mut Self {
        self.update(&v.to_be_bytes());
        self
    }

    fn update_u64(&mut self, v: u64) -> &mut Self {
        self.update(&v.to_be_bytes());
        self
    }

    fn update_usize(&mut self, v: usize) -> &mut Self {
        let v = v as u64;
        self.update(&v.to_be_bytes());
        self
    }

    fn update_i8(&mut self, v: i8) -> &mut Self {
        self.update(&[v as u8]);
        self
    }

    fn update_i16(&mut self, v: i16) -> &mut Self {
        self.update(&v.to_be_bytes());
        self
    }

    fn update_i32(&mut self, v: i32) -> &mut Self {
        self.update(&v.to_be_bytes());
        self
    }

    fn update_i64(&mut self, v: i64) -> &mut Self {
        self.update(&v.to_be_bytes());
        self
    }

    fn update_isize(&mut self, v: isize) -> &mut Self {
        let v = v as i64;
        self.update(&v.to_be_bytes());
        self
    }

    fn update_prefixed(&mut self, v: &[u8]) -> &mut Self {
        self.update_usize(v.len());
        self.update(v);
        self
    }
}

#[derive(Debug)]
pub enum SupportedHasher {
    Blake3(blake3::Hasher),
}

impl SupportedHasher {
    pub fn blake3() -> Self {
        Self::Blake3(blake3::Hasher::new())
    }
}

impl DeterministicHasher for SupportedHasher {
    type Result = SupportedHash;

    fn update(&mut self, bytes: &[u8]) {
        match self {
            Self::Blake3(hasher) => hasher.update(bytes),
        };
    }

    fn finalize(self) -> Self::Result {
        match self {
            Self::Blake3(hasher) => SupportedHash::Blake3(*hasher.finalize().as_bytes()),
        }
    }
}

pub trait DeterministicHash {
    fn update<H: DeterministicHasher>(&self, h: &mut H);
}

pub trait DeterministicHashExt {
    fn hash<H: DeterministicHasher>(&self, h: H) -> H::Result;
}

impl<T: DeterministicHash> DeterministicHashExt for T {
    fn hash<H: DeterministicHasher>(&self, mut h: H) -> H::Result {
        h.update_hash(self);
        h.finalize()
    }
}

impl<T: DeterministicHash> DeterministicHash for &T {
    fn update<H: DeterministicHasher>(&self, h: &mut H) {
        (*self).update(h)
    }
}

impl DeterministicHash for OsString {
    fn update<H: DeterministicHasher>(&self, h: &mut H) {
        h.update_prefixed(self.as_encoded_bytes());
    }
}

impl DeterministicHash for PathBuf {
    fn update<H: DeterministicHasher>(&self, h: &mut H) {
        h.update_prefixed(self.as_os_str().as_encoded_bytes());
    }
}

impl DeterministicHash for String {
    fn update<H: DeterministicHasher>(&self, h: &mut H) {
        h.update_prefixed(self.as_bytes());
    }
}

impl<T: DeterministicHash> DeterministicHash for Vec<T> {
    fn update<H: DeterministicHasher>(&self, h: &mut H) {
        h.update_usize(self.len());
        for i in self.iter() {
            i.update(h);
        }
    }
}

// Not implemented for Set and Map because the order of those is undefined

impl<T: DeterministicHash> DeterministicHash for BTreeSet<T> {
    fn update<H: DeterministicHasher>(&self, h: &mut H) {
        h.update_usize(self.len());
        for i in self.iter() {
            i.update(h);
        }
    }
}

impl<K: DeterministicHash, V: DeterministicHash> DeterministicHash for BTreeMap<K, V> {
    fn update<H: DeterministicHasher>(&self, h: &mut H) {
        h.update_usize(self.len());
        for (k, v) in self.iter() {
            k.update(h);
            v.update(h);
        }
    }
}

impl<T1: DeterministicHash, T2: DeterministicHash> DeterministicHash for (T1, T2) {
    fn update<H: DeterministicHasher>(&self, h: &mut H) {
        h.update_hash(&self.0).update_hash(&self.1);
    }
}

impl DeterministicHash for bool {
    fn update<H: DeterministicHasher>(&self, h: &mut H) {
        h.update_bool(*self);
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SupportedHash {
    Blake3([u8; 32]),
}

impl SupportedHash {
    pub fn create_matching_hasher(&self) -> SupportedHasher {
        match self {
            SupportedHash::Blake3(_) => SupportedHasher::blake3(),
        }
    }
}

impl DeterministicHash for SupportedHash {
    fn update<H: DeterministicHasher>(&self, h: &mut H) {
        match self {
            SupportedHash::Blake3(hash) => h.update_u8(1).update(hash),
        }
    }
}
