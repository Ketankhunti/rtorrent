//! The PeerManager, responsible for managing the pool of active peer connections.

use crate::messages::{ControlMessage, PeerEvent};
use crate::peer::PeerSession;
use crate::tracker::Peer;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// A handle to an active peer session, allowing the manager to send commands.
pub struct PeerHandle {
    pub to_worker_tx: mpsc::Sender<ControlMessage>,
}

/// A handle returned by the PeerManager, giving the TorrentManager
/// the ability to receive events from and send commands to the peer pool.
pub struct PeerManagerHandle {
    /// The receiver for events coming FROM all workers.
    pub from_workers_rx: mpsc::Receiver<(String, PeerEvent)>,
    /// A map to send commands TO specific workers.
    pub to_workers_tx: HashMap<String, mpsc::Sender<ControlMessage>>,
}

/// Manages the lifecycle of all peer connections.
pub struct PeerManager {
    info_hash: [u8; 20],
    our_peer_id: [u8; 20],
}

impl PeerManager {
    pub fn new(info_hash: [u8; 20], our_peer_id: [u8; 20]) -> Self {
        PeerManager {
            info_hash,
            our_peer_id,
        }
    }

    /// Spawns a session for each peer and returns a handle for communication.
    /// This method is now non-blocking.
    pub fn run(&mut self, initial_peers: Vec<Peer>) -> PeerManagerHandle {
        println!("PeerManager is running and spawning workers.");

        let (to_manager_tx, from_workers_rx) = mpsc::channel(100);
        let mut to_workers_tx = HashMap::new();

        for peer in initial_peers {
            let (to_worker_tx, from_manager_rx) = mpsc::channel(100);
            let peer_id_str = peer.socket_addr.to_string();

            to_workers_tx.insert(peer_id_str.clone(), to_worker_tx);

            let to_manager_tx_clone = to_manager_tx.clone();
            let info_hash = self.info_hash;
            let our_peer_id = self.our_peer_id;

            tokio::spawn(async move {
                match PeerSession::new(peer, &info_hash, &our_peer_id, to_manager_tx_clone, from_manager_rx).await {
                    Ok(mut session) => {
                        if let Err(e) = session.run().await {
                            eprintln!("Session with {} ended with error: {:?}", peer_id_str, e);
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to start session with {}: {:?}", peer_id_str, e);
                    }
                }
            });
        }

        PeerManagerHandle {
            from_workers_rx,
            to_workers_tx,
        }
    }
}
