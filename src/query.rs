/// `--query` mode: filter records in a DOTSV database using a `.qtv` query file.
///
/// A `.qtv` file defines filter criteria matched against the `.kv.rtv` and
/// `.vk.rtv` indexes produced by `--relate`. The caller must run `--relate`
/// before calling `run_query` so the indexes are current.

use crate::error::{Result, TsdbError};
use crate::escape::unescape;
use crate::relate::{kv_rtv_path, vk_rtv_path};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

/// Filter combination mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FilterMode {
    /// A UUID is included if it satisfies **at least one** criterion.
    Union,
    /// A UUID is included only if it satisfies **all** criteria. (default)
    Intersect,
}

/// A single query criterion.
#[derive(Debug, Clone)]
enum Criterion {
    /// A bare token (no tab): search both kv index (col1=key) and vk index (col1=value).
    Token(String),
    /// A key\tvalue pair: search kv index for exact (key, value) match.
    KeyValue(String, String),
}

/// In-memory representation of a `.rtv` index.
/// Outer key = column 1, inner key = column 2, value = UUID list.
type RtvIndex = HashMap<String, HashMap<String, Vec<String>>>;

/// Execute a `.qtv` query against `dov_path` and print matching UUIDs to stdout.
///
/// The `.kv.rtv` and `.vk.rtv` indexes for `dov_path` must already be current
/// (caller must have run `--relate` first).
pub fn run_query(qtv_path: &Path, dov_path: &Path) -> Result<()> {
    if !qtv_path.exists() {
        return Err(TsdbError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("query file not found: {}", qtv_path.display()),
        )));
    }

    let kv_path = kv_rtv_path(dov_path);
    let vk_path = vk_rtv_path(dov_path);

    if !kv_path.exists() || !vk_path.exists() {
        return Err(TsdbError::Other(format!(
            "index files not found for {}; run --relate first",
            dov_path.display()
        )));
    }

    let (mode, criteria) = parse_qtv(qtv_path)?;

    if criteria.is_empty() {
        return Ok(());
    }

    let kv_index = load_rtv_index(&kv_path)?;
    let vk_index = load_rtv_index(&vk_path)?;

    let mut result = execute_query(&criteria, mode, &kv_index, &vk_index);
    result.sort();

    for uuid in &result {
        println!("{}", uuid);
    }

    Ok(())
}

/// Parse a `.qtv` file into a filter mode and a list of criteria.
fn parse_qtv(path: &Path) -> Result<(FilterMode, Vec<Criterion>)> {
    let content = fs::read_to_string(path).map_err(|e| {
        TsdbError::Io(std::io::Error::new(
            e.kind(),
            format!("cannot read query file {}: {}", path.display(), e),
        ))
    })?;

    let mut mode = FilterMode::Intersect;
    let mut criteria: Vec<Criterion> = Vec::new();
    let mut mode_checked = false;

    for line in content.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }

        // Check for mode declaration on the very first non-blank line.
        if !mode_checked {
            mode_checked = true;
            if let Some(m) = try_parse_mode_decl(line) {
                mode = m;
                continue;
            }
            // Not a mode declaration: fall through to criterion parsing below.
        }

        // Skip remaining comment lines.
        if line.starts_with('#') {
            continue;
        }

        // Criterion: key\tvalue or bare token.
        if let Some(tab_pos) = line.find('\t') {
            let key = line[..tab_pos].to_string();
            let value = line[tab_pos + 1..].to_string();
            criteria.push(Criterion::KeyValue(key, value));
        } else {
            criteria.push(Criterion::Token(line.to_string()));
        }
    }

    Ok((mode, criteria))
}

/// Parse `# mode\t<union|intersect>` from a line, returning `Some(mode)` on
/// a valid declaration or `None` otherwise.
fn try_parse_mode_decl(line: &str) -> Option<FilterMode> {
    // Must start with "# mode\t"
    let rest = line.strip_prefix("# mode\t")?;
    match rest {
        "union" => Some(FilterMode::Union),
        "intersect" => Some(FilterMode::Intersect),
        _ => None,
    }
}

/// Load a `.rtv` file into a two-level HashMap (col1 → col2 → uuid_list).
fn load_rtv_index(path: &Path) -> Result<RtvIndex> {
    let content = fs::read_to_string(path)?;
    let mut index: RtvIndex = HashMap::new();

    for line in content.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(3, '\t');
        let col1_raw = match parts.next() {
            Some(s) => s,
            None => continue,
        };
        let col2_raw = match parts.next() {
            Some(s) => s,
            None => continue,
        };
        let col3 = match parts.next() {
            Some(s) => s,
            None => continue,
        };
        // Unescape col1 and col2 to recover the original key/value strings.
        let col1 = unescape(col1_raw).unwrap_or_else(|_| col1_raw.to_string());
        let col2 = unescape(col2_raw).unwrap_or_else(|_| col2_raw.to_string());
        let uuids: Vec<String> = col3
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
        index
            .entry(col1)
            .or_default()
            .insert(col2, uuids);
    }

    Ok(index)
}

