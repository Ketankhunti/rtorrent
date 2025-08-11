
pub mod peer;

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

pub fn build_tracker_url(
    bencode_data: &BencodeValue,
    info_hash: &[u8;20],
    peer_id: &[u8;20],
) -> Result<String, AppError> {
        let (announce_url, total_size) = if let BencodeValue::Dictionary(root) = bencode_data {
            let announce = root
                .get(&b"announce".to_vec())
                .and_then(|v|  if let BencodeValue::String(s) = v {Some(s)} else {None})
            .ok_or(BencodeError::MissingAnnounceURL)?;
        

        let info = root
                .get(&b"info".to_vec())
                .and_then(|v| if let BencodeValue::Dictionary(d) = v { Some(d) } else { None })
                .ok_or(BencodeError::MissingInfoDict)?;

        let size = if let Some(BencodeValue::Integer(length)) = info.get(&b"length".to_vec()) {
                    *length as u64
        } else if let Some(BencodeValue::List(files)) = info.get(&b"files".to_vec()) {
            files.iter().map(|file| {
                if let BencodeValue::Dictionary(file_dict) = file {
                    if let Some(BencodeValue::Integer(len)) = file_dict.get(&b"length".to_vec()) {
                        return *len as u64;
                    }
                }
                0
            }).sum()
        }else{
            0
        };

        (from_utf8(announce).unwrap().to_string(),size) 
        
    } else{
        return Err(BencodeError::RootNotADictionary.into());
    };

    let info_hash_encoded  = urlencoding::encode_binary(info_hash);
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
