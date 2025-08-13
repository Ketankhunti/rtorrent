
use std::str::from_utf8;
use crate::{bencode_parser::BencodeValue, errors::{AppError, BencodeError}};
use sha1::{Sha1,Digest};

pub fn encode(value: &BencodeValue) -> Vec<u8> {
    let mut result = Vec::new();
    match value {
        BencodeValue::String(s) => {
            result.extend_from_slice(s.len().to_string().as_bytes());
            result.push(b':');
            result.extend_from_slice(s);
        }
        BencodeValue::Integer(i) => {
            result.push(b'i');
            result.extend_from_slice(i.to_string().as_bytes());
            result.push(b'e');
        }
        BencodeValue::List(l) => {
            result.push(b'l');
            for item in l {
                result.extend_from_slice(&encode(item));
            }
            result.push(b'e');
        }
        BencodeValue::Dictionary(d) => {
            result.push(b'd');
            for (key,val) in d {
                result.extend_from_slice(key.len().to_string().as_bytes());
                result.push(b':');
                result.extend_from_slice(key);
                // Encode the value
                result.extend_from_slice(&encode(val));
            }
            result.push(b'e');
        }
    }
    result
}

pub fn generate_peer_id() -> [u8; 20] {
    let mut peer_id = [0u8; 20];
    peer_id[0..8].copy_from_slice(b"-BC0001-"); 
    for i in 8..20 {
        peer_id[i] = (rand::random::<u8>() % 10) + b'0';
    }
    peer_id
}


/// Parses the bencoded torrent data to extract essential information.
pub fn get_torrent_info(
    bencode_data: &BencodeValue,
) -> Result<(String, [u8; 20], Vec<[u8; 20]>, u32, u64), AppError> {
    let root_dict = match bencode_data {
        BencodeValue::Dictionary(d) => d,
        _ => return Err(BencodeError::RootNotADictionary.into()),
    };

    let announce_url = root_dict
        .get(&b"announce".to_vec())
        .and_then(|v| if let BencodeValue::String(s) = v { Some(s) } else { None })
        .ok_or(BencodeError::MissingAnnounceURL)?;

    let info_dict = root_dict
        .get(&b"info".to_vec())
        .ok_or(BencodeError::MissingInfoDict)?;
    
    let info_map = match info_dict {
        BencodeValue::Dictionary(d) => d,
        _ => return Err(BencodeError::InvalidType.into()),
    };

    let info_bytes = encode(info_dict);
    let mut hasher = Sha1::new();
    hasher.update(&info_bytes);
    let info_hash: [u8; 20] = hasher.finalize().into();

    let pieces_bytes = info_map
        .get(&b"pieces".to_vec())
        .and_then(|v| if let BencodeValue::String(s) = v { Some(s) } else { None })
        .ok_or(BencodeError::InvalidType)?;
    
    let piece_hashes: Vec<[u8; 20]> = pieces_bytes
        .chunks_exact(20)
        .map(|chunk| chunk.try_into().expect("Chunk is not 20 bytes"))
        .collect();

    let num_pieces = pieces_bytes.len() / 20;
    
    let piece_length = info_map
        .get(&b"piece length".to_vec())
        .and_then(|v| if let BencodeValue::Integer(i) = v { Some(*i as u32) } else { None })
        .ok_or(BencodeError::InvalidType)?; // Or a more specific error

    let total_size = if let Some(BencodeValue::Integer(length)) = info_map.get(&b"length".to_vec()) {
        // Single file torrent
        let size = *length as u64;
        println!("[DEBUG] Single file torrent, size: {} bytes", size);
        size
    } else if let Some(BencodeValue::List(files)) = info_map.get(&b"files".to_vec()) {
        // Multi-file torrent
        println!("[DEBUG] Multi-file torrent with {} files", files.len());
        let total: u64 = files.iter().map(|file| {
            if let BencodeValue::Dictionary(file_dict) = file {
                if let Some(BencodeValue::Integer(len)) = file_dict.get(&b"length".to_vec()) {
                    let file_size = *len as u64;
                    if let Some(BencodeValue::String(path)) = file_dict.get(&b"path".to_vec()) {
                        println!("[DEBUG] File: {} - {} bytes", String::from_utf8_lossy(path), file_size);
                    }
                    return file_size;
                }
            }
            0
        }).sum();
        println!("[DEBUG] Total multi-file size: {} bytes", total);
        total
    } else {
        println!("[DEBUG] No length or files found in torrent info");
        0
    };

    // Validate the calculated size
    if total_size > 10_000_000_000 { // 10GB limit
        println!("[DEBUG] WARNING: Calculated size {} bytes exceeds 10GB limit!", total_size);
        println!("[DEBUG] This may indicate a parsing error or corrupted torrent file");
    }
    
    if total_size == 0 {
        println!("[DEBUG] ERROR: Calculated size is 0 bytes!");
    }

    Ok((
        String::from_utf8_lossy(announce_url).to_string(),
        info_hash,
        piece_hashes,
        piece_length,
        total_size,
    ))
}

/// Constructs the full tracker URL for the initial "started" announce event.
pub fn build_tracker_url(
    announce_url: &str,
    info_hash: &[u8; 20],
    peer_id: &[u8; 20],
    total_size: u64,
) -> Result<String, AppError> {
    let info_hash_encoded = urlencoding::encode_binary(info_hash);
    let peer_id_encoded = urlencoding::encode_binary(peer_id);

    let request_url = format!(
        "{}?info_hash={}&peer_id={}&port=6881&uploaded=0&downloaded=0&left={}&compact=1&event=started",
        announce_url,
        info_hash_encoded,
        peer_id_encoded,
        total_size
    );

    Ok(request_url)
}

pub fn calculate_info_hash(bencode_data: &BencodeValue) -> Result<[u8;20], &'static str> {
    if let BencodeValue::Dictionary(root_dict) = bencode_data {
        if let Some(info_value) = root_dict.get(&b"info".to_vec()) {
            let info_bytes = encode(info_value);

            let mut hasher = Sha1::new();
            hasher.update(&info_bytes);
            let hash = hasher.finalize();
            
            Ok(hash.into())
        } else {
            Err("Torrent metadata is missing 'info' dictionary")
        }
    } else {
        Err("Root of torrent file is not a dictionary")
    }
}
