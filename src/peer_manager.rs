//! The PeerManager, responsible for managing the pool of active peer connections.

use crate::messages::{ControlMessage, PeerEvent, PieceManagerMessage};
use crate::peer::PeerSession;
use crate::storage::StorageManager;
use crate::tracker::Peer;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

pub struct PeerManagerHandle {
    /// The receiver for events coming FROM all workers.
    pub from_peers_rx: mpsc::Receiver<(String, PeerEvent)>,
}

/// Manages the lifecycle of all peer connections.
pub struct PeerManager {
    info_hash: [u8; 20],
    our_peer_id: [u8; 20],
    storage: Arc<StorageManager>,
    to_piece_manager_tx: mpsc::Sender<PieceManagerMessage>,
    to_peers_tx: Arc<Mutex<HashMap<String, mpsc::Sender<ControlMessage>>>>,

}

impl PeerManager {
    pub fn new(
        info_hash: [u8; 20], 
        our_peer_id: [u8; 20],
        storage: Arc<StorageManager>,
        to_piece_manager_tx: mpsc::Sender<PieceManagerMessage>,
        to_peers_tx: Arc<Mutex<HashMap<String, mpsc::Sender<ControlMessage>>>>,
    ) -> Self {
        PeerManager {
            info_hash,
            our_peer_id,
            storage,
            to_piece_manager_tx,
            to_peers_tx,
        }
    }

    /// Spawns a session for each peer and returns a handle for communication.
    /// This method is now non-blocking.
    pub async fn run(&mut self, initial_peers: Vec<Peer>) -> PeerManagerHandle {
        println!("PeerManager is running and spawning workers.");

        let (to_peer_manager_tx, from_peers_rx) = mpsc::channel(100);
        let to_peers_tx = Arc::new(Mutex::new(HashMap::new()));

        for peer in initial_peers {
            let (to_peer_tx, from_peer_manager_rx) = mpsc::channel(100);
            let peer_id_str = peer.socket_addr.to_string();

            to_peers_tx.lock().await.insert(peer_id_str.clone(), to_peer_tx);

            let to_peer_manager_tx_clone = to_peer_manager_tx.clone();
            let to_piece_manager_tx_clone = self.to_piece_manager_tx.clone();
            let storage_clone = self.storage.clone();
            let info_hash = self.info_hash;
            let our_peer_id = self.our_peer_id;

            let to_peers_tx_clone = to_peers_tx.clone();

            tokio::spawn(async move {
                match PeerSession::new(
                    peer, 
                    &info_hash, 
                    &our_peer_id,
                    storage_clone,
                    to_peer_manager_tx_clone, 
                    to_piece_manager_tx_clone,
                    from_peer_manager_rx,
                ).await {
                    Ok(mut session) => {
                        if let Err(e) = session.run().await {
                            eprintln!("Session with {} ended with error: {:?}", &peer_id_str, e);
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to start session with {}: {:?}", &peer_id_str, e);
                    }
                }
                   // When the session ends (success or error), remove the peer from the map.
            to_peers_tx_clone.lock().await.remove(&peer_id_str);
            println!("[PeerManager] Cleaned up session for peer {}", &peer_id_str);
        
            });
            
        }

        PeerManagerHandle {
            from_peers_rx,
        }
    }
}
