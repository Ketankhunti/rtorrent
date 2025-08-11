use std::{env, sync::Arc};

use rtorrent::{bencode_parser::parse, peer, tracker::{ HttpTrackerClient, TrackerFactory, UdpTrackerClient}, utils::{build_tracker_url, calculate_info_hash, generate_peer_id}};

#[tokio::test]
async fn test_handshake() {

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
    
    let peers = match client.announce(&tracker_url).await {
        Ok(peers) => {
            peers            // ...
        }
        Err(e) => {
            eprintln!("\n❌ Error querying tracker: {:?}", e);
            return;
        }
    };

    if peers.is_empty() {
        println!("\nNo peers found.");
        return;
    }

    println!("\n✅ Success! Found {} peers. Attempting handshakes...", peers.len());

    // --- NEW: Loop through the peers until we find one that works ---
    let mut successful_stream: Option<tokio::net::TcpStream> = None;

    for peer in &peers {
        println!("\nAttempting handshake with {}...", peer.socket_addr);
        match peer::perform_handshake(peer, &info_hash, &peer_id).await {
            Ok(stream) => {
                // We found a working peer!
                successful_stream = Some(stream);
                break; // Exit the loop since we have a connection.
            }
            Err(e) => {
                // This peer failed, but that's okay. We'll just try the next one.
                eprintln!("Handshake with {} failed: {:?}. Trying next peer.", peer.socket_addr, e);
            }
        }
    }

    if let Some(_stream) = successful_stream {
        println!("\n🎉 Successfully established a connection with a peer!");
        // The `_stream` is the active connection. We can now use it to exchange messages.
    } else {
        println!("\n❌ Could not establish a connection with any of the {} peers.", peers.len());
    }
}
