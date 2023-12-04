#![feature(inline_const)]

pub mod io;
pub mod pool;
pub mod thread;
pub mod transport;

use std::path::PathBuf;

use bytes::BytesMut;
use pool::Pool;
use speedy::{Context, Readable, Writable};

const BUFFERS_MB: usize = 4;
const BUFFERS_B: usize = BUFFERS_MB * 131072;
const MAX_BUFFER_LEN: usize = 16384;
const MAX_BUFFERS: usize = BUFFERS_B / MAX_BUFFER_LEN;
const DEFAULT_BUFFER_LEN: usize = 4096;

pub static BUFFER_POOL: Pool<'static, MAX_BUFFERS, BytesMut> =
    Pool::new(&|| BytesMut::with_capacity(DEFAULT_BUFFER_LEN), &|mut v| {
        if v.capacity() <= MAX_BUFFER_LEN {
            v.clear();
            Some(v)
        } else {
            None
        }
    });
