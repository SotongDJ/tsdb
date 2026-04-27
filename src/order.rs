/// Numeric-normal-form encoding and `*.ord.ptv` companion plane writer
/// (v0.5, Gap 2).
///
/// `*.ord.ptv` mirrors `*.kv.ptv` but its first column is a sortable
/// "numeric-normal form" (`norm`) that lex-orders consistent with numeric
/// magnitude. Only values matching the regex `^-?\d+(\.\d+)?$` (parsed as
/// finite decimal) are emitted; non-numeric values are simply absent so
/// numeric ops cannot match them.
///
/// Encoding (per banana.md §2.3, Pie review §2.8):
///   norm = sign , magnitude
///   sign = "P" for non-negative, "N" for negative
///   magnitude (non-negative): 4-decimal-digit integer-part-length , "_",
///                             integer-part , [ "." , fraction-trimmed ]
///   magnitude (negative):     same shape, but each digit (and the length
///                             field) digit-complemented (0↔9, 1↔8, …),
///                             AND a trailing "~" (byte 0x7E) terminator
///                             so that shorter fractional strings sort
///                             AFTER longer ones in lex order — a
///                             well-known pitfall the spec acknowledges
///                             in Risk Register §5.
///
/// The `~` terminator deviation from banana.md §2.3 was added during
/// implementation because the unaltered scheme misorders pairs like
/// `(-3.1, -3.14)` (the encoded magnitudes "6.8" and "6.85" lex-compare
/// in the wrong direction). The pinned property test
/// `norm_sort_agrees_with_numeric_property_1000_random_pairs` would
/// otherwise fail on the first such pair the RNG produces.
use crate::dotsv::{DotsvFile, Record};
use crate::error::{Result, TsdbError};
use crate::escape::{decode_array, escape, is_array_value};
use crate::keytype::is_numeric_shape;
use crate::relate::read_last_nonempty_line;
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

/// Derive the `.ord.ptv` path from a `.dov` path.
/// `target.dov` → `target.ord.ptv`
pub fn ord_ptv_path(dov_path: &Path) -> PathBuf {
    let stem = dov_path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    dov_path.with_file_name(format!("{}.ord.ptv", stem))
}

/// Encode a numeric string into `norm`. Returns `None` if `s` is not
/// numeric per `keytype::is_numeric_shape` (single source-of-truth —
/// Pie finding 2; the predicate now lives in `src/keytype.rs` and is
/// shared between `--plane`'s numeric ordering and the v0.6 type
/// classifier).
pub fn encode_norm(s: &str) -> Option<String> {
    if !is_numeric_shape(s) {
        return None;
    }
    let (negative, body) = if let Some(rest) = s.strip_prefix('-') {
        (true, rest)
    } else {
        (false, s)
    };
    let (int_part, frac_part) = match body.find('.') {
        Some(idx) => (&body[..idx], &body[idx + 1..]),
        None => (body, ""),
    };
    // Trim trailing zeros in fraction.
    let frac_trimmed = frac_part.trim_end_matches('0');

    // For "-0", "-0.0", etc: numerically 0. Emit as positive zero so the
    // sort agrees with the convention `-0 == 0` and avoids a double
    // representation.
    if negative && int_part == "0" && frac_trimmed.is_empty() {
        return Some(format!("P{:04}_0", 1));
    }

    let int_len = int_part.len();
    if int_len > 9999 {
        // Length wider than the 4-digit field. Treat as out-of-range,
        // i.e. not numeric (the value cannot be encoded).
        return None;
    }

    if !negative {
        let mut out = String::with_capacity(8 + int_len + frac_trimmed.len());
        out.push('P');
        out.push_str(&format!("{:04}", int_len));
        out.push('_');
        out.push_str(int_part);
        if !frac_trimmed.is_empty() {
            out.push('.');
            out.push_str(frac_trimmed);
        }
        Some(out)
    } else {
        // Complement each digit; complement length-field digits too.
        let len_str = format!("{:04}", int_len);
        let mut out = String::with_capacity(8 + int_len + frac_trimmed.len() + 1);
        out.push('N');
        for b in len_str.bytes() {
            out.push(complement_digit(b) as char);
        }
        out.push('_');
        for b in int_part.bytes() {
            out.push(complement_digit(b) as char);
        }
        if !frac_trimmed.is_empty() {
            out.push('.');
            for b in frac_trimmed.bytes() {
                out.push(complement_digit(b) as char);
            }
        }
        // Terminator: ensures shorter fractions sort AFTER longer ones
        // (so e.g. -3.1 sorts after -3.14, matching numeric order).
        out.push('~');
        Some(out)
    }
}

