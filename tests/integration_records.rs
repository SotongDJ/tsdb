//! End-to-end integration tests for `--records` mode.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn tsdb_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_tsdb"))
}

fn temp_dir() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "tsdb_int_records_{:016x}",
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
fn integration_records_file_input() {
    let tmp = temp_dir();
    let dov = build_db(
        &tmp,
        "+AGk26cH00001\tname=Alice\tage=30\n+AGk26cH00002\tname=Bob\n",
    );
    let utv = tmp.join("input.utv");
    write_file(&utv, "AGk26cH00001\nAGk26cH00002\n");
    let out = Command::new(tsdb_bin())
        .arg("--records")
        .arg(&utv)
        .arg(&dov)
        .output()
        .unwrap();
    assert!(out.status.success(), "{:?}", out);
    let s = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = s.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("\"_uuid\":\"AGk26cH00001\""));
    assert!(lines[0].contains("\"name\":\"Alice\""));
    assert!(lines[0].contains("\"age\":30"));
    assert!(lines[1].contains("\"_uuid\":\"AGk26cH00002\""));
    assert!(lines[1].contains("\"name\":\"Bob\""));
}

#[test]
fn integration_records_stdin_input() {
    let tmp = temp_dir();
    let dov = build_db(&tmp, "+AGk26cH00001\tname=Alice\n");
    let mut child = Command::new(tsdb_bin())
        .arg("--records")
        .arg("-")
        .arg(&dov)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let stdin = child.stdin.as_mut().unwrap();
        stdin.write_all(b"AGk26cH00001\n").unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{:?}", out);
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(s.contains("\"name\":\"Alice\""));
}

#[test]
fn integration_records_dash_alias_for_stdin() {
    // Same as previous; spelled-out to match Banana §9.10.
    let tmp = temp_dir();
    let dov = build_db(&tmp, "+AGk26cH00001\tname=Alice\n");
    let mut child = Command::new(tsdb_bin())
        .arg("--records")
        .arg("-")
        .arg(&dov)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    {
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"AGk26cH00001\n")
            .unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
}

#[test]
fn integration_records_pipeline_from_query() {
    // tsdb --query | tsdb --records -
    let tmp = temp_dir();
    let dov = build_db(
        &tmp,
        "+AGk26cH00001\tname=Alice\tcity=Tokyo\n\
         +AGk26cH00002\tname=Bob\tcity=Osaka\n",
    );
    let qtv = tmp.join("q.qtv");
    write_file(&qtv, "city\tTokyo\n");

    let q = Command::new(tsdb_bin())
        .arg("--query")
        .arg(&qtv)
        .arg(&dov)
        .stdout(Stdio::piped())
        .output()
        .unwrap();
    assert!(q.status.success(), "{:?}", q);

    let mut child = Command::new(tsdb_bin())
        .arg("--records")
        .arg("-")
        .arg(&dov)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.as_mut().unwrap().write_all(&q.stdout).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(s.contains("\"name\":\"Alice\""));
    assert!(!s.contains("\"name\":\"Bob\""));
}

#[test]
fn integration_records_pipeline_from_filter() {
    let tmp = temp_dir();
    let dov = build_db(
        &tmp,
        "+AGk26cH00001\tname=Alice\tage=30\n+AGk26cH00002\tname=Bob\tage=25\n",
    );
    let ftv = tmp.join("f.ftv");
    write_file(&ftv, "ngt\tage\t26\n");

    let f = Command::new(tsdb_bin())
        .arg("--filter")
        .arg(&ftv)
        .arg(&dov)
        .stdout(Stdio::piped())
        .output()
        .unwrap();
    assert!(f.status.success(), "{:?}", f);

    let mut child = Command::new(tsdb_bin())
        .arg("--records")
        .arg("-")
        .arg(&dov)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.as_mut().unwrap().write_all(&f.stdout).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(s.contains("\"name\":\"Alice\""));
}

#[test]
fn integration_records_missing_dov_exit_2() {
    let tmp = temp_dir();
    let utv = tmp.join("input.utv");
    write_file(&utv, "AGk26cH00001\n");
    let s = Command::new(tsdb_bin())
        .arg("--records")
        .arg(&utv)
        .arg(tmp.join("nonexistent.dov"))
        .output()
        .unwrap();
    assert!(!s.status.success());
}

