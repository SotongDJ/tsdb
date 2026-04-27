/// `--records` mode (v0.6, Round-2 Req B): consume a `.utv` UUID list,
/// emit one JSONL object per record on stdout.
///
/// Per-line shape:
///   `{"_uuid":"<u>","<k1>":<v1>,"<k2>":<v2>,...}\n`
///
/// `_uuid` is always the first member. Remaining keys appear in raw-key
/// (unescaped) lex order — identical to `Record::serialize`'s sort
/// semantics (Pie finding 4). Missing UUIDs emit a sentinel
/// `{"_uuid":"<u>","_missing":true}` line in input position.
///
/// Value typing is shared with `--plane`'s `*.kt.ptv` writer through the
/// single `keytype::classify` source-of-truth (Risk #1 / Decision #17):
///
///   array     → JSON array of JSON strings (decoded via `decode_array`)
///   number    → JSON number, emitted verbatim (no f64 round-trip)
///   boolean   → JSON literal `true` / `false`
///   timestamp → JSON string (lexical form preserved)
///   string    → JSON string
///
/// JSON string escaping: RFC 8259 §7. Forward slash is intentionally NOT
/// escaped. Multi-byte UTF-8 (≥ 0x80) passes through byte-for-byte; no
/// `\u` surrogate-pair rewriting.
use crate::base62::validate_uuid;
use crate::dotsv::{DotsvFile, Record};
use crate::error::{Result, TsdbError};
use crate::escape::decode_array;
use crate::keytype::{classify, KtType};
use crate::lock::LockManager;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

// ---------------------------------------------------------------------
// .utv parser
// ---------------------------------------------------------------------

/// Parse a UUID list from a `BufRead` source.
///
/// Grammar (Banana §3.2):
///   - non-blank, non-comment line MUST be exactly 12 valid base62-Gu chars
///   - blank lines and `#…` comments are skipped
///   - BOM at file start is rejected (Pie finding 8 — only checked on the
///     first non-blank, non-comment line; later BOMs would already fail
///     the length check)
///   - CRLF line endings are rejected with a clear error message (Pie
///     finding 5 — choice (a) from the two options)
///   - leading/trailing whitespace on a UUID line is a parse error
///
/// Input order is preserved; duplicates are kept (one output per
/// occurrence).
///
/// CRLF detection: `BufRead::lines()` strips both `\n` and `\r\n`, so
/// the CRLF presence cannot be observed from the iterator output. We
/// instead read the entire input into a buffer and split on `\n` only,
/// then check each line for a trailing `\r`. This keeps the CRLF
/// rejection precise (option (a) from Pie finding 5).
pub fn parse_uuid_lines<R: BufRead>(mut reader: R) -> Result<Vec<String>> {
    let mut buf = String::new();
    reader.read_to_string(&mut buf)?;

    let mut out = Vec::new();
    let mut first_real_line = true;
    // Split on '\n' only so we can observe a trailing '\r' from CRLF
    // files.  The last "line" after a trailing newline will be empty
    // and is silently skipped by the blank-line check below.
    for (idx, raw) in buf.split('\n').enumerate() {
        let line_no = idx + 1;

        // Reject CRLF line endings explicitly.
        if raw.ends_with('\r') {
            return Err(TsdbError::ParseError {
                line: line_no,
                message: "CRLF line endings not supported; convert with dos2unix".to_string(),
            });
        }

        let line = raw;

        // Skip blanks and comments.
        if line.is_empty() {
            continue;
        }
        if line.starts_with('#') {
            continue;
        }

        // Pie finding 8: BOM check only applies to the first non-blank,
        // non-comment line; the length check is the actual gatekeeper
        // for later lines.
        if first_real_line && line.starts_with('\u{FEFF}') {
            return Err(TsdbError::ParseError {
                line: line_no,
                message: "BOM at file start rejected; .utv must be UTF-8 without BOM".to_string(),
            });
        }
        first_real_line = false;

        // Length check is the gatekeeper.
        if line.len() != 12 {
            return Err(TsdbError::ParseError {
                line: line_no,
                message: format!(
                    "expected 12-char base62-Gu UUID, got {} bytes: {:?}",
                    line.len(),
                    line
                ),
            });
        }

        // Reject leading/trailing whitespace explicitly.
        if line != line.trim() {
            return Err(TsdbError::ParseError {
                line: line_no,
                message: format!("UUID line has leading/trailing whitespace: {:?}", line),
            });
        }

        // Validate the UUID structure.
        validate_uuid(line).map_err(|e| TsdbError::ParseError {
            line: line_no,
            message: format!("{}", e),
        })?;
        out.push(line.to_string());
    }
    Ok(out)
}

