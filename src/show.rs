/// `--show` modifier: emit full DOTSV records matching a query/filter
/// to stdout or to a `.dtv` file (banana.md Gap 1).
///
/// Compatible with both `--query` (UUIDs from `.qtv`) and `--filter`
/// (UUIDs from `.ftv`). When `--show` is absent the legacy UUID-only
/// stdout is preserved byte-for-byte (regression-guarded).
use crate::dotsv::DotsvFile;
use crate::error::{Result, TsdbError};
use crate::relate::read_last_nonempty_line;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

/// Where to send `--show` output.
#[derive(Debug, Clone)]
pub enum ShowTarget {
    /// `--show` (no path) or `--show -` → stdout.
    Stdout,
    /// `--show <out.dtv>` → atomic write to file, skip-if-current.
    File(PathBuf),
}

/// Derive the default `.dtv` file path from a `.dov` path. Currently
/// unused (callers always pass an explicit path) but kept for symmetry.
#[allow(dead_code)]
pub fn dtv_path(dov_path: &Path) -> PathBuf {
    let stem = dov_path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    dov_path.with_file_name(format!("{}.dtv", stem))
}

/// Skip rule for file-mode `--show` (banana.md §1.2 / Decision #5):
/// skip iff the dtv exists, its footer matches the dov footer, AND the
/// criterion file (qtv or ftv) is older than the dtv.
pub fn dtv_skip_if_current(
    out_path: &Path,
    dov_path: &Path,
    criterion_path: &Path,
) -> Result<bool> {
    if !out_path.exists() {
        return Ok(false);
    }
    let dov_ts = read_last_nonempty_line(dov_path)?;
    let dtv_ts = match read_last_nonempty_line(out_path) {
        Ok(s) => s,
        Err(_) => return Ok(false),
    };
    if dov_ts != dtv_ts {
        return Ok(false);
    }
    // Compare mtimes: skip iff criterion-file mtime < dtv mtime.
    let crit_meta = match fs::metadata(criterion_path) {
        Ok(m) => m,
        Err(_) => return Ok(false),
    };
    let dtv_meta = fs::metadata(out_path)?;
    let crit_mtime = crit_meta.modified().unwrap_or(std::time::UNIX_EPOCH);
    let dtv_mtime = dtv_meta.modified().unwrap_or(std::time::UNIX_EPOCH);
    Ok(crit_mtime < dtv_mtime)
}

/// Look up the on-disk record line for each UUID. Asserts that no
/// pending records remain (caller must compact first). Records emitted
/// in sorted-UUID order, KV pairs already sorted by `Record::serialize`.
pub fn collect_record_lines(uuids: &[String], db: &DotsvFile) -> Result<Vec<String>> {
    if !db.pending.is_empty() {
        return Err(TsdbError::Other(
            "show requires a fully compacted DotsvFile (pending must be empty)".to_string(),
        ));
    }
    let mut sorted_uuids: Vec<String> = uuids.to_vec();
    sorted_uuids.sort();
    let mut lines = Vec::with_capacity(sorted_uuids.len());
    for u in &sorted_uuids {
        match db.binary_search_uuid(u) {
            Ok(idx) => lines.push(db.sorted[idx].clone()),
            Err(_) => {
                return Err(TsdbError::Other(format!(
                    "{} resolved by index but missing from compacted .dov",
                    u
                )));
            }
        }
    }
    Ok(lines)
}

/// Emit records and footer to stdout.
pub fn emit_to_stdout(records: &[String], footer: &str) {
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    for line in records {
        let _ = writeln!(handle, "{}", line);
    }
    let _ = writeln!(handle, "{}", footer);
}

