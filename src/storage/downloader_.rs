use crate::storage::state_::DownloadId;
use crate::types::state::ChunkRef;


pub struct Downloader {

}


impl Downloader {
    pub fn is_ready(&self) -> bool {
        todo!()
    }

    /// Abort download procedure and delete leftover files
    pub async fn cancel_download(&self, id: DownloadId) {
        todo!()
    }

    pub fn download_chunk(&self, chunk: ChunkRef) -> DownloadId {
        todo!()
    }

    pub async fn get_updates(&self) -> DownloaderUpdates {
        todo!()
    }
}


pub struct DownloaderUpdates {
    completed: Vec<DownloadId>,
    is_ready: bool
}