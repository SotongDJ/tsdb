//! End-to-end integration tests for `--filter`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn tsdb_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_tsdb"))
}

fn temp_dir() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "tsdb_int_filter_{:016x}",
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
        .expect("seed");
    assert!(out.status.success(), "seed failed: {:?}", out);
    dov
}

#[test]
fn filter_show_combination_emits_full_records() {
    let tmp = temp_dir();
    let dov = build_db(
        &tmp,
        "+AGk26cH00001\tname=Alice\tage=30\n+AGk26cH00002\tname=Bob\tage=25\n",
    );
    let ftv = tmp.join("f.ftv");
    write_file(&ftv, "ngt\tage\t26\n");
    let out = Command::new(tsdb_bin())
        .arg("--filter")
        .arg(&ftv)
        .arg(&dov)
        .arg("--show")
        .output()
        .expect("filter --show");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("AGk26cH00001\t"));
    assert!(!stdout.contains("AGk26cH00002\t"));
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn filter_show_to_dtv_atomic_write() {
    let tmp = temp_dir();
    let dov = build_db(&tmp, "+AGk26cH00001\tname=Alice\n");
    let ftv = tmp.join("f.ftv");
    write_file(&ftv, "has\tname\n");
    let out_path = tmp.join("o.dtv");
    let out = Command::new(tsdb_bin())
        .arg("--filter")
        .arg(&ftv)
        .arg(&dov)
        .arg("--show")
        .arg(&out_path)
        .output()
        .expect("filter --show <file>");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(out_path.exists());
    // No leftover .tmp.
    let mut tmp_path = out_path.as_os_str().to_os_string();
    tmp_path.push(".tmp");
    assert!(!Path::new(&tmp_path).exists());
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn filter_lock_compatible_with_query() {
    // Smoke test: --filter without --show emits UUIDs to stdout.
    let tmp = temp_dir();
    let dov = build_db(&tmp, "+AGk26cH00001\tname=Alice\n");
    let ftv = tmp.join("f.ftv");
    write_file(&ftv, "eq\tname\tAlice\n");
    let out = Command::new(tsdb_bin())
        .arg("--filter")
        .arg(&ftv)
        .arg(&dov)
        .output()
        .expect("filter");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout, "AGk26cH00001\n");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn filter_invalid_ftv_exits_nonzero_no_dtv_written() {
    let tmp = temp_dir();
    let dov = build_db(&tmp, "+AGk26cH00001\tname=Alice\n");
    let ftv = tmp.join("f.ftv");
    write_file(&ftv, "bogus\tname\tAlice\n");
    let out_path = tmp.join("o.dtv");
    let out = Command::new(tsdb_bin())
        .arg("--filter")
        .arg(&ftv)
        .arg(&dov)
        .arg("--show")
        .arg(&out_path)
        .output()
        .expect("filter");
    assert!(!out.status.success());
    assert!(!out_path.exists());
    let _ = std::fs::remove_dir_all(&tmp);
}
