//! Defines the message types used for communication between the components.

use bytes::Bytes;

use crate::peer::Bitfield;
use crate::storage::BlockId;

// --- Messages for the PeerManager ---
#[derive(Debug)]
pub enum PeerEvent {
    BitfieldReceived(Bitfield),
    Choked,
    Unchoked,
    BlockDownloaded,
    BlockRequested {
        piece_index: u32,
        block_begin: u32,
        block_length: u32,
    },
}
#[derive(Debug,Clone)]
pub enum ControlMessage {
    RequestBlock {
        piece_index: u32,
        block_begin: u32,
        block_length: u32,
    },
    SendHave {
        piece_index: u32,
    },
    SendBlock {
        piece_index: u32,
        block_begin: u32,
        block_data: Bytes,
    },
   
}

// --- Messages for the PieceManager ---
#[derive(Debug)]
pub enum PieceManagerMessage {
    /// A block of data has been successfully downloaded and stored.
    /// It now includes the `block_begin` offset.
    BlockStored {
        peer_id: String,
        piece_index: u32,
        block_begin: u32,
        block_id: BlockId,
    },
    PeerHasBitfield {
        peer_id: String,
        bitfield: Bitfield,
    },
    FindPieceToDownload {
        peer_id: String,
    },
    PeerChoked {peer_id: String},
}

#[derive(Debug)]
pub enum PieceManagerEvent {
    PieceVerified {
        piece_index: u32,
        block_ids: Vec<BlockId>,
    },
    /// The response from the PieceManager with its strategic decision.
    /// It now includes the specific block to download.
    BlockSelected {
        peer_id: String,
        piece_index: u32,
        block_begin: u32,
        block_length: u32,
    },
}

#[derive(Debug)]
pub enum DiskMessage {
    /// A command to write a completed piece to the file on disk.
    WritePiece {
        piece_index: u32,
        block_ids: Vec<BlockId>,
    },
    ReadBlock {
        peer_id: String, // We need to know who to send the data back to
        piece_index: u32,
        block_begin: u32,
        block_length: u32,
    },
    
}

#[derive(Debug)]
pub enum DiskEvent {
    /// The data for a requested block has been successfully read from disk.
    BlockRead {
        peer_id: String,
        piece_index: u32,
        block_begin: u32,
        block_data: Bytes,
    },
}

#[derive(Debug, Clone)]
pub enum BroadcastMessage {
    Have(u32),
    // We could add other messages here later, like Shutdown
}