/// `--query` mode: filter records in a DOTSV database using a `.qtv` query file.
///
/// A `.qtv` file defines filter criteria matched against the `.kv.rtv` and
/// `.vk.rtv` indexes produced by `--relate`. The caller must run `--relate`
/// before calling `run_query` so the indexes are current.
use crate::error::{Result, TsdbError};
use crate::escape::unescape;
use crate::relate::{kv_rtv_path, uuid_rtv_path, vk_rtv_path};
use std::collections::{BTreeSet, HashMap, HashSet};
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
    /// `@present\tkey` — UUIDs that have at least one binding for `key`.
    Present(String),
    /// `@absent\tkey` — UUIDs that have NO binding for `key`.
    AbsentKey(String),
    /// `@absent\tkey\tvalue` — UUIDs lacking the exact `key=value` pair
    /// (records lacking `key` entirely OR with `key` set to a different value).
    AbsentKeyValue(String, String),
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

    // Lazy: only load uuid.rtv when at least one absence criterion exists.
    let universe: Option<BTreeSet<String>> = if criteria_need_universe(&criteria) {
        Some(load_uuid_universe(&uuid_rtv_path(dov_path))?)
    } else {
        None
    };

    let mut result = execute_query(&criteria, mode, &kv_index, &vk_index, universe.as_ref());
    result.sort();

    for uuid in &result {
        println!("{}", uuid);
    }

    Ok(())
}

/// Resolve a `.qtv` query and return the matching UUIDs (sorted),
/// without printing. Used by `--query --show` so the caller can fetch
/// records from the `.dov` and emit them.
pub fn resolve_query_uuids(qtv_path: &Path, dov_path: &Path) -> Result<Vec<String>> {
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
        return Ok(Vec::new());
    }

    let kv_index = load_rtv_index(&kv_path)?;
    let vk_index = load_rtv_index(&vk_path)?;
    let universe: Option<BTreeSet<String>> = if criteria_need_universe(&criteria) {
        Some(load_uuid_universe(&uuid_rtv_path(dov_path))?)
    } else {
        None
    };
    let mut result = execute_query(&criteria, mode, &kv_index, &vk_index, universe.as_ref());
    result.sort();
    Ok(result)
}

/// True iff any criterion needs the full UUID universe (`@absent` forms).
fn criteria_need_universe(criteria: &[Criterion]) -> bool {
    criteria
        .iter()
        .any(|c| matches!(c, Criterion::AbsentKey(_) | Criterion::AbsentKeyValue(_, _)))
}

