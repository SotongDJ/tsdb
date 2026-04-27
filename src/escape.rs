/// DOTSV escaping/unescaping.
///
/// The five bytes that must be escaped inside keys and values:
///   \t  (0x09) → \x09
///   \n  (0x0A) → \x0A
///   \r  (0x0D) → \x0D
///   =   (0x3D) → \x3D   (inside values only; keys may not contain = either)
///   \   (0x5C) → \\
///
/// The null byte (0x00) is used as a sentinel meaning "delete this key" in
/// patch operations; it is represented literally in the escaped form as `\x00`.
use crate::error::TsdbError;

/// Escape a raw key or value string for embedding in a DOTSV record.
pub fn escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '\t' => out.push_str("\\x09"),
            '\n' => out.push_str("\\x0A"),
            '\r' => out.push_str("\\x0D"),
            '=' => out.push_str("\\x3D"),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out
}

/// Unescape a DOTSV-encoded key or value back to its raw form.
/// Returns `Err` if the input contains an invalid escape sequence.
pub fn unescape(encoded: &str) -> Result<String, TsdbError> {
    let mut out = String::with_capacity(encoded.len());
    let bytes = encoded.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            if i + 1 >= bytes.len() {
                return Err(TsdbError::EscapeError(format!(
                    "trailing backslash in: {:?}",
                    encoded
                )));
            }
            match bytes[i + 1] {
                b'\\' => {
                    out.push('\\');
                    i += 2;
                }
                b'x' => {
                    if i + 3 >= bytes.len() {
                        return Err(TsdbError::EscapeError(format!(
                            "incomplete \\x escape in: {:?}",
                            encoded
                        )));
                    }
                    let hi = hex_digit(bytes[i + 2]).ok_or_else(|| {
                        TsdbError::EscapeError(format!(
                            "invalid escape sequence \\x{}{}: not valid hex in: {:?}",
                            bytes[i + 2] as char,
                            bytes[i + 3] as char,
                            encoded
                        ))
                    })?;
                    let lo = hex_digit(bytes[i + 3]).ok_or_else(|| {
                        TsdbError::EscapeError(format!(
                            "invalid escape sequence \\x{}{}: not valid hex in: {:?}",
                            bytes[i + 2] as char,
                            bytes[i + 3] as char,
                            encoded
                        ))
                    })?;
                    let byte_val = (hi << 4) | lo;
                    // Push the single byte as a char (only valid for ASCII/latin1 range used by spec)
                    out.push(byte_val as char);
                    i += 4;
                }
                other => {
                    return Err(TsdbError::EscapeError(format!(
                        "unknown escape \\{} in: {:?}",
                        other as char, encoded
                    )));
                }
            }
        } else {
            // Copy a full UTF-8 char at a time, not byte by byte
            let ch_str = &encoded[i..];
            let ch = ch_str.chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    Ok(out)
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Encode a list of string elements as a canonical DOTSV array value
/// `["a","b","c"]`. Inside each element only `"` and `\` are escaped
/// (to `\"` and `\\`); everything else passes through. The outer value
/// is then subject to normal DOTSV escaping when written via `escape()`.
pub fn encode_array(values: &[String]) -> String {
    let mut out = String::with_capacity(values.iter().map(|v| v.len() + 3).sum::<usize>() + 2);
    out.push('[');
    for (i, v) in values.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('"');
        for ch in v.chars() {
            match ch {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                other => out.push(other),
            }
        }
        out.push('"');
    }
    out.push(']');
    out
}

/// Return `true` iff `s` is in canonical array form `[...]` —
/// starts with `[` and ends with `]`.
pub fn is_array_value(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() >= 2 && bytes[0] == b'[' && bytes[bytes.len() - 1] == b']'
}

