use rtorrent::{bencode_parser::parse, tracker, utils::{build_tracker_url, calculate_info_hash, generate_peer_id}};

#[tokio::test]
async fn test_tracker_connection() {
    // 1. Define the path to the torrent file
    let torrent_path = "C:\\Users\\khunt\\Downloads\\ubuntu-25.04-desktop-amd64.iso.torrent";
    println!("Reading torrent file: {}", torrent_path);

    // 2. Read the file's raw bytes
    let torrent_bytes = match std::fs::read(torrent_path) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("Error reading file: {}", e);
            return;
        }
    };

    // 3. Parse the torrent file
    let bencode_data = match parse(&torrent_bytes) {
        Ok(data) => data,
        Err(e) => {
            eprintln!("Error parsing torrent file: {:?}", e);
            return;
        }
    };

    // 4. Calculate info_hash and generate peer_id
    let info_hash = calculate_info_hash(&bencode_data).unwrap();
    let peer_id = generate_peer_id();

    // 5. Build the tracker URL
    let tracker_url = build_tracker_url(&bencode_data, &info_hash, &peer_id).unwrap();
    println!("\nConstructed Tracker URL:\n{}", tracker_url);

    // 6. Query the tracker to get the peer list
    eprintln!("\nQuerying tracker...");
    match tracker::query_tracker(&tracker_url).await {
        Ok(peers) => {
            println!("\n✅ Success! Found {} peers:", peers.len());
            for peer in peers.iter().take(20) { // Print the first 20 peers
                println!("  - {}", peer.socket_addr);
            }
        }
        Err(e) => {
            eprintln!("\n❌ Error querying tracker: {:?}", e);
        }
    }
}
