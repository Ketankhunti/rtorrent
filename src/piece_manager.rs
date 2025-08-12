use crate::messages::{PieceManagerEvent, PieceManagerMessage};
use crate::peer::Bitfield;
use crate::storage::{BlockId, StorageManager};
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

const BLOCK_SIZE: u32 = 16384; // 16 KB

/// The state of a single 16KB block within a piece.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BlockState {
    Needed,      // We need this block.
    Requested,   // We have requested this block from a peer.
    Have(BlockId), // We have downloaded this block and stored it.
}

/// Represents the state of a single piece in the torrent.
#[derive(Debug, Clone)]
struct PieceState {
    availability: usize,
    /// We now track the state of every individual block.
    blocks: Vec<BlockState>,
    is_complete: bool,
}

/// Manages the state of all pieces and implements the download strategy.
pub struct PieceManager {
    pieces: Vec<PieceState>,
    peer_bitfields: HashMap<String, Bitfield>,
    piece_hashes: Vec<[u8; 20]>,
    piece_length: u32,
    storage: Arc<StorageManager>,
    from_others_rx: mpsc::Receiver<PieceManagerMessage>,
    to_torrent_manager_tx: mpsc::Sender<PieceManagerEvent>,
}

impl PieceManager {
    pub fn new(
        piece_hashes: Vec<[u8; 20]>,
        piece_length: u32,
        storage: Arc<StorageManager>,
        from_others_rx: mpsc::Receiver<PieceManagerMessage>,
        to_torrent_manager_tx: mpsc::Sender<PieceManagerEvent>,
    ) -> Self {
        let num_pieces = piece_hashes.len();
        let num_blocks_per_piece = (piece_length as f64 / BLOCK_SIZE as f64).ceil() as usize;

        PieceManager {
            pieces: vec![
                PieceState {
                    availability: 0,
                    blocks: vec![BlockState::Needed; num_blocks_per_piece],
                    is_complete: false,
                };
                num_pieces
            ],
            peer_bitfields: HashMap::new(),
            piece_hashes,
            piece_length,
            storage,
            from_others_rx,
            to_torrent_manager_tx,
        }
    }

    pub async fn run(&mut self) {
        println!("[PieceManager] Running.");
        while let Some(message) = self.from_others_rx.recv().await {
            self.handle_message(message).await;
        }
    }

    async fn handle_message(&mut self, message: PieceManagerMessage) {
        match message {
            PieceManagerMessage::PeerHasBitfield { peer_id, bitfield } => {
                self.add_peer_bitfield(peer_id, bitfield);
            }
            PieceManagerMessage::BlockStored { piece_index, block_begin, block_id, .. } => {
                let block_index = (block_begin / BLOCK_SIZE) as usize;
                let piece_state = &mut self.pieces[piece_index as usize];

                if !piece_state.is_complete && piece_state.blocks[block_index] == BlockState::Requested {
                    piece_state.blocks[block_index] = BlockState::Have(block_id);
                    
                    let all_blocks_have = piece_state.blocks.iter().all(|b| matches!(b, BlockState::Have(_)));
                    if all_blocks_have {
                        self.verify_and_complete_piece(piece_index).await;
                    }
                }
            }
            PieceManagerMessage::FindPieceToDownload { peer_id } => {
                if let Some(bitfield) = self.peer_bitfields.get(&peer_id).cloned() {
                    if let Some((piece_index, block_index)) = self.select_block_for_peer(&bitfield) {
                        let block_begin = (block_index as u32) * BLOCK_SIZE;
                        
                        // Handle the last block which might be smaller
                        let piece_size = self.piece_length;
                        let last_block_size = piece_size % BLOCK_SIZE;
                        let block_length = if block_begin + BLOCK_SIZE > piece_size && last_block_size > 0 {
                            last_block_size
                        } else {
                            BLOCK_SIZE
                        };

                        self.to_torrent_manager_tx
                            .send(PieceManagerEvent::BlockSelected {
                                peer_id,
                                piece_index,
                                block_begin,
                                block_length,
                            })
                            .await.unwrap();
                    }
                }
            }
        }
    }

    fn add_peer_bitfield(&mut self, peer_id: String, bitfield: Bitfield) {
        for i in 0..self.pieces.len() {
            if bitfield.has_piece(i as u32) {
                if let Some(piece) = self.pieces.get_mut(i) {
                    piece.availability += 1;
                }
            }
        }
        self.peer_bitfields.insert(peer_id, bitfield);
    }

    async fn verify_and_complete_piece(&mut self, piece_index: u32) {
        let piece_state = &mut self.pieces[piece_index as usize];
        
        let mut assembled_piece = Vec::with_capacity(self.piece_length as usize);
        let mut block_ids_to_keep = Vec::new();

        for block_state in &piece_state.blocks {
            if let BlockState::Have(block_id) = block_state {
                if let Some(block_data) = self.storage.get_block(*block_id) {
                    assembled_piece.extend_from_slice(&block_data);
                    block_ids_to_keep.push(*block_id);
                } else { return; }
            }
        }

        let mut hasher = Sha1::new();
        hasher.update(&assembled_piece);
        let hash: [u8; 20] = hasher.finalize().into();

        if hash == self.piece_hashes[piece_index as usize] {
            println!("[PieceManager] Piece #{} verified successfully!", piece_index);
            piece_state.is_complete = true;
            
            self.to_torrent_manager_tx
                .send(PieceManagerEvent::PieceVerified {
                    piece_index,
                    block_ids: block_ids_to_keep,
                })
                .await.unwrap();
        } else {
            println!("[PieceManager] Piece #{} failed verification!", piece_index);
            piece_state.blocks.fill(BlockState::Needed);
        }
    }

    fn select_block_for_peer(&mut self, peer_bitfield: &Bitfield) -> Option<(u32, u32)> {
        // First, find a piece that is already in progress (partially downloaded).
        if let Some((piece_index, _)) = self.pieces.iter().enumerate().find(|(idx, state)| {
            !state.is_complete &&
            state.blocks.iter().any(|b| matches!(b, BlockState::Requested)) &&
            peer_bitfield.has_piece(*idx as u32)
        }) {
            let piece_state = &mut self.pieces[piece_index];
            if let Some((block_index, _)) = piece_state.blocks.iter().enumerate().find(|(_, b)| **b == BlockState::Needed) {
                piece_state.blocks[block_index] = BlockState::Requested;
                return Some((piece_index as u32, block_index as u32));
            }
        }
        
        // If no pieces are in progress, find a new, rare piece to start.
        let rarest_piece = self.pieces
            .iter_mut()
            .enumerate()
            .filter(|(index, state)| {
                !state.is_complete && peer_bitfield.has_piece(*index as u32)
            })
            .min_by_key(|(_, state)| state.availability);

        if let Some((piece_index, piece_state)) = rarest_piece {
            if let Some((block_index, _)) = piece_state.blocks.iter().enumerate().find(|(_, b)| **b == BlockState::Needed) {
                piece_state.blocks[block_index] = BlockState::Requested;
                return Some((piece_index as u32, block_index as u32));
            }
        }
        None
    }
}
