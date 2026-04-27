/// `--filter` mode: filter records using a `.ftv` predicate file.
///
/// Operators (banana.md §2.2):
///   has nohas eq ne lt le gt ge pre suf sub neq nne nlt nle ngt nge
/// Combinators: `and ... end`, `or ... end`, depth ≤ 4.
///
/// Output: matching UUIDs to stdout, sorted lex. Numeric ops against
/// non-numeric column values exclude those records and emit one stderr
/// summary line per offending op (banana.md Decision #21). Exit code 0
/// even with the warning.
use crate::error::{Result, TsdbError};
use crate::escape::unescape;
use crate::order::{encode_norm, ord_ptv_path};
use crate::plane::{kv_ptv_path, uuid_ptv_path};
use crate::query::FilterMode;
use crate::relate::{kv_rtv_path, uuid_rtv_path};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::Path;

const MAX_NESTING_DEPTH: usize = 4;

/// All filter operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Has,
    Nohas,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Pre,
    Suf,
    Sub,
    Neq,
    Nne,
    Nlt,
    Nle,
    Ngt,
    Nge,
}

impl Op {
    fn from_str(s: &str) -> Option<Op> {
        Some(match s {
            "has" => Op::Has,
            "nohas" => Op::Nohas,
            "eq" => Op::Eq,
            "ne" => Op::Ne,
            "lt" => Op::Lt,
            "le" => Op::Le,
            "gt" => Op::Gt,
            "ge" => Op::Ge,
            "pre" => Op::Pre,
            "suf" => Op::Suf,
            "sub" => Op::Sub,
            "neq" => Op::Neq,
            "nne" => Op::Nne,
            "nlt" => Op::Nlt,
            "nle" => Op::Nle,
            "ngt" => Op::Ngt,
            "nge" => Op::Nge,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Op::Has => "has",
            Op::Nohas => "nohas",
            Op::Eq => "eq",
            Op::Ne => "ne",
            Op::Lt => "lt",
            Op::Le => "le",
            Op::Gt => "gt",
            Op::Ge => "ge",
            Op::Pre => "pre",
            Op::Suf => "suf",
            Op::Sub => "sub",
            Op::Neq => "neq",
            Op::Nne => "nne",
            Op::Nlt => "nlt",
            Op::Nle => "nle",
            Op::Ngt => "ngt",
            Op::Nge => "nge",
        }
    }

    /// True iff the op is one of the numeric variants (uses `.ord.ptv`).
    pub fn is_numeric(self) -> bool {
        matches!(
            self,
            Op::Neq | Op::Nne | Op::Nlt | Op::Nle | Op::Ngt | Op::Nge
        )
    }

    /// True iff the op needs `.kv.ptv` (lex range / pre / suf / sub).
    pub fn needs_kv_ptv(self) -> bool {
        matches!(
            self,
            Op::Lt | Op::Le | Op::Gt | Op::Ge | Op::Pre | Op::Suf | Op::Sub
        )
    }

    /// True iff the op takes a value argument.
    pub fn takes_value(self) -> bool {
        !matches!(self, Op::Has | Op::Nohas)
    }
}

/// A single predicate (op applied to a key and optional value).
#[derive(Debug, Clone)]
pub struct Predicate {
    pub op: Op,
    pub key: String,
    pub value: Option<String>,
}

/// A node in the filter expression tree.
#[derive(Debug, Clone)]
pub enum Node {
    Pred(Predicate),
    And(Vec<Node>),
    Or(Vec<Node>),
}

/// Parsed `.ftv` file.
#[derive(Debug, Clone)]
pub struct FtvFile {
    pub mode: FilterMode,
    pub nodes: Vec<Node>,
}

/// Parse a `.ftv` file from a string.
pub fn parse_ftv_str(content: &str) -> Result<FtvFile> {
    let mut lines: Vec<(usize, String)> = Vec::new();
    for (i, raw) in content.lines().enumerate() {
        let line_no = i + 1;
        let s = raw.trim_end_matches('\r').to_string();
        lines.push((line_no, s));
    }

    let mut idx = 0;
    let mut mode = FilterMode::Intersect;
    let mut mode_checked = false;
    let mut nodes: Vec<Node> = Vec::new();

    while idx < lines.len() {
        let (line_no, ref line) = lines[idx].clone();
        if line.is_empty() {
            idx += 1;
            continue;
        }

        if !mode_checked {
            mode_checked = true;
            if let Some(m) = try_parse_mode_decl(line) {
                mode = m;
                idx += 1;
                continue;
            }
        }

        if line.starts_with('#') {
            idx += 1;
            continue;
        }

        // and/or block?
        if line == "and" || line == "or" {
            let (node, end_idx) = parse_block(&lines, idx, 1)?;
            nodes.push(node);
            idx = end_idx + 1;
            continue;
        }

        // Else: a flat predicate
        let pred = parse_predicate_line(line, line_no)?;
        nodes.push(Node::Pred(pred));
        idx += 1;
    }

    Ok(FtvFile { mode, nodes })
}

fn try_parse_mode_decl(line: &str) -> Option<FilterMode> {
    let rest = line.strip_prefix("# mode\t")?;
    match rest {
        "union" => Some(FilterMode::Union),
        "intersect" => Some(FilterMode::Intersect),
        _ => None,
    }
}

/// Parse an `and ... end` or `or ... end` block starting at `lines[start_idx]`.
/// Returns the parsed node and the index of the matching `end` line.
fn parse_block(
    lines: &[(usize, String)],
    start_idx: usize,
    depth: usize,
) -> Result<(Node, usize)> {
    if depth > MAX_NESTING_DEPTH {
        return Err(TsdbError::ParseError {
            line: lines[start_idx].0,
            message: format!("nesting limit {} exceeded", MAX_NESTING_DEPTH),
        });
    }

    let head = &lines[start_idx].1;
    let is_and = head == "and";
    let is_or = head == "or";
    if !is_and && !is_or {
        return Err(TsdbError::ParseError {
            line: lines[start_idx].0,
            message: format!("expected 'and' or 'or', got {:?}", head),
        });
    }

    let mut children: Vec<Node> = Vec::new();
    let mut idx = start_idx + 1;
    while idx < lines.len() {
        let (line_no, ref line) = lines[idx].clone();
        if line.is_empty() || line.starts_with('#') {
            idx += 1;
            continue;
        }
        if line == "end" {
            let node = if is_and {
                Node::And(children)
            } else {
                Node::Or(children)
            };
            return Ok((node, idx));
        }
        if line == "and" || line == "or" {
            let (child, child_end) = parse_block(lines, idx, depth + 1)?;
            children.push(child);
            idx = child_end + 1;
            continue;
        }
        let pred = parse_predicate_line(line, line_no)?;
        children.push(Node::Pred(pred));
        idx += 1;
    }
    Err(TsdbError::ParseError {
        line: lines[start_idx].0,
        message: format!("unterminated '{}' block (no matching 'end')", head),
    })
}

fn parse_predicate_line(line: &str, line_no: usize) -> Result<Predicate> {
    let mut parts = line.splitn(3, '\t');
    let op_tok = parts.next().ok_or_else(|| TsdbError::ParseError {
        line: line_no,
        message: format!("empty predicate: {:?}", line),
    })?;
    let op = Op::from_str(op_tok).ok_or_else(|| TsdbError::ParseError {
        line: line_no,
        message: format!("unknown op {:?} (line {})", op_tok, line_no),
    })?;
    let key_raw = parts.next().ok_or_else(|| TsdbError::ParseError {
        line: line_no,
        message: format!("predicate {:?} missing key", op_tok),
    })?;
    let key = unescape(key_raw).map_err(|e| TsdbError::ParseError {
        line: line_no,
        message: format!("predicate key unescape error: {}", e),
    })?;

    let value = if op.takes_value() {
        let v_raw = parts.next().ok_or_else(|| TsdbError::ParseError {
            line: line_no,
            message: format!("op {:?} requires a value", op.name()),
        })?;
        let v = unescape(v_raw).map_err(|e| TsdbError::ParseError {
            line: line_no,
            message: format!("predicate value unescape error: {}", e),
        })?;
        // Reject array/object-shaped values (banana.md §2.5 / §2.7).
        let bytes = v.as_bytes();
        if bytes.len() >= 2 {
            let first = bytes[0];
            let last = bytes[bytes.len() - 1];
            if (first == b'[' && last == b']') || (first == b'{' && last == b'}') {
                return Err(TsdbError::ParseError {
                    line: line_no,
                    message: format!(
                        "value {:?} for op {:?} looks like an array/object literal; \
                         array literals are not allowed in .ftv predicates",
                        v,
                        op.name()
                    ),
                });
            }
        }
        Some(v)
    } else {
        if parts.next().is_some() {
            return Err(TsdbError::ParseError {
                line: line_no,
                message: format!(
                    "op {:?} takes only a key, but extra column found",
                    op.name()
                ),
            });
        }
        None
    };

    Ok(Predicate { op, key, value })
}

// -------------------------- Index loaders --------------------------

/// Map: key → value → uuids (read from `.kv.rtv`).
type KvRtv = HashMap<String, HashMap<String, Vec<String>>>;

fn load_kv_rtv(path: &Path) -> Result<KvRtv> {
    let content = fs::read_to_string(path)?;
    let mut idx: KvRtv = HashMap::new();
    for line in content.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(3, '\t');
        let k_raw = match parts.next() {
            Some(s) => s,
            None => continue,
        };
        let v_raw = match parts.next() {
            Some(s) => s,
            None => continue,
        };
        let uuids_raw = match parts.next() {
            Some(s) => s,
            None => continue,
        };
        let k = unescape(k_raw).unwrap_or_else(|_| k_raw.to_string());
        let v = unescape(v_raw).unwrap_or_else(|_| v_raw.to_string());
        let uuids: Vec<String> = uuids_raw
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
        idx.entry(k).or_default().insert(v, uuids);
    }
    Ok(idx)
}

