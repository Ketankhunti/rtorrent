use crate::messages::{PieceManagerEvent, PieceManagerMessage};
use crate::peer::{self, Bitfield};
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
    Requested{peer_id: String},   // We have requested this block from a peer.
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
    // Queue for peers that want pieces but don't have bitfields yet
    pending_requests: Vec<String>,
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

        // Validate num_blocks_per_piece to prevent massive memory allocation
        if num_blocks_per_piece > 100_000 { // Reasonable limit: 100K blocks per piece
            panic!("Piece length {} results in {} blocks per piece, which is too large (max 100K)", 
                   piece_length, num_blocks_per_piece);
        }

        println!("[PieceManager] Creating {} pieces with {} blocks each", num_pieces, num_blocks_per_piece);

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
            pending_requests: Vec::new(),
        }
    }

    pub async fn run(&mut self) {
        // println!("[PieceManager] Running.");
        while let Some(message) = self.from_others_rx.recv().await {
            self.handle_message(message).await;
        }
    }

    fn handle_peer_disconnection(&mut self, peer_id: &str) {
        println!("[PieceManager] Peer {} disconnected, redistributing their work", peer_id);
        
        // Remove their bitfield
        self.peer_bitfields.remove(peer_id);
        
        // Reset all their requested blocks back to "Needed" so other peers can pick them up
        let mut redistributed_blocks = 0;
        for piece in self.pieces.iter_mut() {
            for block in piece.blocks.iter_mut() {
                if let BlockState::Requested { peer_id: requestor_id } = block {
                    if requestor_id == peer_id {
                        *block = BlockState::Needed;
                        redistributed_blocks += 1;
                    }
                }
            }
        }
        
        println!("[PieceManager] Redistributed {} blocks from disconnected peer {}", redistributed_blocks, peer_id);
        
        // Update piece availability counts - fix the borrowing issue
        for piece_index in 0..self.pieces.len() {
            if let Some(piece) = self.pieces.get_mut(piece_index) {
                // Recalculate availability for this piece
                let mut availability = 0;
                for (peer_id, bitfield) in &self.peer_bitfields {
                    if bitfield.has_piece(piece_index as u32) {
                        availability += 1;
                    }
                }
                piece.availability = availability;
            }
        }
    }

    async fn handle_message(&mut self, message: PieceManagerMessage) {
        match message {
            PieceManagerMessage::PeerHasBitfield { peer_id, bitfield } => {
                println!("[PieceManager] Received bitfield from peer {} with {} pieces", 
                        peer_id, bitfield.bits.iter().map(|&b| b.count_ones() as usize).sum::<usize>());
                self.add_peer_bitfield(peer_id, bitfield);
                println!("[PieceManager] Total peers with bitfields: {}", self.peer_bitfields.len());
            }
            PieceManagerMessage::BlockStored { peer_id, piece_index, block_begin, block_id } => {
                let block_index = (block_begin / BLOCK_SIZE) as usize;
                let piece_state = &mut self.pieces[piece_index as usize];

                if !piece_state.is_complete {
                    // Only accept the block if it was requested from this peer.
                    if let BlockState::Requested { peer_id: requester_id } = &piece_state.blocks[block_index] {
                        // println!("Block Stored: {} {}", requester_id, peer_id);
                        if *requester_id == peer_id {
                            piece_state.blocks[block_index] = BlockState::Have(block_id);
                            let all_blocks_have = piece_state.blocks.iter().all(|b| matches!(b, BlockState::Have(_)));
                            // println!("Blocks Length:{}  all_blocks_have:{}", piece_state.blocks.len(), all_blocks_have);
                            if all_blocks_have {
                                self.verify_and_complete_piece(piece_index).await;
                            }
                        }
                    }
                }
            }
            PieceManagerMessage::FindPieceToDownload { peer_id } => {
                // Check if we have a bitfield for this peer
                if let Some(bitfield) = self.peer_bitfields.get(&peer_id).cloned() {
                    if let Some((piece_index, block_index)) = self.select_block_for_peer(&peer_id, &bitfield) {
                        let block_begin = (block_index as u32) * BLOCK_SIZE;
                        
                        // Handle the last block which might be smaller
                        let piece_size = self.piece_length;
                        let last_block_size = piece_size % BLOCK_SIZE;
                        let block_length = if block_begin + BLOCK_SIZE > piece_size && last_block_size > 0 {
                            last_block_size
                        } else {
                            BLOCK_SIZE
                        };

                        println!("[PieceManager] Selected piece #{} block #{} for peer {}", piece_index, block_index, peer_id);
                        
                        self.to_torrent_manager_tx
                            .send(PieceManagerEvent::BlockSelected {
                                peer_id,
                                piece_index,
                                block_begin,
                                block_length,
                            })
                            .await.unwrap();
                    } else {
                        println!("[PieceManager] No available blocks for peer {} (all pieces complete or no suitable pieces)", peer_id);
                    }
                } else {
                    println!("[PieceManager] No bitfield available for peer {} yet, waiting for bitfield message", peer_id);
                    // We could queue this request or handle it differently
                }
            }
            PieceManagerMessage::PeerChoked { peer_id } => {
                // println!("[PieceManager] Peer {} choked. Resetting their requested blocks.", peer_id);
                self.handle_peer_disconnection(&peer_id);
            }
            PieceManagerMessage::PeerDisconnected { peer_id } => {
                println!("[PieceManager] Peer {} disconnected, cleaning up their state", peer_id);
                self.handle_peer_disconnection(&peer_id);
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

    fn select_block_for_peer(&mut self, peer_id: &str, peer_bitfield: &Bitfield) -> Option<(u32, u32)> {
        println!("[PieceManager] Selecting block for peer {} (has {} pieces available)", peer_id, 
                 peer_bitfield.bits.iter().map(|&b| b.count_ones() as usize).sum::<usize>());
        
        // First, find a piece that is already in progress for this peer.
        if let Some((piece_index, _)) = self.pieces.iter().enumerate().find(|(idx, state)| {
            !state.is_complete &&
            state.blocks.iter().any(|b| matches!(b, BlockState::Requested { peer_id: p } if p == peer_id)) &&
            peer_bitfield.has_piece(*idx as u32)
        }) {
            let piece_state = &mut self.pieces[piece_index];
            if let Some((block_index, _)) = piece_state.blocks.iter().enumerate().find(|(_, b)| **b == BlockState::Needed) {
                piece_state.blocks[block_index] = BlockState::Requested { peer_id: peer_id.to_string() };
                println!("[PieceManager] Continuing in-progress piece #{} block #{} for peer {}", piece_index, block_index, peer_id);
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
                piece_state.blocks[block_index] = BlockState::Requested { peer_id: peer_id.to_string() };
                println!("[PieceManager] Starting new piece #{} block #{} for peer {} (availability: {})", 
                        piece_index, block_index, peer_id, piece_state.availability);
                return Some((piece_index as u32, block_index as u32));
            }
        }
        
        // Check if there are any redistributed blocks available
        let redistributed_blocks: Vec<_> = self.pieces.iter().enumerate()
            .flat_map(|(piece_idx, piece)| {
                piece.blocks.iter().enumerate()
                    .filter(|(_, block)| matches!(block, BlockState::Needed))
                    .map(move |(block_idx, _)| (piece_idx, block_idx))
            })
            .collect();
        
        if !redistributed_blocks.is_empty() {
            println!("[PieceManager] Found {} redistributed blocks available for peer {}", redistributed_blocks.len(), peer_id);
        }
        
        // If no blocks are available, show detailed state
        if redistributed_blocks.is_empty() {
            self.show_detailed_block_state();
        }
        
        println!("[PieceManager] No suitable blocks found for peer {}", peer_id);
        None
    }

    fn show_detailed_block_state(&self) {
        let mut total_blocks = 0;
        let mut needed_blocks = 0;
        let mut requested_blocks = 0;
        let mut have_blocks = 0;
        let mut complete_pieces = 0;
        
        for (piece_idx, piece) in self.pieces.iter().enumerate() {
            if piece.is_complete {
                complete_pieces += 1;
                continue;
            }
            
            for (block_idx, block) in piece.blocks.iter().enumerate() {
                total_blocks += 1;
                match block {
                    BlockState::Needed => needed_blocks += 1,
                    BlockState::Requested { peer_id } => {
                        requested_blocks += 1;
                        println!("[Debug] Piece #{} block #{} requested by peer {}", piece_idx, block_idx, peer_id);
                    },
                    BlockState::Have(_) => have_blocks += 1,
                }
            }
        }
        
        println!("[Debug] Block state summary: Total={}, Needed={}, Requested={}, Have={}, Complete pieces={}", 
                total_blocks, needed_blocks, requested_blocks, have_blocks, complete_pieces);
    }
}

