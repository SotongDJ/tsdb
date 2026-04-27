/// Base62-Gu UUID encoding/decoding and validation.
///
/// Format-G 60-char alphabet (standard base62 minus ambiguous `l` and `O`):
///   0123456789abcdefghijkmnopqrstuvwxyzABCDEFGHIJKLMNPQRSTUVWXYZ
///
/// UUID structure (12 chars):
///   {C}{G}{century}{YY}{M}{D}{h}{m}{s}{XX}
///    1   1    1      2   1  1  1  1  1   2   = 12 chars
///
/// C       = uppercase A-Z class prefix
/// G       = literal 'G' format marker
/// century = year/100 encoded via Format-G alphabet
/// YY      = year%100 as 2-digit decimal string ("00".."99")
/// M       = month via MONTH_TABLE
/// D       = day via DAY_TABLE
/// h       = hour via HOUR_TABLE
/// m       = minute via Format-G alphabet (0-59)
/// s       = second via Format-G alphabet (0-59)
/// XX      = 2-char order suffix (alphanumeric, starts at "01")
use crate::error::{Result, TsdbError};

/// Format-G 60-char alphabet.
pub(crate) const FORMAT_G: &[u8; 60] =
    b"0123456789abcdefghijkmnopqrstuvwxyzABCDEFGHIJKLMNPQRSTUVWXYZ";

/// Month encoding table: index 0 = unused, 1..=12 → char
pub(crate) const MONTH_TABLE: &[u8; 13] = b"\x00abcdefABCDEF";

/// Day encoding table: index 0 = unused, 1..=31 → char
/// 1→'0', 2→'1', ..., 10→'9', 11→'a', ..., 21→'k', 22→'A', ..., 31→'J'
pub(crate) const DAY_TABLE: &[u8; 32] = b"\x000123456789abcdefghijkABCDEFGHIJ";

/// Hour encoding table: index 0..=23 → char
/// 0→'0', 1→'a', 2→'b', ..., 12→'l', 13→'A', ..., 23→'K'
/// Note: 'l' IS valid in hour table; excluded only from the 60-char alphabet.
pub(crate) const HOUR_TABLE: &[u8; 24] = b"0abcdefghijklABCDEFGHIJK";

/// Decode a Format-G character to its index (0-59), or None if invalid.
pub(crate) fn fg_decode(ch: u8) -> Option<u8> {
    FORMAT_G.iter().position(|&b| b == ch).map(|i| i as u8)
}

