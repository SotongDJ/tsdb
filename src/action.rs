/// Action file parser and opcode enum.
///
/// Action file format (each line):
///   # comment          → ignored
///   (blank)            → ignored
///   +<uuid>\t<kv>...   Append: insert; error if UUID exists
///   -<uuid>            Delete: remove; error if UUID missing
///   ~<uuid>\t<kv>...   Patch: update KV pairs; error if UUID missing
///   !<uuid>\t<kv>...   Upsert: insert or replace; never errors

use crate::base62::validate_uuid;
use crate::error::{Result, TsdbError};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub enum Opcode {
    Append,
    Delete,
    Patch,
    Upsert,
}

/// A parsed action from an action file line.
#[derive(Debug, Clone)]
pub struct Action {
    pub opcode: Opcode,
    pub uuid: String,
    /// Key-value pairs (empty for Delete; for Patch a value of "\x00" means delete that key)
    pub fields: HashMap<String, String>,
    /// Original line number (1-based) for error reporting
    pub line_no: usize,
}

/// Parse an action file from disk, returning all actions.
pub fn parse_action_file(path: &Path) -> Result<Vec<Action>> {
    let content = fs::read_to_string(path)?;
    parse_action_str(&content)
}

/// Parse action lines from a string slice.
pub fn parse_action_str(content: &str) -> Result<Vec<Action>> {
    let mut actions = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        let line_no = idx + 1;
        let trimmed = line.trim_end_matches('\r');

        // Skip blanks and comments
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let action = parse_action_line(trimmed, line_no)?;
        actions.push(action);
    }
    Ok(actions)
}

/// Parse a single non-blank, non-comment action line.
pub fn parse_action_line(line: &str, line_no: usize) -> Result<Action> {
    let bytes = line.as_bytes();
    if bytes.is_empty() {
        return Err(TsdbError::ParseError {
            line: line_no,
            message: "empty line".to_string(),
        });
    }

    let opcode = match bytes[0] {
        b'+' => Opcode::Append,
        b'-' => Opcode::Delete,
        b'~' => Opcode::Patch,
        b'!' => Opcode::Upsert,
        other => {
            return Err(TsdbError::ParseError {
                line: line_no,
                message: format!("unknown opcode byte '{}' (0x{:02X})", other as char, other),
            });
        }
    };

    let rest = &line[1..]; // skip opcode byte

    // Extract UUID (first 12 chars after opcode)
    if rest.len() < 12 {
        return Err(TsdbError::ParseError {
            line: line_no,
            message: format!("line too short to contain UUID: {:?}", line),
        });
    }

    let uuid = &rest[..12];
    validate_uuid(uuid).map_err(|e| TsdbError::ParseError {
        line: line_no,
        message: format!("invalid UUID: {}", e),
    })?;

    let after_uuid = &rest[12..];

    // Delete has no fields
    if opcode == Opcode::Delete {
        if !after_uuid.is_empty() {
            // Allow trailing whitespace
            if !after_uuid.trim().is_empty() {
                return Err(TsdbError::ParseError {
                    line: line_no,
                    message: format!("delete line has unexpected content after UUID: {:?}", line),
                });
            }
        }
        return Ok(Action {
            opcode,
            uuid: uuid.to_string(),
            fields: HashMap::new(),
            line_no,
        });
    }

    // For Append/Patch/Upsert, expect \t then key=value pairs
    if after_uuid.is_empty() || after_uuid.as_bytes()[0] != b'\t' {
        return Err(TsdbError::ParseError {
            line: line_no,
            message: format!("expected tab after UUID in: {:?}", line),
        });
    }

    let kv_part = &after_uuid[1..]; // skip the leading tab
    let fields = parse_kv_fields(kv_part, line_no)?;

    // Append requires at least one KV field
    if opcode == Opcode::Append && fields.is_empty() {
        return Err(TsdbError::ParseError {
            line: line_no,
            message: format!("Append (+) line has no key-value fields: {:?}", line),
        });
    }

    Ok(Action {
        opcode,
        uuid: uuid.to_string(),
        fields,
        line_no,
    })
}

/// Parse tab-separated key=value pairs.
pub fn parse_kv_fields(s: &str, line_no: usize) -> Result<HashMap<String, String>> {
    let mut map = HashMap::new();
    for part in s.split('\t') {
        if part.is_empty() {
            continue;
        }
        // Find the first '=' that is NOT an escape sequence
        let eq_pos = find_unescaped_equals(part, line_no)?;
        match eq_pos {
            None => {
                return Err(TsdbError::ParseError {
                    line: line_no,
                    message: format!("field missing '=': {:?}", part),
                });
            }
            Some(pos) => {
                let raw_key = &part[..pos];
                let raw_val = &part[pos + 1..];
                let key = crate::escape::unescape(raw_key).map_err(|e| TsdbError::ParseError {
                    line: line_no,
                    message: format!("key unescape error: {}", e),
                })?;
                let val = crate::escape::unescape(raw_val).map_err(|e| TsdbError::ParseError {
                    line: line_no,
                    message: format!("value unescape error: {}", e),
                })?;
                map.insert(key, val);
            }
        }
    }
    Ok(map)
}