/// Open a `.utv` source: a path on disk, or `-` for stdin.
pub fn parse_uuid_input(input: &str) -> Result<Vec<String>> {
    if input == "-" {
        let stdin = std::io::stdin();
        let lock = stdin.lock();
        parse_uuid_lines(lock)
    } else {
        let path = Path::new(input);
        if !path.exists() {
            return Err(TsdbError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("UUID input file not found: {}", path.display()),
            )));
        }
        let f = std::fs::File::open(path)?;
        parse_uuid_lines(BufReader::new(f))
    }
}

// ---------------------------------------------------------------------
// JSON encoder primitives
// ---------------------------------------------------------------------

/// RFC 8259 §7 JSON string escape. The byte `/` is intentionally NOT
/// escaped (RFC makes it optional and modern consumers don't need it).
/// Multi-byte UTF-8 (≥ 0x80) passes through unchanged byte-for-byte.
pub fn write_json_string<W: Write>(out: &mut W, s: &str) -> std::io::Result<()> {
    out.write_all(b"\"")?;
    for &byte in s.as_bytes() {
        match byte {
            0x22 => out.write_all(b"\\\"")?,
            0x5C => out.write_all(b"\\\\")?,
            0x08 => out.write_all(b"\\b")?,
            0x0C => out.write_all(b"\\f")?,
            0x0A => out.write_all(b"\\n")?,
            0x0D => out.write_all(b"\\r")?,
            0x09 => out.write_all(b"\\t")?,
            b if b < 0x20 => write!(out, "\\u00{:02x}", b)?,
            _ => out.write_all(&[byte])?,
        }
    }
    out.write_all(b"\"")?;
    Ok(())
}

/// Write a JSON array `[…]` of JSON strings.
pub fn write_json_array<W: Write>(out: &mut W, elements: &[String]) -> std::io::Result<()> {
    out.write_all(b"[")?;
    for (i, e) in elements.iter().enumerate() {
        if i > 0 {
            out.write_all(b",")?;
        }
        write_json_string(out, e)?;
    }
    out.write_all(b"]")?;
    Ok(())
}

/// Encode a single (already-unescaped) value into JSON per the classifier
/// dispatch. Returns a TsdbError if the value is a malformed canonical
/// array (decoder failure).
pub fn encode_value<W: Write>(out: &mut W, raw_value: &str) -> Result<()> {
    match classify(raw_value) {
        KtType::Array => {
            let elements = decode_array(raw_value)?;
            write_json_array(out, &elements)?;
        }
        KtType::Number => {
            // Shape pre-checked by classify(); emit verbatim
            // (no f64 round-trip — `30.50` stays `30.50`).
            out.write_all(raw_value.as_bytes())?;
        }
        KtType::Boolean => {
            // Exactly "true" or "false" — emit verbatim as JSON literal.
            out.write_all(raw_value.as_bytes())?;
        }
        KtType::Timestamp => {
            // Logical type: 14-digit lexical form preserved as a JSON
            // string so consumers parse it explicitly.
            write_json_string(out, raw_value)?;
        }
        KtType::String => {
            write_json_string(out, raw_value)?;
        }
    }
    Ok(())
}

/// Emit one JSONL object for an existing record.
///
/// The record's KV fields are taken directly from `rec.fields` (already
/// DOTSV-unescaped — no double-unescape per Pie finding 3). Keys are
/// sorted lexicographically on the raw (unescaped) key — matching
/// `Record::serialize` (Pie finding 4).
pub fn emit_jsonl_line<W: Write>(out: &mut W, rec: &Record) -> Result<()> {
    out.write_all(b"{\"_uuid\":\"")?;
    out.write_all(rec.uuid.as_bytes())?; // base62-Gu UUID — no JSON escape needed
    out.write_all(b"\"")?;
    let mut keys: Vec<&String> = rec.fields.keys().collect();
    keys.sort();
    for k in keys {
        let v = &rec.fields[k];
        out.write_all(b",")?;
        write_json_string(out, k)?;
        out.write_all(b":")?;
        encode_value(out, v)?;
    }
    out.write_all(b"}\n")?;
    Ok(())
}

/// Emit a sentinel line for a missing UUID.
pub fn emit_missing_line<W: Write>(out: &mut W, uuid: &str) -> std::io::Result<()> {
    out.write_all(b"{\"_uuid\":\"")?;
    out.write_all(uuid.as_bytes())?;
    out.write_all(b"\",\"_missing\":true}\n")?;
    Ok(())
}