/// Load the sorted UUID list from `<stem>.uuid.rtv` (without the
/// timestamp footer comment).
fn load_uuid_universe(path: &Path) -> Result<BTreeSet<String>> {
    let content = fs::read_to_string(path).map_err(|e| {
        TsdbError::Io(std::io::Error::new(
            e.kind(),
            format!("cannot read uuid.rtv {}: {}", path.display(), e),
        ))
    })?;
    let mut set: BTreeSet<String> = BTreeSet::new();
    for line in content.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        set.insert(line.to_string());
    }
    Ok(set)
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

        // Reserved-sigil directives (`@present` / `@absent`).
        // First column is delimited by tab. Examples:
        //   @present\tkey          → Present
        //   @absent\tkey           → AbsentKey
        //   @absent\tkey\tvalue    → AbsentKeyValue
        // Any other `@`-prefixed first column is a parse error.
        if line.starts_with('@') {
            // Determine the directive name (first column up to first tab).
            let (head, rest) = match line.find('\t') {
                Some(t) => (&line[..t], Some(&line[t + 1..])),
                None => (&line[..], None),
            };
            match head {
                "@present" => {
                    let r = rest.ok_or_else(|| TsdbError::ParseError {
                        line: 0,
                        message: "@present requires a key".to_string(),
                    })?;
                    if r.contains('\t') {
                        return Err(TsdbError::ParseError {
                            line: 0,
                            message: format!(
                                "@present takes one column (a key); got: {:?}",
                                line
                            ),
                        });
                    }
                    let key = unescape(r).map_err(|e| TsdbError::ParseError {
                        line: 0,
                        message: format!("@present key unescape error: {}", e),
                    })?;
                    criteria.push(Criterion::Present(key));
                    continue;
                }
                "@absent" => {
                    let r = rest.ok_or_else(|| TsdbError::ParseError {
                        line: 0,
                        message: "@absent requires a key".to_string(),
                    })?;
                    if let Some(tab2) = r.find('\t') {
                        let key_raw = &r[..tab2];
                        let val_raw = &r[tab2 + 1..];
                        if val_raw.contains('\t') {
                            return Err(TsdbError::ParseError {
                                line: 0,
                                message: format!(
                                    "@absent takes 1 or 2 columns; got: {:?}",
                                    line
                                ),
                            });
                        }
                        let key = unescape(key_raw).map_err(|e| TsdbError::ParseError {
                            line: 0,
                            message: format!("@absent key unescape error: {}", e),
                        })?;
                        let value = unescape(val_raw).map_err(|e| TsdbError::ParseError {
                            line: 0,
                            message: format!("@absent value unescape error: {}", e),
                        })?;
                        criteria.push(Criterion::AbsentKeyValue(key, value));
                    } else {
                        let key = unescape(r).map_err(|e| TsdbError::ParseError {
                            line: 0,
                            message: format!("@absent key unescape error: {}", e),
                        })?;
                        criteria.push(Criterion::AbsentKey(key));
                    }
                    continue;
                }
                other => {
                    return Err(TsdbError::ParseError {
                        line: 0,
                        message: format!("unknown qtv directive {:?}", other),
                    });
                }
            }
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
        index.entry(col1).or_default().insert(col2, uuids);
    }

    Ok(index)
}

