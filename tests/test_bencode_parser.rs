
#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use rtorrent::bencode_parser::{parse, BencodeValue};

    #[test]
    fn test_simple_string() {
        assert_eq!(
            parse("5:hello"),
            Ok(BencodeValue::String(b"hello".to_vec()))
        );
    }
    
    #[test]
    fn test_simple_integer() {
        assert_eq!(parse("i42e"), Ok(BencodeValue::Integer(42)));
        assert_eq!(parse("i-42e"), Ok(BencodeValue::Integer(-42)));
        assert_eq!(parse("i0e"), Ok(BencodeValue::Integer(0)));
    }

    #[test]
    fn test_integer_leading_zero_error() {
        assert!(parse("i01e").is_err());
    }

    #[test]
    fn test_simple_list() {
        assert_eq!(
            parse("l5:helloi42ee"),
            Ok(BencodeValue::List(vec![
                BencodeValue::String(b"hello".to_vec()),
                BencodeValue::Integer(42)
            ]))
        );
    }

    #[test]
    fn test_simple_dictionary() {
        let mut expected_dict = BTreeMap::new();
        expected_dict.insert(b"foo".to_vec(), BencodeValue::String(b"bar".to_vec()));
        expected_dict.insert(b"key".to_vec(), BencodeValue::Integer(123));

        assert_eq!(
            parse("d3:foo3:bar3:keyi123ee"),
            Ok(BencodeValue::Dictionary(expected_dict))
        );
    }

    #[test]
    fn test_unsorted_dictionary_keys_error() {
        // 'z' comes after 'a'
        assert!(parse("d1:z1:a1:a1:be").is_err());
    }
    
    #[test]
    fn test_trailing_data_error() {
        assert!(parse("i42ee").is_err());
    }
}