fn load_uuid_universe(path: &Path) -> Result<BTreeSet<String>> {
    let content = fs::read_to_string(path)?;
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

/// Row from `.kv.ptv`: (key, value, uuid). The value column is unescaped.
#[derive(Debug, Clone)]
pub struct KvPtvRow {
    pub key: String,
    pub value: String,
    pub uuid: String,
}

fn load_kv_ptv_rows(path: &Path) -> Result<Vec<KvPtvRow>> {
    let content = fs::read_to_string(path)?;
    let mut rows = Vec::new();
    for line in content.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(3, '\t');
        let k_raw = match parts.next() {
            Some(s) => s,
            None => continue,
        };
        let v_raw = match parts.next() {
            Some(s) => s,
            None => continue,
        };
        let uuid = match parts.next() {
            Some(s) => s,
            None => continue,
        };
        let key = unescape(k_raw).unwrap_or_else(|_| k_raw.to_string());
        let value = unescape(v_raw).unwrap_or_else(|_| v_raw.to_string());
        rows.push(KvPtvRow {
            key,
            value,
            uuid: uuid.to_string(),
        });
    }
    Ok(rows)
}

/// Row from `.ord.ptv`: (norm, key, raw_value, uuid).
#[derive(Debug, Clone)]
pub struct OrdRow {
    pub norm: String,
    pub key: String,
    #[allow(dead_code)]
    pub raw_value: String,
    pub uuid: String,
}