/// Decode a canonical DOTSV array value `["a","b","c"]` into its elements.
/// Elements are double-quoted strings; only `\"` and `\\` are recognised
/// as escapes inside an element.
pub fn decode_array(encoded: &str) -> Result<Vec<String>, TsdbError> {
    let bytes = encoded.as_bytes();
    if !is_array_value(encoded) {
        return Err(TsdbError::EscapeError(format!(
            "not an array value (must start with '[' and end with ']'): {:?}",
            encoded
        )));
    }
    let mut out = Vec::new();
    let end = bytes.len() - 1;
    let mut i = 1;
    if i == end {
        return Ok(out);
    }
    loop {
        if i >= end || bytes[i] != b'"' {
            return Err(TsdbError::EscapeError(format!(
                "expected '\"' at byte {} in: {:?}",
                i, encoded
            )));
        }
        i += 1;
        let mut elem = String::new();
        loop {
            if i >= end {
                return Err(TsdbError::EscapeError(format!(
                    "unterminated string element in: {:?}",
                    encoded
                )));
            }
            match bytes[i] {
                b'"' => {
                    i += 1;
                    break;
                }
                b'\\' => {
                    if i + 1 >= end {
                        return Err(TsdbError::EscapeError(format!(
                            "trailing backslash in array element: {:?}",
                            encoded
                        )));
                    }
                    match bytes[i + 1] {
                        b'"' => elem.push('"'),
                        b'\\' => elem.push('\\'),
                        other => {
                            return Err(TsdbError::EscapeError(format!(
                                "invalid escape \\{} in array element: {:?}",
                                other as char, encoded
                            )));
                        }
                    }
                    i += 2;
                }
                _ => {
                    let ch = encoded[i..].chars().next().unwrap();
                    elem.push(ch);
                    i += ch.len_utf8();
                }
            }
        }
        out.push(elem);
        if i >= end {
            break;
        }
        if bytes[i] != b',' {
            return Err(TsdbError::EscapeError(format!(
                "expected ',' or ']' at byte {} in: {:?}",
                i, encoded
            )));
        }
        i += 1;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_plain() {
        let s = "hello world";
        assert_eq!(unescape(&escape(s)).unwrap(), s);
    }

    #[test]
    fn round_trip_special() {
        let s = "tab\there\nnewline\\backslash=equals";
        let escaped = escape(s);
        assert!(!escaped.contains('\t'));
        assert!(!escaped.contains('\n'));
        assert_eq!(unescape(&escaped).unwrap(), s);
    }

    #[test]
    fn round_trip_multibyte_utf8() {
        let s = "héllo wörld 日本語";
        assert_eq!(unescape(&escape(s)).unwrap(), s);
    }

    #[test]
    fn escape_tab() {
        assert_eq!(escape("\t"), "\\x09");
    }

    #[test]
    fn escape_newline() {
        assert_eq!(escape("\n"), "\\x0A");
    }

    #[test]
    fn escape_equals() {
        assert_eq!(escape("="), "\\x3D");
    }

    #[test]
    fn escape_backslash() {
        assert_eq!(escape("\\"), "\\\\");
    }

    #[test]
    fn unescape_null_byte() {
        // \x00 in the file means delete-key sentinel
        assert_eq!(unescape("\\x00").unwrap(), "\x00");
    }

    #[test]
    fn unescape_invalid_escape() {
        assert!(unescape("\\q").is_err());
    }

    #[test]
    fn unescape_incomplete_hex() {
        assert!(unescape("\\x0").is_err());
    }

    #[test]
    fn unescape_uppercase_x_rejected() {
        // \X (uppercase) is not in the spec
        assert!(unescape("\\X09").is_err());
    }

    #[test]
    fn unescape_invalid_hex_digits_error_message() {
        let err = unescape("\\xZZ").unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.contains("invalid escape sequence"), "got: {}", msg);
    }

    #[test]
    fn encode_array_basic() {
        let vs = vec![
            "admin".to_string(),
            "editor".to_string(),
            "viewer".to_string(),
        ];
        assert_eq!(encode_array(&vs), r#"["admin","editor","viewer"]"#);
    }

    #[test]
    fn encode_array_empty() {
        let vs: Vec<String> = vec![];
        assert_eq!(encode_array(&vs), "[]");
    }

    #[test]
    fn encode_array_single() {
        let vs = vec!["alone".to_string()];
        assert_eq!(encode_array(&vs), r#"["alone"]"#);
    }

    #[test]
    fn encode_array_with_quotes_and_backslashes() {
        let vs = vec![r#"bob "the hammer""#.to_string(), r"c:\path".to_string()];
        assert_eq!(encode_array(&vs), r#"["bob \"the hammer\"","c:\\path"]"#);
    }

    #[test]
    fn encode_array_preserves_commas_and_brackets_in_elements() {
        let vs = vec!["Baker St, London".to_string(), "[tag]".to_string()];
        assert_eq!(encode_array(&vs), r#"["Baker St, London","[tag]"]"#);
    }

    #[test]
    fn is_array_value_yes() {
        assert!(is_array_value("[]"));
        assert!(is_array_value(r#"["a"]"#));
        assert!(is_array_value(r#"["a","b"]"#));
    }

    #[test]
    fn is_array_value_no() {
        assert!(!is_array_value(""));
        assert!(!is_array_value("a"));
        assert!(!is_array_value("[incomplete"));
        assert!(!is_array_value("trailing]"));
        assert!(!is_array_value("[oneChar"));
    }

    #[test]
    fn decode_array_basic() {
        let vs = decode_array(r#"["admin","editor","viewer"]"#).unwrap();
        assert_eq!(vs, vec!["admin", "editor", "viewer"]);
    }

    #[test]
    fn decode_array_empty() {
        let vs = decode_array("[]").unwrap();
        let empty: Vec<String> = vec![];
        assert_eq!(vs, empty);
    }

    #[test]
    fn decode_array_single() {
        let vs = decode_array(r#"["alone"]"#).unwrap();
        assert_eq!(vs, vec!["alone"]);
    }

    #[test]
    fn decode_array_with_quotes_and_backslashes() {
        let vs = decode_array(r#"["bob \"the hammer\"","c:\\path"]"#).unwrap();
        assert_eq!(vs, vec![r#"bob "the hammer""#, r"c:\path"]);
    }

    #[test]
    fn decode_array_preserves_commas_and_brackets_in_elements() {
        let vs = decode_array(r#"["Baker St, London","[tag]"]"#).unwrap();
        assert_eq!(vs, vec!["Baker St, London", "[tag]"]);
    }

    #[test]
    fn decode_array_round_trip_multibyte() {
        let vs = vec!["日本語".to_string(), "café".to_string()];
        let encoded = encode_array(&vs);
        let decoded = decode_array(&encoded).unwrap();
        assert_eq!(decoded, vs);
    }

    #[test]
    fn decode_array_rejects_non_array() {
        assert!(decode_array("plain").is_err());
        assert!(decode_array("[no-close").is_err());
        assert!(decode_array("no-open]").is_err());
    }

    #[test]
    fn decode_array_rejects_unquoted_element() {
        assert!(decode_array("[bare]").is_err());
        assert!(decode_array("[\"a\",bare]").is_err());
    }

    #[test]
    fn decode_array_rejects_invalid_escape() {
        assert!(decode_array(r#"["bad\x"]"#).is_err());
    }

    #[test]
    fn decode_array_rejects_unterminated_string() {
        assert!(decode_array(r#"["unterminated]"#).is_err());
    }
}