// ---------------------------------------------------------------------
// Orchestrator
// ---------------------------------------------------------------------

/// Top-level `--records` runner.
///
/// Lock semantics (Pie finding 9 / Risk #3): like `--show`/`--query`,
/// `--records` acquires the empty-UUID-set lock. Concurrent invocations
/// against the same `.dov` may fail at register with `LockConflict`
/// rather than queue — this matches v0.5 semantics for `--show` and is
/// not new behaviour. The lock is held through the stdout drain because
/// the output is streamed (UUID lists may be 10^6 lines; an in-memory
/// buffer is unjustified). See Banana §4.4.
pub fn run_records_mode(input: &str, dov_path: &Path) -> Result<()> {
    if !dov_path.exists() {
        return Err(TsdbError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("database file not found: {}", dov_path.display()),
        )));
    }

    // Validate input UP FRONT (no partial output on bad input).
    let uuids = parse_uuid_input(input)?;

    // Acquire empty-UUID-set lock — same shape as --show/--query.
    let lock_mgr = LockManager::new(dov_path, Vec::new());
    lock_mgr.register()?;
    lock_mgr.wait_for_exec()?;

    let result = (|| -> Result<()> {
        // Auto-relate (compacts + writes .rtv if stale).  Note: NOT
        // auto-plane — the classifier is shared CODE, not a file
        // dependency. `.kt.ptv` is purely a diagnostic file for `--plane`
        // consumers; `--records` re-classifies on the fly.
        crate::run_relate_locked(dov_path)?;

        let db = DotsvFile::load(dov_path)?;
        if !db.pending.is_empty() {
            return Err(TsdbError::Other(
                "post-compact pending non-empty (internal invariant broken)".to_string(),
            ));
        }

        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        for u in &uuids {
            match db.binary_search_uuid(u) {
                Ok(idx) => {
                    let rec = Record::parse(&db.sorted[idx], idx + 1)?;
                    emit_jsonl_line(&mut out, &rec)?;
                }
                Err(_) => {
                    emit_missing_line(&mut out, u)?;
                }
            }
        }
        out.flush()?;
        Ok(())
    })();

    let release_result = lock_mgr.release();
    result?;
    release_result?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::parse_action_str;
    use crate::dotsv::{apply_actions, atomic_write, DotsvFile};
    use std::collections::HashMap;
    use std::io::Cursor;

    mod tmp {
        use std::path::{Path, PathBuf};
        pub struct TempDir {
            path: PathBuf,
        }
        impl TempDir {
            pub fn new() -> Self {
                let path = std::env::temp_dir()
                    .join(format!("tsdb_records_test_{:016x}", rand::random::<u64>()));
                std::fs::create_dir_all(&path).unwrap();
                TempDir { path }
            }
            pub fn path(&self) -> &Path {
                &self.path
            }
        }
        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.path);
            }
        }
    }

    fn make_db(tmp: &tmp::TempDir, action: &str) -> std::path::PathBuf {
        let dov = tmp.path().join("test.dov");
        let mut db = DotsvFile::empty();
        let actions = parse_action_str(action).unwrap();
        apply_actions(&mut db, &actions).unwrap();
        db.compact().unwrap();
        atomic_write(&db, &dov).unwrap();
        dov
    }

    fn rec(uuid: &str, fields: &[(&str, &str)]) -> Record {
        let mut map = HashMap::new();
        for (k, v) in fields {
            map.insert(k.to_string(), v.to_string());
        }
        Record {
            uuid: uuid.to_string(),
            fields: map,
        }
    }

    fn emit_to_string(r: &Record) -> String {
        let mut buf = Vec::new();
        emit_jsonl_line(&mut buf, r).unwrap();
        String::from_utf8(buf).unwrap()
    }

    fn encode_to_string(v: &str) -> String {
        let mut buf = Vec::new();
        encode_value(&mut buf, v).unwrap();
        String::from_utf8(buf).unwrap()
    }

    fn json_str(s: &str) -> String {
        let mut buf = Vec::new();
        write_json_string(&mut buf, s).unwrap();
        String::from_utf8(buf).unwrap()
    }

    // ---------- parse_uuid_lines (Banana §9.4) ----------

    #[test]
    fn parse_uuid_lines_empty() {
        let r = Cursor::new(b"");
        assert!(parse_uuid_lines(r).unwrap().is_empty());
    }

    #[test]
    fn parse_uuid_lines_single_line() {
        let r = Cursor::new(b"AGk26cH00001\n");
        let v = parse_uuid_lines(r).unwrap();
        assert_eq!(v, vec!["AGk26cH00001"]);
    }

    #[test]
    fn parse_uuid_lines_multiple_lines_preserve_order() {
        let r = Cursor::new(b"BGk26cH00001\nAGk26cH00001\nCGk26cH00001\n");
        let v = parse_uuid_lines(r).unwrap();
        assert_eq!(v, vec!["BGk26cH00001", "AGk26cH00001", "CGk26cH00001"]);
    }

    #[test]
    fn parse_uuid_lines_skip_blank() {
        let r = Cursor::new(b"\nAGk26cH00001\n\n");
        let v = parse_uuid_lines(r).unwrap();
        assert_eq!(v, vec!["AGk26cH00001"]);
    }

    #[test]
    fn parse_uuid_lines_skip_comment() {
        let r = Cursor::new(b"# header\nAGk26cH00001\n# footer\n");
        let v = parse_uuid_lines(r).unwrap();
        assert_eq!(v, vec!["AGk26cH00001"]);
    }

    #[test]
    fn parse_uuid_lines_rejects_short_uuid() {
        let r = Cursor::new(b"AGk26cH0001\n"); // 11 chars
        assert!(parse_uuid_lines(r).is_err());
    }

    #[test]
    fn parse_uuid_lines_rejects_long_uuid() {
        let r = Cursor::new(b"AGk26cH000012\n"); // 13 chars
        assert!(parse_uuid_lines(r).is_err());
    }

    #[test]
    fn parse_uuid_lines_rejects_trailing_whitespace() {
        let r = Cursor::new(b"AGk26cH0001 \n"); // 13 chars, trailing space
        assert!(parse_uuid_lines(r).is_err());
    }

    #[test]
    fn parse_uuid_lines_rejects_leading_whitespace() {
        let r = Cursor::new(b" AGk26cH00001\n"); // 13 chars, leading space
        assert!(parse_uuid_lines(r).is_err());
    }

    #[test]
    fn parse_uuid_lines_rejects_bad_base62_char() {
        // byte 5 (month) must be in MONTH_TABLE — 'z' is not
        let r = Cursor::new(b"AGk26zH00001\n");
        assert!(parse_uuid_lines(r).is_err());
    }

    #[test]
    fn parse_uuid_lines_rejects_lowercase_l() {
        // byte 8 (minute) is in Format-G; 'l' is excluded from FG
        let r = Cursor::new(b"AGk26cH0l001\n");
        assert!(parse_uuid_lines(r).is_err());
    }

    #[test]
    fn parse_uuid_lines_rejects_capital_o() {
        // byte 8 (minute) — 'O' excluded from FG
        let r = Cursor::new(b"AGk26cH0O001\n");
        assert!(parse_uuid_lines(r).is_err());
    }

    #[test]
    fn parse_uuid_lines_rejects_bom_at_start() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice("\u{FEFF}".as_bytes());
        bytes.extend_from_slice(b"AGk26cH00001\n");
        let r = Cursor::new(bytes);
        assert!(parse_uuid_lines(r).is_err());
    }

    #[test]
    fn parse_uuid_lines_rejects_crlf_line_ending() {
        // Pie finding 5: explicit reject of CRLF.
        let r = Cursor::new(b"AGk26cH00001\r\n");
        let err = parse_uuid_lines(r).unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.contains("CRLF"), "got: {}", msg);
    }

    #[test]
    fn parse_uuid_lines_preserves_duplicates() {
        let r = Cursor::new(b"AGk26cH00001\nAGk26cH00001\n");
        let v = parse_uuid_lines(r).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0], v[1]);
    }

    #[test]
    fn parse_uuid_lines_reports_line_number_in_error() {
        let r = Cursor::new(b"AGk26cH00001\nbadline\n");
        let err = parse_uuid_lines(r).unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.contains("line 2"), "got: {}", msg);
    }

    #[test]
    fn parse_uuid_lines_from_stdin_dash_alias_is_separate_path() {
        // `parse_uuid_input("-")` reads from stdin; we can't stub stdin
        // here, so this just exercises the file-path branch with a
        // real temporary file (the dash branch is covered indirectly
        // via integration tests that pipe stdin to the binary).
        let tmp = tmp::TempDir::new();
        let p = tmp.path().join("input.utv");
        std::fs::write(&p, b"AGk26cH00001\n").unwrap();
        let v = parse_uuid_input(&p.to_string_lossy()).unwrap();
        assert_eq!(v, vec!["AGk26cH00001"]);
    }

    // ---------- JSON string escape (Banana §9.5) ----------

    #[test]
    fn json_string_basic_ascii() {
        assert_eq!(json_str("hello"), "\"hello\"");
    }

    #[test]
    fn json_string_double_quote_escaped() {
        assert_eq!(json_str("a\"b"), "\"a\\\"b\"");
    }

    #[test]
    fn json_string_backslash_escaped() {
        assert_eq!(json_str("a\\b"), "\"a\\\\b\"");
    }

    #[test]
    fn json_string_newline_short_form() {
        assert_eq!(json_str("a\nb"), "\"a\\nb\"");
    }

    #[test]
    fn json_string_tab_short_form() {
        assert_eq!(json_str("a\tb"), "\"a\\tb\"");
    }

    #[test]
    fn json_string_cr_short_form() {
        assert_eq!(json_str("a\rb"), "\"a\\rb\"");
    }

    #[test]
    fn json_string_backspace_short_form() {
        assert_eq!(json_str("\x08"), "\"\\b\"");
    }

    #[test]
    fn json_string_form_feed_short_form() {
        assert_eq!(json_str("\x0C"), "\"\\f\"");
    }

    #[test]
    fn json_string_low_control_long_form_0x07() {
        // BEL (0x07) → 
        assert_eq!(json_str("\x07"), "\"\\u0007\"");
    }

    #[test]
    fn json_string_zero_byte_long_form_0x00() {
        assert_eq!(json_str("\x00"), "\"\\u0000\"");
    }

    #[test]
    fn json_string_passes_unicode_verbatim_cjk() {
        assert_eq!(json_str("東京"), "\"東京\"");
    }

    #[test]
    fn json_string_emoji_round_trip() {
        assert_eq!(json_str("🚀"), "\"🚀\"");
    }

    #[test]
    fn json_string_does_not_escape_forward_slash() {
        assert_eq!(json_str("a/b"), "\"a/b\"");
    }

    #[test]
    fn json_string_empty_is_two_quotes() {
        assert_eq!(json_str(""), "\"\"");
    }

    #[test]
    fn json_string_high_byte_utf8_passes_through_byte_for_byte() {
        // Pie test-plan-audit gap: byte-explicit pass-through for ≥ 0x80.
        // "é" = 0xC3 0xA9 → "é" (no \u escape).
        let mut buf = Vec::new();
        write_json_string(&mut buf, "\u{00E9}").unwrap();
        // Expect three bytes of payload between quotes: 0xC3 0xA9.
        assert_eq!(buf, vec![b'"', 0xC3, 0xA9, b'"']);
    }

    // ---------- value encoder (Banana §9.6) ----------

    #[test]
    fn encode_value_string_plain() {
        assert_eq!(encode_to_string("hello"), "\"hello\"");
    }

    #[test]
    fn encode_value_string_with_quote() {
        assert_eq!(encode_to_string("he said \"hi\""), "\"he said \\\"hi\\\"\"");
    }

    #[test]
    fn encode_value_number_int_no_quotes() {
        assert_eq!(encode_to_string("30"), "30");
    }

    #[test]
    fn encode_value_number_decimal_verbatim() {
        // 30.50 stays 30.50 (no f64 round-trip).
        assert_eq!(encode_to_string("30.50"), "30.50");
    }

    #[test]
    fn encode_value_number_negative() {
        assert_eq!(encode_to_string("-3.14"), "-3.14");
    }

    #[test]
    fn encode_value_boolean_true_literal() {
        assert_eq!(encode_to_string("true"), "true");
    }

    #[test]
    fn encode_value_boolean_false_literal() {
        assert_eq!(encode_to_string("false"), "false");
    }

    #[test]
    fn encode_value_timestamp_quoted_string() {
        assert_eq!(encode_to_string("20262903143022"), "\"20262903143022\"");
    }

    #[test]
    fn encode_value_array_basic() {
        assert_eq!(
            encode_to_string(r#"["admin","editor","viewer"]"#),
            r#"["admin","editor","viewer"]"#
        );
    }

    #[test]
    fn encode_value_array_empty_brackets() {
        assert_eq!(encode_to_string("[]"), "[]");
    }

    #[test]
    fn encode_value_array_with_quoted_element() {
        // input element has a literal " inside (encoded as \" in array form)
        assert_eq!(
            encode_to_string(r#"["he said \"hi\""]"#),
            "[\"he said \\\"hi\\\"\"]"
        );
    }

    #[test]
    fn encode_value_array_with_newline_element() {
        // The array element contains a literal newline. After
        // decode_array, that's a real \n, which JSON-escapes to \n.
        let raw = "[\"a\nb\"]";
        let out = encode_to_string(raw);
        assert_eq!(out, "[\"a\\nb\"]");
    }

    #[test]
    fn encode_value_array_does_not_recursively_classify_elements() {
        // Element "30" stays a JSON STRING (not a JSON number).
        assert_eq!(encode_to_string(r#"["30"]"#), "[\"30\"]");
    }

    #[test]
    fn encode_value_dotsv_escaped_tab_value_becomes_json_tab_escape() {
        // Note: rec.fields values are already DOTSV-unescaped, so we
        // pass the literal tab byte directly to encode_value.
        assert_eq!(encode_to_string("hi\tthere"), "\"hi\\tthere\"");
    }

    // ---------- line emitter (Banana §9.7) ----------

    #[test]
    fn emit_jsonl_line_uuid_first_member() {
        let r = rec("AGk26cH00001", &[("name", "Alice"), ("age", "30")]);
        let line = emit_to_string(&r);
        assert!(line.starts_with("{\"_uuid\":\"AGk26cH00001\","));
    }

    #[test]
    fn emit_jsonl_line_keys_in_raw_key_lex_order_matching_record_serialize() {
        // Pie finding 4: spec the sort key axis explicitly.
        let r = rec(
            "AGk26cH00001",
            &[("zeta", "z"), ("alpha", "a"), ("middle", "m")],
        );
        let line = emit_to_string(&r);
        let pa = line.find("\"alpha\"").unwrap();
        let pm = line.find("\"middle\"").unwrap();
        let pz = line.find("\"zeta\"").unwrap();
        assert!(pa < pm && pm < pz);
    }

    #[test]
    fn emit_jsonl_line_single_field_record() {
        let r = rec("AGk26cH00001", &[("name", "Alice")]);
        assert_eq!(
            emit_to_string(&r),
            "{\"_uuid\":\"AGk26cH00001\",\"name\":\"Alice\"}\n"
        );
    }

    #[test]
    fn emit_jsonl_line_no_trailing_comma() {
        let r = rec("AGk26cH00001", &[("a", "1"), ("b", "2")]);
        let line = emit_to_string(&r);
        assert!(line.contains("}\n"));
        assert!(!line.contains(",}\n"));
    }

    #[test]
    fn emit_jsonl_line_terminates_with_lf() {
        let r = rec("AGk26cH00001", &[("name", "Alice")]);
        let line = emit_to_string(&r);
        assert!(line.ends_with('\n'));
        assert!(!line.ends_with("\r\n"));
    }

    #[test]
    fn emit_jsonl_line_no_trailing_whitespace() {
        let r = rec("AGk26cH00001", &[("name", "Alice")]);
        let line = emit_to_string(&r);
        // The character before the closing brace and newline should be `}`.
        let trimmed = line.trim_end_matches('\n');
        assert!(trimmed.ends_with('}'));
        assert!(!trimmed.ends_with(' '));
    }

    #[test]
    fn emit_jsonl_line_unicode_key_and_value() {
        let r = rec("AGk26cH00001", &[("都市", "東京")]);
        let line = emit_to_string(&r);
        assert!(line.contains("\"都市\":\"東京\""));
    }

    #[test]
    fn emit_jsonl_line_with_array_value() {
        let r = rec("AGk26cH00001", &[("role", r#"["admin","editor"]"#)]);
        let line = emit_to_string(&r);
        assert!(line.contains(r#""role":["admin","editor"]"#));
    }

    #[test]
    fn emit_jsonl_line_with_mixed_types() {
        let r = rec(
            "AGk26cH00001",
            &[
                ("name", "Alice"),
                ("age", "30"),
                ("active", "true"),
                ("created", "20262903143022"),
                ("tags", r#"["x","y"]"#),
            ],
        );
        let line = emit_to_string(&r);
        assert!(line.contains("\"active\":true"));
        assert!(line.contains("\"age\":30"));
        assert!(line.contains("\"created\":\"20262903143022\""));
        assert!(line.contains("\"name\":\"Alice\""));
        assert!(line.contains(r#""tags":["x","y"]"#));
    }

    #[test]
    fn emit_jsonl_line_underscore_uuid_user_key_documented() {
        // Risk #5: a user record with `_uuid=hi` produces a JSON object
        // with two `_uuid` members. RFC 8259 §4 says behaviour is
        // unpredictable; document the current emission so future
        // regressions are visible.
        let r = rec("AGk26cH00001", &[("_uuid", "hi")]);
        let line = emit_to_string(&r);
        // Keys sorted lex AFTER _uuid is injected: the user `_uuid` key
        // sorts equal to the injected one (both literal "_uuid").
        // The emitted shape is therefore:
        //   {"_uuid":"AGk26cH00001","_uuid":"hi"}
        assert_eq!(
            line,
            "{\"_uuid\":\"AGk26cH00001\",\"_uuid\":\"hi\"}\n"
        );
    }

    #[test]
    fn emit_jsonl_line_underscore_missing_user_key_documented() {
        let r = rec("AGk26cH00001", &[("_missing", "yes")]);
        let line = emit_to_string(&r);
        assert_eq!(
            line,
            "{\"_uuid\":\"AGk26cH00001\",\"_missing\":\"yes\"}\n"
        );
    }

    #[test]
    fn emit_jsonl_line_underscore_user_key_sorts_lex_with_other_underscore_keys() {
        // Pie finding 6: pin the position of a second `_uuid` user key
        // when other `_*` keys are present.  Lex sort puts:
        //   _aaa  <  _uuid
        // so the order in the JSON is:
        //   {"_uuid":"<inj>","_aaa":"y","_uuid":"x"}
        let r = rec("AGk26cH00001", &[("_uuid", "x"), ("_aaa", "y")]);
        let line = emit_to_string(&r);
        assert_eq!(
            line,
            "{\"_uuid\":\"AGk26cH00001\",\"_aaa\":\"y\",\"_uuid\":\"x\"}\n"
        );
    }

    #[test]
    fn emit_jsonl_line_empty_record_emits_just_uuid() {
        let r = rec("AGk26cH00001", &[]);
        let line = emit_to_string(&r);
        assert_eq!(line, "{\"_uuid\":\"AGk26cH00001\"}\n");
    }

    #[test]
    fn emit_missing_line_format_exact() {
        let mut buf = Vec::new();
        emit_missing_line(&mut buf, "AGk26cH00001").unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s, "{\"_uuid\":\"AGk26cH00001\",\"_missing\":true}\n");
    }

    // ---------- orchestrator (Banana §9.8) ----------

    fn run_records_to_buffer(uuids: &[&str], dov: &Path) -> String {
        // Direct test of the loop body (the actual `run_records_mode`
        // takes a lock and writes to stdout, which we can't easily
        // intercept in a unit test; the integration tests cover that).
        let db = DotsvFile::load(dov).unwrap();
        let mut buf = Vec::new();
        for u in uuids {
            match db.binary_search_uuid(u) {
                Ok(idx) => {
                    let r = Record::parse(&db.sorted[idx], idx + 1).unwrap();
                    emit_jsonl_line(&mut buf, &r).unwrap();
                }
                Err(_) => emit_missing_line(&mut buf, u).unwrap(),
            }
        }
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn run_records_input_order_preserved() {
        let tmp = tmp::TempDir::new();
        let dov = make_db(
            &tmp,
            "+CGk26cH00001\tname=Carol\n\
             +AGk26cH00001\tname=Alice\n\
             +BGk26cH00001\tname=Bob\n",
        );
        let out = run_records_to_buffer(
            &["BGk26cH00001", "CGk26cH00001", "AGk26cH00001"],
            &dov,
        );
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[0].contains("Bob"));
        assert!(lines[1].contains("Carol"));
        assert!(lines[2].contains("Alice"));
    }

    #[test]
    fn run_records_missing_uuid_emits_sentinel_in_position() {
        let tmp = tmp::TempDir::new();
        let dov = make_db(&tmp, "+AGk26cH00001\tname=Alice\n");
        let out = run_records_to_buffer(
            &["AGk26cH00001", "ZGk26cH00001", "AGk26cH00001"],
            &dov,
        );
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[0].contains("Alice"));
        assert!(lines[1].contains("\"_missing\":true"));
        assert!(lines[2].contains("Alice"));
    }

    #[test]
    fn run_records_duplicate_uuid_emits_two_lines() {
        let tmp = tmp::TempDir::new();
        let dov = make_db(&tmp, "+AGk26cH00001\tname=Alice\n");
        let out = run_records_to_buffer(&["AGk26cH00001", "AGk26cH00001"], &dov);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], lines[1]);
    }

    #[test]
    fn run_records_empty_input_empty_output() {
        let tmp = tmp::TempDir::new();
        let dov = make_db(&tmp, "+AGk26cH00001\tname=Alice\n");
        let out = run_records_to_buffer(&[], &dov);
        assert!(out.is_empty());
    }

    #[test]
    fn run_records_uses_compacted_dov() {
        // Build with a compacted .dov; verify lookups succeed.
        let tmp = tmp::TempDir::new();
        let dov = make_db(&tmp, "+AGk26cH00001\tname=Alice\n");
        let db = DotsvFile::load(&dov).unwrap();
        assert!(db.pending.is_empty());
        let out = run_records_to_buffer(&["AGk26cH00001"], &dov);
        assert!(out.contains("Alice"));
    }

    #[test]
    fn run_records_acquires_lock() {
        // Smoke test: run_records_mode should not error against a
        // never-locked .dov. Doesn't intercept the lock file but the
        // released-lock invariant is exercised.
        let tmp = tmp::TempDir::new();
        let dov = make_db(&tmp, "+AGk26cH00001\tname=Alice\n");
        let utv = tmp.path().join("input.utv");
        std::fs::write(&utv, b"AGk26cH00001\n").unwrap();
        // run_records_mode writes to stdout — safe to call in tests.
        run_records_mode(&utv.to_string_lossy(), &dov).unwrap();
    }

    #[test]
    fn run_records_register_conflicts_with_concurrent_writer() {
        // Pie finding 9: assert LockConflict, NOT wall-clock blocking.
        // Pre-seed the lock file with an EXEC entry that occupies an
        // overlapping (empty-set) UUID range; then run_records_mode
        // should fail at register.
        use crate::lock::{serialize_lock_file, EntryStatus, LockEntry};
        use std::time::{SystemTime, UNIX_EPOCH};
        let tmp = tmp::TempDir::new();
        let dov = make_db(&tmp, "+AGk26cH00001\tname=Alice\n");
        let lock_path = {
            let mut s = dov.as_os_str().to_os_string();
            s.push(".lock");
            std::path::PathBuf::from(s)
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let entries = vec![LockEntry {
            status: EntryStatus::Exec,
            pid: 0xDEADBEEF,
            uuids: vec![], // empty = full-file conflict
            timestamp: now,
        }];
        std::fs::write(&lock_path, serialize_lock_file(&entries)).unwrap();
        let utv = tmp.path().join("input.utv");
        std::fs::write(&utv, b"AGk26cH00001\n").unwrap();
        let result = run_records_mode(&utv.to_string_lossy(), &dov);
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("Lock conflict") || msg.contains("conflict"),
            "expected lock conflict, got: {}",
            msg
        );
    }

    #[test]
    fn run_records_invalid_uuid_input_aborts_no_output() {
        let tmp = tmp::TempDir::new();
        let dov = make_db(&tmp, "+AGk26cH00001\tname=Alice\n");
        let utv = tmp.path().join("bad.utv");
        std::fs::write(&utv, b"not-a-valid-uuid\n").unwrap();
        let result = run_records_mode(&utv.to_string_lossy(), &dov);
        assert!(result.is_err());
    }

    #[test]
    fn run_records_lock_held_through_stdout_drain() {
        // Smoke test: a successful end-to-end run with multiple UUIDs
        // does not error or panic. Cannot easily observe lock state
        // mid-drain in a unit test; the integration tests are stronger.
        let tmp = tmp::TempDir::new();
        let dov = make_db(
            &tmp,
            "+AGk26cH00001\tname=Alice\n\
             +AGk26cH00002\tname=Bob\n\
             +AGk26cH00003\tname=Carol\n",
        );
        let utv = tmp.path().join("input.utv");
        std::fs::write(
            &utv,
            b"AGk26cH00001\nAGk26cH00002\nAGk26cH00003\n",
        )
        .unwrap();
        run_records_mode(&utv.to_string_lossy(), &dov).unwrap();
    }

    #[test]
    fn run_records_does_not_auto_plane() {
        // Decision: --records does NOT trigger --plane (the classifier
        // is shared CODE, not a file dependency). After a `--records`
        // run, kt.ptv is NOT created.
        let tmp = tmp::TempDir::new();
        let dov = make_db(&tmp, "+AGk26cH00001\tname=Alice\n");
        let utv = tmp.path().join("input.utv");
        std::fs::write(&utv, b"AGk26cH00001\n").unwrap();
        run_records_mode(&utv.to_string_lossy(), &dov).unwrap();
        let kt = crate::keytype::kt_ptv_path(&dov);
        assert!(
            !kt.exists(),
            "--records must NOT auto-create kt.ptv (classifier shared via code, not file)"
        );
    }
}
