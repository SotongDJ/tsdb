/// `--relate` mode: generate `.kv.rtv` and `.vk.rtv` inverted-index files
/// from a compacted DOTSV database.
///
/// The caller is responsible for compacting and atomically writing the .dov
/// before calling `generate_rtvs`. This module only reads from the file system
/// and writes the two index files.
use crate::dotsv::{DotsvFile, Record};
use crate::error::{Result, TsdbError};
use crate::escape::escape;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

/// Derive the `.kv.rtv` path from a `.dov` path.
/// `target.dov` → `target.kv.rtv`
pub fn kv_rtv_path(dov_path: &Path) -> PathBuf {
    let stem = dov_path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    dov_path.with_file_name(format!("{}.kv.rtv", stem))
}

/// Derive the `.vk.rtv` path from a `.dov` path.
/// `target.dov` → `target.vk.rtv`
pub fn vk_rtv_path(dov_path: &Path) -> PathBuf {
    let stem = dov_path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    dov_path.with_file_name(format!("{}.vk.rtv", stem))
}

/// Derive the `.uuid.rtv` path from a `.dov` path.
/// `target.dov` → `target.uuid.rtv`
pub fn uuid_rtv_path(dov_path: &Path) -> PathBuf {
    let stem = dov_path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    dov_path.with_file_name(format!("{}.uuid.rtv", stem))
}

/// Generate (or update) `<target>.kv.rtv` and `<target>.vk.rtv` from `db`.
///
/// `dov_path` is used to:
///   1. Derive the output file paths.
///   2. Read the timestamp footer from the just-written .dov (for skip check
///      and for writing the matching footer in the .rtv files).
///
/// Skip condition: if both .rtv files already exist and their last lines
/// exactly match the .dov's last line, generation is skipped.
pub fn generate_rtvs(dov_path: &Path, db: &DotsvFile) -> Result<()> {
    // Indexes are built solely from the sorted section. Pending records would
    // be silently omitted, so we require the caller to compact first.
    if !db.pending.is_empty() {
        return Err(TsdbError::Other(
            "generate_rtvs requires a fully compacted DotsvFile (pending section must be empty)"
                .to_string(),
        ));
    }

    let kv_path = kv_rtv_path(dov_path);
    let vk_path = vk_rtv_path(dov_path);
    let uuid_path = uuid_rtv_path(dov_path);

    let dov_ts = read_last_nonempty_line(dov_path)?;

    // Skip if all three indexes are already current.
    if kv_path.exists() && vk_path.exists() && uuid_path.exists() {
        let kv_ts = read_last_nonempty_line(&kv_path).unwrap_or_default();
        let vk_ts = read_last_nonempty_line(&vk_path).unwrap_or_default();
        let uuid_ts = read_last_nonempty_line(&uuid_path).unwrap_or_default();
        if kv_ts == dov_ts && vk_ts == dov_ts && uuid_ts == dov_ts {
            return Ok(());
        }
    }

    let (kv_rows, vk_rows) = build_index_rows(db)?;
    let uuids = collect_uuids(db)?;
    write_rtv_file(&kv_path, &kv_rows, &dov_ts)?;
    write_rtv_file(&vk_path, &vk_rows, &dov_ts)?;
    write_uuid_file(&uuid_path, &uuids, &dov_ts)?;

    Ok(())
}

/// Collect the sorted list of UUIDs from the sorted section.
fn collect_uuids(db: &DotsvFile) -> Result<Vec<String>> {
    let mut uuids: BTreeSet<String> = BTreeSet::new();
    for (i, line) in db.sorted.iter().enumerate() {
        if line.is_empty() {
            continue;
        }
        let rec = Record::parse(line, i + 1)?;
        uuids.insert(rec.uuid);
    }
    Ok(uuids.into_iter().collect())
}

/// Write a sorted list of UUIDs to a `.uuid.rtv` file, one per line,
/// followed by the timestamp footer.
fn write_uuid_file(path: &Path, uuids: &[String], timestamp: &str) -> Result<()> {
    let file = File::create(path)?;
    let mut w = BufWriter::new(file);
    for uuid in uuids {
        w.write_all(uuid.as_bytes())?;
        w.write_all(b"\n")?;
    }
    w.write_all(timestamp.as_bytes())?;
    w.write_all(b"\n")?;
    w.flush()?;
    Ok(())
}

