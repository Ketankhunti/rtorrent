use std::sync::Arc;

use tokio::{fs::{File, OpenOptions}, io::{AsyncSeekExt, AsyncWriteExt}, sync::mpsc};

use crate::{messages::DiskMessage, storage::StorageManager};

pub struct DiskManager {
    // --- State ---
    storage: Arc<StorageManager>, // Handle to the central block warehouse
    file_handle: File,     // The handle to the output file
    piece_length: u32,
    from_torrent_manager_rx: mpsc::Receiver<DiskMessage>,
}

impl DiskManager {
    pub async fn new(
        output_path: &str,
        total_size: u64,
        piece_length: u32,
        storage: Arc<StorageManager>,
        from_torrent_manager_rx: mpsc::Receiver<DiskMessage>,
    ) -> Result<Self, std::io::Error>  {
         
         let file = OpenOptions::new()
            .write(true)
            .create(true)
            .open(output_path)
            .await?;

        file.set_len(total_size).await?;

        Ok(DiskManager { storage, file_handle: file, piece_length, from_torrent_manager_rx })

    }

    pub async fn run(&mut self) {
        println!("[DiskManager] Running.");
        while let Some(message) = self.from_torrent_manager_rx.recv().await {
            self.handle_message(message).await;
        }
    }

    async fn handle_message(&mut self, message:DiskMessage) {
        match message {
            DiskMessage::WritePiece { 
                piece_index, 
                block_ids 
            } => {
                println!("[DiskManager] Received command to write piece #{}.", piece_index);
                let mut assembled_piece = Vec::with_capacity(self.piece_length as usize);
                for block_id in block_ids {
                    if let Some(block_data) = self.storage.get_block(block_id) {
                        assembled_piece.extend_from_slice(&block_data);
                    } else {
                        eprintln!("[DiskManager] Error: Could not find block {} in store!", block_id);
                        return; // Skip writing this piece if a block is missing
                    }
                }
                let offset = (piece_index as u64) * (self.piece_length as u64);

                if let Err(e) = self.file_handle.seek(std::io::SeekFrom::Start(offset)).await {
                    eprintln!("[DiskManager] Error seeking in file: {}", e);
                    return;
                }
                if let Err(e) = self.file_handle.write_all(&assembled_piece).await {
                    eprintln!("[DiskManager] Error writing to file: {}", e);
                }else {
                    println!("[DiskManager] Successfully wrote piece #{} to disk.", piece_index);
                    println!("[DiskManager] Downloaded: {}", String::from_utf8_lossy(&assembled_piece));
                }
            }
        }
    }
}