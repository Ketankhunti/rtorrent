use std::sync::Arc;

use bytes::Bytes;
use tokio::{fs::{File, OpenOptions}, io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt}, sync::mpsc};

use crate::{messages::{ControlMessage, DiskEvent, DiskMessage}, storage::StorageManager};

pub struct DiskManager {
    // --- State ---
    storage: Arc<StorageManager>, // Handle to the central block warehouse
    file_handle: File,     // The handle to the output file
    piece_length: u32,
    from_torrent_manager_rx: mpsc::Receiver<DiskMessage>,
    to_torrent_manager_tx_disk:  mpsc::Sender<DiskEvent>
}

impl DiskManager {
    pub async fn new(
        output_path: &str,
        total_size: u64,
        piece_length: u32,
        storage: Arc<StorageManager>,
        from_torrent_manager_rx: mpsc::Receiver<DiskMessage>,
        to_torrent_manager_tx_disk: mpsc::Sender<DiskEvent>,
    ) -> Result<Self, std::io::Error>  {
         
         let file = OpenOptions::new()
            .write(true)
            .create(true)
            .open(output_path)
            .await?;

        file.set_len(total_size).await?;

        Ok(DiskManager { storage, file_handle: file, piece_length, from_torrent_manager_rx, to_torrent_manager_tx_disk })

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
            DiskMessage::ReadBlock {
                peer_id, // We need this to know who the data is for
                piece_index,
                block_begin,
                block_length,
            } => {
                println!("[DiskManager] Received command to read block at {} for piece #{}.", block_begin, piece_index);

            // 1. Calculate the absolute byte offset in the file.
            let offset = (piece_index as u64 * self.piece_length as u64) + block_begin as u64;

            // 2. Seek to the correct position in the file.
            if let Err(e) = self.file_handle.seek(std::io::SeekFrom::Start(offset)).await {
                eprintln!("[DiskManager] Error seeking in file: {}", e);
                return;
            }

            // 3. Read the exact number of bytes for the block.
            let mut block_data_buffer = vec![0; block_length as usize];
            if let Err(e) = self.file_handle.read_exact(&mut block_data_buffer).await {
                eprintln!("[DiskManager] Error reading from file: {}", e);
                return;
            }

            let response = DiskEvent::BlockRead {
                peer_id,
                piece_index,
                block_begin,
                block_data: Bytes::from(block_data_buffer),
            };

            if self.to_torrent_manager_tx_disk.send(response).await.is_err() {
                eprintln!("[DiskManager] Error: Failed to send block data back to manager. Channel closed.");
            }

            }
        }
    }
}