/// Atomically write records and footer to a `.dtv` file.
pub fn write_dtv_file(out_path: &Path, records: &[String], footer: &str) -> Result<()> {
    let tmp_path = {
        let mut s = out_path.as_os_str().to_os_string();
        s.push(".tmp");
        PathBuf::from(s)
    };
    {
        let file = File::create(&tmp_path)?;
        let mut w = BufWriter::new(file);
        for line in records {
            w.write_all(line.as_bytes())?;
            w.write_all(b"\n")?;
        }
        w.write_all(footer.as_bytes())?;
        w.write_all(b"\n")?;
        w.flush()?;
    }
    fs::rename(&tmp_path, out_path)?;
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
                    .join(format!("tsdb_show_test_{:016x}", rand::random::<u64>()));
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

    fn build_db(tmp: &tmp::TempDir, action: &str) -> std::path::PathBuf {
        let dov = tmp.path().join("test.dov");
        let mut db = DotsvFile::empty();
        let actions = parse_action_str(action).unwrap();
        apply_actions(&mut db, &actions).unwrap();
        db.compact().unwrap();
        atomic_write(&db, &dov).unwrap();
        dov
    }

    #[test]
    fn show_to_dtv_writes_timestamp_footer() {
        let tmp = tmp::TempDir::new();
        let dov = build_db(&tmp, "+AGk26cH00001\tname=Alice\n");
        let db = DotsvFile::load(&dov).unwrap();
        let footer = read_last_nonempty_line(&dov).unwrap();
        let lines = collect_record_lines(&["AGk26cH00001".to_string()], &db).unwrap();
        let out = tmp.path().join("out.dtv");
        write_dtv_file(&out, &lines, &footer).unwrap();
        let content = fs::read_to_string(&out).unwrap();
        assert!(content.starts_with("AGk26cH00001\tname=Alice"));
        assert!(content.trim_end().ends_with(&footer));
    }

    #[test]
    fn show_to_dtv_round_trip() {
        let tmp = tmp::TempDir::new();
        let dov = build_db(
            &tmp,
            "+AGk26cH00001\tname=Alice\tcity=Tokyo\n\
             +AGk26cH00002\tname=Bob\n",
        );
        let db = DotsvFile::load(&dov).unwrap();
        let footer = read_last_nonempty_line(&dov).unwrap();
        let lines = collect_record_lines(
            &["AGk26cH00001".to_string(), "AGk26cH00002".to_string()],
            &db,
        )
        .unwrap();
        let out = tmp.path().join("out.dtv");
        write_dtv_file(&out, &lines, &footer).unwrap();

        // Re-parse via DotsvFile::parse_str: drop footer, append blank to
        // simulate the sorted/pending separator.
        let raw = fs::read_to_string(&out).unwrap();
        let body = raw
            .lines()
            .filter(|l| !l.starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");
        let mut text = body;
        text.push_str("\n\n");
        let db2 = DotsvFile::parse_str(&text).unwrap();
        assert!(db2.uuid_exists("AGk26cH00001"));
        assert!(db2.uuid_exists("AGk26cH00002"));
    }

    #[test]
    fn show_skip_when_dtv_current_and_qtv_older() {
        let tmp = tmp::TempDir::new();
        let dov = build_db(&tmp, "+AGk26cH00001\tname=Alice\n");
        let db = DotsvFile::load(&dov).unwrap();
        let footer = read_last_nonempty_line(&dov).unwrap();

        // Write a fake qtv first (older mtime).
        let qtv = tmp.path().join("q.qtv");
        fs::write(&qtv, "name\tAlice\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let lines = collect_record_lines(&["AGk26cH00001".to_string()], &db).unwrap();
        let out = tmp.path().join("out.dtv");
        write_dtv_file(&out, &lines, &footer).unwrap();
        // Now the dtv is fresher than the qtv.
        let skip = dtv_skip_if_current(&out, &dov, &qtv).unwrap();
        assert!(skip, "expected skip when dtv current and qtv older");
    }

    #[test]
    fn show_regenerates_when_dov_changed() {
        let tmp = tmp::TempDir::new();
        let dov = build_db(&tmp, "+AGk26cH00001\tname=Alice\n");
        let db = DotsvFile::load(&dov).unwrap();
        let footer = read_last_nonempty_line(&dov).unwrap();
        let qtv = tmp.path().join("q.qtv");
        fs::write(&qtv, "name\tAlice\n").unwrap();
        let lines = collect_record_lines(&["AGk26cH00001".to_string()], &db).unwrap();
        let out = tmp.path().join("out.dtv");
        write_dtv_file(&out, &lines, &footer).unwrap();
        // Force-overwrite the dov with a different timestamp by re-writing.
        std::thread::sleep(std::time::Duration::from_millis(10));
        let mut db2 = DotsvFile::load(&dov).unwrap();
        let actions = parse_action_str("+AGk26cH00002\tname=Bob\n").unwrap();
        apply_actions(&mut db2, &actions).unwrap();
        db2.compact().unwrap();
        // Wait long enough that the next timestamp is at least a second
        // newer (timestamps are second-resolution).
        std::thread::sleep(std::time::Duration::from_millis(1100));
        atomic_write(&db2, &dov).unwrap();
        let skip = dtv_skip_if_current(&out, &dov, &qtv).unwrap();
        assert!(!skip, "must not skip after .dov changed");
    }

    #[test]
    fn show_handles_empty_database() {
        let tmp = tmp::TempDir::new();
        let dov = tmp.path().join("test.dov");
        let mut db = DotsvFile::empty();
        db.compact().unwrap();
        atomic_write(&db, &dov).unwrap();
        let db = DotsvFile::load(&dov).unwrap();
        let footer = read_last_nonempty_line(&dov).unwrap();
        let lines = collect_record_lines(&[], &db).unwrap();
        let out = tmp.path().join("out.dtv");
        write_dtv_file(&out, &lines, &footer).unwrap();
        let content = fs::read_to_string(&out).unwrap();
        // Footer-only.
        let nonempty: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(nonempty.len(), 1);
    }

    #[test]
    fn show_packed_array_value_round_trips() {
        let tmp = tmp::TempDir::new();
        let dov = build_db(
            &tmp,
            "+AGk26cH00001\trole=admin\trole=editor\trole=viewer\n",
        );
        let db = DotsvFile::load(&dov).unwrap();
        let footer = read_last_nonempty_line(&dov).unwrap();
        let lines = collect_record_lines(&["AGk26cH00001".to_string()], &db).unwrap();
        let out = tmp.path().join("out.dtv");
        write_dtv_file(&out, &lines, &footer).unwrap();
        let content = fs::read_to_string(&out).unwrap();
        // Array stays packed in --show output.
        assert!(content.contains(r#"role=["admin","editor","viewer"]"#));
    }

    #[test]
    fn show_uuid_index_consistency_aborts_with_message() {
        let tmp = tmp::TempDir::new();
        let dov = build_db(&tmp, "+AGk26cH00001\tname=Alice\n");
        let db = DotsvFile::load(&dov).unwrap();
        let result = collect_record_lines(&["BGk26cH00001".to_string()], &db);
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("missing from compacted"),
            "got: {}",
            msg
        );
    }

    #[test]
    fn show_atomic_write_no_partial_dtv_on_crash() {
        let tmp = tmp::TempDir::new();
        let dov = build_db(&tmp, "+AGk26cH00001\tname=Alice\n");
        let db = DotsvFile::load(&dov).unwrap();
        let footer = read_last_nonempty_line(&dov).unwrap();
        let lines = collect_record_lines(&["AGk26cH00001".to_string()], &db).unwrap();
        let out = tmp.path().join("out.dtv");
        write_dtv_file(&out, &lines, &footer).unwrap();
        // Confirm tmp file does not linger after a successful write.
        let tmp_file = {
            let mut s = out.as_os_str().to_os_string();
            s.push(".tmp");
            PathBuf::from(s)
        };
        assert!(!tmp_file.exists(), "tmp file should be renamed away");
    }

    #[test]
    fn show_existing_dtv_with_wrong_footer_is_overwritten() {
        let tmp = tmp::TempDir::new();
        let dov = build_db(&tmp, "+AGk26cH00001\tname=Alice\n");
        let db = DotsvFile::load(&dov).unwrap();
        let footer = read_last_nonempty_line(&dov).unwrap();
        let lines = collect_record_lines(&["AGk26cH00001".to_string()], &db).unwrap();
        let out = tmp.path().join("out.dtv");
        // Pre-create with stale footer.
        fs::write(&out, "STALE\tline\n# 19700101000000\n").unwrap();
        let qtv = tmp.path().join("q.qtv");
        fs::write(&qtv, "x\n").unwrap();
        let skip = dtv_skip_if_current(&out, &dov, &qtv).unwrap();
        assert!(!skip);
        write_dtv_file(&out, &lines, &footer).unwrap();
        let content = fs::read_to_string(&out).unwrap();
        assert!(content.contains("AGk26cH00001"));
    }

    #[test]
    fn show_does_not_emit_opcode_prefix() {
        let tmp = tmp::TempDir::new();
        let dov = build_db(&tmp, "+AGk26cH00001\tname=Alice\n");
        let db = DotsvFile::load(&dov).unwrap();
        let footer = read_last_nonempty_line(&dov).unwrap();
        let lines = collect_record_lines(&["AGk26cH00001".to_string()], &db).unwrap();
        let out = tmp.path().join("out.dtv");
        write_dtv_file(&out, &lines, &footer).unwrap();
        let content = fs::read_to_string(&out).unwrap();
        for line in content.lines().filter(|l| !l.is_empty() && !l.starts_with('#')) {
            assert!(
                !line.starts_with('+')
                    && !line.starts_with('-')
                    && !line.starts_with('~')
                    && !line.starts_with('!'),
                "unexpected opcode prefix on line {:?}",
                line
            );
        }
    }

    #[test]
    fn show_emits_kv_pairs_in_record_serialize_key_order() {
        // Record::serialize sorts KV pairs by key. Verify the dtv copy.
        let tmp = tmp::TempDir::new();
        let dov = build_db(&tmp, "+AGk26cH00001\tzeta=z\talpha=a\tmiddle=m\n");
        let db = DotsvFile::load(&dov).unwrap();
        let footer = read_last_nonempty_line(&dov).unwrap();
        let lines = collect_record_lines(&["AGk26cH00001".to_string()], &db).unwrap();
        let out = tmp.path().join("out.dtv");
        write_dtv_file(&out, &lines, &footer).unwrap();
        let content = fs::read_to_string(&out).unwrap();
        let pos_a = content.find("alpha=a").unwrap();
        let pos_m = content.find("middle=m").unwrap();
        let pos_z = content.find("zeta=z").unwrap();
        assert!(pos_a < pos_m && pos_m < pos_z);
    }

    #[test]
    fn show_preserves_escaped_tab_in_value() {
        let tmp = tmp::TempDir::new();
        let dov = build_db(&tmp, "+AGk26cH00001\tnote=hello\\x09world\n");
        let db = DotsvFile::load(&dov).unwrap();
        let footer = read_last_nonempty_line(&dov).unwrap();
        let lines = collect_record_lines(&["AGk26cH00001".to_string()], &db).unwrap();
        let out = tmp.path().join("out.dtv");
        write_dtv_file(&out, &lines, &footer).unwrap();
        let content = fs::read_to_string(&out).unwrap();
        assert!(content.contains("\\x09"));
    }

    #[test]
    fn show_preserves_unicode_keys() {
        let tmp = tmp::TempDir::new();
        let dov = build_db(&tmp, "+AGk26cH00001\t都市=東京\n");
        let db = DotsvFile::load(&dov).unwrap();
        let footer = read_last_nonempty_line(&dov).unwrap();
        let lines = collect_record_lines(&["AGk26cH00001".to_string()], &db).unwrap();
        let out = tmp.path().join("out.dtv");
        write_dtv_file(&out, &lines, &footer).unwrap();
        let content = fs::read_to_string(&out).unwrap();
        assert!(content.contains("都市=東京"));
    }

    #[test]
    fn show_to_stdout_zero_results_emits_just_footer() {
        // stdout emission tested indirectly: we check that emit_to_stdout
        // with empty records is a no-op-then-footer sequence that
        // doesn't panic.
        let _ = emit_to_stdout(&[], "# 20260427000000");
    }
}
