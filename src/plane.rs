/// `--plane` mode: generate `.kv.ptv` and `.vk.ptv` fully-flattened index
/// files from a compacted DOTSV database.
///
/// A `.ptv` file is the "plane" (denormalised) version of a `.rtv` file:
/// every `(col1, col2, uuid)` triple appears on its own line, so an rtv row
/// with an i-element col2 set and j-uuid list for each becomes i*j lines.
///
/// The caller is responsible for compacting and atomically writing the .dov
/// before calling `generate_ptvs`.

use crate::dotsv::{DotsvFile, Record};
use crate::error::{Result, TsdbError};
use crate::escape::escape;
use crate::relate::read_last_nonempty_line;
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

/// Derive the `.kv.ptv` path from a `.dov` path.
/// `target.dov` → `target.kv.ptv`
pub fn kv_ptv_path(dov_path: &Path) -> PathBuf {
    let stem = dov_path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    dov_path.with_file_name(format!("{}.kv.ptv", stem))
}

/// Derive the `.vk.ptv` path from a `.dov` path.
/// `target.dov` → `target.vk.ptv`
pub fn vk_ptv_path(dov_path: &Path) -> PathBuf {
    let stem = dov_path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    dov_path.with_file_name(format!("{}.vk.ptv", stem))
}

/// Generate (or update) `<target>.kv.ptv` and `<target>.vk.ptv` from `db`.
///
/// Skip condition: if both .ptv files exist and their last lines match the
/// .dov's last line, generation is skipped.
pub fn generate_ptvs(dov_path: &Path, db: &DotsvFile) -> Result<()> {
    if !db.pending.is_empty() {
        return Err(TsdbError::Other(
            "generate_ptvs requires a fully compacted DotsvFile (pending section must be empty)"
                .to_string(),
        ));
    }

    let kv_path = kv_ptv_path(dov_path);
    let vk_path = vk_ptv_path(dov_path);

    let dov_ts = read_last_nonempty_line(dov_path)?;

    if kv_path.exists() && vk_path.exists() {
        let kv_ts = read_last_nonempty_line(&kv_path).unwrap_or_default();
        let vk_ts = read_last_nonempty_line(&vk_path).unwrap_or_default();
        if kv_ts == dov_ts && vk_ts == dov_ts {
            return Ok(());
        }
    }

    let (kv_rows, vk_rows) = build_plane_rows(db)?;
    write_ptv_file(&kv_path, &kv_rows, &dov_ts)?;
    write_ptv_file(&vk_path, &vk_rows, &dov_ts)?;

    Ok(())
}

/// Build fully-flattened triples from the sorted section.
///
/// Returns:
///   - `kv_rows`: sorted by (key, value, uuid)
///   - `vk_rows`: sorted by (value, key, uuid)
fn build_plane_rows(
    db: &DotsvFile,
) -> Result<(
    Vec<(String, String, String)>,
    Vec<(String, String, String)>,
)> {
    let mut kv_set: BTreeSet<(String, String, String)> = BTreeSet::new();
    let mut vk_set: BTreeSet<(String, String, String)> = BTreeSet::new();

    for (i, line) in db.sorted.iter().enumerate() {
        if line.is_empty() {
            continue;
        }
        let rec = Record::parse(line, i + 1)?;
        for (k, v) in &rec.fields {
            kv_set.insert((k.clone(), v.clone(), rec.uuid.clone()));
            vk_set.insert((v.clone(), k.clone(), rec.uuid.clone()));
        }
    }

    Ok((
        kv_set.into_iter().collect(),
        vk_set.into_iter().collect(),
    ))
}

