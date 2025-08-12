use std::sync::{atomic::{self, AtomicU64}, Arc};

use bytes::Bytes;
use dashmap::DashMap;

pub type BlockId = u64;

pub struct StorageManager {
    store: Arc<DashMap<BlockId, Bytes>>,
    next_id: Arc<AtomicU64>,
}

impl StorageManager {
    pub fn new() -> Self {
        Self {
            store: Arc::new(DashMap::new()),
            next_id: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn store_block(&self, block_data: Vec<u8>) -> BlockId {
        let id =  self.next_id.fetch_add(1,atomic::Ordering::Relaxed);
        let bytes = Bytes::from(block_data);
        self.store.insert(id, bytes);
        id
    }

    pub fn get_block(&self, id: BlockId) -> Option<Bytes> {
        // `get` returns a read guard, so we clone the Bytes to release the lock.
        self.store.get(&id).map(|bytes| bytes.clone())
    }

}