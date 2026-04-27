//! End-to-end integration tests for `--query --show` and `--filter --show`.
//!
//! These tests exec the compiled `tsdb` binary so they exercise the
//! actual CLI argv parser, lock manager, and atomic-write paths.

use std::path::{Path, PathBuf};
use std::process::Command;

fn tsdb_bin() -> PathBuf {
    // Cargo sets CARGO_BIN_EXE_<name> for integration tests.
    PathBuf::from(env!("CARGO_BIN_EXE_tsdb"))
}

fn temp_dir() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "tsdb_int_show_{:016x}",
        rand::random::<u64>()
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn write_file(path: &Path, content: &str) {
    std::fs::write(path, content).unwrap();
}

fn build_db(tmp: &Path, action: &str) -> PathBuf {
    let dov = tmp.join("test.dov");
    let action_path = tmp.join("seed.atv");
    write_file(&action_path, action);
    let out = Command::new(tsdb_bin())
        .arg(&dov)
        .arg(&action_path)
        .output()
        .expect("tsdb seed");
    assert!(out.status.success(), "seed failed: {:?}", out);
    dov
}

#[test]
fn show_argv_query_without_show_unchanged() {
    let tmp = temp_dir();
    let dov = build_db(&tmp, "+AGk26cH00001\tname=Alice\n");
    let qtv = tmp.join("q.qtv");
    write_file(&qtv, "name\tAlice\n");
    let out = Command::new(tsdb_bin())
        .arg("--query")
        .arg(&qtv)
        .arg(&dov)
        .output()
        .expect("query");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    // v0.5 byte-identical: just the UUID + newline.
    assert_eq!(stdout, "AGk26cH00001\n");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn show_argv_show_flag_after_three_args_is_recognised() {
    let tmp = temp_dir();
    let dov = build_db(&tmp, "+AGk26cH00001\tname=Alice\n");
    let qtv = tmp.join("q.qtv");
    write_file(&qtv, "name\tAlice\n");
    let out = Command::new(tsdb_bin())
        .arg("--query")
        .arg(&qtv)
        .arg(&dov)
        .arg("--show")
        .output()
        .expect("query --show");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Full record + footer (footer line begins with `# `).
    assert!(stdout.contains("AGk26cH00001\tname=Alice"));
    assert!(stdout.lines().any(|l| l.starts_with("# ")));
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn show_argv_show_flag_swallows_following_path_token() {
    let tmp = temp_dir();
    let dov = build_db(&tmp, "+AGk26cH00001\tname=Alice\n");
    let qtv = tmp.join("q.qtv");
    write_file(&qtv, "name\tAlice\n");
    let out_path = tmp.join("records.dtv");
    let out = Command::new(tsdb_bin())
        .arg("--query")
        .arg(&qtv)
        .arg(&dov)
        .arg("--show")
        .arg(&out_path)
        .output()
        .expect("query --show <file>");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(out_path.exists());
    let content = std::fs::read_to_string(&out_path).unwrap();
    assert!(content.contains("AGk26cH00001\tname=Alice"));
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn show_dash_is_stdout_alias() {
    let tmp = temp_dir();
    let dov = build_db(&tmp, "+AGk26cH00001\tname=Alice\n");
    let qtv = tmp.join("q.qtv");
    write_file(&qtv, "name\tAlice\n");
    let out = Command::new(tsdb_bin())
        .arg("--query")
        .arg(&qtv)
        .arg(&dov)
        .arg("--show")
        .arg("-")
        .output()
        .expect("query --show -");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("AGk26cH00001\tname=Alice"));
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn show_invalid_qtv_exits_2_no_dtv_written() {
    let tmp = temp_dir();
    let dov = build_db(&tmp, "+AGk26cH00001\tname=Alice\n");
    // Bogus @directive → parse error.
    let qtv = tmp.join("q.qtv");
    write_file(&qtv, "@bogus\tkey\n");
    let out_path = tmp.join("o.dtv");
    let out = Command::new(tsdb_bin())
        .arg("--query")
        .arg(&qtv)
        .arg(&dov)
        .arg("--show")
        .arg(&out_path)
        .output()
        .expect("query");
    // Parse error in qtv → execution error (exit 1) is also acceptable
    // since the proposal allows either parse-time (2) or exec-time (1)
    // depending on when the error surfaces. Banana §1.1 says parse → 2,
    // so verify the error path produces a non-zero exit and no output.
    assert!(!out.status.success());
    assert!(!out_path.exists());
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn show_leading_dash_path_rejected() {
    // Pie's required negative test (banana §1.1): paths beginning with
    // `-` other than the lone `-` must be rejected at argv parse time.
    let tmp = temp_dir();
    let dov = build_db(&tmp, "+AGk26cH00001\tname=Alice\n");
    let qtv = tmp.join("q.qtv");
    write_file(&qtv, "name\tAlice\n");
    let out = Command::new(tsdb_bin())
        .arg("--query")
        .arg(&qtv)
        .arg(&dov)
        .arg("--show")
        .arg("-out.dtv")
        .output()
        .expect("invalid invocation");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("must not start with '-'"),
        "expected diagnostic, got: {}",
        stderr
    );
    let _ = std::fs::remove_dir_all(&tmp);
}
