use crate::{peer::Bitfield};

#[derive(Debug)]
pub enum PeerEvent {
    /// A peer has sent us its complete bitfield.
    BitfieldReceived(Bitfield),

    /// A block of a piece has been successfully downloaded.
    BlockDownloaded {
        piece_index: u32,
        block_begin: u32,
        block_data: Vec<u8>,
    },

    /// The peer choked us (they are not ready to send us data).
    Choked,

    /// The peer unchoked us (they are now ready to send us data).
    Unchoked,
    
    // We can add more events here, like PeerDisconnected, etc.
}

#[derive(Debug)]
pub enum ControlMessage {
    /// Instructs the worker to request a specific block from its peer.
    RequestBlock {
        piece_index: u32,
        block_begin: u32,
        block_length: u32,
    },

    /// Instructs the worker to send a 'Have' message to its peer.
    SendHave {
        piece_index: u32,
    },

    // We can add more commands here, like Choke, Unchoke, etc.
}