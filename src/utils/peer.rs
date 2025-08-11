use std::{net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6}, str::FromStr};

use crate::{bencode_parser::BencodeValue, errors::{AppError, TrackerError}};



#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub socket_addr: SocketAddr,
}


pub fn parse_non_compact_peers(peer_list: &[BencodeValue]) -> Result<Vec<Peer>, AppError> {
    let mut peers = Vec::new();
    for peer_info in peer_list {
        if let BencodeValue::Dictionary(peer_dict) = peer_info {
            let ip_bytes = peer_dict
                .get(&b"ip".to_vec())
                .and_then(|v| if let BencodeValue::String(s) = v { Some(s) } else { None });
            
            let port = peer_dict
                .get(&b"port".to_vec())
                .and_then(|v| if let BencodeValue::Integer(i) = v { Some(*i as u16) } else { None });

            if let (Some(ip_bytes), Some(port)) = (ip_bytes, port) {
                let ip_str = String::from_utf8_lossy(ip_bytes);
                if let Ok(ip) = IpAddr::from_str(&ip_str) {
                    peers.push(Peer { socket_addr: SocketAddr::new(ip, port) });
                }
            }
        }
    }
    Ok(peers)
}

pub fn parse_compact_peers(peers_bytes: &[u8]) -> Result<Vec<Peer>, AppError> {

    if peers_bytes.len() % 18 == 0  {
        Ok(
            peers_bytes
            .chunks_exact(18)
            .map(|chunk| {
                let mut ip_bytes = [0u8; 16];
                ip_bytes.copy_from_slice(&chunk[0..16]);
                let ip = Ipv6Addr::from(ip_bytes);
                let port = u16::from_be_bytes([chunk[16], chunk[17]]);
                Peer {
                    socket_addr: SocketAddr::V6(SocketAddrV6::new(ip, port, 0, 0)),
                }
            })
            .collect()
        )
    } else if peers_bytes.len() % 6 == 0 {
        Ok(peers_bytes
            .chunks_exact(6)
            .map(|chunk| {
                let ip = Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]);
                let port = u16::from_be_bytes([chunk[4], chunk[5]]);
                Peer {
                    socket_addr: SocketAddr::V4(SocketAddrV4::new(ip, port)),
                }
            })
            .collect())
    } else{
        Err(TrackerError::InvalidPeerListFormat.into())
    }

}