/// Resolve a single criterion against the indexes and return the UUID set.
fn resolve_criterion(
    criterion: &Criterion,
    kv_index: &RtvIndex,
    vk_index: &RtvIndex,
    universe: Option<&BTreeSet<String>>,
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
        Criterion::Present(key) => {
            if let Some(val_map) = kv_index.get(key) {
                for uid_list in val_map.values() {
                    uuids.extend(uid_list.iter().cloned());
                }
            }
        }
        Criterion::AbsentKey(key) => {
            // universe − Present(key)
            if let Some(u) = universe {
                let mut present: HashSet<String> = HashSet::new();
                if let Some(val_map) = kv_index.get(key) {
                    for uid_list in val_map.values() {
                        present.extend(uid_list.iter().cloned());
                    }
                }
                for uid in u {
                    if !present.contains(uid) {
                        uuids.insert(uid.clone());
                    }
                }
            }
        }
        Criterion::AbsentKeyValue(key, value) => {
            // universe − exact(key=value)
            if let Some(u) = universe {
                let mut exact: HashSet<String> = HashSet::new();
                if let Some(val_map) = kv_index.get(key) {
                    if let Some(uid_list) = val_map.get(value) {
                        exact.extend(uid_list.iter().cloned());
                    }
                }
                for uid in u {
                    if !exact.contains(uid) {
                        uuids.insert(uid.clone());
                    }
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
    universe: Option<&BTreeSet<String>>,
) -> Vec<String> {
    let mut result: Option<HashSet<String>> = None;

    for criterion in criteria {
        let set = resolve_criterion(criterion, kv_index, vk_index, universe);
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
                let path = std::env::temp_dir()
                    .join(format!("tsdb_query_test_{:016x}", rand::random::<u64>()));
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
        let result = execute_query(&criteria, mode, &kv, &vk, None);
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
        let result = execute_query(&criteria, mode, &kv, &vk, None);
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
        let result = execute_query(&criteria, mode, &kv, &vk, None);
        assert!(result.is_empty());
    }

    // ---------- Gap 3 — @present / @absent directives ----------

    fn make_mixed_db_with_indexes(tmp: &tmp::TempDir) -> std::path::PathBuf {
        let dov = tmp.path().join("test.dov");
        let mut db = DotsvFile::empty();
        let actions = parse_action_str(
            "+AGk26cH00001\tname=Alice\tdone=true\n\
             +AGk26cH00002\tname=Bob\tdone=false\n\
             +AGk26cH00003\tname=Carol\n",
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
    fn parse_qtv_present_form() {
        let tmp = tmp::TempDir::new();
        let qtv = tmp.path().join("q.qtv");
        fs::write(&qtv, "@present\tdone\n").unwrap();
        let (_, crit) = parse_qtv(&qtv).unwrap();
        assert_eq!(crit.len(), 1);
        match &crit[0] {
            Criterion::Present(k) => assert_eq!(k, "done"),
            _ => panic!("expected Present"),
        }
    }

    #[test]
    fn parse_qtv_absent_form_key_only() {
        let tmp = tmp::TempDir::new();
        let qtv = tmp.path().join("q.qtv");
        fs::write(&qtv, "@absent\tdone\n").unwrap();
        let (_, crit) = parse_qtv(&qtv).unwrap();
        match &crit[0] {
            Criterion::AbsentKey(k) => assert_eq!(k, "done"),
            _ => panic!("expected AbsentKey"),
        }
    }

    #[test]
    fn parse_qtv_absent_form_key_value() {
        let tmp = tmp::TempDir::new();
        let qtv = tmp.path().join("q.qtv");
        fs::write(&qtv, "@absent\tdone\ttrue\n").unwrap();
        let (_, crit) = parse_qtv(&qtv).unwrap();
        match &crit[0] {
            Criterion::AbsentKeyValue(k, v) => {
                assert_eq!(k, "done");
                assert_eq!(v, "true");
            }
            _ => panic!("expected AbsentKeyValue"),
        }
    }

    #[test]
    fn parse_qtv_rejects_unknown_at_directive() {
        let tmp = tmp::TempDir::new();
        let qtv = tmp.path().join("q.qtv");
        fs::write(&qtv, "@bogus\tkey\n").unwrap();
        let result = parse_qtv(&qtv);
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("unknown qtv directive"), "got: {}", msg);
    }

    #[test]
    fn parse_qtv_at_in_value_passes_as_two_column() {
        // Lines whose FIRST byte after a tab is `@` aren't directive lines
        // — they're regular `key\tvalue` rows where the value happens to
        // start with `@`. Here, `name\t@hidden` is a normal pair.
        let tmp = tmp::TempDir::new();
        let qtv = tmp.path().join("q.qtv");
        fs::write(&qtv, "name\t@hidden\n").unwrap();
        let (_, crit) = parse_qtv(&qtv).unwrap();
        match &crit[0] {
            Criterion::KeyValue(k, v) => {
                assert_eq!(k, "name");
                assert_eq!(v, "@hidden");
            }
            _ => panic!("expected KeyValue, got {:?}", crit[0]),
        }
    }

    #[test]
    fn parse_qtv_at_directive_with_unicode_key() {
        let tmp = tmp::TempDir::new();
        let qtv = tmp.path().join("q.qtv");
        fs::write(&qtv, "@present\t都市\n").unwrap();
        let (_, crit) = parse_qtv(&qtv).unwrap();
        match &crit[0] {
            Criterion::Present(k) => assert_eq!(k, "都市"),
            _ => panic!("expected Present"),
        }
    }

    #[test]
    fn parse_qtv_at_directive_with_tab_in_key_unescapes() {
        let tmp = tmp::TempDir::new();
        let qtv = tmp.path().join("q.qtv");
        fs::write(&qtv, "@absent\tweird\\x09key\n").unwrap();
        let (_, crit) = parse_qtv(&qtv).unwrap();
        match &crit[0] {
            Criterion::AbsentKey(k) => assert_eq!(k, "weird\tkey"),
            _ => panic!("expected AbsentKey"),
        }
    }

    #[test]
    fn query_present_returns_uuids_with_key() {
        let tmp = tmp::TempDir::new();
        let dov = make_mixed_db_with_indexes(&tmp);
        let kv = load_rtv_index(&kv_rtv_path(&dov)).unwrap();
        let vk = load_rtv_index(&vk_rtv_path(&dov)).unwrap();
        let crit = vec![Criterion::Present("done".to_string())];
        let result = execute_query(&crit, FilterMode::Intersect, &kv, &vk, None);
        // Alice + Bob have `done`
        assert_eq!(result.len(), 2);
        assert!(result.contains(&"AGk26cH00001".to_string()));
        assert!(result.contains(&"AGk26cH00002".to_string()));
    }

    #[test]
    fn query_absent_returns_uuids_without_key() {
        let tmp = tmp::TempDir::new();
        let dov = make_mixed_db_with_indexes(&tmp);
        let kv = load_rtv_index(&kv_rtv_path(&dov)).unwrap();
        let vk = load_rtv_index(&vk_rtv_path(&dov)).unwrap();
        let universe = load_uuid_universe(&uuid_rtv_path(&dov)).unwrap();
        let crit = vec![Criterion::AbsentKey("done".to_string())];
        let result = execute_query(&crit, FilterMode::Intersect, &kv, &vk, Some(&universe));
        // Only Carol lacks `done`
        assert_eq!(result, vec!["AGk26cH00003".to_string()]);
    }

    #[test]
    fn query_absent_key_value_excludes_matching_pair() {
        let tmp = tmp::TempDir::new();
        let dov = make_mixed_db_with_indexes(&tmp);
        let kv = load_rtv_index(&kv_rtv_path(&dov)).unwrap();
        let vk = load_rtv_index(&vk_rtv_path(&dov)).unwrap();
        let universe = load_uuid_universe(&uuid_rtv_path(&dov)).unwrap();
        // @absent done=true → Bob (done=false) + Carol (no done)
        let crit = vec![Criterion::AbsentKeyValue(
            "done".to_string(),
            "true".to_string(),
        )];
        let result = execute_query(&crit, FilterMode::Intersect, &kv, &vk, Some(&universe));
        assert_eq!(result.len(), 2);
        assert!(result.contains(&"AGk26cH00002".to_string()));
        assert!(result.contains(&"AGk26cH00003".to_string()));
    }

    #[test]
    fn query_absent_empty_db() {
        let tmp = tmp::TempDir::new();
        let dov = tmp.path().join("test.dov");
        let mut db = DotsvFile::empty();
        db.compact().unwrap();
        atomic_write(&db, &dov).unwrap();
        let db = DotsvFile::load(&dov).unwrap();
        generate_rtvs(&dov, &db).unwrap();

        let kv = load_rtv_index(&kv_rtv_path(&dov)).unwrap();
        let vk = load_rtv_index(&vk_rtv_path(&dov)).unwrap();
        let universe = load_uuid_universe(&uuid_rtv_path(&dov)).unwrap();
        let crit = vec![Criterion::AbsentKey("done".to_string())];
        let result = execute_query(&crit, FilterMode::Intersect, &kv, &vk, Some(&universe));
        assert!(result.is_empty());
    }

    #[test]
    fn query_absent_no_one_has_key_returns_universe() {
        let tmp = tmp::TempDir::new();
        let dov = make_mixed_db_with_indexes(&tmp);
        let kv = load_rtv_index(&kv_rtv_path(&dov)).unwrap();
        let vk = load_rtv_index(&vk_rtv_path(&dov)).unwrap();
        let universe = load_uuid_universe(&uuid_rtv_path(&dov)).unwrap();
        // No record has `nonexistent` → @absent returns all 3
        let crit = vec![Criterion::AbsentKey("nonexistent".to_string())];
        let result = execute_query(&crit, FilterMode::Intersect, &kv, &vk, Some(&universe));
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn query_absent_intersect_with_value_filter() {
        let tmp = tmp::TempDir::new();
        let dov = make_mixed_db_with_indexes(&tmp);
        let kv = load_rtv_index(&kv_rtv_path(&dov)).unwrap();
        let vk = load_rtv_index(&vk_rtv_path(&dov)).unwrap();
        let universe = load_uuid_universe(&uuid_rtv_path(&dov)).unwrap();
        // name=Alice AND @absent done=false → Alice (done=true)
        let crit = vec![
            Criterion::KeyValue("name".to_string(), "Alice".to_string()),
            Criterion::AbsentKeyValue("done".to_string(), "false".to_string()),
        ];
        let result = execute_query(&crit, FilterMode::Intersect, &kv, &vk, Some(&universe));
        assert_eq!(result, vec!["AGk26cH00001".to_string()]);
    }

    #[test]
    fn query_present_in_union_mode() {
        let tmp = tmp::TempDir::new();
        let dov = make_mixed_db_with_indexes(&tmp);
        let kv = load_rtv_index(&kv_rtv_path(&dov)).unwrap();
        let vk = load_rtv_index(&vk_rtv_path(&dov)).unwrap();
        // @present done OR name=Carol → Alice, Bob, Carol
        let crit = vec![
            Criterion::Present("done".to_string()),
            Criterion::KeyValue("name".to_string(), "Carol".to_string()),
        ];
        let result = execute_query(&crit, FilterMode::Union, &kv, &vk, None);
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn query_uuid_rtv_loaded_lazily() {
        // Without absence criteria, criteria_need_universe must be false
        let no_abs = vec![Criterion::Token("foo".to_string())];
        assert!(!criteria_need_universe(&no_abs));
        let with_abs = vec![Criterion::AbsentKey("k".to_string())];
        assert!(criteria_need_universe(&with_abs));
    }

    #[test]
    fn query_at_present_stricter_than_bare_token() {
        // @present\tcity hits records WITH `city` as a key (kv only).
        // Bare token `city` hits BOTH kv col1 AND vk col1; in this fixture
        // no record has the value `city`, so both yield the same set.
        // But we ensure @present does not search vk.
        let tmp = tmp::TempDir::new();
        let dov = tmp.path().join("test.dov");
        let mut db = DotsvFile::empty();
        // record where the VALUE happens to be `done`
        let actions = parse_action_str(
            "+AGk26cH00001\tname=Alice\ttag=done\n\
             +AGk26cH00002\tname=Bob\tdone=true\n",
        )
        .unwrap();
        apply_actions(&mut db, &actions).unwrap();
        db.compact().unwrap();
        atomic_write(&db, &dov).unwrap();
        let db = DotsvFile::load(&dov).unwrap();
        generate_rtvs(&dov, &db).unwrap();

        let kv = load_rtv_index(&kv_rtv_path(&dov)).unwrap();
        let vk = load_rtv_index(&vk_rtv_path(&dov)).unwrap();

        // Bare token `done`: matches both records (Alice has tag=done in vk; Bob has done=true in kv)
        let bare = vec![Criterion::Token("done".to_string())];
        let bare_result = execute_query(&bare, FilterMode::Intersect, &kv, &vk, None);
        assert_eq!(bare_result.len(), 2);

        // @present\tdone: matches only Bob (kv col1 = done)
        let present = vec![Criterion::Present("done".to_string())];
        let present_result = execute_query(&present, FilterMode::Intersect, &kv, &vk, None);
        assert_eq!(present_result, vec!["AGk26cH00002".to_string()]);
    }

    #[test]
    fn absence_qtv_at_absent_with_value_includes_records_lacking_key() {
        // Pinned guard for the documented semantic asymmetry between
        // @absent k v (.qtv) and ne k v (.ftv). @absent must include both
        // the "lacks key entirely" records AND the "value differs" records.
        let tmp = tmp::TempDir::new();
        let dov = make_mixed_db_with_indexes(&tmp);
        let kv = load_rtv_index(&kv_rtv_path(&dov)).unwrap();
        let vk = load_rtv_index(&vk_rtv_path(&dov)).unwrap();
        let universe = load_uuid_universe(&uuid_rtv_path(&dov)).unwrap();
        let crit = vec![Criterion::AbsentKeyValue(
            "done".to_string(),
            "true".to_string(),
        )];
        let result = execute_query(&crit, FilterMode::Intersect, &kv, &vk, Some(&universe));
        // Bob (done=false) AND Carol (no done) — Carol lacks the key entirely
        assert!(result.contains(&"AGk26cH00003".to_string()));
        assert!(result.contains(&"AGk26cH00002".to_string()));
        assert!(!result.contains(&"AGk26cH00001".to_string()));
    }
}