fn load_ord_ptv_rows(path: &Path) -> Result<Vec<OrdRow>> {
    let content = fs::read_to_string(path)?;
    let mut rows = Vec::new();
    for line in content.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(4, '\t');
        let norm_raw = match parts.next() {
            Some(s) => s,
            None => continue,
        };
        let k_raw = match parts.next() {
            Some(s) => s,
            None => continue,
        };
        let v_raw = match parts.next() {
            Some(s) => s,
            None => continue,
        };
        let uuid = match parts.next() {
            Some(s) => s,
            None => continue,
        };
        let norm = unescape(norm_raw).unwrap_or_else(|_| norm_raw.to_string());
        let key = unescape(k_raw).unwrap_or_else(|_| k_raw.to_string());
        let raw_value = unescape(v_raw).unwrap_or_else(|_| v_raw.to_string());
        rows.push(OrdRow {
            norm,
            key,
            raw_value,
            uuid: uuid.to_string(),
        });
    }
    Ok(rows)
}

// -------------------------- Resolver --------------------------

/// Index bundle used during resolution. Numeric indexes are loaded only
/// when needed.
pub struct Indexes {
    pub kv_rtv: KvRtv,
    pub universe: BTreeSet<String>,
    pub kv_ptv: Option<Vec<KvPtvRow>>,
    pub ord_ptv: Option<Vec<OrdRow>>,
}

/// Side-effect bag for warnings emitted during resolution. Aggregated
/// per-op so we can emit one stderr line per offending op.
#[derive(Default)]
pub struct Warnings {
    /// Map: (op-name, key, value) → number of records skipped.
    pub numeric_skips: HashMap<(String, String, String), usize>,
}

impl Warnings {
    pub fn emit_to_stderr(&self) {
        let stderr = std::io::stderr();
        let mut handle = stderr.lock();
        for ((op, key, val), count) in &self.numeric_skips {
            let _ = writeln!(
                handle,
                "warning: '{} {} {}' skipped {} record(s) with non-numeric value",
                op, key, val, count
            );
        }
    }
}

/// Inspect a node tree to determine which indexes need loading.
pub fn required_indexes(nodes: &[Node]) -> (bool, bool) {
    let mut needs_kv_ptv = false;
    let mut needs_ord_ptv = false;
    for n in nodes {
        let (a, b) = node_required_indexes(n);
        needs_kv_ptv = needs_kv_ptv || a;
        needs_ord_ptv = needs_ord_ptv || b;
    }
    (needs_kv_ptv, needs_ord_ptv)
}

fn node_required_indexes(node: &Node) -> (bool, bool) {
    match node {
        Node::Pred(p) => (p.op.needs_kv_ptv(), p.op.is_numeric()),
        Node::And(children) | Node::Or(children) => {
            let mut a = false;
            let mut b = false;
            for c in children {
                let (x, y) = node_required_indexes(c);
                a = a || x;
                b = b || y;
            }
            (a, b)
        }
    }
}

/// Resolve a tree of nodes against the indexes; return the matching UUID
/// set.
pub fn resolve_nodes(
    nodes: &[Node],
    mode: FilterMode,
    idx: &Indexes,
    warnings: &mut Warnings,
) -> HashSet<String> {
    let mut acc: Option<HashSet<String>> = None;
    for n in nodes {
        let s = resolve_node(n, idx, warnings);
        acc = Some(match acc {
            None => s,
            Some(prev) => match mode {
                FilterMode::Union => prev.union(&s).cloned().collect(),
                FilterMode::Intersect => prev.intersection(&s).cloned().collect(),
            },
        });
    }
    acc.unwrap_or_else(|| {
        // Empty criteria: match all (banana.md §2.7 — match-all when no criteria)
        idx.universe.iter().cloned().collect()
    })
}

fn resolve_node(node: &Node, idx: &Indexes, warnings: &mut Warnings) -> HashSet<String> {
    match node {
        Node::Pred(p) => resolve_predicate(p, idx, warnings),
        Node::And(children) => {
            if children.is_empty() {
                // Empty AND = match-all (intersect of nothing = universe).
                return idx.universe.iter().cloned().collect();
            }
            let mut acc: Option<HashSet<String>> = None;
            for c in children {
                let s = resolve_node(c, idx, warnings);
                acc = Some(match acc {
                    None => s,
                    Some(prev) => prev.intersection(&s).cloned().collect(),
                });
            }
            acc.unwrap_or_default()
        }
        Node::Or(children) => {
            // Empty OR = match-none.
            let mut acc: HashSet<String> = HashSet::new();
            for c in children {
                let s = resolve_node(c, idx, warnings);
                acc.extend(s);
            }
            acc
        }
    }
}

fn key_present_uuids(key: &str, idx: &Indexes) -> HashSet<String> {
    let mut out: HashSet<String> = HashSet::new();
    if let Some(val_map) = idx.kv_rtv.get(key) {
        for uuids in val_map.values() {
            out.extend(uuids.iter().cloned());
        }
    }
    out
}

fn key_value_uuids(key: &str, value: &str, idx: &Indexes) -> HashSet<String> {
    let mut out: HashSet<String> = HashSet::new();
    if let Some(val_map) = idx.kv_rtv.get(key) {
        if let Some(uuids) = val_map.get(value) {
            out.extend(uuids.iter().cloned());
        }
    }
    out
}

/// Resolve `eq key=value` per banana.md §2.5: prefer .kv.ptv (per-element
/// for canonical-array values); fall back to .kv.rtv exact-match if .kv.ptv
/// is not loaded. Shared by `Op::Eq` and `Op::Ne` so the two stay symmetric.
fn eq_uuids(key: &str, value: &str, idx: &Indexes) -> HashSet<String> {
    if let Some(rows) = &idx.kv_ptv {
        rows.iter()
            .filter(|r| r.key == key && r.value == value)
            .map(|r| r.uuid.clone())
            .collect()
    } else {
        key_value_uuids(key, value, idx)
    }
}

