use crate::peer::Bitfield;
use std::collections::HashMap;

/// Represents the state of a single piece in the torrent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PieceState {
    /// We still need this piece. The usize counts how many peers have it.
    Needed { availability: usize },
    /// We have successfully downloaded and verified this piece.
    Have,
}

/// Manages the state of all pieces and implements the download strategy.
pub struct PieceManager {
    /// The state of every piece in the torrent.
    pieces: Vec<PieceState>,
    /// A map from a peer's ID to the pieces they have (their bitfield).
    peer_bitfields: HashMap<String, Bitfield>,
    /// The total number of pieces in the torrent.
    num_pieces: usize,
}

impl PieceManager {
    /// Creates a new PieceManager.
    pub fn new(num_pieces: usize) -> Self {
        PieceManager {
            // Initially, we need all pieces, and we assume 0 availability.
            pieces: vec![PieceState::Needed { availability: 0 }; num_pieces],
            peer_bitfields: HashMap::new(),
            num_pieces,
        }
    }

    pub fn add_peer_bitfield(&mut self, peer_id: String, bitfield: Bitfield) {
        for i in 0..self.num_pieces {
            if bitfield.has_piece(i as u32) {
                if let PieceState::Needed { availability } = &mut self.pieces[i] {
                    *availability += 1;
                }
            }
        }
        self.peer_bitfields.insert(peer_id, bitfield);
    }

    pub fn select_piece_to_download(&self) -> Option<u32> {
        let mut rarest_piece_index = None;
        let mut min_availability = usize::MAX;

        // Find the piece we need that is owned by the fewest number of peers.
        for (index, state) in self.pieces.iter().enumerate() {
            if let PieceState::Needed { availability } = state {
                if *availability > 0 && *availability < min_availability {
                    min_availability = *availability;
                    rarest_piece_index = Some(index as u32);
                }
            }
        }
        rarest_piece_index
    }
    /// Records that we have successfully downloaded a piece.
    pub fn piece_completed(&mut self, piece_index: u32) {
        if let Some(piece) = self.pieces.get_mut(piece_index as usize) {
            *piece = PieceState::Have;
            // When we get a new piece, we need to recalculate availability
            // for all other pieces, as peers might send us 'Have' messages.
            // (This is a simplification for now).
        }
    }

    // In a full client, we would also add functions here to:
    // - Validate a piece's hash against the torrent's info dictionary.
    // - Handle incoming 'Have' messages to update availability.
    // - Manage which blocks of a piece have been requested (for multi-block pieces).

}