/// Complement an ASCII decimal digit byte: '0' ↔ '9', '1' ↔ '8', etc.
/// Non-digit bytes pass through unchanged (defensive — callers only feed
/// digit bytes).
fn complement_digit(b: u8) -> u8 {
    if b.is_ascii_digit() {
        b'9' - (b - b'0')
    } else {
        b
    }
}

/// Generate (or update) `<target>.ord.ptv` from `db`.
///
/// Preconditions: `db.pending` is empty (caller compacted first).
/// Skip condition: handled by the caller (`plane::generate_ptvs`); this
/// function unconditionally rewrites.
pub fn generate_ord_ptv(dov_path: &Path, db: &DotsvFile) -> Result<()> {
    if !db.pending.is_empty() {
        return Err(TsdbError::Other(
            "generate_ord_ptv requires a fully compacted DotsvFile".to_string(),
        ));
    }

    let path = ord_ptv_path(dov_path);
    let dov_ts = read_last_nonempty_line(dov_path)?;
    let rows = build_ord_rows(db)?;
    write_ord_file(&path, &rows, &dov_ts)?;
    Ok(())
}

/// Build sorted (norm, key, raw-value, uuid) tuples from the sorted
/// section. Only numeric values are included; arrays expand per-element
/// (per banana.md §2.5).
fn build_ord_rows(db: &DotsvFile) -> Result<Vec<(String, String, String, String)>> {
    let mut set: BTreeSet<(String, String, String, String)> = BTreeSet::new();

    for (i, line) in db.sorted.iter().enumerate() {
        if line.is_empty() {
            continue;
        }
        let rec = Record::parse(line, i + 1)?;
        for (k, v) in &rec.fields {
            if is_array_value(v) {
                let elements = decode_array(v).map_err(|e| TsdbError::ParseError {
                    line: i + 1,
                    message: format!("array value for key {:?}: {}", k, e),
                })?;
                for elem in elements {
                    if let Some(norm) = encode_norm(&elem) {
                        set.insert((norm, k.clone(), elem, rec.uuid.clone()));
                    }
                }
            } else if let Some(norm) = encode_norm(v) {
                set.insert((norm, k.clone(), v.clone(), rec.uuid.clone()));
            }
        }
    }

    Ok(set.into_iter().collect())
}

