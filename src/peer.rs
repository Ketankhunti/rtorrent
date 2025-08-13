use std::sync::Arc;
use std::time::{Duration};

use crate::errors::{AppError, PeerError};
use crate::messages::{ControlMessage, PeerEvent, PieceManagerMessage};
use crate::storage::StorageManager;
use crate::tracker::Peer;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex};
use tokio::time;

const PROTOCOL_STRING: &[u8] = b"BitTorrent protocol";

// These data structures remain, as they are fundamental to the protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerMessage {
    KeepAlive,
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have(u32),
    Bitfield(Bitfield),
    Request { index: u32, begin: u32, length: u32 },
    Piece { index: u32, begin: u32, block: Vec<u8> },
    Cancel { index: u32, begin: u32, length: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitfield {
    pub bits: Vec<u8>,
}

impl Bitfield {
    pub fn has_piece(&self, index: u32) -> bool {
        let byte_index = (index / 8) as usize;
        if byte_index >= self.bits.len() {
            return false;
        }
        let bit_index = 7 - (index % 8);
        (self.bits[byte_index] >> bit_index) & 1 != 0
    }

    pub fn set_piece(&mut self, index: u32) {
        let byte_index = (index / 8) as usize;
        if byte_index < self.bits.len() {
            let bit_index = 7 - (index % 8);
            // Use a bitwise OR to set only the specific bit to 1.
            // 1 << bit_index creates a mask (e.g., 00100000)
            self.bits[byte_index] |= 1 << bit_index;
        }
    }
}

/// The main struct for managing the state and logic of a single peer connection.
pub struct PeerSession {
    #[allow(dead_code)]
    peer: Peer,
    stream: TcpStream,
    storage: Arc<StorageManager>,
    to_peer_manager_tx: mpsc::Sender<(String,PeerEvent)>,
    to_piece_manager_tx: mpsc::Sender<PieceManagerMessage>,
    from_torrent_manager_rx: mpsc::Receiver<ControlMessage>,
    our_master_bitfield: Arc<Mutex<Bitfield>>,
    // --- Peer State ---
    peer_is_interested: bool,
    we_are_unchoked: bool, // Are we unchoked by the peer?
    // --- Our State ---
    we_are_interested: bool,
    peer_is_choked: bool, // Are we choking the peer?
    
}

impl PeerSession {
    /// Connects to a peer, performs the handshake, and returns a new PeerSession.
    pub async fn new(
        peer: Peer,
        info_hash: &[u8; 20],
        our_peer_id: &[u8; 20],
        storage: Arc<StorageManager>,
        our_master_bitfield: Arc<Mutex<Bitfield>>,
        to_peer_manager_tx: mpsc::Sender<(String,PeerEvent)>,
        to_piece_manager_tx: mpsc::Sender<PieceManagerMessage>,
        from_torrent_manager_rx: mpsc::Receiver<ControlMessage>,

    ) -> Result<Self, AppError> {
        let mut stream = Self::connect(&peer).await?;
        Self::perform_handshake(&mut stream, info_hash, our_peer_id).await?;

        Ok(PeerSession {
            peer,
            stream,
            storage,
            our_master_bitfield,
            to_peer_manager_tx,
            to_piece_manager_tx,
            from_torrent_manager_rx,
            peer_is_interested: false,
            we_are_unchoked: false,
            we_are_interested: false,
            peer_is_choked: true, // start by chocking every peer
        })
    }

    /// The main loop for a peer session. It concurrently listens for incoming
    /// peer messages and commands from the manager.
    pub async fn run(&mut self) -> Result<(), AppError> {
        // println!("Session running for peer {}", self.peer.socket_addr);
        let mut keep_alive_tick = time::interval(Duration::from_secs(10));
        // The main loop uses tokio::select! to handle multiple async sources.
        loop {
            tokio::select! {
                result = Self::read_message(&mut self.stream, &self.peer) => {
                    match result {
                        Ok(message) => self.handle_peer_message(message).await?,
                        Err(e) => return Err(e),
                    }
                }
                Some(command) = self.from_torrent_manager_rx.recv() => {
                    self.handle_manager_command(command).await?;
                }
                // _ = keep_alive_tick.tick() => {
                //     println!("[PeerSession {}] Sending KeepAlive.", self.peer.socket_addr);
                //     Self::send_message(&mut self.stream, PeerMessage::KeepAlive).await?;
                // }
            }
        }
    }

    // --- Private Helper Methods ---

    async fn connect(peer: &Peer) -> Result<TcpStream, AppError> {
        TcpStream::connect(peer.socket_addr)
            .await
            .map_err(|e| PeerError::ConnectionFailed(e.to_string()).into())
    }

    async fn perform_handshake(
        stream: &mut TcpStream,
        info_hash: &[u8; 20],
        our_peer_id: &[u8; 20],
    ) -> Result<(), AppError> {
        let handshake_msg = Self::build_handshake(info_hash, our_peer_id);
        stream
            .write_all(&handshake_msg)
            .await
            .map_err(|e| PeerError::HandshakeSendFailed(e.to_string()))?;

        let mut response_buffer = [0u8; 68];
        stream
            .read_exact(&mut response_buffer)
            .await
            .map_err(|e| PeerError::HandshakeReadFailed(e.to_string()))?;

        Self::validate_handshake(&response_buffer, info_hash)
    }
    
    /// Handles messages received FROM the remote peer.
    async fn handle_peer_message(&mut self, message: PeerMessage) -> Result<(), AppError> {
        let peer_id = self.peer.socket_addr.to_string();

        match message {
            PeerMessage::Bitfield(bitfield) => {
               // Bitfield is a data-related event, so it goes to the PieceManager.
               let msg = PieceManagerMessage::PeerHasBitfield { peer_id, bitfield };
               self.to_piece_manager_tx.send(msg).await.unwrap();
            }, 
            PeerMessage::Unchoke => {
                self.to_peer_manager_tx.send((peer_id.clone(),PeerEvent::Unchoked)).await.unwrap();
            }
            PeerMessage::Piece { index, begin, block } => {
                let block_len = block.len();
                let block_id = self.storage.store_block(block);
                let msg = PieceManagerMessage::BlockStored {
                    peer_id: peer_id.clone(),
                    piece_index: index,
                    block_begin: begin,
                    block_id,
                };
        
                self.to_piece_manager_tx.send(msg).await.unwrap();

                let control_event = PeerEvent::BlockDownloaded;
                self.to_peer_manager_tx.send((peer_id.clone(), control_event)).await.unwrap();

                let bytes_event = PeerEvent::BytesDownloaded(block_len);
                self.to_peer_manager_tx.send((peer_id, bytes_event)).await.unwrap();

            }
            PeerMessage::Interested => {
                // println!("[PeerSession {}] Peer is now interested in us.", peer_id);
                self.peer_is_interested = true;
                // If we are choking them, let's unchoke them so they can download.
                if self.peer_is_choked {
                    self.peer_is_choked = false;
                    Self::send_message(&mut self.stream, PeerMessage::Unchoke).await?;
                }
            }
            PeerMessage::Request { index, begin, length } => {
                // println!("[PeerSession {}] Received request for piece #{}, block at {}", peer_id, index, begin);
                let event = PeerEvent::BlockRequested {
                    piece_index: index,
                    block_begin: begin,
                    block_length: length,
                };
                self.to_peer_manager_tx.send((peer_id, event)).await.unwrap();
            }
            _ => {}
        }
        Ok(())
    }

    /// Handles commands received FROM our manager.
    async fn handle_manager_command(&mut self, command: ControlMessage) -> Result<(), AppError> {
        match command {
            ControlMessage::RequestBlock { piece_index, block_begin, block_length } => {
                let request_msg = PeerMessage::Request {
                    index: piece_index,
                    begin: block_begin,
                    length: block_length,
                };
                Self::send_message(&mut self.stream,request_msg).await?;
            }
            ControlMessage::SendHave { piece_index } => {
                // println!("[PeerSession {}] Sending Have for piece #{}", self.peer.socket_addr, piece_index);
                let have_msg = PeerMessage::Have(piece_index);
                Self::send_message(&mut self.stream,have_msg).await?;
            }
            ControlMessage::SendBlock { piece_index, block_begin, block_data } => {
                let block_len = block_data.len(); 
                // println!("[PeerSession {}] Sending block at {} for piece #{}", self.peer.socket_addr, block_begin, piece_index);
                let piece_msg = PeerMessage::Piece {
                    index: piece_index,
                    begin: block_begin,
                    block: block_data.to_vec(), // Convert Bytes to Vec<u8>
                };
                Self::send_message(&mut self.stream,piece_msg).await?;
                let peer_id = self.peer.socket_addr.to_string();
                let bytes_event = PeerEvent::BytesUploaded(block_len);
                self.to_peer_manager_tx.send((peer_id, bytes_event)).await.unwrap();
            }
            _ => {}
        }
        Ok(())
    }

    
    fn build_handshake(info_hash: &[u8; 20], our_peer_id: &[u8; 20]) -> [u8; 68] {
        let mut handshake = [0u8; 68];
        handshake[0] = 19;
        handshake[1..20].copy_from_slice(PROTOCOL_STRING);
        handshake[28..48].copy_from_slice(info_hash);
        handshake[48..68].copy_from_slice(our_peer_id);
        handshake
    }

    fn validate_handshake(response: &[u8; 68], expected_info_hash: &[u8; 20]) -> Result<(), AppError> {
        let response_info_hash = &response[28..48];
        if response_info_hash != expected_info_hash {
            return Err(PeerError::MismatchedInfoHash.into());
        }
        Ok(())
    }

    fn build_message(message: PeerMessage) -> Vec<u8> {
        match message {
            PeerMessage::KeepAlive => vec![0, 0, 0, 0],
            PeerMessage::Interested => vec![0, 0, 0, 1, 2],
            PeerMessage::Request { index, begin, length } => {
                let mut payload = Vec::with_capacity(12);
                payload.extend_from_slice(&index.to_be_bytes());
                payload.extend_from_slice(&begin.to_be_bytes());
                payload.extend_from_slice(&length.to_be_bytes());
                let mut full_message = Vec::with_capacity(17);
                full_message.extend_from_slice(&13u32.to_be_bytes());
                full_message.push(6);
                full_message.extend_from_slice(&payload);
                full_message
            }
            PeerMessage::Have(piece_index) => {
                let mut full_message = Vec::with_capacity(9);
                // <len=0005><id=4><piece_index>
                full_message.extend_from_slice(&5u32.to_be_bytes());
                full_message.push(4); // Message ID for 'Have'
                full_message.extend_from_slice(&piece_index.to_be_bytes());
                full_message
            }
            _ => vec![],
        }
    }
    
    // In src/peer.rs

    async fn read_message(stream: &mut TcpStream, peer: &Peer) -> Result<PeerMessage, AppError> {
        let mut length_buf = [0u8; 4];
        stream.read_exact(&mut length_buf).await
            .map_err(|e| PeerError::MessageReadFailed(e.to_string()))?;
        
        let length_prefix = u32::from_be_bytes(length_buf);

        // Validate message length to prevent massive memory allocation
        if length_prefix > 1_000_000 { // 1MB limit per message
            return Err(PeerError::MessageReadFailed(
                format!("Message length {} bytes is too large (max 1MB)", length_prefix)
            ).into());
        }

        if length_prefix == 0 {
            return Ok(PeerMessage::KeepAlive);
        }

        let mut id_buf = [0u8; 1];
        stream.read_exact(&mut id_buf).await
            .map_err(|e| PeerError::MessageReadFailed(e.to_string()))?;
        
        let message_id = id_buf[0];

        let payload_len = (length_prefix - 1) as usize;
        
        // Additional validation for payload length
        if payload_len > 999_999 { // 999KB limit (since length_prefix is already limited to 1MB)
            return Err(PeerError::MessageReadFailed(
                format!("Payload length {} bytes is too large (max 999KB)", payload_len)
            ).into());
        }
        
        if payload_len > 0 {
            let mut payload = vec![0u8; payload_len];
            stream.read_exact(&mut payload).await
                .map_err(|e| PeerError::MessageReadFailed(e.to_string()))?;
            
            match message_id {
                0 => Ok(PeerMessage::Choke),
                1 => Ok(PeerMessage::Unchoke),
                2 => Ok(PeerMessage::Interested),
                3 => Ok(PeerMessage::NotInterested),
                4 => Ok(PeerMessage::Have(u32::from_be_bytes(payload.try_into().unwrap()))),
                5 => Ok(PeerMessage::Bitfield(Bitfield { bits: payload })),
                7 => {
                    if payload.len() < 8 { return Err(PeerError::InvalidMessageFormat.into()); }
                    let index = u32::from_be_bytes(payload[0..4].try_into().unwrap());
                    let begin = u32::from_be_bytes(payload[4..8].try_into().unwrap());
                    let block = payload[8..].to_vec();
                    Ok(PeerMessage::Piece { index, begin, block })
                }
                _ => Err(PeerError::UnknownMessageId(message_id).into()),
            }
        } else {
            // Handle messages with no payload
            match message_id {
                0 => Ok(PeerMessage::Choke),
                1 => Ok(PeerMessage::Unchoke),
                2 => Ok(PeerMessage::Interested),
                3 => Ok(PeerMessage::NotInterested),
                _ => Err(PeerError::UnknownMessageId(message_id).into()),
            }
        }
    }

    async fn send_message(stream: &mut TcpStream, message: PeerMessage) -> Result<(), AppError> {
        let message_bytes = Self::build_message(message);
        stream
            .write_all(&message_bytes)
            .await
            .map_err(|e| PeerError::MessageSendFailed(e.to_string()))?; // You'll need a new MessageSendFailed error
        Ok(())
    }

    

}

