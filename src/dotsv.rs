/// DOTSV file parsing, binary search, record operations, and compaction.
///
/// File structure:
///   <sorted section lines>\n
///   \n                          ← single blank line separator
///   <pending section lines>\n
///
/// Sorted section: UUID-ordered DOTSV records (binary searchable by UUID prefix).
/// Pending section: opcode-prefixed lines (+/-/~) acting as a write-ahead log.
///
/// When pending section exceeds 100 lines → compact (merge pass → tmp → rename).

use crate::action::{parse_kv_fields, Action, Opcode};
use crate::error::{Result, TsdbError};
use crate::escape::escape;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;

pub const PENDING_COMPACT_THRESHOLD: usize = 100;

/// A fully parsed DOTSV record.
#[derive(Debug, Clone)]
pub struct Record {
    pub uuid: String,
    pub fields: HashMap<String, String>,
}

impl Record {
    /// Serialize to a DOTSV line (without trailing newline).
    pub fn serialize(&self) -> String {
        let mut parts = vec![self.uuid.clone()];
        // Sort fields for deterministic output (git-traceable)
        let mut keys: Vec<&String> = self.fields.keys().collect();
        keys.sort();
        for k in keys {
            let v = &self.fields[k];
            parts.push(format!("{}={}", escape(k), escape(v)));
        }
        parts.join("\t")
    }

    /// Parse a DOTSV record line (no opcode prefix).
    /// Trailing spaces (in-place patch padding) are stripped only from the last
    /// tab-separated field to avoid silently removing spaces from field values.
    pub fn parse(line: &str, line_no: usize) -> Result<Self> {
        let mut parts = line.splitn(2, '\t');
        let uuid = parts.next().unwrap_or("").to_string();
        crate::base62::validate_uuid(&uuid).map_err(|e| TsdbError::ParseError {
            line: line_no,
            message: format!("{}", e),
        })?;
        let kv_part = parts.next().unwrap_or("");
        // Strip in-place padding spaces from the tail of the last field only.
        // Split the kv_part on tabs, trim the trailing spaces off the last segment,
        // then reassemble before passing to parse_kv_fields.
        let kv_part_trimmed: String = if kv_part.is_empty() {
            String::new()
        } else {
            let mut segs: Vec<&str> = kv_part.split('\t').collect();
            if let Some(last) = segs.last_mut() {
                *last = last.trim_end_matches(' ');
            }
            segs.join("\t")
        };
        let fields = if kv_part_trimmed.is_empty() {
            HashMap::new()
        } else {
            parse_kv_fields(&kv_part_trimmed, line_no)?
        };
        Ok(Record { uuid, fields })
    }

    /// Apply a patch action, updating fields in place.
    /// Fields with value "\x00" (null byte) are deleted.
    /// Returns an error if the patch would leave the record with no fields.
    pub fn apply_patch(&mut self, fields: &HashMap<String, String>) -> Result<()> {
        for (k, v) in fields {
            if v == "\x00" {
                self.fields.remove(k);
            } else {
                self.fields.insert(k.clone(), v.clone());
            }
        }
        if self.fields.is_empty() {
            return Err(TsdbError::Other(format!(
                "patch would remove all fields from record {}",
                self.uuid
            )));
        }
        Ok(())
    }
}

/// In-memory representation of a DOTSV file.
pub struct DotsvFile {
    /// Lines in sorted section (without newlines).
    pub sorted: Vec<String>,
    /// Lines in pending section (without newlines), include opcode prefix.
    pub pending: Vec<String>,
}

impl DotsvFile {
    /// Create an empty DOTSV file structure.
    pub fn empty() -> Self {
        DotsvFile {
            sorted: Vec::new(),
            pending: Vec::new(),
        }
    }

