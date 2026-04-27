/// Key-type classifier and `*.kt.ptv` companion plane writer (v0.6, Round-2 Req A).
///
/// The classifier is the single source of truth shared by both `--plane`
/// (which uses it to populate the `*.kt.ptv` companion file) and
/// `--records` (which uses it to decide whether each value becomes a JSON
/// number, boolean, array, or string). Risk #1 in the round-2 design
/// register calls this out explicitly: the dispatch chain MUST NOT be
/// inlined elsewhere — there must be exactly one `classify()`.
///
/// Type vocabulary (5 committed types):
///
///   array      — value is in canonical `[…]` shape (`escape::is_array_value`)
///   timestamp  — 14 ASCII digits with month ∈ 01..=12, day ∈ 01..=31,
///                hour ∈ 00..=23, minute ∈ 00..=59, second ∈ 00..=59
///                (year unconstrained per DOTSV §5)
///   boolean    — exactly the literal "true" or "false"
///   number     — matches `^-?\d+(\.\d+)?$` (no scientific, no hex,
///                no leading zero except a lone "0", no leading whitespace,
///                no `+` prefix); see `is_numeric_shape`
///   string     — default fallback for anything else
///
/// Detection precedence: `array → timestamp → boolean → number → string`.
/// `boolean` precedes `number` to keep the rule "more specific then more
/// general" uniform; `true`/`false` cannot match `is_numeric_shape` so the
/// order is functionally equivalent to `number → boolean`, but the spec
/// pins the more defensive ordering.
///
/// `object` is intentionally NOT a vocabulary item — the `.atv` parser
/// rejects `{...}` literals at write time so the value class cannot exist
/// on disk.
use crate::dotsv::{DotsvFile, Record};
use crate::error::{Result, TsdbError};
use crate::escape::{escape, is_array_value};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

/// One of the five committed `kt.ptv` value types.
///
/// `Ord` derived from declaration order is NOT used for sorting on disk —
/// the writer sorts by `(key, token-string)` so that the alphabetic order
/// of the token strings determines row order. See §3.1.1 of the design.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum KtType {
    Array,
    Boolean,
    Number,
    String,
    Timestamp,
}

impl KtType {
    /// Stable spec token written to `kt.ptv` column 2. The five tokens
    /// sort lexicographically as: `array < boolean < number < string < timestamp`.
    pub fn token(self) -> &'static str {
        match self {
            KtType::Array => "array",
            KtType::Boolean => "boolean",
            KtType::Number => "number",
            KtType::String => "string",
            KtType::Timestamp => "timestamp",
        }
    }
}

/// Single source-of-truth classifier (Risk #1).
///
/// Both `keytype::generate_kt_ptv` (Requirement A) and
/// `records::encode_value` (Requirement B) call this function. There must
/// be exactly one definition.
pub fn classify(v: &str) -> KtType {
    if is_array_value(v) {
        return KtType::Array;
    }
    if is_timestamp(v) {
        return KtType::Timestamp;
    }
    if v == "true" || v == "false" {
        return KtType::Boolean;
    }
    if is_numeric_shape(v) {
        return KtType::Number;
    }
    KtType::String
}

/// Validate-shape: the value must match `^-?\d+(\.\d+)?$`. Hex,
/// scientific notation, leading zeros (except a lone `0`), leading
/// whitespace, and `+` sign are NOT numeric.
///
/// Public so `src/order.rs::encode_norm` can call it (single
/// source-of-truth — Pie finding 2).
pub fn is_numeric_shape(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let mut i = 0;
    if bytes[0] == b'-' {
        if bytes.len() == 1 {
            return false;
        }
        i = 1;
    }
    let int_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == int_start {
        return false; // no integer digits
    }
    let int_len = i - int_start;
    // Reject leading zeros except a lone "0" (e.g. "007", "0.5" allowed,
    // "0" allowed, "00" rejected).
    if int_len > 1 && bytes[int_start] == b'0' {
        return false;
    }
    if i == bytes.len() {
        return true;
    }
    if bytes[i] != b'.' {
        return false;
    }
    i += 1;
    let frac_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i != bytes.len() || i == frac_start {
        return false;
    }
    true
}

