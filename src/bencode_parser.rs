use std::{collections::BTreeMap, str::from_utf8};

use crate::errors::{AppError, BencodeError};


#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BencodeValue {
    String(Vec<u8>),
    Integer(i64),
    List(Vec<BencodeValue>),
    Dictionary(BTreeMap< Vec<u8>,BencodeValue>),
}

pub fn parse<T: AsRef<[u8]>>(input: T) -> Result<BencodeValue, AppError> {
    let (value, remainder) = parse_internal(input.as_ref())?;

    if remainder.is_empty() {
        Ok(value)
    } else{
        Err(BencodeError::TrailingData.into())
    }
}

fn parse_internal(input: &[u8]) -> Result<(BencodeValue, &[u8]), AppError> {
    if input.is_empty() {
        return Err(BencodeError::UnexpectedEof.into());
    }

    match input[0] {
        b'i' => parse_integer(input),
        b'l' => parse_list(input),
        b'd' => parse_dictionary(input),
        b'0'..=b'9' => parse_string(input),
        _ => Err(BencodeError::InvalidType.into()),
    }
}

fn parse_string(input: &[u8]) -> Result<(BencodeValue, &[u8]), AppError> {
    let colon_pos = input.iter().position(|&b| b == b':')
        .ok_or(BencodeError::StringMissingColon)?;

    let len_str = from_utf8(&input[0..colon_pos])
        .map_err(|_| BencodeError::StringInvalidLength)?;
    let len:usize = len_str.parse()
        .map_err(|_| BencodeError::StringInvalidLength)?;
    let content_start = colon_pos + 1;
    let content_end = content_start + len;

    if content_end > input.len() {
        return Err(BencodeError::UnexpectedEof.into());
    }

    let content = input[content_start..content_end].to_vec();

    let remainder = &input[content_end..];

    Ok((BencodeValue::String(content),remainder))
}

fn parse_integer(input: &[u8]) -> Result<(BencodeValue, &[u8]), AppError> {
    let end_pos = input.iter().position(|&b| b == b'e')
        .ok_or(BencodeError::UnexpectedEof)?;

    let int_slice = &input[1..end_pos];

    if (int_slice.starts_with(&[b'0']) && int_slice.len() > 1) || (int_slice.starts_with(&[b'-', b'0'])) {
        return Err(BencodeError::IntegerLeadingZero.into());
    }
    if int_slice.is_empty() {
        return Err(BencodeError::IntegerEmpty.into());
    }

    let int_str = std::str::from_utf8(int_slice)
        .map_err(|_| BencodeError::InvalidIntegerFormat)?;
    let value: i64 = int_str.parse()
        .map_err(|_| BencodeError::InvalidIntegerFormat)?;
    let remainder = &input[end_pos + 1..];

    Ok((BencodeValue::Integer(value), remainder))
}

fn parse_list(input: &[u8]) -> Result<(BencodeValue, &[u8]), AppError> {
    let mut elements = Vec::new();

    let mut current_input = &input[1..];

    while !current_input.starts_with(&[b'e']) {
        let (value, remainder) = parse_internal(current_input)?;
        elements.push(value);
        current_input = remainder;
    }

    let remainder = &current_input[1..];
    Ok((BencodeValue::List(elements), remainder))
}

fn parse_dictionary(input: &[u8]) -> Result<(BencodeValue, &[u8]), AppError> {
    let mut dict = BTreeMap::new();
    let mut current_input = &input[1..];
    let mut last_key: Option<Vec<u8>> = None;

    while !current_input.starts_with(&[b'e']) {
        let (key_value, remainder_after_key) = parse_string(current_input)?;
        let key = match key_value {
            BencodeValue::String(s) => s,
            _ => return Err(BencodeError::DictKeyNotString.into()),
        };
        
        if let Some(ref last) = last_key {
            if &key <= last {
                return Err(BencodeError::DictKeysNotSorted.into());
            }
        }
        last_key = Some(key.clone());

        let (value, remainder_after_value) = parse_internal(remainder_after_key)?;

        dict.insert(key, value);
        current_input = remainder_after_value;
    }

    let remainder = &current_input[1..];
    Ok((BencodeValue::Dictionary(dict), remainder))
}