/// Resolve a single criterion against the indexes and return the UUID set.
fn resolve_criterion(
    criterion: &Criterion,
    kv_index: &RtvIndex,
    vk_index: &RtvIndex,
) -> HashSet<String> {
    let mut uuids: HashSet<String> = HashSet::new();
    match criterion {
        Criterion::Token(token) => {
            // Token searched as a key in kv index (col1 = key)
            if let Some(val_map) = kv_index.get(token) {
                for uid_list in val_map.values() {
                    uuids.extend(uid_list.iter().cloned());
                }
            }
            // Token searched as a value in vk index (col1 = value)
            if let Some(key_map) = vk_index.get(token) {
                for uid_list in key_map.values() {
                    uuids.extend(uid_list.iter().cloned());
                }
            }
        }
        Criterion::KeyValue(key, value) => {
            // Exact (key, value) lookup in kv index
            if let Some(val_map) = kv_index.get(key) {
                if let Some(uid_list) = val_map.get(value) {
                    uuids.extend(uid_list.iter().cloned());
                }
            }
        }
    }
    uuids
}

/// Apply all criteria and combine results according to the filter mode.
fn execute_query(
    criteria: &[Criterion],
    mode: FilterMode,
    kv_index: &RtvIndex,
    vk_index: &RtvIndex,
) -> Vec<String> {
    let mut result: Option<HashSet<String>> = None;

    for criterion in criteria {
        let set = resolve_criterion(criterion, kv_index, vk_index);
        result = Some(match result {
            None => set,
            Some(acc) => match mode {
                FilterMode::Union => acc.union(&set).cloned().collect(),
                FilterMode::Intersect => acc.intersection(&set).cloned().collect(),
            },
        });
    }

    let mut out: Vec<String> = result.unwrap_or_default().into_iter().collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::parse_action_str;
    use crate::dotsv::{apply_actions, atomic_write, DotsvFile};
    use crate::relate::generate_rtvs;
    use std::fs;

    mod tmp {
        use std::path::{Path, PathBuf};
        pub struct TempDir {
            path: PathBuf,
        }
        impl TempDir {
            pub fn new() -> Self {
                let path = std::env::temp_dir().join(format!(
                    "tsdb_query_test_{:016x}",
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

    fn make_test_db_with_indexes(tmp: &tmp::TempDir) -> std::path::PathBuf {
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
        let db = DotsvFile::load(&dov).unwrap();
        generate_rtvs(&dov, &db).unwrap();
        dov
    }

    #[test]
    fn test_parse_qtv_default_intersect() {
        let tmp = tmp::TempDir::new();
        let qtv = tmp.path().join("q.qtv");
        fs::write(&qtv, "city\tTokyo\n").unwrap();
        let (mode, crit) = parse_qtv(&qtv).unwrap();
        assert_eq!(mode, FilterMode::Intersect);
        assert_eq!(crit.len(), 1);
    }

    #[test]
    fn test_parse_qtv_union_mode() {
        let tmp = tmp::TempDir::new();
        let qtv = tmp.path().join("q.qtv");
        fs::write(&qtv, "# mode\tunion\ncity\tTokyo\n").unwrap();
        let (mode, crit) = parse_qtv(&qtv).unwrap();
        assert_eq!(mode, FilterMode::Union);
        assert_eq!(crit.len(), 1);
    }

    #[test]
    fn test_query_key_value_intersect() {
        let tmp = tmp::TempDir::new();
        let dov = make_test_db_with_indexes(&tmp);
        let qtv = tmp.path().join("q.qtv");
        // Only Alice is in Tokyo with name=Alice
        fs::write(&qtv, "# mode\tintersect\ncity\tTokyo\nname\tAlice\n").unwrap();

        run_query(&qtv, &dov).unwrap();
        // Result checked by running the function without error;
        // actual stdout verified by integration test.
    }

    #[test]
    fn test_query_token_searches_both_indexes() {
        let tmp = tmp::TempDir::new();
        let dov = make_test_db_with_indexes(&tmp);
        let qtv = tmp.path().join("q.qtv");
        // "city" is a key; should find all records that have a "city" key
        fs::write(&qtv, "city\n").unwrap();

        let (mode, criteria) = parse_qtv(&qtv).unwrap();
        let kv = load_rtv_index(&kv_rtv_path(&dov)).unwrap();
        let vk = load_rtv_index(&vk_rtv_path(&dov)).unwrap();
        let result = execute_query(&criteria, mode, &kv, &vk);
        // All three records have a "city" key
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_query_union_mode() {
        let tmp = tmp::TempDir::new();
        let dov = make_test_db_with_indexes(&tmp);
        let qtv = tmp.path().join("q.qtv");
        // London OR name=Bob → Carol + Bob
        fs::write(&qtv, "# mode\tunion\ncity\tLondon\nname\tBob\n").unwrap();

        let (mode, criteria) = parse_qtv(&qtv).unwrap();
        let kv = load_rtv_index(&kv_rtv_path(&dov)).unwrap();
        let vk = load_rtv_index(&vk_rtv_path(&dov)).unwrap();
        let result = execute_query(&criteria, mode, &kv, &vk);
        assert_eq!(result.len(), 2);
        assert!(result.contains(&"AGk26cH00002".to_string())); // Bob
        assert!(result.contains(&"AGk26cH00003".to_string())); // Carol
    }

    #[test]
    fn test_query_intersect_empty_when_no_match() {
        let tmp = tmp::TempDir::new();
        let dov = make_test_db_with_indexes(&tmp);
        let qtv = tmp.path().join("q.qtv");
        // No record is in both Tokyo and London
        fs::write(&qtv, "# mode\tintersect\ncity\tTokyo\ncity\tLondon\n").unwrap();

        let (mode, criteria) = parse_qtv(&qtv).unwrap();
        let kv = load_rtv_index(&kv_rtv_path(&dov)).unwrap();
        let vk = load_rtv_index(&vk_rtv_path(&dov)).unwrap();
        let result = execute_query(&criteria, mode, &kv, &vk);
        assert!(result.is_empty());
    }
}
