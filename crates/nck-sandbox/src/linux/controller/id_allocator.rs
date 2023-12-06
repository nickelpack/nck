use std::sync::{atomic::AtomicU32, Arc};

use nck_util::pool::OwnedPoolItem;

pub type PooledId = OwnedPoolItem<u32, flume::Sender<u32>>;

#[derive(Debug, Clone)]
pub struct IdAllocator {
    return_item: flume::Sender<u32>,
    next_free: flume::Receiver<u32>,
    next: Arc<AtomicU32>,
    max: u32,
}

impl IdAllocator {
    pub fn new(min: u32, max: u32) -> Self {
        let (return_item, next_free) = flume::unbounded();
        Self {
            return_item,
            next_free,
            next: Arc::new(AtomicU32::new(min)),
            max,
        }
    }

    pub async fn allocate(&self) -> PooledId {
        let result = if let Ok(result) = self.next_free.try_recv() {
            result
        } else if self.next.load(std::sync::atomic::Ordering::Acquire) < self.max {
            let result = self.next.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            if result <= self.max {
                result
            } else {
                self.next_free.recv_async().await.unwrap()
            }
        } else {
            self.next_free.recv_async().await.unwrap()
        };
        OwnedPoolItem::new(result, self.return_item.clone())
    }
}