/// Write flattened triples to a `.ptv` file, followed by the timestamp footer.
fn write_ptv_file(
    path: &Path,
    rows: &[(String, String, String)],
    timestamp: &str,
) -> Result<()> {
    let file = File::create(path)?;
    let mut w = BufWriter::new(file);
    for (col1, col2, uuid) in rows {
        w.write_all(escape(col1).as_bytes())?;
        w.write_all(b"\t")?;
        w.write_all(escape(col2).as_bytes())?;
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
    use std::fs;

    mod tmp {
        use std::path::{Path, PathBuf};
        pub struct TempDir {
            path: PathBuf,
        }
        impl TempDir {
            pub fn new() -> Self {
                let path = std::env::temp_dir().join(format!(
                    "tsdb_plane_test_{:016x}",
                    rand::random::<u64>()
                ));
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
    fn test_kv_ptv_path() {
        let p = Path::new("/data/store.dov");
        assert_eq!(kv_ptv_path(p), Path::new("/data/store.kv.ptv"));
    }

    #[test]
    fn test_vk_ptv_path() {
        let p = Path::new("/data/store.dov");
        assert_eq!(vk_ptv_path(p), Path::new("/data/store.vk.ptv"));
    }

    #[test]
    fn test_generate_ptvs_creates_files() {
        let tmp = tmp::TempDir::new();
        let dov = make_test_db(&tmp);
        let db = DotsvFile::load(&dov).unwrap();
        generate_ptvs(&dov, &db).unwrap();

        assert!(kv_ptv_path(&dov).exists());
        assert!(vk_ptv_path(&dov).exists());
    }

    #[test]
    fn test_kv_ptv_is_flattened() {
        let tmp = tmp::TempDir::new();
        let dov = make_test_db(&tmp);
        let db = DotsvFile::load(&dov).unwrap();
        generate_ptvs(&dov, &db).unwrap();

        let content = fs::read_to_string(kv_ptv_path(&dov)).unwrap();
        // city/Tokyo has 2 UUIDs → 2 separate rows (not a comma list)
        assert!(content.contains("city\tTokyo\tAGk26cH00001\n"));
        assert!(content.contains("city\tTokyo\tAGk26cH00002\n"));
        // No comma-joined uuid list anywhere
        assert!(
            !content.contains(","),
            "ptv must not contain comma-joined uuid lists: {}",
            content
        );
    }

    #[test]
    fn test_kv_ptv_row_count() {
        // 3 records × 2 fields each = 6 (key,value,uuid) triples
        let tmp = tmp::TempDir::new();
        let dov = make_test_db(&tmp);
        let db = DotsvFile::load(&dov).unwrap();
        generate_ptvs(&dov, &db).unwrap();

        let content = fs::read_to_string(kv_ptv_path(&dov)).unwrap();
        let data_rows = content
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .count();
        assert_eq!(data_rows, 6);
    }

    #[test]
    fn test_vk_ptv_is_flattened() {
        let tmp = tmp::TempDir::new();
        let dov = make_test_db(&tmp);
        let db = DotsvFile::load(&dov).unwrap();
        generate_ptvs(&dov, &db).unwrap();

        let content = fs::read_to_string(vk_ptv_path(&dov)).unwrap();
        assert!(content.contains("Tokyo\tcity\tAGk26cH00001\n"));
        assert!(content.contains("Tokyo\tcity\tAGk26cH00002\n"));
        assert!(!content.contains(","));
    }

    #[test]
    fn test_ptv_timestamp_matches_dov() {
        let tmp = tmp::TempDir::new();
        let dov = make_test_db(&tmp);
        let db = DotsvFile::load(&dov).unwrap();
        generate_ptvs(&dov, &db).unwrap();

        let dov_ts = read_last_nonempty_line(&dov).unwrap();
        let kv_ts = read_last_nonempty_line(&kv_ptv_path(&dov)).unwrap();
        let vk_ts = read_last_nonempty_line(&vk_ptv_path(&dov)).unwrap();
        assert_eq!(kv_ts, dov_ts);
        assert_eq!(vk_ts, dov_ts);
    }

    #[test]
    fn test_generate_ptvs_skip_when_current() {
        let tmp = tmp::TempDir::new();
        let dov = make_test_db(&tmp);
        let db = DotsvFile::load(&dov).unwrap();

        generate_ptvs(&dov, &db).unwrap();
        let mtime1 = fs::metadata(kv_ptv_path(&dov)).unwrap().modified().unwrap();

        generate_ptvs(&dov, &db).unwrap();
        let mtime2 = fs::metadata(kv_ptv_path(&dov)).unwrap().modified().unwrap();
        assert_eq!(mtime1, mtime2);
    }

    #[test]
    fn test_generate_ptvs_rejects_pending() {
        let tmp = tmp::TempDir::new();
        let dov = tmp.path().join("test.dov");
        let mut db = DotsvFile::empty();
        let actions = parse_action_str("+AGk26cH00001\tname=Alice\n").unwrap();
        apply_actions(&mut db, &actions).unwrap();
        // Do not compact; write so the dov file exists for timestamp read
        atomic_write(&db, &dov).unwrap();
        assert!(generate_ptvs(&dov, &db).is_err());
    }
}
