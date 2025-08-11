use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpStream};

use crate::{errors::{AppError, PeerError}, utils::peer::Peer};

const PROTOCOL_STRING: &[u8] = b"BitTorrent protocol";

pub async fn perform_handshake(
    peer: &Peer,
    info_hash: &[u8;20],
    peer_id: &[u8;20],
) -> Result<TcpStream, AppError> {

    let mut stream = TcpStream::connect(peer.socket_addr)
        .await
        .map_err(|e| PeerError::ConnectionFailed(e.to_string()))?;

    println!("Connected to peer: {}", peer.socket_addr);

    let handshake_msg = build_handshake(info_hash, peer_id);
    stream
        .write_all(&handshake_msg)
        .await
        .map_err(|e| PeerError::HandshakeSendFailed(e.to_string()))?;
    
    println!("Sent handshake to peer.");

        let mut response_buffer = [0u8; 68];
        stream
            .read_exact(&mut response_buffer)
            .await
            .map_err(|e| PeerError::HandshakeReadFailed(e.to_string()))?;
        println!("Received handshake from peer.");

        // 4. Validate the response.
        validate_handshake(&response_buffer, info_hash)?;
    
        println!("✅ Handshake with {} successful!", peer.socket_addr);
        
        Ok(stream)
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