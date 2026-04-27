//! End-to-end integration tests for `--plane` extension (`*.kt.ptv`).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn tsdb_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_tsdb"))
}

fn temp_dir() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "tsdb_int_keytype_{:016x}",
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
    let atv = tmp.join("seed.atv");
    write_file(&atv, action);
    let s = Command::new(tsdb_bin())
        .arg(&dov)
        .arg(&atv)
        .status()
        .unwrap();
    assert!(s.success());
    dov
}

#[test]
fn integration_plane_creates_kt_ptv() {
    let tmp = temp_dir();
    let dov = build_db(
        &tmp,
        "+AGk26cH00001\tname=Alice\tage=30\n+AGk26cH00002\tname=Bob\n",
    );
    let s = Command::new(tsdb_bin())
        .arg("--plane")
        .arg(&dov)
        .status()
        .unwrap();
    assert!(s.success());
    let kt = dov.with_file_name("test.kt.ptv");
    assert!(kt.exists());
    let content = fs::read_to_string(&kt).unwrap();
    assert!(content.contains("name\tstring\t"));
    assert!(content.contains("age\tnumber\t"));
}

#[test]
fn integration_plane_kt_ptv_grep_one_key_returns_all_types() {
    let tmp = temp_dir();
    let dov = build_db(
        &tmp,
        "+AGk26cH00001\tage=30\n\
         +AGk26cH00002\tage=many\n\
         +AGk26cH00003\tage=true\n",
    );
    Command::new(tsdb_bin())
        .arg("--plane")
        .arg(&dov)
        .status()
        .unwrap();
    let kt = dov.with_file_name("test.kt.ptv");
    let content = fs::read_to_string(&kt).unwrap();
    let age_rows: Vec<&str> = content
        .lines()
        .filter(|l| l.starts_with("age\t"))
        .collect();
    assert_eq!(age_rows.len(), 3);
    let types: std::collections::HashSet<&str> = age_rows
        .iter()
        .map(|l| l.split('\t').nth(1).unwrap())
        .collect();
    assert!(types.contains("number"));
    assert!(types.contains("string"));
    assert!(types.contains("boolean"));
}

#[test]
fn integration_plane_skip_when_kt_ptv_present_and_current() {
    let tmp = temp_dir();
    let dov = build_db(&tmp, "+AGk26cH00001\tname=Alice\n");
    Command::new(tsdb_bin())
        .arg("--plane")
        .arg(&dov)
        .status()
        .unwrap();
    let kt = dov.with_file_name("test.kt.ptv");
    let mtime1 = fs::metadata(&kt).unwrap().modified().unwrap();
    // Run again immediately — skip-if-current should fire.
    Command::new(tsdb_bin())
        .arg("--plane")
        .arg(&dov)
        .status()
        .unwrap();
    let mtime2 = fs::metadata(&kt).unwrap().modified().unwrap();
    assert_eq!(mtime1, mtime2, "kt.ptv should not be rewritten when current");
}

#[test]
fn integration_relate_does_not_create_kt_ptv() {
    // Decision #3 guard: --relate must NOT emit kt.ptv.
    let tmp = temp_dir();
    let dov = build_db(&tmp, "+AGk26cH00001\tname=Alice\n");
    Command::new(tsdb_bin())
        .arg("--relate")
        .arg(&dov)
        .status()
        .unwrap();
    let kt = dov.with_file_name("test.kt.ptv");
    assert!(!kt.exists(), "--relate must not create kt.ptv");
}
