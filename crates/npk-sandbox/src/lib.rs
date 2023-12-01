#![feature(result_option_inspect)]
#![feature(async_closure)]
#![feature(result_flattening)]
#[cfg(target_os = "linux")]
pub mod linux;

use bitcode::{Buffer, Decode};
#[cfg(target_os = "linux")]
pub use linux as current;

use lockfree_object_pool::{LinearObjectPool, LinearReusable};
use once_cell::sync::Lazy;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[cfg(target_os = "linux")]
    pub linux: linux::Config,
}

static BITCODE_POOL: Lazy<&'static LinearObjectPool<Buffer>> = Lazy::new(|| {
    Box::leak(Box::new(LinearObjectPool::<Buffer>::new(
        Default::default,
        |_| {},
    )))
});

fn bitcode_pull() -> LinearReusable<'static, Buffer> {
    BITCODE_POOL.pull()
}

fn bitcode_decode<T: Decode>(data: impl AsRef<[u8]>) -> std::io::Result<T> {
    let mut buffer = bitcode_pull();
    buffer
        .decode(data.as_ref())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}
