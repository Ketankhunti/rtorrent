use std::{collections::HashMap, sync::Arc, time::Duration};

use tokio::{sync::{mpsc, Mutex}, time};

use crate::{disk::DiskManager, errors::AppError, messages::{BroadcastMessage, ControlMessage, DiskEvent, DiskMessage, DownloadStats, PeerEvent, PieceManagerEvent, PieceManagerMessage}, peer::Bitfield, peer_manager::{PeerManager, PeerManagerHandle}, piece_manager::PieceManager, storage::StorageManager, tracker::TrackerFactory, ui::UiManager, utils::build_tracker_url};

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
    file_name: String,
    to_piece_manager_tx: mpsc::Sender<PieceManagerMessage>,
    from_piece_manager_rx: mpsc::Receiver<PieceManagerEvent>,
    to_disk_manager_tx: mpsc::Sender<DiskMessage>,
    peer_states: HashMap<String, PeerState>,
    total_pieces: usize,
    pieces_verified: usize,
    to_broadcaster_tx: mpsc::Sender<BroadcastMessage>,
    our_master_bitfield: Arc<Mutex<Bitfield>>, // Our master download state
    from_disk_manager_rx: mpsc::Receiver<DiskEvent>,
    to_ui_tx: mpsc::Sender<DownloadStats>,

    bytes_downloaded_in_tick: usize,
    bytes_uploaded_in_tick: usize,
    // Add peer health monitoring
    peer_last_activity: HashMap<String, std::time::Instant>,
    peer_health_check_interval: time::Interval,
    // Add dynamic peer management
    disconnected_peers: HashMap<String, std::time::Instant>, // Track when peers disconnected
    peer_refresh_interval: time::Interval, // Get fresh peers from tracker
    tracker_url: String, // Store tracker URL for re-announcing
}

impl TorrentManager{
    pub async fn new(
        announce_url: String,
        output_path: &str,
        file_name: String,
        info_hash: [u8; 20],
        piece_hashes: Vec<[u8; 20]>,
        piece_length: u32,
        total_size: u64,
    ) -> Self {
        let our_peer_id = crate::utils::generate_peer_id(); // Assuming this is in a utils mod
        let storage = Arc::new(StorageManager::new());
        let total_pieces = piece_hashes.len();
        
        // Validate total_pieces to prevent massive memory allocation
        if total_pieces > 1_000_000 { // 1M pieces limit
            panic!("Total pieces {} is too large (max 1M), this suggests corrupted torrent data", total_pieces);
        }
        
        let to_peers_tx:Arc<Mutex<HashMap<String, mpsc::Sender<ControlMessage>>>> = Arc::new(Mutex::new(HashMap::new()));

         // It's all zeros initially because we have no pieces.
         let num_bitfield_bytes = (total_pieces as f64 / 8.0).ceil() as usize;
         
         // Validate bitfield size to prevent massive memory allocation
         if num_bitfield_bytes > 1_000_000 { // 1MB bitfield limit
             panic!("Bitfield size {} bytes is too large (max 1MB), this suggests corrupted torrent data", num_bitfield_bytes);
         }
         
         let our_master_bitfield = Arc::new(Mutex::new(Bitfield {
             bits: vec![0; num_bitfield_bytes],
         }));

        let (to_piece_manager_tx, from_peers_rx) = mpsc::channel(100);
        let (to_torrent_manager_tx, from_piece_manager_rx) = mpsc::channel(100);
        let (to_broadcaster_tx, mut from_torrent_manager_bdst) = mpsc::channel(100);
        let (to_disk_manager_tx, from_torrent_manager_rx) = mpsc::channel(100);
        let (to_ui_tx, from_torrent_manager_rx_ui) = mpsc::channel(100); 
        let (to_torrent_manager_tx_disk, from_disk_manager_rx) = mpsc::channel(100); // NEW


        // Build tracker URL for later use
        let tracker_url = build_tracker_url(
            &announce_url,
            &info_hash,
            &our_peer_id,
            total_size,
        ).unwrap_or_else(|_| announce_url.clone());

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

        // let mut disk_manager = DiskManager::new(
        //     output_path,
        //     total_size,
        //     piece_length,
        //     storage.clone(),
        //     from_torrent_manager_rx,
        //     to_torrent_manager_tx_disk,
        // ).await.unwrap();

        // tokio::spawn(async move {
        //     disk_manager.run().await;
        // });

        let mut ui_manager = UiManager::new(from_torrent_manager_rx_ui); // NEW
        tokio::spawn(async move { ui_manager.run().await; });

        TorrentManager {
            peer_manager,
            tracker_factory: Arc::new(TrackerFactory::new()),
            announce_url,
            info_hash,
            our_peer_id,
            total_size,
            file_name,
            to_piece_manager_tx,
            from_piece_manager_rx,
            to_disk_manager_tx,
            peer_states: HashMap::new(),
            total_pieces,
            pieces_verified:0,
            to_broadcaster_tx,
            our_master_bitfield,
            from_disk_manager_rx,
            to_ui_tx,
            bytes_downloaded_in_tick: 0,
            bytes_uploaded_in_tick: 0,
            peer_last_activity: HashMap::new(),
            peer_health_check_interval: time::interval(Duration::from_millis(1000)), // Check every 1 second
            disconnected_peers: HashMap::new(),
            peer_refresh_interval: time::interval(Duration::from_secs(30)), // Get fresh peers every 30 seconds
            tracker_url, // Use the tracker_url we built earlier
        }
    }

