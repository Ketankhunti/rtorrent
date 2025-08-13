use std::{collections::HashMap, sync::{Arc}};

use tokio::sync::{mpsc, Mutex};

use crate::{disk::DiskManager, errors::AppError, messages::{BroadcastMessage, ControlMessage, DiskEvent, DiskMessage, PeerEvent, PieceManagerEvent, PieceManagerMessage}, peer::Bitfield, peer_manager::PeerManager, piece_manager::PieceManager, storage::StorageManager, tracker::TrackerFactory, utils::build_tracker_url};

const MAX_PIPELINED_REQUESTS: usize = 5;

struct PeerState {
    is_choked: bool,
    in_flight_requests: usize,
}

impl Default for PeerState {
    fn default() -> Self {
        Self {
            is_choked: true, // Peers start as choked by default
            in_flight_requests: 0,
        }
    }
}

pub struct TorrentManager {
    // --- Sub-Managers ---
    peer_manager: PeerManager,
    // --- Static Info ---
    tracker_factory: Arc<TrackerFactory>,
    announce_url: String,
    info_hash: [u8; 20],
    our_peer_id: [u8; 20],
    total_size: u64,
    // storage: Arc<StorageManager>,
    to_piece_manager_tx: mpsc::Sender<PieceManagerMessage>,
    from_piece_manager_rx: mpsc::Receiver<PieceManagerEvent>,
    to_disk_manager_tx: mpsc::Sender<DiskMessage>,
    peer_states: HashMap<String, PeerState>,
    total_pieces: usize,
    pieces_verified: usize,
    to_broadcaster_tx: mpsc::Sender<BroadcastMessage>,
    our_master_bitfield: Arc<Mutex<Bitfield>>, // Our master download state
    from_disk_manager_rx: mpsc::Receiver<DiskEvent>
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
        let total_pieces = piece_hashes.len();
        let to_peers_tx:Arc<Mutex<HashMap<String, mpsc::Sender<ControlMessage>>>> = Arc::new(Mutex::new(HashMap::new()));

         // It's all zeros initially because we have no pieces.
         let num_bitfield_bytes = (total_pieces as f64 / 8.0).ceil() as usize;
         let our_master_bitfield = Arc::new(Mutex::new(Bitfield {
             bits: vec![0; num_bitfield_bytes],
         }));

        let (to_piece_manager_tx, from_peers_rx) = mpsc::channel(100);
        let (to_torrent_manager_tx, from_piece_manager_rx) = mpsc::channel(100);
        let (to_broadcaster_tx, mut from_torrent_manager_bdst) = mpsc::channel(100);
        let (to_disk_manager_tx, from_torrent_manager_rx) = mpsc::channel(100);

        let (to_torrent_manager_tx_disk, from_disk_manager_rx) = mpsc::channel(100); // NEW


        let channels_for_broadcast = to_peers_tx.clone();
        tokio::spawn(async move {
            while let Some(msg) = from_torrent_manager_bdst.recv().await {
                match msg {
                    BroadcastMessage::Have(piece_index) => {
                        println!("[Broadcast Task] Announcing Have for piece #{}", piece_index);
                        let have_msg = ControlMessage::SendHave { piece_index };
                        let channels = channels_for_broadcast.lock().await;
                        for tx in channels.values() {
                            let _ = tx.send(have_msg.clone()).await;
                        }
                    }
                }
            }
        });

        let peer_manager = PeerManager::new(
            info_hash,
            our_peer_id,
            storage.clone(),
            our_master_bitfield.clone(),
            to_piece_manager_tx.clone(),
            to_peers_tx.clone(),
        );
        

