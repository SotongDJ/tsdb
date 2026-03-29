/// DOTSV escaping/unescaping.
///
/// The four bytes that must be escaped inside keys and values:
///   \t  (0x09) → \x09
///   \n  (0x0A) → \x0A
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
}