fn write_ord_file(
    path: &Path,
    rows: &[(String, String, String, String)],
    timestamp: &str,
) -> Result<()> {
    let file = File::create(path)?;
    let mut w = BufWriter::new(file);
    for (norm, key, raw, uuid) in rows {
        // norm is ASCII-only (P/N + digits + _ + . + ~), no escape needed,
        // but we escape defensively to keep the column invariant tight.
        w.write_all(escape(norm).as_bytes())?;
        w.write_all(b"\t")?;
        w.write_all(escape(key).as_bytes())?;
        w.write_all(b"\t")?;
        w.write_all(escape(raw).as_bytes())?;
        w.write_all(b"\t")?;
        w.write_all(uuid.as_bytes())?;
        w.write_all(b"\n")?;
    }
    w.write_all(timestamp.as_bytes())?;
    w.write_all(b"\n")?;
    w.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::parse_action_str;
    use crate::dotsv::{apply_actions, atomic_write};

    mod tmp {
        use std::path::{Path, PathBuf};
        pub struct TempDir {
            path: PathBuf,
        }
        impl TempDir {
            pub fn new() -> Self {
                let path = std::env::temp_dir()
                    .join(format!("tsdb_order_test_{:016x}", rand::random::<u64>()));
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

    // ---------- shape detection ----------

    #[test]
    fn norm_rejects_scientific_notation() {
        assert_eq!(encode_norm("1e3"), None);
        assert_eq!(encode_norm("1E3"), None);
        assert_eq!(encode_norm("3.14e2"), None);
    }

    #[test]
    fn norm_rejects_hex() {
        assert_eq!(encode_norm("0x1A"), None);
        assert_eq!(encode_norm("0X1A"), None);
    }

    #[test]
    fn norm_rejects_leading_zero() {
        assert_eq!(encode_norm("007"), None);
        assert_eq!(encode_norm("00"), None);
        assert_eq!(encode_norm("01"), None);
    }

    #[test]
    fn norm_rejects_leading_whitespace() {
        assert_eq!(encode_norm(" 5"), None);
        assert_eq!(encode_norm("\t5"), None);
        assert_eq!(encode_norm("5 "), None);
    }

    #[test]
    fn norm_rejects_plus_sign() {
        assert_eq!(encode_norm("+5"), None);
    }

    #[test]
    fn norm_rejects_trailing_dot() {
        assert_eq!(encode_norm("5."), None);
    }

    #[test]
    fn norm_rejects_lone_minus() {
        assert_eq!(encode_norm("-"), None);
    }

    #[test]
    fn norm_rejects_empty() {
        assert_eq!(encode_norm(""), None);
    }

    #[test]
    fn norm_accepts_zero() {
        assert!(encode_norm("0").is_some());
    }

    // ---------- worked examples (banana.md §2.3) ----------

    #[test]
    fn norm_zero() {
        assert_eq!(encode_norm("0").unwrap(), "P0001_0");
    }

    #[test]
    fn norm_positive_int() {
        assert_eq!(encode_norm("5").unwrap(), "P0001_5");
        assert_eq!(encode_norm("30").unwrap(), "P0002_30");
        assert_eq!(encode_norm("100").unwrap(), "P0003_100");
    }

    #[test]
    fn norm_negative_int() {
        assert_eq!(encode_norm("-5").unwrap(), "N9998_4~");
        assert_eq!(encode_norm("-30").unwrap(), "N9997_69~");
    }

    #[test]
    fn norm_positive_decimal() {
        assert_eq!(encode_norm("3.14").unwrap(), "P0001_3.14");
        assert_eq!(encode_norm("0.5").unwrap(), "P0001_0.5");
    }

    #[test]
    fn norm_negative_decimal() {
        assert_eq!(encode_norm("-3.14").unwrap(), "N9998_6.85~");
    }

    #[test]
    fn norm_trims_trailing_zeros() {
        assert_eq!(encode_norm("30").unwrap(), encode_norm("30.0").unwrap());
        assert_eq!(
            encode_norm("3.14").unwrap(),
            encode_norm("3.14000").unwrap()
        );
    }

    #[test]
    fn norm_negative_zero_normalises_to_positive_zero() {
        assert_eq!(encode_norm("-0").unwrap(), encode_norm("0").unwrap());
        assert_eq!(encode_norm("-0.0").unwrap(), encode_norm("0").unwrap());
    }

    // ---------- sort stability across sign boundary ----------

    #[test]
    fn norm_sort_agrees_with_numeric_across_sign_boundary() {
        let mut samples: Vec<(&str, f64)> = vec![
            ("-100", -100.0),
            ("-30", -30.0),
            ("-3.14", -3.14),
            ("-3.1", -3.1),
            ("-3", -3.0),
            ("-1", -1.0),
            ("0", 0.0),
            ("1", 1.0),
            ("3", 3.0),
            ("3.1", 3.1),
            ("3.14", 3.14),
            ("30", 30.0),
            ("100", 100.0),
        ];
        // Sort by norm
        let mut by_norm: Vec<(String, f64)> = samples
            .iter()
            .map(|(s, v)| (encode_norm(s).unwrap(), *v))
            .collect();
        by_norm.sort_by(|a, b| a.0.cmp(&b.0));
        // Verify f64 order matches
        samples.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        for ((_, expected_v), (_, actual_v)) in samples.iter().zip(by_norm.iter()) {
            assert!(
                (*expected_v - *actual_v).abs() < 1e-9,
                "sort mismatch: expected {} got {}",
                expected_v,
                actual_v
            );
        }
    }

    #[test]
    fn norm_sort_agrees_with_numeric_property_1000_random_pairs() {
        // Fixed seed for determinism — uses `rand` already in budget.
        use rand::rngs::StdRng;
        use rand::SeedableRng;
        let mut rng = StdRng::seed_from_u64(0xB014_06_27);
        for _ in 0..1000 {
            let a = sample_decimal(&mut rng);
            let b = sample_decimal(&mut rng);
            let na = encode_norm(&a).unwrap_or_else(|| panic!("a={:?}", a));
            let nb = encode_norm(&b).unwrap_or_else(|| panic!("b={:?}", b));
            let fa: f64 = a.parse().unwrap();
            let fb: f64 = b.parse().unwrap();
            let lex = na.cmp(&nb);
            let num = fa.partial_cmp(&fb).unwrap();
            // norm equality may legitimately collapse 30 / 30.0 / -0 / 0
            // → check that lex order agrees with numeric order whenever
            //   the norms differ; if norms tie, the f64 values must be
            //   equal (within rounding).
            if lex != std::cmp::Ordering::Equal {
                assert_eq!(
                    lex, num,
                    "lex_cmp({:?},{:?}) = {:?} but numeric_cmp({},{}) = {:?}",
                    na, nb, lex, fa, fb, num
                );
            } else {
                assert!(
                    (fa - fb).abs() < 1e-9,
                    "norm collision for non-equal numerics: {:?} ({}) vs {:?} ({})",
                    a,
                    fa,
                    b,
                    fb
                );
            }
        }
    }

    fn sample_decimal<R: rand::Rng>(rng: &mut R) -> String {
        // Build a string representation matching `is_numeric_shape`.
        let negative = rng.gen_bool(0.5);
        let int_len = rng.gen_range(1..=4);
        let int_part: String = if int_len == 1 {
            rng.gen_range(0u8..=9).to_string()
        } else {
            let mut s = String::with_capacity(int_len);
            // First digit 1-9 to avoid leading zeros
            s.push(char::from(b'0' + rng.gen_range(1u8..=9)));
            for _ in 1..int_len {
                s.push(char::from(b'0' + rng.gen_range(0u8..=9)));
            }
            s
        };
        let has_frac = rng.gen_bool(0.5);
        let frac_part: String = if has_frac {
            let frac_len = rng.gen_range(1..=4);
            let mut s = String::with_capacity(frac_len);
            for _ in 0..frac_len {
                s.push(char::from(b'0' + rng.gen_range(0u8..=9)));
            }
            s
        } else {
            String::new()
        };
        let mut out = String::new();
        if negative {
            out.push('-');
        }
        out.push_str(&int_part);
        if !frac_part.is_empty() {
            out.push('.');
            out.push_str(&frac_part);
        }
        out
    }

    // ---------- file generation ----------

    fn make_test_db(tmp: &tmp::TempDir, action: &str) -> std::path::PathBuf {
        let dov = tmp.path().join("test.dov");
        let mut db = DotsvFile::empty();
        let actions = parse_action_str(action).unwrap();
        apply_actions(&mut db, &actions).unwrap();
        db.compact().unwrap();
        atomic_write(&db, &dov).unwrap();
        dov
    }

    #[test]
    fn generate_ord_ptv_emits_only_numeric_values() {
        let tmp = tmp::TempDir::new();
        let dov = make_test_db(
            &tmp,
            "+AGk26cH00001\tname=Alice\tage=30\n\
             +AGk26cH00002\tname=Bob\tage=25\n",
        );
        let db = DotsvFile::load(&dov).unwrap();
        generate_ord_ptv(&dov, &db).unwrap();
        let path = ord_ptv_path(&dov);
        let content = std::fs::read_to_string(&path).unwrap();
        // age rows present, name rows absent (Alice/Bob are not numeric)
        assert!(content.contains("\tage\t25\tAGk26cH00002"));
        assert!(content.contains("\tage\t30\tAGk26cH00001"));
        assert!(!content.contains("\tname\t"));
    }

    #[test]
    fn generate_ord_ptv_expands_array_elements() {
        let tmp = tmp::TempDir::new();
        let dov = make_test_db(
            &tmp,
            "+AGk26cH00001\tscore=10\tscore=60\tscore=70\n",
        );
        let db = DotsvFile::load(&dov).unwrap();
        generate_ord_ptv(&dov, &db).unwrap();
        let content = std::fs::read_to_string(ord_ptv_path(&dov)).unwrap();
        let data_rows = content
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .count();
        assert_eq!(data_rows, 3);
    }

    #[test]
    fn generate_ord_ptv_footer_matches_dov() {
        let tmp = tmp::TempDir::new();
        let dov = make_test_db(&tmp, "+AGk26cH00001\tage=30\n");
        let db = DotsvFile::load(&dov).unwrap();
        generate_ord_ptv(&dov, &db).unwrap();
        let dov_ts = read_last_nonempty_line(&dov).unwrap();
        let ord_ts = read_last_nonempty_line(&ord_ptv_path(&dov)).unwrap();
        assert_eq!(ord_ts, dov_ts);
    }

    #[test]
    fn generate_ord_ptv_byte_identical_across_runs() {
        let tmp = tmp::TempDir::new();
        let dov = make_test_db(
            &tmp,
            "+AGk26cH00001\tage=30\n\
             +AGk26cH00002\tage=25\n\
             +AGk26cH00003\tage=-5\n",
        );
        let db = DotsvFile::load(&dov).unwrap();
        generate_ord_ptv(&dov, &db).unwrap();
        let bytes1 = std::fs::read(ord_ptv_path(&dov)).unwrap();
        generate_ord_ptv(&dov, &db).unwrap();
        let bytes2 = std::fs::read(ord_ptv_path(&dov)).unwrap();
        assert_eq!(bytes1, bytes2);
    }
}