    pub async fn run(&mut self) -> Result<(), AppError> {
        println!("[TorrentManager] is running.");
        // 1. Get the initial list of peers from the tracker.
        let tracker_url = build_tracker_url(
            &self.announce_url,
            &self.info_hash,
            &self.our_peer_id,
            self.total_size,
        )?;
        // println!("{}", tracker_url);
        let tracker_client = self.tracker_factory.get_client(&tracker_url).unwrap();
        let initial_peers = tracker_client.announce(&tracker_url).await?;

        let peer_manager_handle = self.peer_manager.run(initial_peers).await;

        // 3. The TorrentManager's main event loop.
        // println!("TorrentManager entering main event loop.");

        let mut ui_tick = time::interval(Duration::from_millis(100));

        while self.pieces_verified < self.total_pieces {
            tokio::select! {
                // Listen for control events from the PeerManager (e.g., a peer is ready)
                Some((peer_id, event)) = async {
                    let mut guard = peer_manager_handle.lock().await;
                    guard.from_peers_rx.recv().await
                } => {
                    match event {
                        PeerEvent::Unchoked => {
                            println!("[TorrentManager] Event from {}: Unchoked. Asking PieceManager for a piece.", peer_id);
                            // This peer is ready. Ask the strategist what to do.
                            if !self.peer_states.contains_key(&peer_id) {
                                self.peer_states.insert(peer_id.clone(), PeerState::default());
                                println!("[TorrentManager] Created new peer state for {}", peer_id);
                                println!("[PeerCount] Active peers: {}, Disconnected peers: {}", 
                                        self.peer_states.len(), self.disconnected_peers.len());
                            }
                            self.update_peer_activity(&peer_id);
                            println!("[TorrentManager] Sending FindPieceToDownload for peer {}", peer_id);
                            self.to_piece_manager_tx
                                .send(PieceManagerMessage::FindPieceToDownload { peer_id })
                                .await
                                .unwrap();
                        }
                        // This event is the key to pipelining. When a block is downloaded,
                        // we immediately ask for the next one.
                        PeerEvent::BlockDownloaded => {
                            println!("[Manager] Event from {}: BlockDownloaded. Requesting next block.", peer_id);
                            self.update_peer_activity(&peer_id);
                            // if let Some(state) = self.peer_states.get_mut(&peer_id) {
                            //     state.in_flight_requests -= 1;
                            // }
                            // // Immediately request the next block to keep the pipeline full.
                            // self.request_next_blocks_for_peer(&peer_id).await;
                            self.to_piece_manager_tx
                                .send(PieceManagerMessage::FindPieceToDownload { peer_id })
                                .await
                                .unwrap();
                        }
                        PeerEvent::Choked => {
                            self.update_peer_activity(&peer_id);
                            if let Some(state) = self.peer_states.get_mut(&peer_id) {
                                state.is_choked = true;
                            }
                            self.to_piece_manager_tx
                                .send(PieceManagerMessage::PeerChoked { peer_id })
                                .await.unwrap();
                        }
                        PeerEvent::BlockRequested { piece_index, block_begin, block_length } => {
                            println!("[Manager] Event from {}: BlockRequested for piece #{}", peer_id, piece_index);
                            self.update_peer_activity(&peer_id);
                            let disk_msg = DiskMessage::ReadBlock { peer_id, piece_index, block_begin, block_length };
                            self.to_disk_manager_tx.send(disk_msg).await.unwrap();
                        }
                        PeerEvent::BytesDownloaded(count) => {
                            self.update_peer_activity(&peer_id);
                            self.bytes_downloaded_in_tick += count;
                        }
                        PeerEvent::BytesUploaded(count) => {
                            self.update_peer_activity(&peer_id);
                            self.bytes_uploaded_in_tick += count;
                        }
                        PeerEvent::PeerDisconnected { peer_id } => {
                            println!("[TorrentManager] Peer {} disconnected, notifying PieceManager", peer_id);
                            // Remove peer state
                            self.peer_states.remove(&peer_id);
                            // Remove from activity tracking
                            self.peer_last_activity.remove(&peer_id);
                            // Track for potential reconnection
                            self.disconnected_peers.insert(peer_id.clone(), std::time::Instant::now());
                            println!("[TorrentManager] Added peer {} to reconnection queue", peer_id);
                            println!("[PeerCount] Active peers: {}, Disconnected peers: {}", 
                                    self.peer_states.len(), self.disconnected_peers.len());
                            // Notify PieceManager to redistribute work
                            self.to_piece_manager_tx
                                .send(PieceManagerMessage::PeerDisconnected { peer_id })
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
                            if let Some(peer_tx) = peer_manager_handle.lock().await.to_peers_tx.lock().await.get(&peer_id) {
                                if peer_tx.send(command).await.is_err() {
                                    eprintln!("[TorrentManager] Error: Failed to send command to peer {}. Channel closed.", peer_id);
                                }
                            }
                        }
                        PieceManagerEvent::PieceVerified { piece_index, block_ids } => {
                            self.pieces_verified += 1;
                            println!("[TorrentManager] Event from PieceManager: Piece #{} is verified!", piece_index);
                            
                            { // Scoped lock
                                let mut bitfield = self.our_master_bitfield.lock().await;
                                bitfield.set_piece(piece_index);
                            }

                            let disk_msg = DiskMessage::WritePiece { piece_index, block_ids };
                            // self.to_disk_manager_tx.send(disk_msg).await.unwrap();
                            
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
                            if let Some(peer_tx) = peer_manager_handle.lock().await.to_peers_tx.lock().await.get(&peer_id) {
                                if peer_tx.send(command).await.is_err() {
                                    eprintln!("[Manager] Error: Failed to send block to peer {}.", peer_id);
                                }
                            }
                        }
                    }
                }

                _ = ui_tick.tick() => {
                    let stats = self.gather_stats(peer_manager_handle.clone()).await;
                    if self.to_ui_tx.send(stats).await.is_err() {
                        // UI task has closed, but we can continue downloading.
                        // println!("[Manager] UI task has shut down.");
                    }
               
                }
                
                // Peer health monitoring - check for stuck peers every second
                _ = self.peer_health_check_interval.tick() => {
                    self.check_peer_health().await;
                }
                
                // Peer refresh - get fresh peers from tracker every 30 seconds
                _ = self.peer_refresh_interval.tick() => {
                    self.refresh_peer_list(peer_manager_handle.clone()).await;
                }
            }
        }
        println!("\n🎉🎉🎉 DOWNLOAD COMPLETE! 🎉🎉🎉");
        // In a full client, we would now transition to "seeding" mode.
        // For now, we will just exit.
        Ok(())
    }