        let mut piece_manager = PieceManager::new(
            piece_hashes.clone(),
            piece_length,
            storage.clone(),
            from_peers_rx,
            to_torrent_manager_tx,
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
            to_torrent_manager_tx_disk,
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
            // storage,
            to_piece_manager_tx,
            from_piece_manager_rx,
            to_disk_manager_tx,
            peer_states: HashMap::new(),
            total_pieces,
            pieces_verified:0,
            to_broadcaster_tx,
            our_master_bitfield,
            from_disk_manager_rx,
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

        let mut peer_manager_handle = self.peer_manager.run(initial_peers).await;

        // 3. The TorrentManager's main event loop.
        println!("TorrentManager entering main event loop.");
        while self.pieces_verified < self.total_pieces {
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
                        PeerEvent::BlockDownloaded => {
                            println!("[Manager] Event from {}: BlockDownloaded. Requesting next block.", peer_id);
                            if let Some(state) = self.peer_states.get_mut(&peer_id) {
                                state.in_flight_requests -= 1;
                            }
                            // Immediately request the next block to keep the pipeline full.
                            self.request_next_blocks_for_peer(&peer_id).await;
                        }
                        PeerEvent::Choked => {
                            if let Some(state) = self.peer_states.get_mut(&peer_id) {
                                state.is_choked = true;
                            }
                            self.to_piece_manager_tx
                                .send(PieceManagerMessage::PeerChoked { peer_id })
                                .await.unwrap();
                        }
                        PeerEvent::BlockRequested { piece_index, block_begin, block_length } => {
                            println!("[Manager] Event from {}: BlockRequested for piece #{}", peer_id, piece_index);
                            let disk_msg = DiskMessage::ReadBlock { peer_id, piece_index, block_begin, block_length };
                            self.to_disk_manager_tx.send(disk_msg).await.unwrap();
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
                            if let Some(peer_tx) = peer_manager_handle.to_peers_tx.lock().await.get(&peer_id) {
                                if peer_tx.send(command).await.is_err() {
                                    eprintln!("[TorrentManager] Error: Failed to send command to peer {}. Channel closed.", peer_id);
                                }
                            }
                        }
                        PieceManagerEvent::PieceVerified { piece_index, block_ids } => {
                            self.pieces_verified += 1;
                            let progress = (self.pieces_verified as f32 / self.total_pieces as f32) * 100.0;
                            println!("[TorrentManager] Event from PieceManager: Piece #{} is verified!", piece_index);
                            
                            { // Scoped lock
                                let mut bitfield = self.our_master_bitfield.lock().await;
                                bitfield.set_piece(piece_index);
                            }

                            let disk_msg = DiskMessage::WritePiece { piece_index, block_ids };
                            self.to_disk_manager_tx.send(disk_msg).await.unwrap();
                            
                            self.to_broadcaster_tx
                                .send(BroadcastMessage::Have(piece_index))
                                .await
                                .unwrap();
                        }
                    }
                }
                Some(event) = self.from_disk_manager_rx.recv() => {
                    match event {
                        DiskEvent::BlockRead { peer_id, piece_index, block_begin, block_data } => {
                            println!("[Manager] Event from DiskManager: Sending block at {} for piece #{} to peer {}", block_begin, piece_index, peer_id);
                            let command = ControlMessage::SendBlock { piece_index, block_begin, block_data };
                            if let Some(peer_tx) = peer_manager_handle.to_peers_tx.lock().await.get(&peer_id) {
                                if peer_tx.send(command).await.is_err() {
                                    eprintln!("[Manager] Error: Failed to send block to peer {}.", peer_id);
                                }
                            }
                        }
                    }
                }
            }
        }
        println!("\n🎉🎉🎉 DOWNLOAD COMPLETE! 🎉🎉🎉");
        // In a full client, we would now transition to "seeding" mode.
        // For now, we will just exit.
        Ok(())
    }

    async fn request_next_blocks_for_peer(&mut self, peer_id: &str) {
        if let Some(state) = self.peer_states.get(peer_id) {
            if !state.is_choked {
                // Keep sending requests until the pipeline is full.
                while state.in_flight_requests < MAX_PIPELINED_REQUESTS {
                    self.to_piece_manager_tx
                        .send(PieceManagerMessage::FindPieceToDownload { peer_id: peer_id.to_string() })
                        .await
                        .unwrap();
                }
            }
        }
    }
}