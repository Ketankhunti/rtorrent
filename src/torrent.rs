use std::sync::Arc;

use tokio::sync::mpsc;

use crate::{disk::DiskManager, errors::AppError, messages::{ControlMessage, DiskMessage, PeerEvent, PieceManagerEvent, PieceManagerMessage}, peer_manager::PeerManager, piece_manager::PieceManager, storage::StorageManager, tracker::TrackerFactory, utils::build_tracker_url};

pub struct TorrentManager {
    // --- Sub-Managers ---
    peer_manager: PeerManager,
    // --- Static Info ---
    tracker_factory: Arc<TrackerFactory>,
    announce_url: String,
    info_hash: [u8; 20],
    our_peer_id: [u8; 20],
    total_size: u64,
    storage: Arc<StorageManager>,
    to_piece_manager_tx: mpsc::Sender<PieceManagerMessage>,
    from_piece_manager_rx: mpsc::Receiver<PieceManagerEvent>,
    to_disk_manager_tx: mpsc::Sender<DiskMessage>,
}

impl TorrentManager{
    pub async fn new(
        announce_url: String,
        output_path: &str,
        info_hash: [u8; 20],
        piece_hashes: Vec<[u8; 20]>,
        piece_length: u32,
        total_size: u64,
    ) -> Self {
        let our_peer_id = crate::utils::generate_peer_id(); // Assuming this is in a utils mod
        let storage = Arc::new(StorageManager::new());

        let (to_piece_manager_tx, from_peers_rx) = mpsc::channel(100);
        let (to_torrent_manager_tx, from_piece_manager_rx) = mpsc::channel(100);

        let (to_disk_manager_tx, from_torrent_manager_rx) = mpsc::channel(100);

        let mut piece_manager = PieceManager::new(
            piece_hashes.clone(),
            piece_length,
            storage.clone(),
            from_peers_rx,
            to_torrent_manager_tx,
        );

        let peer_manager = PeerManager::new(
            info_hash,
            our_peer_id,
            storage.clone(),
            to_piece_manager_tx.clone(),
        );
        tokio::spawn(async move {
            piece_manager.run().await;
        });

        let mut disk_manager = DiskManager::new(
            output_path,
            total_size,
            piece_length,
            storage.clone(),
            from_torrent_manager_rx,
        ).await.unwrap();

        tokio::spawn(async move {
            disk_manager.run().await;
        });

        TorrentManager {
            peer_manager,
            tracker_factory: Arc::new(TrackerFactory::new()),
            announce_url,
            info_hash,
            our_peer_id,
            total_size,
            storage,
            to_piece_manager_tx,
            from_piece_manager_rx,
            to_disk_manager_tx,
        }
    }

    pub async fn run(&mut self) -> Result<(), AppError> {
        println!("TorrentManager is running.");
        // 1. Get the initial list of peers from the tracker.
        let tracker_url = build_tracker_url(
            &self.announce_url,
            &self.info_hash,
            &self.our_peer_id,
            self.total_size,
        )?;
        println!("{}", tracker_url);
        let tracker_client = self.tracker_factory.get_client(&tracker_url).unwrap();
        let initial_peers = tracker_client.announce(&tracker_url).await?;

        let mut peer_manager_handle = self.peer_manager.run(initial_peers);

        // 3. The TorrentManager's main event loop.
        println!("TorrentManager entering main event loop.");
        loop {
            tokio::select! {
                // Listen for control events from the PeerManager (e.g., a peer is ready)
                Some((peer_id, event)) = peer_manager_handle.from_peers_rx.recv() => {
                    match event {
                        PeerEvent::Unchoked => {
                            println!("[TorrentManager] Event from {}: Unchoked. Asking PieceManager for a piece.", peer_id);
                            // This peer is ready. Ask the strategist what to do.
                            self.to_piece_manager_tx
                                .send(PieceManagerMessage::FindPieceToDownload { peer_id })
                                .await
                                .unwrap();
                        }
                        // This event is the key to pipelining. When a block is downloaded,
                        // we immediately ask for the next one.
                        PeerEvent::BlockDownloaded { .. } => {
                             self.to_piece_manager_tx
                                .send(PieceManagerMessage::FindPieceToDownload { peer_id })
                                .await
                                .unwrap();
                        }
                        _ => {}
                    }
                }

                // Listen for strategic decisions from the PieceManager
                Some(event) = self.from_piece_manager_rx.recv() => {
                    match event {
                        PieceManagerEvent::BlockSelected { peer_id, piece_index, block_begin, block_length } => {
                            println!("[TorrentManager] Decision from PieceManager: Tell peer {} to download block at {} for piece #{}.", peer_id, block_begin, piece_index);
                            // The strategist has spoken. Give the command to the worker.
                            let command = ControlMessage::RequestBlock {
                                piece_index,
                                block_begin,
                                block_length,
                            };
                            if let Some(peer_tx) = peer_manager_handle.to_peers_tx.get(&peer_id) {
                                if peer_tx.send(command).await.is_err() {
                                    eprintln!("[TorrentManager] Error: Failed to send command to peer {}. Channel closed.", peer_id);
                                }
                            }
                        }
                        PieceManagerEvent::PieceVerified { piece_index, block_ids } => {
                            println!("[TorrentManager] Event from PieceManager: Piece #{} is verified!", piece_index);
                            // The piece is good. Tell the DiskManager to write it.
                            let disk_msg = DiskMessage::WritePiece { piece_index, block_ids };
                            self.to_disk_manager_tx.send(disk_msg).await.unwrap();
                        }
                    }
                }
            }
        }
        Ok(())
    }
}