    async fn gather_stats(&mut self, handle: Arc<Mutex<PeerManagerHandle>>) -> DownloadStats {
        let handle_guard = handle.lock().await;
        let connected_peers = handle_guard.to_peers_tx.lock().await.len();
        let stats = DownloadStats {
            file_name: self.file_name.clone(),
            progress_percent: (self.pieces_verified as f32 / self.total_pieces as f32) * 100.0,
            pieces_verified: self.pieces_verified,
            total_pieces: self.total_pieces,
            connected_peers,
            download_speed_kbps: self.bytes_downloaded_in_tick as f64 / 1024.0,
            upload_speed_kbps: self.bytes_uploaded_in_tick as f64 / 1024.0,
        };
        // Reset counters for the next tick
        self.bytes_downloaded_in_tick = 0;
        self.bytes_uploaded_in_tick = 0;
        stats
    }

    async fn request_next_blocks_for_peer(&mut self, peer_id: &str) {
        if let Some(state) = self.peer_states.get_mut(peer_id) {
            if !state.is_choked && state.in_flight_requests < MAX_PIPELINED_REQUESTS {
                // Only send one request at a time to avoid flooding
                self.to_piece_manager_tx
                    .send(PieceManagerMessage::FindPieceToDownload { peer_id: peer_id.to_string() })
                    .await
                    .unwrap();
                state.in_flight_requests += 1; // Increment the counter
            }
        }
    }

