use std::env;

use rtorrent::{bencode_parser, torrent::TorrentManager, utils::get_torrent_info};
// Declare all the modules that make up our application.

#[tokio::main]
async fn main() {
    // 1. Setup Phase: Load and parse the torrent file.
    let torrent_path = env::var("TORRENT_PATH").expect("Env not set");
    println!("--- Starting BitTorrent Client ---");
    println!("Reading torrent file: {}", torrent_path);

    let torrent_bytes = match std::fs::read(&torrent_path) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("Error reading file '{}': {}", &torrent_path, e);
            return;
        }
    };

    let bencode_data = match bencode_parser::parse(&torrent_bytes) {
        Ok(data) => data,
        Err(e) => {
            eprintln!("Error parsing torrent file: {:?}", e);
            return;
        }
    };

    // Extract all necessary metadata using our utility function.
    let (announce_url, info_hash, piece_hashes, piece_length, total_size) =
        match get_torrent_info(&bencode_data) {
            Ok(info) => info,
            Err(e) => {
                eprintln!("Error extracting info from torrent data: {:?}", e);
                return;
            }
        };

    // 2. Create the main orchestrator, passing all necessary info.
    let mut manager = TorrentManager::new(
        announce_url,
        "sample.txt",
        "sample.txt".to_string(),
        info_hash,
        piece_hashes,
        piece_length,
        total_size,
    ).await;

    // 3. Run the client.
    if let Err(e) = manager.run().await {
        eprintln!("A fatal error occurred during client operation: {:?}", e);
    }

    println!("--- BitTorrent Client Shutting Down ---");
}