/// Read the last non-empty, non-whitespace line from a file.
pub fn read_last_nonempty_line(path: &Path) -> Result<String> {
    let content = fs::read_to_string(path)?;
    content
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.to_string())
        .ok_or_else(|| TsdbError::Other(format!("file has no content: {}", path.display())))
}

/// Build sorted index rows from the sorted section of a DotsvFile.
///
/// Returns:
///   - `kv_rows`: sorted by (key, value); each row is (key, value, uuid_list)
///   - `vk_rows`: sorted by (value, key); each row is (value, key, uuid_list)
fn build_index_rows(
    db: &DotsvFile,
) -> Result<(
    Vec<(String, String, Vec<String>)>,
    Vec<(String, String, Vec<String>)>,
)> {
    // BTreeMap preserves sorted order; BTreeSet keeps UUIDs sorted.
    let mut kv_map: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
    let mut vk_map: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();

    for (i, line) in db.sorted.iter().enumerate() {
        if line.is_empty() {
            continue;
        }
        let rec = Record::parse(line, i + 1)?;
        for (k, v) in &rec.fields {
            kv_map
                .entry((k.clone(), v.clone()))
                .or_default()
                .insert(rec.uuid.clone());
            vk_map
                .entry((v.clone(), k.clone()))
                .or_default()
                .insert(rec.uuid.clone());
        }
    }

    let kv_rows = kv_map
        .into_iter()
        .map(|((k, v), uuids)| (k, v, uuids.into_iter().collect()))
        .collect();

    let vk_rows = vk_map
        .into_iter()
        .map(|((v, k), uuids)| (v, k, uuids.into_iter().collect()))
        .collect();

    Ok((kv_rows, vk_rows))
}