    async fn check_peer_health(&mut self) {
        let now = std::time::Instant::now();
        let timeout_duration = std::time::Duration::from_secs(120); // 2 minutes timeout - less aggressive
        
        // Check for peers that haven't made progress in 2 minutes
        let mut stuck_peers = Vec::new();
        
        for (peer_id, last_activity) in &self.peer_last_activity {
            if now.duration_since(*last_activity) > timeout_duration {
                if let Some(state) = self.peer_states.get(peer_id) {
                    if state.in_flight_requests > 0 {
                        println!("[HealthCheck] Peer {} has been stuck for {} seconds with {} in-flight requests", 
                                peer_id, 
                                now.duration_since(*last_activity).as_secs(),
                                state.in_flight_requests);
                        stuck_peers.push(peer_id.clone());
                    }
                }
            }
        }
        
        // Handle stuck peers by marking them as disconnected
        for peer_id in stuck_peers {
            println!("[HealthCheck] Marking peer {} as disconnected due to inactivity", peer_id);
            
            // Remove from peer states
            self.peer_states.remove(&peer_id);
            
            // Remove from activity tracking
            self.peer_last_activity.remove(&peer_id);
            
            // Add to disconnected peers for potential reconnection
            self.disconnected_peers.insert(peer_id.clone(), std::time::Instant::now());
            
            println!("[PeerCount] After health check - Active peers: {}, Disconnected peers: {}", 
                    self.peer_states.len(), self.disconnected_peers.len());
            
            // Notify PieceManager to redistribute work
            self.to_piece_manager_tx
                .send(PieceManagerMessage::PeerDisconnected { peer_id })
                .await
                .unwrap();
        }
    }

    fn update_peer_activity(&mut self, peer_id: &str) {
        self.peer_last_activity.insert(peer_id.to_string(), std::time::Instant::now());
    }

    async fn refresh_peer_list(&mut self, peer_manager_handle: Arc<Mutex<PeerManagerHandle>>) {
        println!("[PeerRefresh] Refreshing peer list from tracker...");
        
        // Get fresh peers from tracker
        let tracker_client = self.tracker_factory.get_client(&self.tracker_url).unwrap();
        match tracker_client.announce(&self.tracker_url).await {
            Ok(new_peers) => {
                println!("[PeerRefresh] Got {} new peers from tracker", new_peers.len());
                
                // Try to connect to new peers
                for peer in new_peers {
                    let peer_id = peer.socket_addr.to_string();
                    
                    // Check if this peer was recently disconnected
                    if let Some(disconnect_time) = self.disconnected_peers.get(&peer_id) {
                        let time_since_disconnect = std::time::Instant::now().duration_since(*disconnect_time);
                        
                        // Only try to reconnect if it's been at least 60 seconds since disconnect
                        if time_since_disconnect.as_secs() < 60 {
                            println!("[PeerRefresh] Skipping recently disconnected peer {} (disconnected {}s ago)", 
                                    peer_id, time_since_disconnect.as_secs());
                            continue;
                        } else {
                            // Remove from disconnected list since we're trying again
                            self.disconnected_peers.remove(&peer_id);
                            println!("[PeerRefresh] Attempting to reconnect to peer {} after {}s", 
                                    peer_id, time_since_disconnect.as_secs());
                        }
                    }
                    
                    // Check if peer is already connected
                    let handle_guard = peer_manager_handle.lock().await;
                    let is_already_connected = handle_guard.to_peers_tx.lock().await.contains_key(&peer_id);
                    drop(handle_guard); // Release lock early
                    
                    if !is_already_connected {
                        println!("[PeerRefresh] Attempting to connect to new peer {}", peer_id);
                        // Note: In a real implementation, you'd spawn a new connection task here
                        // For now, we'll just log it
                    } else {
                        println!("[PeerRefresh] Peer {} is already connected", peer_id);
                    }
                }
            }
            Err(e) => {
                eprintln!("[PeerRefresh] Failed to get peers from tracker: {:?}", e);
            }
        }
        
        // Also try to reconnect to some recently disconnected peers
        self.try_reconnect_disconnected_peers().await;
    }

    async fn try_reconnect_disconnected_peers(&mut self) {
        let now = std::time::Instant::now();
        let reconnect_delay = std::time::Duration::from_secs(60); // Wait 60 seconds before reconnecting
        
        let mut peers_to_remove = Vec::new();
        
        for (peer_id, disconnect_time) in &self.disconnected_peers {
            if now.duration_since(*disconnect_time) > reconnect_delay {
                println!("[PeerRefresh] Attempting to reconnect to peer {} (disconnected {}s ago)", 
                        peer_id, now.duration_since(*disconnect_time).as_secs());
                
                // In a real implementation, you'd spawn a reconnection task here
                // For now, we'll just remove them from the disconnected list
                peers_to_remove.push(peer_id.clone());
            }
        }
        
        // Remove peers we're trying to reconnect to
        for peer_id in peers_to_remove {
            self.disconnected_peers.remove(&peer_id);
        }
        
        // Show current peer pool status
        self.show_peer_pool_status().await;
    }

    async fn show_peer_pool_status(&self) {
        println!("[PeerPool] Status: Active={}, Disconnected={}, Total tracked={}", 
                self.peer_states.len(),
                self.disconnected_peers.len(),
                self.peer_states.len() + self.disconnected_peers.len());
        
        if self.peer_states.is_empty() && !self.disconnected_peers.is_empty() {
            println!("[PeerPool] WARNING: No active peers! All peers are disconnected.");
        }
    }
}