/// `true` iff `v` is exactly 14 ASCII digits AND the component
/// breakdown (per DOTSV §5: `YYYYDDMMhhmmss`) has plausible month, day,
/// hour, minute, second ranges. Year is unconstrained — DOTSV §5 doesn't
/// constrain it and `dotsv::current_timestamp` doesn't either. Leap-day
/// calendar correctness is OUT OF SCOPE.
pub fn is_timestamp(v: &str) -> bool {
    let b = v.as_bytes();
    if b.len() != 14 {
        return false;
    }
    if !b.iter().all(|x| x.is_ascii_digit()) {
        return false;
    }
    // YYYY DD MM hh mm ss
    let dd = parse_2(b[4], b[5]);
    let mm = parse_2(b[6], b[7]);
    let hh = parse_2(b[8], b[9]);
    let mn = parse_2(b[10], b[11]);
    let ss = parse_2(b[12], b[13]);
    (1..=12).contains(&mm)
        && (1..=31).contains(&dd)
        && hh < 24
        && mn < 60
        && ss < 60
}

fn parse_2(hi: u8, lo: u8) -> u32 {
    ((hi - b'0') as u32) * 10 + (lo - b'0') as u32
}

/// Derive the `.kt.ptv` path from a `.dov` path.
/// `target.dov` → `target.kt.ptv`
pub fn kt_ptv_path(dov_path: &Path) -> PathBuf {
    let stem = dov_path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    dov_path.with_file_name(format!("{}.kt.ptv", stem))
}