/// Write a sorted set of index rows to a `.rtv` file, followed by the
/// timestamp footer line.
fn write_rtv_file(
    path: &Path,
    rows: &[(String, String, Vec<String>)],
    timestamp: &str,
) -> Result<()> {
    let file = File::create(path)?;
    let mut w = BufWriter::new(file);
    for (col1, col2, uuids) in rows {
        // Escape col1 and col2 so that tabs/newlines/backslashes in key or
        // value strings do not corrupt the three-column row structure.
        w.write_all(escape(col1).as_bytes())?;
        w.write_all(b"\t")?;
        w.write_all(escape(col2).as_bytes())?;
        w.write_all(b"\t")?;
        w.write_all(uuids.join(",").as_bytes())?;
        w.write_all(b"\n")?;
    }
    // Timestamp footer — must match the source .dov footer exactly.
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
    use std::fs;

    mod tmp {
        use std::path::{Path, PathBuf};
        pub struct TempDir {
            path: PathBuf,
        }
        impl TempDir {
            pub fn new() -> Self {
                let path = std::env::temp_dir()
                    .join(format!("tsdb_relate_test_{:016x}", rand::random::<u64>()));
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

    fn make_test_db(tmp: &tmp::TempDir) -> std::path::PathBuf {
        let dov = tmp.path().join("test.dov");
        let mut db = DotsvFile::empty();
        let actions = parse_action_str(
            "+AGk26cH00001\tname=Alice\tcity=Tokyo\n\
             +AGk26cH00002\tname=Bob\tcity=Tokyo\n\
             +AGk26cH00003\tname=Carol\tcity=London\n",
        )
        .unwrap();
        apply_actions(&mut db, &actions).unwrap();
        db.compact().unwrap();
        atomic_write(&db, &dov).unwrap();
        dov
    }

    #[test]
    fn test_kv_rtv_path() {
        let p = Path::new("/data/store.dov");
        assert_eq!(kv_rtv_path(p), Path::new("/data/store.kv.rtv"));
    }

    #[test]
    fn test_vk_rtv_path() {
        let p = Path::new("/data/store.dov");
        assert_eq!(vk_rtv_path(p), Path::new("/data/store.vk.rtv"));
    }

    #[test]
    fn test_generate_rtvs_creates_files() {
        let tmp = tmp::TempDir::new();
        let dov = make_test_db(&tmp);
        let db = DotsvFile::load(&dov).unwrap();
        generate_rtvs(&dov, &db).unwrap();

        assert!(kv_rtv_path(&dov).exists(), "kv.rtv should exist");
        assert!(vk_rtv_path(&dov).exists(), "vk.rtv should exist");
    }

    #[test]
    fn test_kv_rtv_content() {
        let tmp = tmp::TempDir::new();
        let dov = make_test_db(&tmp);
        let db = DotsvFile::load(&dov).unwrap();
        generate_rtvs(&dov, &db).unwrap();

        let content = fs::read_to_string(kv_rtv_path(&dov)).unwrap();
        // city=London should map to Carol only
        assert!(
            content.contains("city\tLondon\tAGk26cH00003"),
            "kv.rtv missing city/London row: {}",
            content
        );
        // city=Tokyo should list both Alice and Bob (sorted)
        assert!(
            content.contains("city\tTokyo\tAGk26cH00001,AGk26cH00002"),
            "kv.rtv missing city/Tokyo row: {}",
            content
        );
    }

    #[test]
    fn test_vk_rtv_content() {
        let tmp = tmp::TempDir::new();
        let dov = make_test_db(&tmp);
        let db = DotsvFile::load(&dov).unwrap();
        generate_rtvs(&dov, &db).unwrap();

        let content = fs::read_to_string(vk_rtv_path(&dov)).unwrap();
        // Value "Tokyo" should appear as first column with key "city"
        assert!(
            content.contains("Tokyo\tcity\tAGk26cH00001,AGk26cH00002"),
            "vk.rtv missing Tokyo/city row: {}",
            content
        );
    }

    #[test]
    fn test_generate_rtvs_timestamp_matches_dov() {
        let tmp = tmp::TempDir::new();
        let dov = make_test_db(&tmp);
        let db = DotsvFile::load(&dov).unwrap();
        generate_rtvs(&dov, &db).unwrap();

        let dov_ts = read_last_nonempty_line(&dov).unwrap();
        let kv_ts = read_last_nonempty_line(&kv_rtv_path(&dov)).unwrap();
        let vk_ts = read_last_nonempty_line(&vk_rtv_path(&dov)).unwrap();
        assert_eq!(kv_ts, dov_ts, "kv.rtv timestamp must match .dov");
        assert_eq!(vk_ts, dov_ts, "vk.rtv timestamp must match .dov");
    }

    #[test]
    fn test_uuid_rtv_path() {
        let p = Path::new("/data/store.dov");
        assert_eq!(uuid_rtv_path(p), Path::new("/data/store.uuid.rtv"));
    }

    #[test]
    fn test_uuid_rtv_content() {
        let tmp = tmp::TempDir::new();
        let dov = make_test_db(&tmp);
        let db = DotsvFile::load(&dov).unwrap();
        generate_rtvs(&dov, &db).unwrap();

        let content = fs::read_to_string(uuid_rtv_path(&dov)).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        // 3 UUID lines + 1 timestamp footer
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0], "AGk26cH00001");
        assert_eq!(lines[1], "AGk26cH00002");
        assert_eq!(lines[2], "AGk26cH00003");
    }

    #[test]
    fn test_uuid_rtv_timestamp_matches_dov() {
        let tmp = tmp::TempDir::new();
        let dov = make_test_db(&tmp);
        let db = DotsvFile::load(&dov).unwrap();
        generate_rtvs(&dov, &db).unwrap();

        let dov_ts = read_last_nonempty_line(&dov).unwrap();
        let uuid_ts = read_last_nonempty_line(&uuid_rtv_path(&dov)).unwrap();
        assert_eq!(uuid_ts, dov_ts);
    }

    #[test]
    fn test_generate_rtvs_skip_when_current() {
        let tmp = tmp::TempDir::new();
        let dov = make_test_db(&tmp);
        let db = DotsvFile::load(&dov).unwrap();

        generate_rtvs(&dov, &db).unwrap();
        let kv_mtime = fs::metadata(kv_rtv_path(&dov)).unwrap().modified().unwrap();

        // Second call should skip (timestamps match)
        generate_rtvs(&dov, &db).unwrap();
        let kv_mtime2 = fs::metadata(kv_rtv_path(&dov)).unwrap().modified().unwrap();
        assert_eq!(
            kv_mtime, kv_mtime2,
            "kv.rtv should not be rewritten when current"
        );
    }
}
