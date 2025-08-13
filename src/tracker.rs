use crate::bencode_parser::BencodeValue;
use crate::errors::{AppError, TrackerError};
use async_trait::async_trait;
use rand::{random};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6, ToSocketAddrs};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::time::timeout;

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

pub struct UdpTrackerClient;

#[async_trait]
impl TrackerClient for UdpTrackerClient {
    async fn announce(&self, request_url: &str) -> Result<Vec<Peer>, AppError> {
        // println!("[UdpTracker] Announcing to {}", request_url);

        // 1. Create a UDP socket and resolve the tracker's address.
        let socket = UdpSocket::bind("0.0.0.0:0").await.map_err(|e| TrackerError::RequestFailed(e.to_string()))?;
        let tracker_addr = resolve_tracker_addr(request_url)?;
        socket.connect(tracker_addr).await.map_err(|e| TrackerError::RequestFailed(e.to_string()))?;

        // 2. Perform the "connect" handshake.
        let connection_id = self.perform_connect(&socket).await?;

        // 3. Perform the "announce" request.
        // This is a placeholder for now. We need the info_hash and other details.
        let announce_response = self.perform_announce(&socket, connection_id, [0; 20], [0; 20]).await?;

        // 4. Parse the peer list from the response.
        Ok(parse_compact_peers_udp(&announce_response.peer_info)?)
    }
}

impl UdpTrackerClient {
    /// Performs the initial Connect request and returns a connection_id.
    async fn perform_connect(&self, socket: &UdpSocket) -> Result<u64, AppError> {
        let transaction_id = random();
        let connect_request = ConnectRequest {
            protocol_id: 0x41727101980, // Magic constant
            action: 0, // 0 for "connect"
            transaction_id,
        };

        // Send the request
        socket.send(&connect_request.to_bytes()).await.map_err(|e| TrackerError::RequestFailed(e.to_string()))?;

        // Wait for the response with a timeout
        let mut response_buf = [0u8; 16];
        timeout(Duration::from_secs(5), socket.recv(&mut response_buf)).await
            .map_err(|_| TrackerError::RequestFailed("Connect request timed out".to_string()))?
            .map_err(|e| TrackerError::RequestFailed(e.to_string()))?;

        let connect_response = ConnectResponse::from_bytes(&response_buf)?;
        if connect_response.transaction_id != transaction_id || connect_response.action != 0 {
            return Err(TrackerError::Failure("Invalid connect response from tracker".to_string()).into());
        }

        // println!("[UdpTracker] Connect successful. Connection ID: {}", connect_response.connection_id);
        Ok(connect_response.connection_id)
    }

    /// Performs the Announce request.
    async fn perform_announce(
        &self,
        socket: &UdpSocket,
        connection_id: u64,
        info_hash: [u8; 20],
        peer_id: [u8; 20],
    ) -> Result<AnnounceResponse, AppError> {
        let transaction_id = random();
        let announce_request = AnnounceRequest {
            connection_id,
            action: 1, // 1 for "announce"
            transaction_id,
            info_hash,
            peer_id,
            downloaded: 0,
            left: 0, // Placeholder
            uploaded: 0,
            event: 2, // 2 for "started"
            ip_address: 0, // 0 for default
            key: 0, // Optional
            num_want: -1, // -1 for default
            port: 6881, // Placeholder
        };

        socket.send(&announce_request.to_bytes()).await.map_err(|e| TrackerError::RequestFailed(e.to_string()))?;

        let mut response_buf = vec![0u8; 2048]; // Buffer large enough for peer list
        let len = timeout(Duration::from_secs(5), socket.recv(&mut response_buf)).await
            .map_err(|_| TrackerError::RequestFailed("Announce request timed out".to_string()))?
            .map_err(|e| TrackerError::RequestFailed(e.to_string()))?;

        let announce_response = AnnounceResponse::from_bytes(&response_buf[..len])?;
        if announce_response.transaction_id != transaction_id || announce_response.action != 1 {
            return Err(TrackerError::Failure("Invalid announce response from tracker".to_string()).into());
        }
        
        // println!("[UdpTracker] Announce successful. Found {} peers.", announce_response.leechers);
        Ok(announce_response)
    }
}