    /// Parse a DOTSV file from raw bytes.
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        let text = std::str::from_utf8(data)
            .map_err(|e| TsdbError::Other(format!("UTF-8 decode error: {}", e)))?;
        Self::parse_str(text)
    }

    /// Parse from a string.
    pub fn parse_str(text: &str) -> Result<Self> {
        let mut sorted = Vec::new();
        let mut pending = Vec::new();
        let mut in_pending = false;
        let mut line_no = 0usize;

        for line in text.lines() {
            line_no += 1;
            let line = line.trim_end_matches('\r');
            if !in_pending && line.is_empty() {
                in_pending = true;
                continue;
            }
            if in_pending {
                if !line.is_empty() {
                    pending.push(line.to_string());
                }
            } else {
                // Validate: sorted lines must be at least 13 bytes and have a valid UUID prefix
                if line.len() < 13 {
                    return Err(TsdbError::ParseError {
                        line: line_no,
                        message: format!(
                            "sorted section line too short (need >=13 bytes): {:?}",
                            line
                        ),
                    });
                }
                let uuid_str = &line[..12];
                crate::base62::validate_uuid(uuid_str).map_err(|e| TsdbError::ParseError {
                    line: line_no,
                    message: format!("invalid UUID in sorted section: {}", e),
                })?;
                sorted.push(line.to_string());
            }
        }

        Ok(DotsvFile { sorted, pending })
    }

    /// Load from a file path, returning empty if the file doesn't exist.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::empty());
        }
        let data = fs::read(path)?;
        Self::from_bytes(&data)
    }

    /// Write to a file (creates or truncates).
    pub fn write_to(&self, path: &Path) -> Result<()> {
        let file = File::create(path)?;
        let mut w = BufWriter::new(file);
        for line in &self.sorted {
            w.write_all(line.as_bytes())?;
            w.write_all(b"\n")?;
        }
        // Only write separator and pending section when pending is non-empty
        if !self.pending.is_empty() {
            w.write_all(b"\n")?;
            for line in &self.pending {
                w.write_all(line.as_bytes())?;
                w.write_all(b"\n")?;
            }
        }
        w.flush()?;
        Ok(())
    }

    /// Binary search the sorted section for a UUID.
    /// Returns `Ok(idx)` if found, `Err(idx)` with insert position if not.
    pub fn binary_search_uuid(&self, uuid: &str) -> std::result::Result<usize, usize> {
        let mut lo = 0usize;
        let mut hi = self.sorted.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let line_uuid = line_uuid(&self.sorted[mid]);
            match line_uuid.cmp(uuid) {
                std::cmp::Ordering::Equal => return Ok(mid),
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
            }
        }
        Err(lo)
    }

    /// Scan pending section for a UUID. Returns index if found.
    pub fn find_in_pending(&self, uuid: &str) -> Option<usize> {
        for (i, line) in self.pending.iter().enumerate() {
            if line.len() >= 13 && &line[1..13] == uuid {
                return Some(i);
            }
        }
        None
    }

    /// Check if a UUID exists anywhere (sorted or pending, accounting for deletes).
    pub fn uuid_exists(&self, uuid: &str) -> bool {
        // Check pending section first (most recent wins, but deletes matter too)
        // Scan pending in reverse order — last mention wins
        let mut found_in_pending = None;
        for line in self.pending.iter().rev() {
            if line.len() >= 13 && &line[1..13] == uuid {
                found_in_pending = Some(line.as_bytes()[0]);
                break;
            }
        }
        match found_in_pending {
            Some(b'-') => return false, // deleted in pending
            Some(_) => return true,     // added/patched in pending
            None => {}
        }
        // Check sorted section
        self.binary_search_uuid(uuid).is_ok()
    }

    /// Returns the number of pending lines.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Compact: merge sorted + pending into a new sorted section, clear pending.
    /// O(n) merge pass.
    pub fn compact(&mut self) -> Result<()> {
        // Parse all sorted records
        let mut records: HashMap<String, Option<Record>> = HashMap::new();
        let mut order: Vec<String> = Vec::new();

        for (i, line) in self.sorted.iter().enumerate() {
            if line.is_empty() {
                continue;
            }
            let rec = Record::parse(line, i + 1)?;
            order.push(rec.uuid.clone());
            records.insert(rec.uuid.clone(), Some(rec));
        }

        // Apply pending ops in order
        for (i, line) in self.pending.iter().enumerate() {
            if line.is_empty() {
                continue;
            }
            let bytes = line.as_bytes();
            let op = bytes[0];
            let rest = &line[1..];
            if rest.len() < 12 {
                return Err(TsdbError::ParseError {
                    line: i + 1,
                    message: format!("pending line too short: {:?}", line),
                });
            }
            let uuid = &rest[..12];
            match op {
                b'+' => {
                    let rec = Record::parse(rest, i + 1)?;
                    if !records.contains_key(uuid) {
                        order.push(uuid.to_string());
                    }
                    records.insert(uuid.to_string(), Some(rec));
                }
                b'-' => {
                    records.insert(uuid.to_string(), None);
                }
                b'~' => {
                    let fields = if rest.len() > 12 && rest.as_bytes()[12] == b'\t' {
                        parse_kv_fields(&rest[13..], i + 1)?
                    } else {
                        HashMap::new()
                    };
                    if let Some(entry) = records.get_mut(uuid) {
                        if let Some(rec) = entry {
                            rec.apply_patch(&fields)?;
                        }
                    }
                    // If not in records yet (shouldn't happen), ignore
                }
                _ => {
                    return Err(TsdbError::ParseError {
                        line: i + 1,
                        message: format!("unknown pending opcode: {:?}", line),
                    });
                }
            }
        }

        // Build new sorted section: keep existing order, then sort
        // Use all UUIDs that have a record (not deleted), sort by UUID
        let mut new_uuids: Vec<String> = order
            .into_iter()
            .filter(|u| records.get(u).and_then(|r| r.as_ref()).is_some())
            .collect();
        // Remove duplicates (keep first occurrence order, they're already deduplicated by the map)
        new_uuids.dedup();
        new_uuids.sort();

        let mut new_sorted = Vec::new();
        for uuid in &new_uuids {
            if let Some(Some(rec)) = records.get(uuid) {
                new_sorted.push(rec.serialize());
            }
        }

        self.sorted = new_sorted;
        self.pending = Vec::new();
        Ok(())
    }
}

