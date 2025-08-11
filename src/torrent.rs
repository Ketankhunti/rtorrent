use std::sync::Arc;

use crate::{bencode_parser::BencodeValue, errors::AppError, messages::{ControlMessage, PeerEvent}, peer_manager::PeerManager, piece_manager::PieceManager, tracker::TrackerFactory, utils::build_tracker_url};

pub struct TorrentManager {
    // --- Sub-Managers ---
    peer_manager: PeerManager,
    piece_manager: PieceManager,

    // --- Static Info ---
    tracker_factory: Arc<TrackerFactory>,
    announce_url: String,
    info_hash: [u8; 20],
    our_peer_id: [u8; 20],
    total_size: u64,
}

impl TorrentManager{
    pub fn new(
        announce_url: String,
        info_hash: [u8; 20],
        num_pieces: usize,
        total_size: u64,
    ) -> Self {
        let our_peer_id = crate::utils::generate_peer_id(); // Assuming this is in a utils mod
        
        TorrentManager {
            peer_manager: PeerManager::new(info_hash, our_peer_id),
            piece_manager: PieceManager::new(num_pieces),
            tracker_factory: Arc::new(TrackerFactory::new()),
            announce_url,
            info_hash,
            our_peer_id,
            total_size
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
        let tracker_client = self.tracker_factory.get_client(&tracker_url).unwrap();
        let initial_peers = tracker_client.announce(&tracker_url).await?;

        let mut peer_manager_handle = self.peer_manager.run(initial_peers);

        // 3. The TorrentManager's main event loop.
        println!("TorrentManager entering main event loop.");
        // --- THIS IS THE CORRECTED LOOP ---
        while let Some((peer_id, event)) = peer_manager_handle.from_workers_rx.recv().await {
            match event {
                PeerEvent::BitfieldReceived(bitfield) => {
                    println!("Manager: Peer {} sent its bitfield.", peer_id);
                    self.piece_manager.add_peer_bitfield(peer_id, bitfield);
                }
                PeerEvent::Unchoked => {
                    println!("Manager: Peer {} is now unchoked.", peer_id);
                    // Now that the peer is ready, let's ask the piece manager
                    // for a piece to download.
                    if let Some(piece_index) = self.piece_manager.select_piece_to_download() {
                        println!("Manager: Strategically selected piece #{} to download.", piece_index);
                        let command = ControlMessage::RequestBlock {
                            piece_index,
                            block_begin: 0,
                            block_length: 16384, // 16 KB
                        };
                        // Send the command to the peer manager, which will route it.
                        if let Some(peer_tx) = peer_manager_handle.to_workers_tx.get(&peer_id) {
                            peer_tx.send(command).await.unwrap();
                        }
                    }
                }
                PeerEvent::BlockDownloaded { piece_index, .. } => {
                    println!("Manager: A block for piece #{} was downloaded from {}. Requesting next piece.", piece_index, peer_id);
                    // This is a simplification. We should check if the piece is complete, then verify.
                    // For now, just request the next rarest piece from the same peer.
                    if let Some(next_piece) = self.piece_manager.select_piece_to_download() {
                         if let Some(peer_tx) = peer_manager_handle.to_workers_tx.get(&peer_id) {
                            peer_tx.send(ControlMessage::RequestBlock {
                                piece_index: next_piece,
                                block_begin: 0,
                                block_length: 16384
                            }).await.unwrap();
                        }
                    }
                }
                // Handle other events like Choked, etc.
                _ => {}
            }
        }

        Ok(())
    }
}