// --- UDP Packet Structures ---

struct ConnectRequest {
    protocol_id: u64,
    action: u32,
    transaction_id: u32,
}

impl ConnectRequest {
    fn to_bytes(&self) -> [u8; 16] {
        let mut buf = [0u8; 16];
        buf[0..8].copy_from_slice(&self.protocol_id.to_be_bytes());
        buf[8..12].copy_from_slice(&self.action.to_be_bytes());
        buf[12..16].copy_from_slice(&self.transaction_id.to_be_bytes());
        buf
    }
}

struct ConnectResponse {
    action: u32,
    transaction_id: u32,
    connection_id: u64,
}

impl ConnectResponse {
    fn from_bytes(buf: &[u8; 16]) -> Result<Self, AppError> {
        Ok(Self {
            action: u32::from_be_bytes(buf[0..4].try_into().unwrap()),
            transaction_id: u32::from_be_bytes(buf[4..8].try_into().unwrap()),
            connection_id: u64::from_be_bytes(buf[8..16].try_into().unwrap()),
        })
    }
}

struct AnnounceRequest {
    connection_id: u64,
    action: u32,
    transaction_id: u32,
    info_hash: [u8; 20],
    peer_id: [u8; 20],
    downloaded: u64,
    left: u64,
    uploaded: u64,
    event: u32,
    ip_address: u32,
    key: u32,
    num_want: i32,
    port: u16,
}

impl AnnounceRequest {
    fn to_bytes(&self) -> [u8; 98] {
        let mut buf = [0u8; 98];
        buf[0..8].copy_from_slice(&self.connection_id.to_be_bytes());
        buf[8..12].copy_from_slice(&self.action.to_be_bytes());
        buf[12..16].copy_from_slice(&self.transaction_id.to_be_bytes());
        buf[16..36].copy_from_slice(&self.info_hash);
        buf[36..56].copy_from_slice(&self.peer_id);
        buf[56..64].copy_from_slice(&self.downloaded.to_be_bytes());
        buf[64..72].copy_from_slice(&self.left.to_be_bytes());
        buf[72..80].copy_from_slice(&self.uploaded.to_be_bytes());
        buf[80..84].copy_from_slice(&self.event.to_be_bytes());
        buf[84..88].copy_from_slice(&self.ip_address.to_be_bytes());
        buf[88..92].copy_from_slice(&self.key.to_be_bytes());
        buf[92..96].copy_from_slice(&self.num_want.to_be_bytes());
        buf[96..98].copy_from_slice(&self.port.to_be_bytes());
        buf
    }
}

struct AnnounceResponse {
    action: u32,
    transaction_id: u32,
    interval: u32,
    leechers: u32,
    seeders: u32,
    peer_info: Vec<u8>,
}

impl AnnounceResponse {
    fn from_bytes(buf: &[u8]) -> Result<Self, AppError> {
        if buf.len() < 20 {
            return Err(TrackerError::InvalidPeerListFormat.into());
        }
        Ok(Self {
            action: u32::from_be_bytes(buf[0..4].try_into().unwrap()),
            transaction_id: u32::from_be_bytes(buf[4..8].try_into().unwrap()),
            interval: u32::from_be_bytes(buf[8..12].try_into().unwrap()),
            leechers: u32::from_be_bytes(buf[12..16].try_into().unwrap()),
            seeders: u32::from_be_bytes(buf[16..20].try_into().unwrap()),
            peer_info: buf[20..].to_vec(),
        })
    }
}

// --- Helper Functions ---

fn resolve_tracker_addr(url: &str) -> Result<SocketAddr, AppError> {
    let url = url.strip_prefix("udp://").unwrap_or(url);
    url.to_socket_addrs()
        .map_err(|_| TrackerError::RequestFailed("Failed to resolve tracker hostname".to_string()))?
        .next()
        .ok_or_else(|| TrackerError::RequestFailed("No IP addresses found for tracker".to_string()).into())
}

fn parse_compact_peers_udp(peers_bytes: &[u8]) -> Result<Vec<Peer>, AppError> {
    if peers_bytes.len() % 6 != 0 {
        return Err(TrackerError::InvalidPeerListFormat.into());
    }
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