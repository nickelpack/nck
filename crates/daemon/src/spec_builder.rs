use std::{ffi::OsString, path::PathBuf};

use dashmap::{DashMap, DashSet};
use nck_core::{
    hashing::{DeterministicHashExt, SupportedHash, SupportedHasher},
    spec::{OutputName, PackageName, Spec},
};

use crate::{settings::STORE_DIRECTORY, string_types::Base32};

#[derive(Debug)]
pub struct PackageReference {
    pub name: PackageName,
    pub hash: SupportedHash,
    pub output: OutputName,
}

impl PackageReference {
    pub fn local_path(&self) -> String {
        match self.hash {
            SupportedHash::Blake3(hash) => {
                let hash = Base32::from(hash);
                format!("{}-{}-blake32-{}", self.name, hash, self.output)
            }
        }
    }

    pub fn path(&self) -> PathBuf {
        STORE_DIRECTORY.join(self.local_path())
    }
}

#[derive(Debug)]
pub struct CopiedFile {
    pub package: PackageReference,
    pub executable: bool,
}

#[derive(Debug)]
pub struct SpecBuilder {
    pub name: PackageName,
    pub env: DashMap<OsString, OsString>,
    pub outputs: DashSet<OutputName>,
    pub copy: DashMap<PathBuf, CopiedFile>,
    pub locked: DashSet<PathBuf>,
}

impl SpecBuilder {
    pub fn new(name: PackageName) -> Self {
        Self {
            name,
            env: DashMap::new(),
            outputs: DashSet::new(),
            copy: DashMap::new(),
            locked: DashSet::new(),
        }
    }

    pub fn build(self, entry: PathBuf, args: impl AsRef<[OsString]>) -> Spec {
        let mut spec = Spec::new(self.name.clone(), entry);
        spec.push_args(args.as_ref().iter());

        for (key, value) in self.env {
            spec.set_env(key, value);
        }

        for (key, value) in self.copy {
            spec.copy_file(value.package.path(), key, value.executable)
        }

        for item in self.outputs.iter() {
            spec.add_output(item.clone());
        }

        let hash = spec.hash(SupportedHasher::blake3());

        for item in self.outputs {
            let r = PackageReference {
                name: self.name.clone(),
                hash,
                output: item.clone(),
            };
            spec.set_output(item, r.path());
        }

        spec
    }
}
