/// Round-2 cross-cutting integration test (Risk #1, Banana §9.10).
///
/// Verifies that for every value class the SAME single classifier is used
/// by `--plane` (which writes the type token to `kt.ptv` col 2) and by
/// `--records` (which uses the type to choose the JSON encoding). A
/// drift between these two would silently break user pipelines.
///
/// The test runs the actual `tsdb` binary in two modes against one .dov,
/// then reads back both outputs and asserts that the class reported by
/// `kt.ptv` matches the JSON shape the records mode emits.
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

mod tmp {
    use std::path::{Path, PathBuf};
    pub struct TempDir {
        pub path: PathBuf,
    }
    impl TempDir {
        pub fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "tsdb_int_classifier_{:016x}",
                rand::random::<u64>()
            ));
            std::fs::create_dir_all(&path).unwrap();
            TempDir { path }
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
    impl AsRef<Path> for TempDir {
        fn as_ref(&self) -> &Path {
            &self.path
        }
    }
}

fn tsdb_bin() -> PathBuf {
    // Cargo sets CARGO_BIN_EXE_<name> when building integration tests.
    let p = env!("CARGO_BIN_EXE_tsdb");
    PathBuf::from(p)
}

#[test]
fn integration_classifier_consistency_kt_ptv_and_records_agree_on_every_value() {
    let tmp = tmp::TempDir::new();
    let dov = tmp.path.join("test.dov");
    let atv = tmp.path.join("seed.atv");

    // Each record exercises one type class. Use a unique key per type
    // so kt.ptv has one row per type and we can grep for it.
    let actions = "+AGk26cH00001\tstr_field=hello\n\
                   +AGk26cH00002\tnum_field=30\n\
                   +AGk26cH00003\tnum_field=-3.14\n\
                   +AGk26cH00004\tbool_field=true\n\
                   +AGk26cH00005\tbool_field=false\n\
                   +AGk26cH00006\tts_field=20262903143022\n\
                   +AGk26cH00007\tarr_field=admin\tarr_field=editor\n\
                   +AGk26cH00008\ttricky_field=00\n";
    fs::write(&atv, actions).unwrap();

    // Apply actions
    let s = Command::new(tsdb_bin())
        .arg(&dov)
        .arg(&atv)
        .status()
        .unwrap();
    assert!(s.success(), "atv apply failed");

    // Run --plane to generate kt.ptv
    let s = Command::new(tsdb_bin())
        .arg("--plane")
        .arg(&dov)
        .status()
        .unwrap();
    assert!(s.success(), "--plane failed");

    let kt_path = dov.with_file_name("test.kt.ptv");
    let kt = fs::read_to_string(&kt_path).unwrap();

    // For each (key, expected_type, expected_json_shape_predicate), assert.
    let expectations: &[(&str, &str, fn(&str) -> bool)] = &[
        ("str_field", "string", |v: &str| {
            v.starts_with('"') && v.ends_with('"')
        }),
        ("num_field", "number", |v: &str| {
            // Bare number token, no surrounding quotes.
            !v.starts_with('"') && !v.starts_with('[') && (v.starts_with('-') || v.chars().next().is_some_and(|c| c.is_ascii_digit()))
        }),
        ("bool_field", "boolean", |v: &str| {
            v == "true" || v == "false"
        }),
        ("ts_field", "timestamp", |v: &str| {
            v == "\"20262903143022\""
        }),
        ("arr_field", "array", |v: &str| {
            v.starts_with('[') && v.ends_with(']')
        }),
        ("tricky_field", "string", |v: &str| {
            // "00" doesn't classify as number (leading zero); should be a JSON string.
            v == "\"00\""
        }),
    ];

    for (key, expected_type, _shape) in expectations {
        // Find a row in kt.ptv matching `<key>\t<type>`.
        let needle = format!("{}\t{}\t", key, expected_type);
        assert!(
            kt.lines().any(|l| l.starts_with(&needle)),
            "kt.ptv missing row for ({}, {}); kt.ptv =\n{}",
            key,
            expected_type,
            kt
        );
    }

    // Run --records on every UUID and check the JSON shape per value.
    let utv = tmp.path.join("input.utv");
    let uuid_list: String = (1..=8u32).map(|n| format!("AGk26cH{:05}\n", n)).collect();
    fs::write(&utv, &uuid_list).unwrap();

    let out = Command::new(tsdb_bin())
        .arg("--records")
        .arg(&utv)
        .arg(&dov)
        .stdout(Stdio::piped())
        .output()
        .unwrap();
    assert!(out.status.success(), "--records failed: {:?}", out);
    let stdout = String::from_utf8(out.stdout).unwrap();

    for (key, expected_type, shape) in expectations {
        // Find every JSONL line that contains "key":
        let key_marker = format!("\"{}\":", key);
        let lines: Vec<&str> = stdout.lines().filter(|l| l.contains(&key_marker)).collect();
        assert!(
            !lines.is_empty(),
            "no records output line contains key {:?}; stdout =\n{}",
            key,
            stdout
        );
        for line in lines {
            // Extract the value substring after `"<key>":` until `,` or `}`.
            let i = line.find(&key_marker).unwrap();
            let after = &line[i + key_marker.len()..];
            // Find end of the value: handle string (quoted), array (bracketed), or bare token.
            let value = if let Some(stripped) = after.strip_prefix('"') {
                // String value: scan for unescaped closing quote
                let bytes = stripped.as_bytes();
                let mut j = 0;
                while j < bytes.len() {
                    if bytes[j] == b'\\' {
                        j += 2;
                        continue;
                    }
                    if bytes[j] == b'"' {
                        break;
                    }
                    j += 1;
                }
                &after[..j + 2]
            } else if after.starts_with('[') {
                // Array: scan for matching ]
                let mut depth = 0i32;
                let bytes = after.as_bytes();
                let mut j = 0;
                let mut in_str = false;
                while j < bytes.len() {
                    let c = bytes[j];
                    if in_str {
                        if c == b'\\' {
                            j += 2;
                            continue;
                        }
                        if c == b'"' {
                            in_str = false;
                        }
                    } else {
                        if c == b'"' {
                            in_str = true;
                        } else if c == b'[' {
                            depth += 1;
                        } else if c == b']' {
                            depth -= 1;
                            if depth == 0 {
                                j += 1;
                                break;
                            }
                        }
                    }
                    j += 1;
                }
                &after[..j]
            } else {
                // Bare token (number/bool): until comma or close brace.
                let end = after
                    .find(|c: char| c == ',' || c == '}')
                    .unwrap_or(after.len());
                &after[..end]
            };
            assert!(
                shape(value),
                "value {:?} for key {:?} (expected type {}) does not match shape predicate; line = {:?}",
                value,
                key,
                expected_type,
                line
            );
        }
    }
}
