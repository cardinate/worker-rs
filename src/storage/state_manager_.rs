use std::sync::Mutex;

use tokio::sync::Notify;

use crate::storage::downloader_::Downloader;
use crate::storage::s3_fs::S3Filesystem;
use crate::storage::state_::State;


pub struct UpdateManager {
    state: Mutex<State>, // I've heard parking_lot::Mutex is much better
    notify: Notify,
    downloader: Downloader,
    archive: S3Filesystem
}


impl UpdateManager {
    async fn run(&self) {
        loop {
            self.notify.notified().await;

            let canceled_downloads = self.state.lock().unwrap().take_canceled_downloads();
            for download_id in canceled_downloads {
                self.downloader.cancel_download(download_id).await;
            }

            // handle deletions
            let deletions = self.state.lock().take_deletions();
            {
                // regular async deletion of directories
            }

            if self.downloader.is_ready() {
                self.state.lock().unwrap().take_next_download(|chunk| {
                    self.downloader.download_chunk(chunk)
                });
            }
        }
    }
}