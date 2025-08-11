use std::env;

use rtorrent::{bencode_parser::parse, torrent::TorrentManager, utils::get_torrent_info};

#[tokio::main]
async fn main() {
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
    let (announce_url, info_hash, num_pieces, total_size) =
    match get_torrent_info(&bencode_data) {
        Ok(info) => info,
        Err(e) => {
            eprintln!("Error extracting info from torrent data: {:?}", e);
            return;
        }
    };

    // 2. Create the main orchestrator.
    let mut manager = TorrentManager::new(announce_url, info_hash, num_pieces, total_size);

    // 3. Run the client.
    if let Err(e) = manager.run().await {
        eprintln!("A fatal error occurred during client operation: {:?}", e);
    }

    println!("--- BitTorrent Client Shutting Down ---");
}