/// Validate that a 12-byte slice is a well-formed Base62-Gu UUID.
///
/// Rules:
/// - byte 0 is uppercase A-Z
/// - byte 1 is literal 'G'
/// - byte 2 (century) is in Format-G alphabet
/// - bytes 3-4 (YY) are ASCII decimal digits
/// - byte 5 (month) is in MONTH_TABLE chars
/// - byte 6 (day) is in DAY_TABLE chars
/// - byte 7 (hour) is in HOUR_TABLE chars
/// - byte 8 (minute) is in Format-G alphabet
/// - byte 9 (second) is in Format-G alphabet
/// - bytes 10-11 (order) are alphanumeric
pub fn validate_uuid(s: &str) -> Result<()> {
    let b = s.as_bytes();
    if b.len() != 12 {
        return Err(TsdbError::InvalidUuid(format!(
            "UUID must be 12 chars, got {}: {:?}",
            b.len(),
            s
        )));
    }

    // byte 0: uppercase A-Z class
    if !b[0].is_ascii_uppercase() {
        return Err(TsdbError::InvalidUuid(format!(
            "UUID byte 0 must be uppercase letter, got '{}': {:?}",
            b[0] as char, s
        )));
    }

    // byte 1: literal 'G'
    if b[1] != b'G' {
        return Err(TsdbError::InvalidUuid(format!(
            "UUID byte 1 must be 'G', got '{}': {:?}",
            b[1] as char, s
        )));
    }

    // byte 2: century in Format-G
    if fg_decode(b[2]).is_none() {
        return Err(TsdbError::InvalidUuid(format!(
            "UUID byte 2 (century) not in Format-G alphabet: {:?}",
            s
        )));
    }

    // bytes 3-4: YY decimal digits
    if !b[3].is_ascii_digit() || !b[4].is_ascii_digit() {
        return Err(TsdbError::InvalidUuid(format!(
            "UUID bytes 3-4 (YY) must be decimal digits: {:?}",
            s
        )));
    }

    // byte 5: month
    let valid_months: &[u8] = &MONTH_TABLE[1..];
    if !valid_months.contains(&b[5]) {
        return Err(TsdbError::InvalidUuid(format!(
            "UUID byte 5 (month) invalid '{}': {:?}",
            b[5] as char, s
        )));
    }

    // byte 6: day
    let valid_days: &[u8] = &DAY_TABLE[1..];
    if !valid_days.contains(&b[6]) {
        return Err(TsdbError::InvalidUuid(format!(
            "UUID byte 6 (day) invalid '{}': {:?}",
            b[6] as char, s
        )));
    }

    // byte 7: hour
    if !HOUR_TABLE.contains(&b[7]) {
        return Err(TsdbError::InvalidUuid(format!(
            "UUID byte 7 (hour) invalid '{}': {:?}",
            b[7] as char, s
        )));
    }

    // byte 8: minute in Format-G
    if fg_decode(b[8]).is_none() {
        return Err(TsdbError::InvalidUuid(format!(
            "UUID byte 8 (minute) not in Format-G alphabet: {:?}",
            s
        )));
    }

    // byte 9: second in Format-G
    if fg_decode(b[9]).is_none() {
        return Err(TsdbError::InvalidUuid(format!(
            "UUID byte 9 (second) not in Format-G alphabet: {:?}",
            s
        )));
    }

    // bytes 10-11: order suffix alphanumeric
    if !b[10].is_ascii_alphanumeric() || !b[11].is_ascii_alphanumeric() {
        return Err(TsdbError::InvalidUuid(format!(
            "UUID bytes 10-11 (order) must be alphanumeric: {:?}",
            s
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fg_decode_roundtrip() {
        for i in 0u8..60 {
            let ch = FORMAT_G[i as usize];
            assert_eq!(fg_decode(ch), Some(i));
        }
    }

    #[test]
    fn test_validate_uuid_good() {
        // C=A, G=G, cent=k(20), YY=26, M=c(3), D=H(29), h=0(0), m=0(0), s=0(0), XX=01
        // = A G k 2 6 c H 0 0 0 0 1  = 12 chars
        let uuid = "AGk26cH00001";
        assert_eq!(uuid.len(), 12);
        assert!(validate_uuid(uuid).is_ok(), "Should be valid: {}", uuid);
    }

    #[test]
    fn test_validate_uuid_bad_length() {
        assert!(validate_uuid("short").is_err());
        assert!(validate_uuid("toolongstring123").is_err());
    }

    #[test]
    fn test_validate_uuid_bad_class() {
        // byte 0 must be A-Z uppercase
        let uuid = "aGk26cH00001";
        assert!(validate_uuid(uuid).is_err());
    }

    #[test]
    fn test_validate_uuid_bad_marker() {
        // byte 1 must be 'G'
        let uuid = "AXk26cH00001";
        assert!(validate_uuid(uuid).is_err());
    }

    #[test]
    fn test_validate_uuid_bad_month() {
        // byte 5 (month) must be valid; 'z' is not in month table
        let uuid = "AGk26zH00001";
        assert!(validate_uuid(uuid).is_err());
    }

    #[test]
    fn test_fg_decode_invalid() {
        assert_eq!(fg_decode(b'l'), None); // 'l' excluded from Format-G
        assert_eq!(fg_decode(b'O'), None); // 'O' excluded from Format-G
    }
}
