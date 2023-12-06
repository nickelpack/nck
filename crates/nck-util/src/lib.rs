#![feature(inline_const)]
#![feature(thread_id_value)]

pub mod base32;
pub mod io;
pub mod pool;
pub mod transport;

use std::sync::atomic::AtomicUsize;

use bytes::BytesMut;
use pool::Pool;

const MB: usize = 131072;
const MAX_TOTAL_BUFFERS: usize = 128 * MB;
const MAX_SINGLE_BUFFER: usize = 16 * MB;
const DEFAULT_BUFFER_LEN: usize = 16384;

static CURRENT_SIZE: AtomicUsize = AtomicUsize::new(0);
pub static BUFFER_POOL: Pool<'static, 128, BytesMut> =
    Pool::new(&|| BytesMut::with_capacity(DEFAULT_BUFFER_LEN))
        .with_max_search(16)
        .with_take_hook(&|v| {
            CURRENT_SIZE.fetch_sub(v.capacity(), std::sync::atomic::Ordering::Release);
            v
        })
        .with_return_hook(&|mut v| {
            let capacity = v.capacity();
            if capacity > MAX_SINGLE_BUFFER
                || CURRENT_SIZE.load(std::sync::atomic::Ordering::Acquire) + capacity
                    > MAX_TOTAL_BUFFERS
            {
                None
            } else {
                // It's a soft limit.
                CURRENT_SIZE.fetch_add(capacity, std::sync::atomic::Ordering::Release);
                v.clear();
                Some(v)
            }
        });