/// Generate (or update) `<target>.kt.ptv` from `db`.
///
/// Each row is `key\ttype\tcount\tuuid-list`, sorted lex by
/// `(key, type-token)`. UUID lists within a row are lex-sorted and
/// `,`-separated (no spaces).
///
/// Skip-if-current is handled by the caller (`plane::generate_ptvs`);
/// this function unconditionally rewrites.
///
/// `dov_ts` is the full footer string returned by
/// `relate::read_last_nonempty_line(dov_path)` — i.e. it INCLUDES the
/// leading `# `. The writer emits it verbatim, matching
/// `plane::write_ptv_file` exactly (Pie finding 1).
pub fn generate_kt_ptv(dov_path: &Path, db: &DotsvFile, dov_ts: &str) -> Result<()> {
    if !db.pending.is_empty() {
        return Err(TsdbError::Other(
            "generate_kt_ptv requires a fully compacted DotsvFile (pending must be empty)"
                .to_string(),
        ));
    }

    // Accumulator keyed by (escaped-key, type-token). Using `&'static str`
    // as the second component guarantees lex-sort of the token name
    // matches what we write to disk — `KtType`'s `Ord` is irrelevant here.
    let mut rows: BTreeMap<(String, &'static str), Vec<String>> = BTreeMap::new();

    for (i, line) in db.sorted.iter().enumerate() {
        if line.is_empty() {
            continue;
        }
        let rec = Record::parse(line, i + 1)?;
        // Sort keys for deterministic insertion (BTreeMap will re-sort
        // anyway, but this keeps debug iteration order stable).
        let mut keys: Vec<&String> = rec.fields.keys().collect();
        keys.sort();
        for k in keys {
            let v = &rec.fields[k];
            // `rec.fields` values are already DOTSV-unescaped (see
            // `action::parse_kv_fields`) so we feed `v` directly to the
            // classifier. Keys are written escaped (col 1) — same shape
            // as `plane::write_ptv_file`.
            let token = classify(v).token();
            rows.entry((escape(k), token))
                .or_default()
                .push(rec.uuid.clone());
        }
    }

    // Sort UUID lists lex for determinism.
    for v in rows.values_mut() {
        v.sort();
    }

    let path = kt_ptv_path(dov_path);
    write_kt_ptv_file(&path, &rows, dov_ts)?;
    Ok(())
}

fn write_kt_ptv_file(
    path: &Path,
    rows: &BTreeMap<(String, &'static str), Vec<String>>,
    dov_ts: &str,
) -> Result<()> {
    let file = File::create(path)?;
    let mut w = BufWriter::new(file);
    for ((key_escaped, type_token), uuids) in rows {
        w.write_all(key_escaped.as_bytes())?;
        w.write_all(b"\t")?;
        w.write_all(type_token.as_bytes())?;
        w.write_all(b"\t")?;
        w.write_all(uuids.len().to_string().as_bytes())?;
        w.write_all(b"\t")?;
        w.write_all(uuids.join(",").as_bytes())?;
        w.write_all(b"\n")?;
    }
    // Footer string already includes the leading `# ` — write verbatim
    // (Pie finding 1, matches `plane::write_ptv_file`).
    w.write_all(dov_ts.as_bytes())?;
    w.write_all(b"\n")?;
    w.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::parse_action_str;
    use crate::dotsv::{apply_actions, atomic_write};
    use crate::relate::read_last_nonempty_line;

    mod tmp {
        use std::path::{Path, PathBuf};
        pub struct TempDir {
            path: PathBuf,
        }
        impl TempDir {
            pub fn new() -> Self {
                let path = std::env::temp_dir()
                    .join(format!("tsdb_keytype_test_{:016x}", rand::random::<u64>()));
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

    // ---------- Classifier (Banana §9.1) ----------

    #[test]
    fn classify_string_plain() {
        assert_eq!(classify("hello"), KtType::String);
    }

    #[test]
    fn classify_string_with_digits_and_letters() {
        assert_eq!(classify("abc123"), KtType::String);
    }

    #[test]
    fn classify_string_empty() {
        assert_eq!(classify(""), KtType::String);
    }

    #[test]
    fn classify_string_uppercase_true_falls_through() {
        assert_eq!(classify("True"), KtType::String);
        assert_eq!(classify("TRUE"), KtType::String);
        assert_eq!(classify("False"), KtType::String);
        assert_eq!(classify("FALSE"), KtType::String);
    }

    #[test]
    fn classify_string_yes_no_falls_through() {
        assert_eq!(classify("yes"), KtType::String);
        assert_eq!(classify("no"), KtType::String);
    }

    #[test]
    fn classify_number_zero() {
        assert_eq!(classify("0"), KtType::Number);
    }

    #[test]
    fn classify_number_positive_int() {
        assert_eq!(classify("30"), KtType::Number);
        assert_eq!(classify("100"), KtType::Number);
    }

    #[test]
    fn classify_number_negative_int() {
        assert_eq!(classify("-5"), KtType::Number);
        assert_eq!(classify("-100"), KtType::Number);
    }

    #[test]
    fn classify_number_decimal() {
        assert_eq!(classify("3.14"), KtType::Number);
        assert_eq!(classify("0.5"), KtType::Number);
    }

    #[test]
    fn classify_number_negative_decimal() {
        assert_eq!(classify("-3.14"), KtType::Number);
    }

    #[test]
    fn classify_number_rejects_plus_prefix() {
        assert_eq!(classify("+5"), KtType::String);
    }

    #[test]
    fn classify_number_rejects_scientific() {
        assert_eq!(classify("1e3"), KtType::String);
        assert_eq!(classify("1E3"), KtType::String);
        assert_eq!(classify("3.14e2"), KtType::String);
    }

    #[test]
    fn classify_number_rejects_hex() {
        assert_eq!(classify("0x1A"), KtType::String);
    }

    #[test]
    fn classify_number_rejects_leading_zero() {
        assert_eq!(classify("007"), KtType::String);
        assert_eq!(classify("00"), KtType::String);
        assert_eq!(classify("01"), KtType::String);
    }

    #[test]
    fn classify_array_canonical_packed() {
        assert_eq!(classify(r#"["admin","editor"]"#), KtType::Array);
    }

    #[test]
    fn classify_array_empty_brackets() {
        assert_eq!(classify("[]"), KtType::Array);
    }

    #[test]
    fn classify_array_with_quoted_elements() {
        assert_eq!(classify(r#"["a","b","c"]"#), KtType::Array);
    }

    #[test]
    fn classify_boolean_true() {
        assert_eq!(classify("true"), KtType::Boolean);
    }

    #[test]
    fn classify_boolean_false() {
        assert_eq!(classify("false"), KtType::Boolean);
    }

    #[test]
    fn classify_boolean_rejects_capital_t() {
        assert_eq!(classify("True"), KtType::String);
    }

    #[test]
    fn classify_timestamp_valid() {
        assert_eq!(classify("20262903143022"), KtType::Timestamp);
    }

    #[test]
    fn classify_timestamp_rejects_13_digits() {
        // 13 digits: not a timestamp (length), not a leading-zero number,
        // falls through to Number. The point of the test is "not Timestamp".
        let t = classify("2026290314302");
        assert_ne!(t, KtType::Timestamp);
    }

    #[test]
    fn classify_timestamp_rejects_15_digits() {
        let t = classify("202629031430220");
        assert_ne!(t, KtType::Timestamp);
    }

    #[test]
    fn classify_timestamp_rejects_alpha_in_position() {
        // Alpha in the digit positions kills both timestamp and number,
        // so this falls all the way through to String.
        assert_eq!(classify("20262903143A22"), KtType::String);
    }

    #[test]
    fn classify_timestamp_rejects_month_zero() {
        // YYYY DD MM hh mm ss → 2026 29 00 14 30 22.  Not a timestamp
        // (month=00); 14 ASCII digits with no leading zero, so it
        // classifies as Number.
        let t = classify("20262900143022");
        assert_ne!(t, KtType::Timestamp);
    }

    #[test]
    fn classify_timestamp_rejects_month_thirteen() {
        let t = classify("20262913143022");
        assert_ne!(t, KtType::Timestamp);
    }

    #[test]
    fn classify_timestamp_rejects_day_thirty_two() {
        // YYYY DD MM hh mm ss → 2026 32 03 14 30 22
        let t = classify("20263203143022");
        assert_ne!(t, KtType::Timestamp);
    }

    #[test]
    fn classify_timestamp_rejects_hour_24() {
        let t = classify("20262903243022");
        assert_ne!(t, KtType::Timestamp);
    }

    #[test]
    fn classify_invalid_timestamp_with_leading_zero_year_is_string() {
        // 14-digit value with leading zero: is_numeric_shape rejects it
        // (int_len > 1, leading 0). With invalid timestamp components
        // (month=13), it falls through all the way to String.
        // Layout YYYY DD MM hh mm ss → 0000 13 13 14 30 22.
        // Banana §8.1 documents this case.
        assert_eq!(classify("00001313143022"), KtType::String);
    }

    #[test]
    fn classify_timestamp_accepts_year_9999() {
        assert_eq!(classify("99992903143022"), KtType::Timestamp);
    }

    #[test]
    fn classify_timestamp_accepts_year_0001() {
        assert_eq!(classify("00012903143022"), KtType::Timestamp);
    }

    #[test]
    fn classify_timestamp_leap_day_in_non_leap_year_passes() {
        // OOS per spec (§4.1 Banana): we don't enforce calendar correctness.
        // 2023 was not a leap year but Feb-29 still classifies as timestamp.
        // YYYY DD MM hh mm ss → 2023 29 02 12 00 00
        assert_eq!(classify("20232902120000"), KtType::Timestamp);
    }

    #[test]
    fn classify_precedence_array_beats_timestamp() {
        // A 14-digit string wrapped in brackets is an array — but 14 digits
        // can't be wrapped in `[…]` and stay 14 digits. Construct an array
        // whose element happens to be a valid timestamp; the OUTER value
        // is array.
        assert_eq!(classify(r#"["20262903143022"]"#), KtType::Array);
    }

    #[test]
    fn classify_precedence_timestamp_beats_number() {
        // 14-digit valid components — would also match the number regex
        // (no leading zero, no decimal). Timestamp wins.
        assert_eq!(classify("20262903143022"), KtType::Timestamp);
    }

    #[test]
    fn classify_precedence_boolean_beats_number_specific_to_general() {
        // boolean is checked before number in the dispatch chain. The
        // strings "true"/"false" can't match is_numeric_shape so this is
        // documenting the order.
        assert_eq!(classify("true"), KtType::Boolean);
        assert_eq!(classify("false"), KtType::Boolean);
    }

    #[test]
    fn classify_precedence_number_beats_string() {
        assert_eq!(classify("42"), KtType::Number);
    }

    #[test]
    fn classify_one_is_number_not_boolean() {
        assert_eq!(classify("1"), KtType::Number);
    }

    #[test]
    fn classify_zero_is_number_not_boolean() {
        assert_eq!(classify("0"), KtType::Number);
    }

    #[test]
    fn classify_all_zero_14_digit_falls_to_string() {
        // 00000000000000 — month=00 fails timestamp; number rejects
        // leading zeros (int_len > 1 with leading 0); falls through
        // to string.
        assert_eq!(classify("00000000000000"), KtType::String);
    }

    #[test]
    fn classify_thirteen_zero_digits_falls_to_string() {
        // Pie test-plan-audit gap: 13 zero digits — too short to be a
        // timestamp; not boolean; `is_numeric_shape` rejects leading
        // zeros with int_len > 1; classifies as string.
        assert_eq!(classify("0000000000000"), KtType::String);
    }

    // ---------- is_numeric_shape boundary ----------

    #[test]
    fn is_numeric_shape_accepts_zero() {
        assert!(is_numeric_shape("0"));
    }

    #[test]
    fn is_numeric_shape_rejects_empty() {
        assert!(!is_numeric_shape(""));
    }

    #[test]
    fn is_numeric_shape_rejects_lone_minus() {
        assert!(!is_numeric_shape("-"));
    }

    // ---------- is_timestamp helper ----------

    #[test]
    fn is_timestamp_basic_ok() {
        assert!(is_timestamp("20262903143022"));
    }

    #[test]
    fn is_timestamp_wrong_length() {
        assert!(!is_timestamp(""));
        assert!(!is_timestamp("123"));
        assert!(!is_timestamp("123456789012345"));
    }

    // ---------- KtType::token() ordering ----------

    #[test]
    fn token_ordering_is_array_boolean_number_string_timestamp() {
        let mut tokens = [
            KtType::String.token(),
            KtType::Timestamp.token(),
            KtType::Number.token(),
            KtType::Array.token(),
            KtType::Boolean.token(),
        ];
        tokens.sort();
        assert_eq!(
            tokens,
            ["array", "boolean", "number", "string", "timestamp"]
        );
    }

    // ---------- kt_ptv_path ----------

    #[test]
    fn kt_ptv_path_basic() {
        let p = Path::new("/data/store.dov");
        assert_eq!(kt_ptv_path(p), Path::new("/data/store.kt.ptv"));
    }

    // ---------- Generator (Banana §9.2) ----------

    fn make_db(tmp: &tmp::TempDir, action: &str) -> std::path::PathBuf {
        let dov = tmp.path().join("test.dov");
        let mut db = DotsvFile::empty();
        let actions = parse_action_str(action).unwrap();
        apply_actions(&mut db, &actions).unwrap();
        db.compact().unwrap();
        atomic_write(&db, &dov).unwrap();
        dov
    }

    fn footer(dov: &Path) -> String {
        read_last_nonempty_line(dov).unwrap()
    }

    #[test]
    fn generate_kt_ptv_empty_db_writes_footer_only() {
        let tmp = tmp::TempDir::new();
        let dov = tmp.path().join("empty.dov");
        let mut db = DotsvFile::empty();
        db.compact().unwrap();
        atomic_write(&db, &dov).unwrap();
        let db = DotsvFile::load(&dov).unwrap();
        let dov_ts = footer(&dov);
        generate_kt_ptv(&dov, &db, &dov_ts).unwrap();
        let content = std::fs::read_to_string(kt_ptv_path(&dov)).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], dov_ts);
    }

    #[test]
    fn generate_kt_ptv_single_record_one_key() {
        let tmp = tmp::TempDir::new();
        let dov = make_db(&tmp, "+AGk26cH00001\tname=Alice\n");
        let db = DotsvFile::load(&dov).unwrap();
        let dov_ts = footer(&dov);
        generate_kt_ptv(&dov, &db, &dov_ts).unwrap();
        let content = std::fs::read_to_string(kt_ptv_path(&dov)).unwrap();
        assert!(content.contains("name\tstring\t1\tAGk26cH00001\n"));
    }

    #[test]
    fn generate_kt_ptv_single_record_multi_key() {
        let tmp = tmp::TempDir::new();
        let dov = make_db(&tmp, "+AGk26cH00001\tname=Alice\tage=30\n");
        let db = DotsvFile::load(&dov).unwrap();
        let dov_ts = footer(&dov);
        generate_kt_ptv(&dov, &db, &dov_ts).unwrap();
        let content = std::fs::read_to_string(kt_ptv_path(&dov)).unwrap();
        assert!(content.contains("age\tnumber\t1\tAGk26cH00001\n"));
        assert!(content.contains("name\tstring\t1\tAGk26cH00001\n"));
    }

    #[test]
    fn generate_kt_ptv_mixed_types_per_key_emits_one_row_per_type() {
        let tmp = tmp::TempDir::new();
        let dov = make_db(
            &tmp,
            "+AGk26cH00001\tage=30\n\
             +AGk26cH00002\tage=many\n",
        );
        let db = DotsvFile::load(&dov).unwrap();
        let dov_ts = footer(&dov);
        generate_kt_ptv(&dov, &db, &dov_ts).unwrap();
        let content = std::fs::read_to_string(kt_ptv_path(&dov)).unwrap();
        assert!(content.contains("age\tnumber\t1\tAGk26cH00001\n"));
        assert!(content.contains("age\tstring\t1\tAGk26cH00002\n"));
    }

    #[test]
    fn generate_kt_ptv_count_column_is_uuid_count() {
        let tmp = tmp::TempDir::new();
        let dov = make_db(
            &tmp,
            "+AGk26cH00001\tname=Alice\n\
             +AGk26cH00002\tname=Bob\n\
             +AGk26cH00003\tname=Carol\n",
        );
        let db = DotsvFile::load(&dov).unwrap();
        let dov_ts = footer(&dov);
        generate_kt_ptv(&dov, &db, &dov_ts).unwrap();
        let content = std::fs::read_to_string(kt_ptv_path(&dov)).unwrap();
        assert!(content.contains(
            "name\tstring\t3\tAGk26cH00001,AGk26cH00002,AGk26cH00003\n"
        ));
    }

    #[test]
    fn generate_kt_ptv_count_is_record_count_not_array_element_count() {
        // Record holding role=["a","b","c"] adds 1 to (role, array).
        let tmp = tmp::TempDir::new();
        let dov = make_db(&tmp, "+AGk26cH00001\trole=admin\trole=editor\trole=viewer\n");
        let db = DotsvFile::load(&dov).unwrap();
        let dov_ts = footer(&dov);
        generate_kt_ptv(&dov, &db, &dov_ts).unwrap();
        let content = std::fs::read_to_string(kt_ptv_path(&dov)).unwrap();
        assert!(content.contains("role\tarray\t1\tAGk26cH00001\n"));
    }

    #[test]
    fn generate_kt_ptv_uuid_list_sorted_lex() {
        let tmp = tmp::TempDir::new();
        let dov = make_db(
            &tmp,
            "+CGk26cH00001\tcity=Tokyo\n\
             +AGk26cH00002\tcity=Tokyo\n\
             +BGk26cH00003\tcity=Tokyo\n",
        );
        let db = DotsvFile::load(&dov).unwrap();
        let dov_ts = footer(&dov);
        generate_kt_ptv(&dov, &db, &dov_ts).unwrap();
        let content = std::fs::read_to_string(kt_ptv_path(&dov)).unwrap();
        assert!(content.contains(
            "city\tstring\t3\tAGk26cH00002,BGk26cH00003,CGk26cH00001\n"
        ));
    }

    #[test]
    fn generate_kt_ptv_uuid_list_comma_separated_no_spaces() {
        let tmp = tmp::TempDir::new();
        let dov = make_db(
            &tmp,
            "+AGk26cH00001\tcity=Tokyo\n\
             +AGk26cH00002\tcity=Tokyo\n",
        );
        let db = DotsvFile::load(&dov).unwrap();
        let dov_ts = footer(&dov);
        generate_kt_ptv(&dov, &db, &dov_ts).unwrap();
        let content = std::fs::read_to_string(kt_ptv_path(&dov)).unwrap();
        assert!(content.contains("AGk26cH00001,AGk26cH00002"));
        assert!(!content.contains(", "));
    }

    #[test]
    fn generate_kt_ptv_rows_sorted_by_key_then_type() {
        let tmp = tmp::TempDir::new();
        let dov = make_db(
            &tmp,
            "+AGk26cH00001\tx=1\ty=true\tz=hi\n\
             +AGk26cH00002\tx=many\ty=20262903143022\n",
        );
        let db = DotsvFile::load(&dov).unwrap();
        let dov_ts = footer(&dov);
        generate_kt_ptv(&dov, &db, &dov_ts).unwrap();
        let content = std::fs::read_to_string(kt_ptv_path(&dov)).unwrap();
        let data: Vec<&str> = content
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect();
        // Expected sorted by (key, type-token):
        //   x  number  ...
        //   x  string  ...
        //   y  boolean ...
        //   y  timestamp ...
        //   z  string  ...
        assert!(data[0].starts_with("x\tnumber\t"));
        assert!(data[1].starts_with("x\tstring\t"));
        assert!(data[2].starts_with("y\tboolean\t"));
        assert!(data[3].starts_with("y\ttimestamp\t"));
        assert!(data[4].starts_with("z\tstring\t"));
    }

    #[test]
    fn generate_kt_ptv_unicode_key_round_trips() {
        let tmp = tmp::TempDir::new();
        let dov = make_db(&tmp, "+AGk26cH00001\t都市=東京\n");
        let db = DotsvFile::load(&dov).unwrap();
        let dov_ts = footer(&dov);
        generate_kt_ptv(&dov, &db, &dov_ts).unwrap();
        let content = std::fs::read_to_string(kt_ptv_path(&dov)).unwrap();
        assert!(content.contains("都市\tstring\t1\tAGk26cH00001\n"));
    }

    #[test]
    fn generate_kt_ptv_escaped_tab_in_key_round_trips() {
        let tmp = tmp::TempDir::new();
        // key "we\tird" is encoded in atv as "we\\x09ird"
        let dov = make_db(&tmp, "+AGk26cH00001\twe\\x09ird=value\n");
        let db = DotsvFile::load(&dov).unwrap();
        let dov_ts = footer(&dov);
        generate_kt_ptv(&dov, &db, &dov_ts).unwrap();
        let content = std::fs::read_to_string(kt_ptv_path(&dov)).unwrap();
        // The key column must be escaped — `we\x09ird` not raw tab.
        assert!(content.contains("we\\x09ird\tstring\t1\tAGk26cH00001\n"));
    }

    #[test]
    fn generate_kt_ptv_escaped_eq_in_key_round_trips() {
        let tmp = tmp::TempDir::new();
        // key "k=q" is encoded in atv as "k\\x3Dq"
        let dov = make_db(&tmp, "+AGk26cH00001\tk\\x3Dq=value\n");
        let db = DotsvFile::load(&dov).unwrap();
        let dov_ts = footer(&dov);
        generate_kt_ptv(&dov, &db, &dov_ts).unwrap();
        let content = std::fs::read_to_string(kt_ptv_path(&dov)).unwrap();
        assert!(content.contains("k\\x3Dq\tstring\t1\tAGk26cH00001\n"));
    }

    #[test]
    fn generate_kt_ptv_footer_matches_dov_exact_bytes() {
        let tmp = tmp::TempDir::new();
        let dov = make_db(&tmp, "+AGk26cH00001\tname=Alice\n");
        let db = DotsvFile::load(&dov).unwrap();
        let dov_ts = footer(&dov);
        generate_kt_ptv(&dov, &db, &dov_ts).unwrap();
        let kt_ts = read_last_nonempty_line(&kt_ptv_path(&dov)).unwrap();
        assert_eq!(kt_ts, dov_ts);
    }

    #[test]
    fn generate_kt_ptv_byte_identical_across_runs() {
        let tmp = tmp::TempDir::new();
        let dov = make_db(
            &tmp,
            "+AGk26cH00001\tname=Alice\tage=30\n\
             +AGk26cH00002\tname=Bob\tage=many\n",
        );
        let db = DotsvFile::load(&dov).unwrap();
        let dov_ts = footer(&dov);
        generate_kt_ptv(&dov, &db, &dov_ts).unwrap();
        let bytes1 = std::fs::read(kt_ptv_path(&dov)).unwrap();
        generate_kt_ptv(&dov, &db, &dov_ts).unwrap();
        let bytes2 = std::fs::read(kt_ptv_path(&dov)).unwrap();
        assert_eq!(bytes1, bytes2);
    }

    #[test]
    fn generate_kt_ptv_packed_array_classified_array_count_one() {
        let tmp = tmp::TempDir::new();
        let dov = make_db(&tmp, "+AGk26cH00001\trole=admin\trole=editor\n");
        let db = DotsvFile::load(&dov).unwrap();
        let dov_ts = footer(&dov);
        generate_kt_ptv(&dov, &db, &dov_ts).unwrap();
        let content = std::fs::read_to_string(kt_ptv_path(&dov)).unwrap();
        assert!(content.contains("role\tarray\t1\tAGk26cH00001\n"));
    }

    #[test]
    fn generate_kt_ptv_no_object_type_in_output() {
        // Negative test: `object` token never appears (the value class
        // can't exist on disk because the .atv parser rejects {…}).
        let tmp = tmp::TempDir::new();
        let dov = make_db(
            &tmp,
            "+AGk26cH00001\tname=Alice\tage=30\trole=admin\trole=editor\tactive=true\n",
        );
        let db = DotsvFile::load(&dov).unwrap();
        let dov_ts = footer(&dov);
        generate_kt_ptv(&dov, &db, &dov_ts).unwrap();
        let content = std::fs::read_to_string(kt_ptv_path(&dov)).unwrap();
        // No \tobject\t row anywhere
        for line in content.lines() {
            assert!(
                !line.contains("\tobject\t"),
                "found `object` type token in: {}",
                line
            );
        }
    }

    #[test]
    fn generate_kt_ptv_rejects_pending() {
        let tmp = tmp::TempDir::new();
        let dov = tmp.path().join("test.dov");
        let mut db = DotsvFile::empty();
        let actions = parse_action_str("+AGk26cH00001\tname=Alice\n").unwrap();
        apply_actions(&mut db, &actions).unwrap();
        // Don't compact — pending non-empty
        atomic_write(&db, &dov).unwrap();
        assert!(generate_kt_ptv(&dov, &db, "# 20260427000000").is_err());
    }

    #[test]
    fn generate_kt_ptv_body_unchanged_when_only_dov_footer_changes() {
        // Pie test-plan-audit gap: pin "stale because footer differs"
        // vs "stale because content differs". Same body, different footer.
        let tmp = tmp::TempDir::new();
        let dov = make_db(&tmp, "+AGk26cH00001\tname=Alice\n");
        let db = DotsvFile::load(&dov).unwrap();
        // Run with footer A
        generate_kt_ptv(&dov, &db, "# 20260427120000").unwrap();
        let bytes_a = std::fs::read(kt_ptv_path(&dov)).unwrap();
        // Run with footer B (same db, different footer string)
        generate_kt_ptv(&dov, &db, "# 20260428120000").unwrap();
        let bytes_b = std::fs::read(kt_ptv_path(&dov)).unwrap();
        // Bodies (everything except the footer line) must be equal.
        let strip_footer = |bs: &[u8]| -> Vec<u8> {
            let s = std::str::from_utf8(bs).unwrap();
            s.lines()
                .filter(|l| !l.starts_with('#'))
                .collect::<Vec<_>>()
                .join("\n")
                .into_bytes()
        };
        assert_eq!(strip_footer(&bytes_a), strip_footer(&bytes_b));
        // But the full bytes differ (footer changed).
        assert_ne!(bytes_a, bytes_b);
    }
}
