use crate::storage::layout::BlockNumber;
use crate::types::state::{ChunkRef, ChunkSet};

pub struct RangeLocks;


pub struct State {
    desired: ChunkSet,
    available: ChunkSet,
    download_queue: ChunkSet,
    delete_queue: ChunkSet,
    locks: RangeLocks,
}


impl State {
    // returns `true` when new desired state leads to updates, `false` otherwise
    pub fn add_chunks(&mut self, new_chunks: &ChunkSet) {
        todo!()
    }

    pub fn delete_chunks(&mut self, delete_chunks: &ChunkSet) {
        todo!()
    }

    pub fn get_and_lock_chunks(&mut self, first_block: BlockNumber) -> Vec<ChunkRef> {
        todo!()
    }

    pub fn unlock_chunks(&mut self, chunks: &Vec<ChunkRef>) {
        todo!()
    }

    pub fn take_deletions(&mut self) -> ChunkSet {
        todo!()
    }

    pub fn take_next_download<F: FnMut(ChunkRef) -> DownloadId>(&mut self, f: F) {
        todo!()
    }

    pub fn complete_download(&mut self, id: DownloadId) {
        todo!()
    }

    pub fn take_canceled_downloads(&mut self) -> Vec<DownloadId> {
        todo!()
    }
}


pub type DownloadId = u64;