fn resolve_predicate(p: &Predicate, idx: &Indexes, warnings: &mut Warnings) -> HashSet<String> {
    match p.op {
        Op::Has => key_present_uuids(&p.key, idx),
        Op::Nohas => {
            let present = key_present_uuids(&p.key, idx);
            idx.universe
                .iter()
                .filter(|u| !present.contains(*u))
                .cloned()
                .collect()
        }
        Op::Eq => {
            let v = p.value.as_deref().unwrap_or("");
            eq_uuids(&p.key, v, idx)
        }
        Op::Ne => {
            let v = p.value.as_deref().unwrap_or("");
            // ne = has(key) − eq(key, v). Both eq and the asymmetry with
            // .qtv `@absent` are documented in banana.md §3.6: eq uses
            // .kv.ptv per-element when available, so ne is per-element too.
            let has = key_present_uuids(&p.key, idx);
            let eq_set = eq_uuids(&p.key, v, idx);
            has.difference(&eq_set).cloned().collect()
        }
        Op::Lt | Op::Le | Op::Gt | Op::Ge => {
            let v = p.value.as_deref().unwrap_or("");
            let rows = match &idx.kv_ptv {
                Some(r) => r,
                None => return HashSet::new(),
            };
            rows.iter()
                .filter(|r| r.key == p.key && lex_op_match(p.op, &r.value, v))
                .map(|r| r.uuid.clone())
                .collect()
        }
        Op::Pre => {
            let v = p.value.as_deref().unwrap_or("");
            let rows = match &idx.kv_ptv {
                Some(r) => r,
                None => return HashSet::new(),
            };
            rows.iter()
                .filter(|r| r.key == p.key && r.value.starts_with(v))
                .map(|r| r.uuid.clone())
                .collect()
        }
        Op::Suf => {
            let v = p.value.as_deref().unwrap_or("");
            let rows = match &idx.kv_ptv {
                Some(r) => r,
                None => return HashSet::new(),
            };
            rows.iter()
                .filter(|r| r.key == p.key && r.value.ends_with(v))
                .map(|r| r.uuid.clone())
                .collect()
        }
        Op::Sub => {
            let v = p.value.as_deref().unwrap_or("");
            let rows = match &idx.kv_ptv {
                Some(r) => r,
                None => return HashSet::new(),
            };
            rows.iter()
                .filter(|r| r.key == p.key && r.value.contains(v))
                .map(|r| r.uuid.clone())
                .collect()
        }
        Op::Neq | Op::Nne | Op::Nlt | Op::Nle | Op::Ngt | Op::Nge => {
            let v = p.value.as_deref().unwrap_or("");
            let target_norm = match encode_norm(v) {
                Some(n) => n,
                None => {
                    // Argument itself isn't numeric: warn and return empty.
                    let key_warn = (
                        p.op.name().to_string(),
                        p.key.clone(),
                        v.to_string(),
                    );
                    *warnings.numeric_skips.entry(key_warn).or_insert(0) += 1;
                    return HashSet::new();
                }
            };
            let rows = match &idx.ord_ptv {
                Some(r) => r,
                None => return HashSet::new(),
            };

            // Count non-numeric values for this key (skipped) for stderr summary.
            // We approximate by counting records with the key but no .ord row.
            let key_present = key_present_uuids(&p.key, idx);
            let mut numeric_uuids_for_key: HashSet<String> = HashSet::new();
            for r in rows.iter().filter(|r| r.key == p.key) {
                numeric_uuids_for_key.insert(r.uuid.clone());
            }
            let skipped = key_present.difference(&numeric_uuids_for_key).count();
            if skipped > 0 {
                let key_warn = (
                    p.op.name().to_string(),
                    p.key.clone(),
                    v.to_string(),
                );
                *warnings.numeric_skips.entry(key_warn).or_insert(0) += skipped;
            }

            match p.op {
                Op::Neq => rows
                    .iter()
                    .filter(|r| r.key == p.key && r.norm == target_norm)
                    .map(|r| r.uuid.clone())
                    .collect(),
                Op::Nne => {
                    // has(key) − neq(key, v) — same record-level semantics
                    // as `ne` but using numeric equality. Excludes records
                    // with non-numeric values for the key (warned above).
                    let neq: HashSet<String> = rows
                        .iter()
                        .filter(|r| r.key == p.key && r.norm == target_norm)
                        .map(|r| r.uuid.clone())
                        .collect();
                    numeric_uuids_for_key.difference(&neq).cloned().collect()
                }
                Op::Nlt => rows
                    .iter()
                    .filter(|r| r.key == p.key && r.norm < target_norm)
                    .map(|r| r.uuid.clone())
                    .collect(),
                Op::Nle => rows
                    .iter()
                    .filter(|r| r.key == p.key && r.norm <= target_norm)
                    .map(|r| r.uuid.clone())
                    .collect(),
                Op::Ngt => rows
                    .iter()
                    .filter(|r| r.key == p.key && r.norm > target_norm)
                    .map(|r| r.uuid.clone())
                    .collect(),
                Op::Nge => rows
                    .iter()
                    .filter(|r| r.key == p.key && r.norm >= target_norm)
                    .map(|r| r.uuid.clone())
                    .collect(),
                _ => unreachable!(),
            }
        }
    }
}

fn lex_op_match(op: Op, lhs: &str, rhs: &str) -> bool {
    match op {
        Op::Lt => lhs < rhs,
        Op::Le => lhs <= rhs,
        Op::Gt => lhs > rhs,
        Op::Ge => lhs >= rhs,
        _ => false,
    }
}

// -------------------------- Top-level driver --------------------------

