use crate::bencode_parser::BencodeValue;
use crate::errors::{AppError, TrackerError};
use async_trait::async_trait;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::str::FromStr;
use std::sync::Arc;

// --- Public Data Structures & Traits ---

/// Represents a peer in the torrent swarm, supporting both IPv4 and IPv6.
/// This struct is defined here because it is the "product" of the tracker module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub socket_addr: SocketAddr,
}

/// The shared behavior for any tracker client.
/// Any struct that wants to be a "tracker client" MUST implement this behavior.
#[async_trait]
pub trait TrackerClient: Send + Sync {
    /// Announces to the tracker and returns a list of peers.
    async fn announce(&self, request_url: &str) -> Result<Vec<Peer>, AppError>;
}

// --- Concrete Implementations ---

/// An implementation of TrackerClient that uses the HTTP(S) protocol.
pub struct HttpTrackerClient;

#[async_trait]
impl TrackerClient for HttpTrackerClient {
    async fn announce(&self, request_url: &str) -> Result<Vec<Peer>, AppError> {
        let response = reqwest::get(request_url)
            .await
            .map_err(|e| TrackerError::RequestFailed(e.to_string()))?;

        if !response.status().is_success() {
            return Err(TrackerError::UnsuccessfulResponse(response.status().as_u16()).into());
        }

        let body_bytes = response
            .bytes()
            .await
            .map_err(|e| TrackerError::RequestFailed(e.to_string()))?;

        let bencode_response = crate::bencode_parser::parse(&body_bytes)?;

        if let BencodeValue::Dictionary(dict) = bencode_response {
            if let Some(BencodeValue::String(reason)) = dict.get(&b"failure reason".to_vec()) {
                let reason_str = String::from_utf8_lossy(reason).to_string();
                return Err(TrackerError::Failure(reason_str).into());
            }

            if let Some(peers_value) = dict.get(&b"peers".to_vec()) {
                return match peers_value {
                    BencodeValue::String(peers_bytes) => parse_compact_peers(peers_bytes),
                    BencodeValue::List(peer_list) => parse_non_compact_peers(peer_list),
                    _ => Err(TrackerError::InvalidPeerListFormat.into()),
                };
            }
        }
        Err(TrackerError::MissingPeers.into())
    }
}

/// A placeholder implementation for a future UDP tracker client.
pub struct UdpTrackerClient;

#[async_trait]
impl TrackerClient for UdpTrackerClient {
    async fn announce(&self, _request_url: &str) -> Result<Vec<Peer>, AppError> {
        println!("(Announcing via UDP - Not yet implemented)");
        // In a future milestone, we would implement the UDP-specific logic here.
        unimplemented!("UDP tracker protocol is not yet supported.")
    }
}

// --- Factory for Creating Clients ---

type ClientBuilder = Box<dyn Fn() -> Arc<dyn TrackerClient> + Send + Sync>;

/// The factory responsible for creating tracker clients based on protocol scheme.
pub struct TrackerFactory {
    registry: HashMap<String, ClientBuilder>,
}

impl TrackerFactory {
    /// Creates a new factory and registers the default clients.
    pub fn new() -> Self {
        let mut factory = TrackerFactory {
            registry: HashMap::new(),
        };
        factory.register("https", || Arc::new(HttpTrackerClient));
        factory.register("http", || Arc::new(HttpTrackerClient));
        factory.register("udp", || Arc::new(UdpTrackerClient));
        factory
    }

    /// Registers a new protocol handler.
    pub fn register<F>(&mut self, scheme: &str, builder: F)
    where
        F: Fn() -> Arc<dyn TrackerClient> + Send + Sync + 'static,
    {
        self.registry.insert(scheme.to_string(), Box::new(builder));
    }

    /// Creates a client for the given URL scheme.
    pub fn get_client(&self, url: &str) -> Option<Arc<dyn TrackerClient>> {
        let scheme = url.split(':').next()?;
        self.registry.get(scheme).map(|builder| builder())
    }
}

// --- Private Helper Functions for Parsing Peer Lists ---

fn parse_non_compact_peers(peer_list: &[BencodeValue]) -> Result<Vec<Peer>, AppError> {
    let mut peers = Vec::new();
    for peer_info in peer_list {
        if let BencodeValue::Dictionary(peer_dict) = peer_info {
            let ip_bytes = peer_dict
                .get(&b"ip".to_vec())
                .and_then(|v| {
                    if let BencodeValue::String(s) = v {
                        Some(s)
                    } else {
                        None
                    }
                });
            let port = peer_dict
                .get(&b"port".to_vec())
                .and_then(|v| {
                    if let BencodeValue::Integer(i) = v {
                        Some(*i as u16)
                    } else {
                        None
                    }
                });

            if let (Some(ip_bytes), Some(port)) = (ip_bytes, port) {
                let ip_str = String::from_utf8_lossy(ip_bytes);
                if let Ok(ip) = IpAddr::from_str(&ip_str) {
                    peers.push(Peer {
                        socket_addr: SocketAddr::new(ip, port),
                    });
                }
            }
        }
    }
    Ok(peers)
}

fn parse_compact_peers(peers_bytes: &[u8]) -> Result<Vec<Peer>, AppError> {
    if peers_bytes.len() % 18 == 0 {
        Ok(peers_bytes
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
            .collect())
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
    } else {
        Err(TrackerError::InvalidPeerListFormat.into())
    }
}