/// Find the position of the first literal '=' that isn't part of an `\x3D` escape.
/// Returns Err if an invalid escape sequence is encountered.
fn find_unescaped_equals(s: &str, line_no: usize) -> Result<Option<usize>> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'=' {
            return Ok(Some(i));
        }
        if bytes[i] == b'\\' {
            // skip escape sequence
            if i + 1 < bytes.len() {
                match bytes[i + 1] {
                    b'\\' => { i += 2; continue; }
                    b'x' => {
                        // Validate that the next two chars are valid hex
                        if i + 3 >= bytes.len() {
                            return Err(TsdbError::ParseError {
                                line: line_no,
                                message: format!(
                                    "incomplete \\x escape sequence at position {} in: {:?}",
                                    i, s
                                ),
                            });
                        }
                        let hi = bytes[i + 2];
                        let lo = bytes[i + 3];
                        if !hi.is_ascii_hexdigit() || !lo.is_ascii_hexdigit() {
                            return Err(TsdbError::ParseError {
                                line: line_no,
                                message: format!(
                                    "invalid escape sequence \\x{}{} at position {} in: {:?}",
                                    hi as char, lo as char, i, s
                                ),
                            });
                        }
                        i += 4;
                        continue;
                    }
                    _ => { i += 2; continue; }
                }
            }
        }
        i += 1;
    }
    Ok(None)
}

/// Collect all UUIDs referenced by a list of actions.
pub fn collect_uuids(actions: &[Action]) -> Vec<String> {
    actions.iter().map(|a| a.uuid.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_append() {
        let line = "+AGk26cH00001\tname=Alice\tage=30";
        let action = parse_action_line(line, 1).unwrap();
        assert_eq!(action.opcode, Opcode::Append);
        assert_eq!(action.uuid, "AGk26cH00001");
        assert_eq!(action.fields["name"], "Alice");
        assert_eq!(action.fields["age"], "30");
    }

    #[test]
    fn test_parse_delete() {
        let line = "-AGk26cH00001";
        let action = parse_action_line(line, 1).unwrap();
        assert_eq!(action.opcode, Opcode::Delete);
        assert_eq!(action.uuid, "AGk26cH00001");
        assert!(action.fields.is_empty());
    }

    #[test]
    fn test_parse_patch() {
        let line = "~AGk26cH00001\tname=Bob";
        let action = parse_action_line(line, 1).unwrap();
        assert_eq!(action.opcode, Opcode::Patch);
        assert_eq!(action.fields["name"], "Bob");
    }

    #[test]
    fn test_parse_upsert() {
        let line = "!AGk26cH00001\tx=1";
        let action = parse_action_line(line, 1).unwrap();
        assert_eq!(action.opcode, Opcode::Upsert);
    }

    #[test]
    fn test_parse_comment_and_blank() {
        let content = "# this is a comment\n\n+AGk26cH00001\tname=Alice\n";
        let actions = parse_action_str(content).unwrap();
        assert_eq!(actions.len(), 1);
    }

    #[test]
    fn test_parse_invalid_opcode() {
        let line = "?AGk26cH00001\tname=Alice";
        assert!(parse_action_line(line, 1).is_err());
    }

    #[test]
    fn test_parse_escaped_value() {
        let line = "+AGk26cH00001\ttext=hello\\x09world";
        let action = parse_action_line(line, 1).unwrap();
        assert_eq!(action.fields["text"], "hello\tworld");
    }

    #[test]
    fn test_collect_uuids() {
        let content = "+AGk26cH00001\tname=Alice\n+AGk26cH00002\tname=Bob\n";
        let actions = parse_action_str(content).unwrap();
        let uuids = collect_uuids(&actions);
        assert_eq!(uuids.len(), 2);
    }

    #[test]
    fn test_append_zero_kv_fields_error() {
        // Append with tab but empty fields should fail
        let line = "+AGk26cH00001\t";
        assert!(parse_action_line(line, 1).is_err());
    }

    #[test]
    fn test_find_unescaped_equals_invalid_hex() {
        // \xZZ should be an error
        let result = find_unescaped_equals("key\\xZZval=foo", 1);
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("invalid escape sequence"), "got: {}", msg);
    }
}