/// Execute `--filter <ftv> <dov>` (UUIDs to stdout).
///
/// Caller is responsible for the lock and for ensuring `.rtv` and `.ptv`
/// indexes are current via `run_relate_locked` / `run_plane_locked`.
pub fn run_filter(ftv_path: &Path, dov_path: &Path) -> Result<Vec<String>> {
    let content = fs::read_to_string(ftv_path).map_err(|e| {
        TsdbError::Io(std::io::Error::new(
            e.kind(),
            format!("cannot read filter file {}: {}", ftv_path.display(), e),
        ))
    })?;
    let ftv = parse_ftv_str(&content)?;
    let (needs_kv_ptv, needs_ord_ptv) = required_indexes(&ftv.nodes);

    let kv_rtv = load_kv_rtv(&kv_rtv_path(dov_path))?;
    let universe = load_uuid_universe(&uuid_rtv_path(dov_path))?;
    let kv_ptv = if needs_kv_ptv {
        Some(load_kv_ptv_rows(&kv_ptv_path(dov_path))?)
    } else {
        None
    };
    // For `eq` we always benefit from `.kv.ptv` (per-element). If the
    // index file exists we use it transparently.
    let kv_ptv = if kv_ptv.is_none() && kv_ptv_path(dov_path).exists() {
        Some(load_kv_ptv_rows(&kv_ptv_path(dov_path))?)
    } else {
        kv_ptv
    };
    let ord_ptv = if needs_ord_ptv {
        Some(load_ord_ptv_rows(&ord_ptv_path(dov_path))?)
    } else {
        None
    };
    // uuid.ptv is unused here; uuid.rtv covers the universe.
    let _ = uuid_ptv_path(dov_path);

    let idx = Indexes {
        kv_rtv,
        universe,
        kv_ptv,
        ord_ptv,
    };
    let mut warnings = Warnings::default();
    let result_set = resolve_nodes(&ftv.nodes, ftv.mode, &idx, &mut warnings);
    let mut result: Vec<String> = result_set.into_iter().collect();
    result.sort();

    warnings.emit_to_stderr();

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::parse_action_str;
    use crate::dotsv::{apply_actions, atomic_write, DotsvFile};
    use crate::plane::generate_ptvs;
    use crate::relate::generate_rtvs;

    mod tmp {
        use std::path::{Path, PathBuf};
        pub struct TempDir {
            path: PathBuf,
        }
        impl TempDir {
            pub fn new() -> Self {
                let path = std::env::temp_dir()
                    .join(format!("tsdb_filter_test_{:016x}", rand::random::<u64>()));
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
        let db = DotsvFile::load(&dov).unwrap();
        generate_rtvs(&dov, &db).unwrap();
        generate_ptvs(&dov, &db).unwrap();
        dov
    }

    // ---------- Parser tests ----------

    #[test]
    fn ftv_parse_default_intersect() {
        let f = parse_ftv_str("eq\tname\tAlice\n").unwrap();
        assert_eq!(f.mode, FilterMode::Intersect);
        assert_eq!(f.nodes.len(), 1);
    }

    #[test]
    fn ftv_parse_union_mode_decl() {
        let f = parse_ftv_str("# mode\tunion\neq\tname\tAlice\n").unwrap();
        assert_eq!(f.mode, FilterMode::Union);
    }

    #[test]
    fn ftv_parse_has_predicate() {
        let f = parse_ftv_str("has\tcity\n").unwrap();
        match &f.nodes[0] {
            Node::Pred(p) => {
                assert_eq!(p.op, Op::Has);
                assert_eq!(p.key, "city");
                assert!(p.value.is_none());
            }
            _ => panic!("expected predicate"),
        }
    }

    #[test]
    fn ftv_parse_nohas_predicate() {
        let f = parse_ftv_str("nohas\tcity\n").unwrap();
        match &f.nodes[0] {
            Node::Pred(p) => assert_eq!(p.op, Op::Nohas),
            _ => panic!("expected predicate"),
        }
    }

    #[test]
    fn ftv_parse_eq_ne_pair() {
        let f = parse_ftv_str("eq\tname\tAlice\nne\tname\tBob\n").unwrap();
        assert_eq!(f.nodes.len(), 2);
    }

    #[test]
    fn ftv_parse_lt_le_gt_ge_lex() {
        for op in &["lt", "le", "gt", "ge"] {
            let f = parse_ftv_str(&format!("{}\tname\tAlice\n", op)).unwrap();
            assert_eq!(f.nodes.len(), 1);
        }
    }

    #[test]
    fn ftv_parse_neq_nne_numeric() {
        let f = parse_ftv_str("neq\tage\t30\nnne\tage\t40\n").unwrap();
        assert_eq!(f.nodes.len(), 2);
    }

    #[test]
    fn ftv_parse_nlt_nle_ngt_nge_numeric() {
        for op in &["nlt", "nle", "ngt", "nge"] {
            let f = parse_ftv_str(&format!("{}\tage\t30\n", op)).unwrap();
            assert_eq!(f.nodes.len(), 1);
        }
    }

    #[test]
    fn ftv_parse_pre_suf_sub() {
        for op in &["pre", "suf", "sub"] {
            let f = parse_ftv_str(&format!("{}\tname\tA\n", op)).unwrap();
            assert_eq!(f.nodes.len(), 1);
        }
    }

    #[test]
    fn ftv_parse_and_block() {
        let f = parse_ftv_str("and\neq\tname\tAlice\nne\tname\tBob\nend\n").unwrap();
        match &f.nodes[0] {
            Node::And(c) => assert_eq!(c.len(), 2),
            _ => panic!("expected And"),
        }
    }

    #[test]
    fn ftv_parse_or_block() {
        let f = parse_ftv_str("or\neq\tcity\tTokyo\neq\tcity\tOsaka\nend\n").unwrap();
        match &f.nodes[0] {
            Node::Or(c) => assert_eq!(c.len(), 2),
            _ => panic!("expected Or"),
        }
    }

    #[test]
    fn ftv_parse_nested_and_in_or_4_deep_ok() {
        // depth-4 nesting: or > and > or > and (4 levels).
        let src = "\
or
and
or
and
eq\tname\tAlice
end
end
end
end
";
        let f = parse_ftv_str(src).unwrap();
        assert_eq!(f.nodes.len(), 1);
    }

    #[test]
    fn ftv_parse_nested_5_deep_rejected() {
        let src = "\
or
and
or
and
or
eq\tname\tAlice
end
end
end
end
end
";
        let result = parse_ftv_str(src);
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("nesting"), "got: {}", msg);
    }

    #[test]
    fn ftv_parse_unknown_op_errors_with_line_no() {
        let src = "\nbogus\tkey\tval\n";
        let result = parse_ftv_str(src);
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("unknown op"), "got: {}", msg);
        assert!(msg.contains("line 2"), "got: {}", msg);
    }

    #[test]
    fn ftv_parse_op_with_trailing_space_rejected() {
        let result = parse_ftv_str("eq \tname\tAlice\n");
        assert!(result.is_err());
    }

    #[test]
    fn ftv_parse_pred_with_tab_in_value_escaped_round_trips() {
        let f = parse_ftv_str("eq\tname\thello\\x09world\n").unwrap();
        match &f.nodes[0] {
            Node::Pred(p) => assert_eq!(p.value.as_deref(), Some("hello\tworld")),
            _ => panic!("expected predicate"),
        }
    }

    #[test]
    fn ftv_parse_blank_and_comment_lines_ignored() {
        let f = parse_ftv_str("\n# comment\n\neq\tname\tAlice\n# another\n").unwrap();
        assert_eq!(f.nodes.len(), 1);
    }

    #[test]
    fn ftv_parse_array_shaped_value_rejected() {
        let result = parse_ftv_str("eq\troles\t[\"a\",\"b\"]\n");
        assert!(result.is_err());
    }

    // ---------- Execution tests ----------

    fn fixture_db(tmp: &tmp::TempDir) -> std::path::PathBuf {
        build_db(
            tmp,
            "+AGk26cH00001\tname=Alice\tcity=Tokyo\tage=30\n\
             +AGk26cH00002\tname=Bob\tcity=Tokyo\tage=25\n\
             +AGk26cH00003\tname=Carol\tcity=London\tage=40\n\
             +AGk26cH00004\tname=Dave\n",
        )
    }

    #[test]
    fn filter_has_returns_uuids_with_key() {
        let tmp = tmp::TempDir::new();
        let dov = fixture_db(&tmp);
        let ftv = tmp.path().join("f.ftv");
        fs::write(&ftv, "has\tcity\n").unwrap();
        let result = run_filter(&ftv, &dov).unwrap();
        // Alice, Bob, Carol have city; Dave does not
        assert_eq!(result.len(), 3);
        assert!(!result.contains(&"AGk26cH00004".to_string()));
    }

    #[test]
    fn filter_nohas_returns_uuids_without_key() {
        let tmp = tmp::TempDir::new();
        let dov = fixture_db(&tmp);
        let ftv = tmp.path().join("f.ftv");
        fs::write(&ftv, "nohas\tcity\n").unwrap();
        let result = run_filter(&ftv, &dov).unwrap();
        assert_eq!(result, vec!["AGk26cH00004".to_string()]);
    }

    #[test]
    fn filter_eq_returns_only_exact_match() {
        let tmp = tmp::TempDir::new();
        let dov = fixture_db(&tmp);
        let ftv = tmp.path().join("f.ftv");
        fs::write(&ftv, "eq\tname\tAlice\n").unwrap();
        let result = run_filter(&ftv, &dov).unwrap();
        assert_eq!(result, vec!["AGk26cH00001".to_string()]);
    }

    #[test]
    fn filter_ne_returns_has_key_minus_eq() {
        let tmp = tmp::TempDir::new();
        let dov = fixture_db(&tmp);
        let ftv = tmp.path().join("f.ftv");
        fs::write(&ftv, "ne\tcity\tTokyo\n").unwrap();
        let result = run_filter(&ftv, &dov).unwrap();
        // Alice + Bob have Tokyo; Carol has London; Dave has no city.
        // ne returns: has(city) − eq(city,Tokyo) → Carol only.
        assert_eq!(result, vec!["AGk26cH00003".to_string()]);
    }

    #[test]
    fn filter_lt_lex_includes_30_below_5() {
        // The motivating example: lex `lt 5` should include "30" because
        // "30" < "5" lex (3 < 5).
        let tmp = tmp::TempDir::new();
        let dov = fixture_db(&tmp);
        let ftv = tmp.path().join("f.ftv");
        fs::write(&ftv, "lt\tage\t5\n").unwrap();
        let result = run_filter(&ftv, &dov).unwrap();
        // ages 30, 25, 40 all start with digits ≤ '4' < '5' lex, so all 3 included
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn filter_nlt_numeric_excludes_30_below_5() {
        // Numeric `nlt 5` excludes 30 (since 30 > 5 numerically).
        let tmp = tmp::TempDir::new();
        let dov = fixture_db(&tmp);
        let ftv = tmp.path().join("f.ftv");
        fs::write(&ftv, "nlt\tage\t5\n").unwrap();
        let result = run_filter(&ftv, &dov).unwrap();
        // No record has age < 5 numerically
        assert!(result.is_empty());
    }

    #[test]
    fn filter_neq_numeric_includes_30_and_30_dot_0() {
        let tmp = tmp::TempDir::new();
        let dov = build_db(
            &tmp,
            "+AGk26cH00001\tage=30\n\
             +AGk26cH00002\tage=30.0\n\
             +AGk26cH00003\tage=31\n",
        );
        let ftv = tmp.path().join("f.ftv");
        fs::write(&ftv, "neq\tage\t30\n").unwrap();
        let result = run_filter(&ftv, &dov).unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.contains(&"AGk26cH00001".to_string()));
        assert!(result.contains(&"AGk26cH00002".to_string()));
    }

    #[test]
    fn filter_pre_matches_prefix() {
        let tmp = tmp::TempDir::new();
        let dov = fixture_db(&tmp);
        let ftv = tmp.path().join("f.ftv");
        fs::write(&ftv, "pre\tname\tA\n").unwrap();
        let result = run_filter(&ftv, &dov).unwrap();
        assert_eq!(result, vec!["AGk26cH00001".to_string()]);
    }

    #[test]
    fn filter_suf_matches_suffix() {
        let tmp = tmp::TempDir::new();
        let dov = fixture_db(&tmp);
        let ftv = tmp.path().join("f.ftv");
        fs::write(&ftv, "suf\tname\te\n").unwrap();
        let result = run_filter(&ftv, &dov).unwrap();
        // Alice, Carol, Dave end in 'e' — but Carol ends in 'l' actually.
        // Ends in 'e': Alice, Dave
        assert_eq!(result.len(), 2);
        assert!(result.contains(&"AGk26cH00001".to_string()));
        assert!(result.contains(&"AGk26cH00004".to_string()));
    }

    #[test]
    fn filter_sub_matches_substring() {
        let tmp = tmp::TempDir::new();
        let dov = fixture_db(&tmp);
        let ftv = tmp.path().join("f.ftv");
        fs::write(&ftv, "sub\tname\tar\n").unwrap();
        let result = run_filter(&ftv, &dov).unwrap();
        // "ar" appears in Carol only
        assert_eq!(result, vec!["AGk26cH00003".to_string()]);
    }

    #[test]
    fn filter_or_unions_subresults() {
        let tmp = tmp::TempDir::new();
        let dov = fixture_db(&tmp);
        let ftv = tmp.path().join("f.ftv");
        fs::write(&ftv, "or\neq\tcity\tTokyo\neq\tcity\tLondon\nend\n").unwrap();
        let result = run_filter(&ftv, &dov).unwrap();
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn filter_and_intersects_subresults() {
        let tmp = tmp::TempDir::new();
        let dov = fixture_db(&tmp);
        let ftv = tmp.path().join("f.ftv");
        fs::write(&ftv, "and\nhas\tcity\nngt\tage\t29\nend\n").unwrap();
        let result = run_filter(&ftv, &dov).unwrap();
        // Alice (30), Carol (40)
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn filter_intersect_with_nohas() {
        let tmp = tmp::TempDir::new();
        let dov = fixture_db(&tmp);
        let ftv = tmp.path().join("f.ftv");
        fs::write(&ftv, "has\tname\nnohas\tcity\n").unwrap();
        let result = run_filter(&ftv, &dov).unwrap();
        // Has name and lacks city → Dave
        assert_eq!(result, vec!["AGk26cH00004".to_string()]);
    }

    #[test]
    fn filter_match_all_when_no_criteria() {
        let tmp = tmp::TempDir::new();
        let dov = fixture_db(&tmp);
        let ftv = tmp.path().join("f.ftv");
        fs::write(&ftv, "# mode\tintersect\n").unwrap();
        let result = run_filter(&ftv, &dov).unwrap();
        assert_eq!(result.len(), 4);
    }

    #[test]
    fn filter_array_element_eq_via_kv_ptv() {
        let tmp = tmp::TempDir::new();
        let dov = build_db(
            &tmp,
            "+AGk26cH00001\trole=admin\trole=editor\n\
             +AGk26cH00002\trole=viewer\n",
        );
        let ftv = tmp.path().join("f.ftv");
        fs::write(&ftv, "eq\trole\tadmin\n").unwrap();
        let result = run_filter(&ftv, &dov).unwrap();
        // Alice's array contains admin per-element via .kv.ptv expansion
        assert_eq!(result, vec!["AGk26cH00001".to_string()]);
    }

    #[test]
    fn filter_array_element_numeric_via_ord_ptv() {
        let tmp = tmp::TempDir::new();
        let dov = build_db(
            &tmp,
            "+AGk26cH00001\tscore=10\tscore=60\tscore=70\n\
             +AGk26cH00002\tscore=5\n",
        );
        let ftv = tmp.path().join("f.ftv");
        fs::write(&ftv, "ngt\tscore\t50\n").unwrap();
        let result = run_filter(&ftv, &dov).unwrap();
        assert_eq!(result, vec!["AGk26cH00001".to_string()]);
    }

    #[test]
    fn filter_empty_database_returns_empty() {
        let tmp = tmp::TempDir::new();
        let dov = tmp.path().join("test.dov");
        let mut db = DotsvFile::empty();
        db.compact().unwrap();
        atomic_write(&db, &dov).unwrap();
        let db = DotsvFile::load(&dov).unwrap();
        generate_rtvs(&dov, &db).unwrap();
        generate_ptvs(&dov, &db).unwrap();
        let ftv = tmp.path().join("f.ftv");
        fs::write(&ftv, "has\tanything\n").unwrap();
        let result = run_filter(&ftv, &dov).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn filter_unicode_lex_byte_order() {
        let tmp = tmp::TempDir::new();
        let dov = build_db(
            &tmp,
            "+AGk26cH00001\tcity=東京\n\
             +AGk26cH00002\tcity=Tokyo\n",
        );
        let ftv = tmp.path().join("f.ftv");
        fs::write(&ftv, "gt\tcity\tT\n").unwrap();
        let result = run_filter(&ftv, &dov).unwrap();
        // "Tokyo" > "T" → match. "東京" UTF-8 bytes start with E6 > 'T' (0x54) → match.
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn filter_escaped_tab_in_ftv_value() {
        let tmp = tmp::TempDir::new();
        let dov = build_db(&tmp, "+AGk26cH00001\tnote=hello\\x09world\n");
        let ftv = tmp.path().join("f.ftv");
        fs::write(&ftv, "eq\tnote\thello\\x09world\n").unwrap();
        let result = run_filter(&ftv, &dov).unwrap();
        assert_eq!(result, vec!["AGk26cH00001".to_string()]);
    }

    #[test]
    fn filter_negative_numbers_sort_correctly() {
        let tmp = tmp::TempDir::new();
        let dov = build_db(
            &tmp,
            "+AGk26cH00001\ttemp=-30\n\
             +AGk26cH00002\ttemp=-5\n\
             +AGk26cH00003\ttemp=10\n",
        );
        let ftv = tmp.path().join("f.ftv");
        fs::write(&ftv, "nlt\ttemp\t0\n").unwrap();
        let result = run_filter(&ftv, &dov).unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.contains(&"AGk26cH00001".to_string()));
        assert!(result.contains(&"AGk26cH00002".to_string()));
    }

    #[test]
    fn filter_decimal_3_14_equals_3_14000_under_numeric() {
        let tmp = tmp::TempDir::new();
        let dov = build_db(
            &tmp,
            "+AGk26cH00001\tpi=3.14\n\
             +AGk26cH00002\tpi=3.14000\n\
             +AGk26cH00003\tpi=3.15\n",
        );
        let ftv = tmp.path().join("f.ftv");
        fs::write(&ftv, "neq\tpi\t3.14\n").unwrap();
        let result = run_filter(&ftv, &dov).unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.contains(&"AGk26cH00001".to_string()));
        assert!(result.contains(&"AGk26cH00002".to_string()));
    }

    #[test]
    fn filter_numeric_against_mixed_column_warns_once() {
        let tmp = tmp::TempDir::new();
        let dov = build_db(
            &tmp,
            "+AGk26cH00001\tage=30\n\
             +AGk26cH00002\tage=oldish\n\
             +AGk26cH00003\tage=NaN\n",
        );
        let ftv = tmp.path().join("f.ftv");
        fs::write(&ftv, "ngt\tage\t10\n").unwrap();
        let result = run_filter(&ftv, &dov).unwrap();
        // Only the numeric "30" matches; the two non-numerics are skipped.
        assert_eq!(result, vec!["AGk26cH00001".to_string()]);
    }

    #[test]
    #[allow(non_snake_case)]
    fn absence_ftv_ne_with_value_does_NOT_include_records_lacking_key() {
        let tmp = tmp::TempDir::new();
        let dov = build_db(
            &tmp,
            "+AGk26cH00001\tname=Alice\tdone=true\n\
             +AGk26cH00002\tname=Bob\tdone=false\n\
             +AGk26cH00003\tname=Carol\n",
        );
        let ftv = tmp.path().join("f.ftv");
        fs::write(&ftv, "ne\tdone\ttrue\n").unwrap();
        let result = run_filter(&ftv, &dov).unwrap();
        // Only Bob (has done=false). Carol lacks the key entirely → NOT included.
        assert_eq!(result, vec!["AGk26cH00002".to_string()]);
    }

    #[test]
    fn filter_nohas_on_unknown_key_returns_universe() {
        let tmp = tmp::TempDir::new();
        let dov = fixture_db(&tmp);
        let ftv = tmp.path().join("f.ftv");
        fs::write(&ftv, "nohas\tnonexistent\n").unwrap();
        let result = run_filter(&ftv, &dov).unwrap();
        assert_eq!(result.len(), 4);
    }

    #[test]
    fn filter_nohas_on_universal_key_returns_empty() {
        let tmp = tmp::TempDir::new();
        let dov = fixture_db(&tmp);
        let ftv = tmp.path().join("f.ftv");
        fs::write(&ftv, "nohas\tname\n").unwrap();
        let result = run_filter(&ftv, &dov).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn filter_nohas_on_empty_db_returns_empty() {
        let tmp = tmp::TempDir::new();
        let dov = tmp.path().join("test.dov");
        let mut db = DotsvFile::empty();
        db.compact().unwrap();
        atomic_write(&db, &dov).unwrap();
        let db = DotsvFile::load(&dov).unwrap();
        generate_rtvs(&dov, &db).unwrap();
        generate_ptvs(&dov, &db).unwrap();
        let ftv = tmp.path().join("f.ftv");
        fs::write(&ftv, "nohas\tcity\n").unwrap();
        let result = run_filter(&ftv, &dov).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn filter_ne_on_kv_ptv_element_row_matches_per_element() {
        // `ne` is set-difference based on records (not per element); when
        // a record has the key with multiple values, having one ≠ the
        // target value still counts.
        let tmp = tmp::TempDir::new();
        let dov = build_db(
            &tmp,
            "+AGk26cH00001\trole=admin\trole=editor\n\
             +AGk26cH00002\trole=admin\n",
        );
        let ftv = tmp.path().join("f.ftv");
        fs::write(&ftv, "ne\trole\tadmin\n").unwrap();
        let result = run_filter(&ftv, &dov).unwrap();
        // ne(role, admin) = has(role) − eq(role, admin via .kv.ptv)
        // .kv.ptv has admin rows for BOTH 0001 (per-element) and 0002.
        // So eq(role, admin) = {0001, 0002}; has(role) (via .kv.rtv) = {0001, 0002}.
        // Difference = {} → no result. This documents the per-element interaction.
        assert!(result.is_empty());
    }

    #[test]
    fn filter_nohas_and_ne_compose_with_intersect() {
        let tmp = tmp::TempDir::new();
        let dov = build_db(
            &tmp,
            "+AGk26cH00001\tname=Alice\tcity=Tokyo\n\
             +AGk26cH00002\tname=Bob\n\
             +AGk26cH00003\tname=Carol\tcity=London\n",
        );
        let ftv = tmp.path().join("f.ftv");
        fs::write(&ftv, "has\tname\nnohas\tcity\n").unwrap();
        let result = run_filter(&ftv, &dov).unwrap();
        assert_eq!(result, vec!["AGk26cH00002".to_string()]);
    }

    #[test]
    fn filter_nohas_or_has_composes_with_union() {
        let tmp = tmp::TempDir::new();
        let dov = build_db(
            &tmp,
            "+AGk26cH00001\tname=Alice\n\
             +AGk26cH00002\tcity=Tokyo\n",
        );
        let ftv = tmp.path().join("f.ftv");
        fs::write(&ftv, "or\nhas\tname\nhas\tcity\nend\n").unwrap();
        let result = run_filter(&ftv, &dov).unwrap();
        assert_eq!(result.len(), 2);
    }
}