/// Extract the UUID prefix (first 12 chars) from a sorted or pending line.
pub fn line_uuid(line: &str) -> &str {
    let start = if line.starts_with(|c: char| matches!(c, '+' | '-' | '~' | '!')) {
        1
    } else {
        0
    };
    let end = (start + 12).min(line.len());
    &line[start..end]
}

/// Apply a list of validated actions to a DotsvFile in memory.
/// Returns Err on the first conflict (strict mode).
pub fn apply_actions(db: &mut DotsvFile, actions: &[Action]) -> Result<()> {
    for action in actions {
        apply_single_action(db, action)?;
    }
    Ok(())
}

fn apply_single_action(db: &mut DotsvFile, action: &Action) -> Result<()> {
    let uuid = &action.uuid;
    match action.opcode {
        Opcode::Append => {
            if db.uuid_exists(uuid) {
                return Err(TsdbError::DuplicateUuid(uuid.clone()));
            }
            // Serialize and append to pending
            let rec = Record {
                uuid: uuid.clone(),
                fields: action.fields.clone(),
            };
            db.pending.push(format!("+{}", rec.serialize()));
        }
        Opcode::Delete => {
            if !db.uuid_exists(uuid) {
                return Err(TsdbError::MissingUuid(uuid.clone()));
            }
            db.pending.push(format!("-{}", uuid));
        }
        Opcode::Patch => {
            if !db.uuid_exists(uuid) {
                return Err(TsdbError::MissingUuid(uuid.clone()));
            }
            // For patch: attempt in-place modify of sorted section if possible
            match db.binary_search_uuid(uuid) {
                Ok(idx) => {
                    let old_line = &db.sorted[idx];
                    // Parse, apply patch, re-serialize
                    let mut rec = Record::parse(old_line, 0)?;
                    rec.apply_patch(&action.fields)?;
                    let new_serialized = rec.serialize();
                    if new_serialized.len() <= old_line.len() {
                        // In-place overwrite: pad with spaces before newline
                        let padded = format!(
                            "{:width$}",
                            new_serialized,
                            width = old_line.len()
                        );
                        db.sorted[idx] = padded;
                    } else {
                        // Append to pending
                        let mut kv_parts: Vec<String> = Vec::new();
                        let mut keys: Vec<&String> = action.fields.keys().collect();
                        keys.sort();
                        for k in keys {
                            let v = &action.fields[k];
                            kv_parts.push(format!("{}={}", escape(k), escape(v)));
                        }
                        let fields_str = kv_parts.join("\t");
                        db.pending.push(format!("~{}\t{}", uuid, fields_str));
                    }
                }
                Err(_) => {
                    // UUID is in pending section; append a patch to pending
                    let mut kv_parts: Vec<String> = Vec::new();
                    let mut keys: Vec<&String> = action.fields.keys().collect();
                    keys.sort();
                    for k in keys {
                        let v = &action.fields[k];
                        kv_parts.push(format!("{}={}", escape(k), escape(v)));
                    }
                    let fields_str = kv_parts.join("\t");
                    db.pending.push(format!("~{}\t{}", uuid, fields_str));
                }
            }
        }
        Opcode::Upsert => {
            let rec = Record {
                uuid: uuid.clone(),
                fields: action.fields.clone(),
            };
            // If exists in sorted section, try in-place replace
            match db.binary_search_uuid(uuid) {
                Ok(idx) => {
                    let old_line = &db.sorted[idx];
                    let new_serialized = rec.serialize();
                    if new_serialized.len() <= old_line.len() {
                        let padded = format!(
                            "{:width$}",
                            new_serialized,
                            width = old_line.len()
                        );
                        db.sorted[idx] = padded;
                    } else {
                        db.pending.push(format!("+{}", rec.serialize()));
                    }
                }
                Err(_) => {
                    // Append upsert to pending (handles both insert and replace cases)
                    db.pending.push(format!("+{}", rec.serialize()));
                }
            }
        }
    }
    Ok(())
}

