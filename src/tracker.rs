use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;

use crate::{bencode_parser::{parse, BencodeValue}, errors::{AppError, TrackerError}, utils::peer::{parse_compact_peers, parse_non_compact_peers, Peer}};





#[async_trait]
pub trait TrackerClient: Send + Sync {
    async fn announce(&self, request_url: &str) -> Result<Vec<Peer>, AppError>;
}

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
    let bencode_response = parse(&body_bytes)?;
    
    if let BencodeValue::Dictionary(dict) = bencode_response {
        if let Some(BencodeValue::String(reason)) = dict.get(&b"failure reason".to_vec()) {
            let reason_str = String::from_utf8_lossy(reason).to_string();
            return Err(TrackerError::Failure(reason_str).into());
        }

        if let Some(peers_value) = dict.get(&b"peers".to_vec()) {
            match peers_value {
                BencodeValue::String(peers_bytes) => return parse_compact_peers(peers_bytes),
                BencodeValue::List(peer_list) => return parse_non_compact_peers(peer_list),
                _ => return Err(TrackerError::InvalidPeerListFormat.into()),
            }
        }
    }

    
    Err(TrackerError::MissingPeers.into())
    }
}

pub struct UdpTrackerClient;
#[async_trait]
impl TrackerClient for UdpTrackerClient {
    async fn announce(&self, request_url: &str) -> Result<Vec<Peer>, AppError> {
        println!("(Announcing via UDP)");
        unimplemented!()
    }
}

type ClientBuilder = Box<dyn Fn() -> Arc<dyn TrackerClient> + Send + Sync>;


pub struct TrackerFactory  {
    registry: HashMap<String, ClientBuilder>,
}

impl TrackerFactory  {
    pub fn new() -> Self {
        TrackerFactory { registry: HashMap::new() }
    }

    pub fn register<F>(&mut self, scheme: &str, builder:F) 
    where F: Fn() -> Arc<dyn TrackerClient> + Send + Sync + 'static,
    {
        self.registry.insert(scheme.to_string(), Box::new(builder));
    }

    pub fn get_client(&self, url: &str) -> Option<Arc<dyn TrackerClient>> {
        let scheme = url.split(':').next()?;
        self.registry.get(scheme).map(|builder| builder())
    }
}

