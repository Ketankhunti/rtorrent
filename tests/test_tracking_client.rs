use std::{env, sync::Arc};

use rtorrent::{bencode_parser::parse, tracker::{self, HttpTrackerClient, TrackerClient, TrackerFactory, UdpTrackerClient}, utils::{build_tracker_url, calculate_info_hash, generate_peer_id}};

#[tokio::test]
async fn test_tracker_connection() {

    let mut factory = TrackerFactory::new();
    factory.register("https", || Arc::new(HttpTrackerClient));
    factory.register("http", || Arc::new(HttpTrackerClient));
    factory.register("udp", || Arc::new(UdpTrackerClient));

    let torrent_path = env::var("TORRENT_PATH")
        .expect("Please set TORRENT_PATH environment variable");
    println!("Reading torrent file: {}", torrent_path);

    let torrent_bytes = match std::fs::read(torrent_path) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("Error reading file: {}", e);
            return;
        }
    };

    let bencode_data = match parse(&torrent_bytes) {
        Ok(data) => data,
        Err(e) => {
            eprintln!("Error parsing torrent file: {:?}", e);
            return;
        }
    };

    let info_hash = calculate_info_hash(&bencode_data).unwrap();
    let peer_id = generate_peer_id();

    
    let tracker_url = build_tracker_url(&bencode_data,&info_hash,&peer_id).unwrap();
    println!("\nConstructed Tracker URL:\n{}", tracker_url);


    let client = match factory.get_client(&tracker_url) {
        Some(client) => client,
        None => {
            eprintln!("Unsupported tracker protocol for URL: {}", tracker_url);
            return;
        }
    };

    println!("\nQuerying tracker...");
    
    match client.announce(&tracker_url).await {
        Ok(peers) => {
            println!("\n✅ Success! Found {} peers:", peers.len());
            // ...
        }
        Err(e) => {
            eprintln!("\n❌ Error querying tracker: {:?}", e);
        }
    }
}