/// Validate all actions before applying (pre-check pass).
/// Returns Err on first detected conflict.
pub fn validate_actions(db: &DotsvFile, actions: &[Action]) -> Result<()> {
    // We need to simulate the state changes to validate correctly
    // Track changes from actions processed so far
    let mut added: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut deleted: std::collections::HashSet<String> = std::collections::HashSet::new();

    for action in actions {
        let uuid = &action.uuid;
        let currently_exists = (db.uuid_exists(uuid) || added.contains(uuid))
            && !deleted.contains(uuid);

        match action.opcode {
            Opcode::Append => {
                if currently_exists {
                    return Err(TsdbError::DuplicateUuid(uuid.clone()));
                }
                added.insert(uuid.clone());
            }
            Opcode::Delete => {
                if !currently_exists {
                    return Err(TsdbError::MissingUuid(uuid.clone()));
                }
                deleted.insert(uuid.clone());
                added.remove(uuid);
            }
            Opcode::Patch => {
                if !currently_exists {
                    return Err(TsdbError::MissingUuid(uuid.clone()));
                }
            }
            Opcode::Upsert => {
                // Always succeeds
                if !currently_exists {
                    added.insert(uuid.clone());
                }
            }
        }
    }
    Ok(())
}

/// Perform compaction if pending section exceeds the threshold.
pub fn maybe_compact(db: &mut DotsvFile) -> Result<bool> {
    if db.pending_count() >= PENDING_COMPACT_THRESHOLD {
        db.compact()?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Atomically write db to target path (write to .tmp then rename).
pub fn atomic_write(db: &DotsvFile, target: &Path) -> Result<()> {
    let tmp_path = target.with_extension("dov.tmp");
    db.write_to(&tmp_path)?;
    fs::rename(&tmp_path, target)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::parse_action_str;

    const GOOD_UUID1: &str = "AGk26cH00001";
    const GOOD_UUID2: &str = "AGk26cH00002";
    const GOOD_UUID3: &str = "AGk26cH00003";

    fn make_db_with(records: &[(&str, &[(&str, &str)])]) -> DotsvFile {
        let mut sorted = Vec::new();
        for (uuid, fields) in records {
            let rec = Record {
                uuid: uuid.to_string(),
                fields: fields
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            };
            sorted.push(rec.serialize());
        }
        sorted.sort(); // keep sorted
        DotsvFile {
            sorted,
            pending: Vec::new(),
        }
    }

    #[test]
    fn test_binary_search_found() {
        let db = make_db_with(&[(GOOD_UUID1, &[("name", "Alice")])]);
        assert!(db.binary_search_uuid(GOOD_UUID1).is_ok());
    }

    #[test]
    fn test_binary_search_not_found() {
        let db = make_db_with(&[(GOOD_UUID1, &[("name", "Alice")])]);
        assert!(db.binary_search_uuid(GOOD_UUID2).is_err());
    }

    #[test]
    fn test_uuid_exists_in_sorted() {
        let db = make_db_with(&[(GOOD_UUID1, &[("name", "Alice")])]);
        assert!(db.uuid_exists(GOOD_UUID1));
        assert!(!db.uuid_exists(GOOD_UUID2));
    }

    #[test]
    fn test_append_then_exists() {
        let mut db = DotsvFile::empty();
        let actions =
            parse_action_str(&format!("+{}\tname=Alice\n", GOOD_UUID1)).unwrap();
        apply_actions(&mut db, &actions).unwrap();
        assert!(db.uuid_exists(GOOD_UUID1));
    }

    #[test]
    fn test_append_duplicate_error() {
        let mut db = make_db_with(&[(GOOD_UUID1, &[("name", "Alice")])]);
        let actions =
            parse_action_str(&format!("+{}\tname=Bob\n", GOOD_UUID1)).unwrap();
        assert!(apply_actions(&mut db, &actions).is_err());
    }

    #[test]
    fn test_delete_removes() {
        let mut db = make_db_with(&[(GOOD_UUID1, &[("name", "Alice")])]);
        let actions = parse_action_str(&format!("-{}\n", GOOD_UUID1)).unwrap();
        apply_actions(&mut db, &actions).unwrap();
        assert!(!db.uuid_exists(GOOD_UUID1));
    }

    #[test]
    fn test_delete_missing_error() {
        let mut db = DotsvFile::empty();
        let actions = parse_action_str(&format!("-{}\n", GOOD_UUID1)).unwrap();
        assert!(apply_actions(&mut db, &actions).is_err());
    }

    #[test]
    fn test_patch_updates_field() {
        let mut db = make_db_with(&[(GOOD_UUID1, &[("name", "Alice"), ("age", "30")])]);
        let actions =
            parse_action_str(&format!("~{}\tname=Bob\n", GOOD_UUID1)).unwrap();
        apply_actions(&mut db, &actions).unwrap();
        // Compact to check final state
        db.compact().unwrap();
        let rec = Record::parse(&db.sorted[0], 1).unwrap();
        assert_eq!(rec.fields["name"], "Bob");
        assert_eq!(rec.fields["age"], "30");
    }

    #[test]
    fn test_patch_delete_key() {
        let mut db = make_db_with(&[(GOOD_UUID1, &[("name", "Alice"), ("tmp", "x")])]);
        let actions =
            parse_action_str(&format!("~{}\ttmp=\\x00\n", GOOD_UUID1)).unwrap();
        apply_actions(&mut db, &actions).unwrap();
        db.compact().unwrap();
        let rec = Record::parse(&db.sorted[0], 1).unwrap();
        assert!(!rec.fields.contains_key("tmp"));
    }

    #[test]
    fn test_upsert_insert() {
        let mut db = DotsvFile::empty();
        let actions =
            parse_action_str(&format!("!{}\tname=Alice\n", GOOD_UUID1)).unwrap();
        apply_actions(&mut db, &actions).unwrap();
        assert!(db.uuid_exists(GOOD_UUID1));
    }

    #[test]
    fn test_compact_clears_pending() {
        let mut db = DotsvFile::empty();
        let actions =
            parse_action_str(&format!("+{}\tname=Alice\n", GOOD_UUID1)).unwrap();
        apply_actions(&mut db, &actions).unwrap();
        assert!(!db.pending.is_empty());
        db.compact().unwrap();
        assert!(db.pending.is_empty());
        assert!(!db.sorted.is_empty());
    }

    #[test]
    fn test_serialize_record() {
        let rec = Record {
            uuid: GOOD_UUID1.to_string(),
            fields: [("name".to_string(), "Alice".to_string())]
                .into_iter()
                .collect(),
        };
        let s = rec.serialize();
        assert!(s.starts_with(GOOD_UUID1));
        assert!(s.contains("name=Alice"));
    }

    #[test]
    fn test_from_str_round_trip() {
        let mut db = DotsvFile::empty();
        let actions = parse_action_str(&format!(
            "+{}\tname=Alice\n+{}\tname=Bob\n",
            GOOD_UUID1, GOOD_UUID2
        ))
        .unwrap();
        apply_actions(&mut db, &actions).unwrap();
        db.compact().unwrap();

        // Serialize to string
        let mut buf = Vec::new();
        for line in &db.sorted {
            buf.extend_from_slice(line.as_bytes());
            buf.push(b'\n');
        }
        buf.push(b'\n'); // separator
        let text = String::from_utf8(buf).unwrap();

        let db2 = DotsvFile::parse_str(&text).unwrap();
        assert_eq!(db2.sorted.len(), 2);
        assert!(db2.uuid_exists(GOOD_UUID1));
        assert!(db2.uuid_exists(GOOD_UUID2));
    }

    #[test]
    fn test_patch_all_fields_to_null_returns_error() {
        // A record with a single field patched to \x00 should fail because
        // the result would have no fields, violating the invariant that every
        // record must have at least one KV pair.
        let mut db = make_db_with(&[(GOOD_UUID1, &[("name", "Alice")])]);
        let actions =
            parse_action_str(&format!("~{}\tname=\\x00\n", GOOD_UUID1)).unwrap();
        let result = apply_actions(&mut db, &actions);
        assert!(
            result.is_err(),
            "expected error when patch removes all fields, got Ok"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("patch would remove all fields"),
            "unexpected error message: {}",
            msg
        );
    }
}