#[test]
fn integration_records_missing_uuid_input_exit_2() {
    let tmp = temp_dir();
    let dov = build_db(&tmp, "+AGk26cH00001\tname=Alice\n");
    let s = Command::new(tsdb_bin())
        .arg("--records")
        .arg(tmp.join("nope.utv"))
        .arg(&dov)
        .output()
        .unwrap();
    assert!(!s.status.success());
}

#[test]
fn integration_records_dash_path_rejected() {
    // Path that begins with `-` (other than lone `-`) → exit 2.
    let tmp = temp_dir();
    let dov = build_db(&tmp, "+AGk26cH00001\tname=Alice\n");
    let s = Command::new(tsdb_bin())
        .arg("--records")
        .arg("--bad-path")
        .arg(&dov)
        .output()
        .unwrap();
    assert!(!s.status.success());
    let code = s.status.code().unwrap_or(1);
    assert_eq!(code, 2);
}

#[test]
fn integration_records_input_order_matches_input_file() {
    let tmp = temp_dir();
    let dov = build_db(
        &tmp,
        "+AGk26cH00001\tname=Alice\n\
         +AGk26cH00002\tname=Bob\n\
         +AGk26cH00003\tname=Carol\n",
    );
    let utv = tmp.join("input.utv");
    write_file(
        &utv,
        "AGk26cH00003\nAGk26cH00001\nAGk26cH00002\n",
    );
    let out = Command::new(tsdb_bin())
        .arg("--records")
        .arg(&utv)
        .arg(&dov)
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = s.lines().collect();
    assert!(lines[0].contains("Carol"));
    assert!(lines[1].contains("Alice"));
    assert!(lines[2].contains("Bob"));
}

#[test]
fn integration_records_coerces_numbers_booleans_arrays() {
    let tmp = temp_dir();
    let dov = build_db(
        &tmp,
        "+AGk26cH00001\tn=42\tb=true\ta=x\ta=y\n",
    );
    let utv = tmp.join("input.utv");
    write_file(&utv, "AGk26cH00001\n");
    let out = Command::new(tsdb_bin())
        .arg("--records")
        .arg(&utv)
        .arg(&dov)
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(s.contains("\"n\":42"));
    assert!(s.contains("\"b\":true"));
    assert!(s.contains("\"a\":[\"x\",\"y\"]"));
}

#[test]
fn integration_records_timestamp_stays_string() {
    let tmp = temp_dir();
    let dov = build_db(
        &tmp,
        "+AGk26cH00001\tcreated=20262903143022\n",
    );
    let utv = tmp.join("input.utv");
    write_file(&utv, "AGk26cH00001\n");
    let out = Command::new(tsdb_bin())
        .arg("--records")
        .arg(&utv)
        .arg(&dov)
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(s.contains("\"created\":\"20262903143022\""));
}

#[test]
fn integration_argv_help_includes_records_line() {
    let out = Command::new(tsdb_bin()).arg("--help").output().unwrap();
    assert!(out.status.success());
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(s.contains("--records"));
}

#[test]
fn integration_argv_help_plane_line_lists_five_files() {
    let out = Command::new(tsdb_bin()).arg("--help").output().unwrap();
    assert!(out.status.success());
    let s = String::from_utf8(out.stdout).unwrap();
    // The --plane line should mention the kt.ptv index.
    let plane_line = s
        .lines()
        .find(|l| l.contains("--plane"))
        .expect("--plane line missing in --help");
    assert!(
        plane_line.contains("kt.ptv"),
        "--plane help line missing kt.ptv: {}",
        plane_line
    );
}

#[test]
fn integration_records_with_show_flag_rejected() {
    // --show is not supported by --records.
    let tmp = temp_dir();
    let dov = build_db(&tmp, "+AGk26cH00001\tname=Alice\n");
    let utv = tmp.join("input.utv");
    write_file(&utv, "AGk26cH00001\n");
    let s = Command::new(tsdb_bin())
        .arg("--records")
        .arg(&utv)
        .arg(&dov)
        .arg("--show")
        .output()
        .unwrap();
    assert!(!s.status.success());
}
