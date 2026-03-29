# DOTSV — Database Oriented Tab Separated Vehicle

**Version:** 0.1 Draft
**File extensions:** `*.dotsv`, `*.dov`
**MIME type:** `text/dotsv`

**Revision history:**
- 0.1 — initial release
- 0.2 — timestamp tracking; related formats atsv, rtsv, qtsv

---

## 1. Overview

DOTSV is a single-line-per-record, flat-file database format designed for:

- High processing rate via `mmap` and zero-copy parsing
- Low memory footprint — data is borrowed directly from the memory-mapped buffer
- Git-traceable diffs — deterministic record ordering produces minimal, meaningful changesets
- Full Unicode support — CJK, emoji, accented characters pass through unescaped
- Human readability — plain text, editable in any text editor

A `.dov` file is a valid UTF-8 text file. Every tool that operates on text (grep, awk, diff, cat) works on DOTSV without modification.

---

## 2. Record Format

Each record occupies exactly one line (`\n`-terminated). A record consists of a fixed-width base62-Gu UUID followed by one or more tab-separated key-value pairs.

```
<12-char-base62-Gu-uuid>\t<key=value>\t<key=value>\t...\n
```

### Example

```
NGk26cHcv001	name=Alice	city=東京	age=30
NGk26cHdn002	name=Bob	city=大阪	msg=hello world
EGk26cICK001	name=Carol	city=London	note=a\x3Db
```

---

## 3. UUID Column — Base62-Gu

The first column is always a **12-character base62-Gu UUID** — a time-based, class-prefixed identifier defined by the Base62 Encoding System whitepaper (Format-Gu).

### 3.1 Structure

```
{class}{G}{century}{year2d}{month}{day}{hour}{minute}{second}{order2}
  1      1    1       2      1     1    1      1       1        2     = 12 chars
```

| Position | Length | Segment      | Example | Notes                                   |
|----------|--------|--------------|---------|-----------------------------------------|
| 1        | 1      | Class prefix | `N`     | Record type (uppercase A–Z)             |
| 2        | 1      | Format marker| `G`     | Always literal `G`                      |
| 3        | 1      | Century      | `k`     | `2026 // 100 = 20` → Format-G char     |
| 4–5      | 2      | Year (2d)    | `26`    | Decimal year mod 100                    |
| 6        | 1      | Month        | `c`     | Format-G month table                    |
| 7        | 1      | Day          | `H`     | Format-G day table                      |
| 8        | 1      | Hour         | `c`     | Format-G hour table                     |
| 9        | 1      | Minute       | `v`     | Format-G 60-char alphabet               |
| 10       | 1      | Second       | `0`     | Format-G 60-char alphabet               |
| 11–12    | 2      | Order number | `01`    | Collision resolution (01–99, then base62)|

### 3.2 Properties

| Property       | Specification                                      |
|----------------|----------------------------------------------------|
| Width          | Exactly 12 bytes (fixed)                           |
| Character set  | `[A-Z]G[0-9a-km-zA-NP-Z]{8}[0-9a-zA-Z]{2}`       |
| Byte position  | `line[0..12]`                                      |
| Followed by    | `\t` at byte 12                                    |

Fixed width enables direct slicing (`&line[..12]`) without scanning. The tab at byte 12 serves as a sanity check, not a parsing necessity.

### 3.3 Sort Order Semantics

Because UUIDs are sorted lexicographically (ASCII byte order), records naturally group and order as:

1. **By class** — all `E`-class records (expenses) sort before `N`-class (notes), etc.
2. **By time within class** — the embedded timestamp sorts chronologically within each class prefix.
3. **By order within second** — the 2-char suffix breaks ties for records created in the same second.

This means a `.dov` file is effectively partitioned by record type, with each partition in chronological order — useful for class-scoped queries without an index.

### 3.4 Format-G Alphabet

The Format-G alphabet removes visually ambiguous characters `l` (lowercase L) and `O` (uppercase O), producing a 60-character set:

```
0123456789abcdefghijkmnopqrstuvwxyzABCDEFGHIJKLMNPQRSTUVWXYZ
```

This ensures UUIDs are unambiguous when read, typed, or printed in any font. See the Base62 Encoding System whitepaper for the complete encoding specification.

---

## 4. Key-Value Pairs

After the UUID and its trailing tab, the remainder of the line is a sequence of `key=value` pairs delimited by `\t`.

```
key1=value1\tkey2=value2\tkey3=value3
```

- Keys and values are UTF-8 strings.
- Keys MUST NOT be empty.
- Values MAY be empty (e.g., `tag=`).
- Key order within a record is not significant for semantics, but implementations SHOULD maintain a consistent order for git diff stability.

---

## 5. Escaping Rules

DOTSV uses backslash-hex escaping. Only four bytes require escaping when they appear inside a key or value:

| Byte   | Escaped Form | Reason                    |
|--------|-------------|---------------------------|
| `\n`   | `\x0A`      | Record delimiter          |
| `\t`   | `\x09`      | Field delimiter           |
| `=`    | `\x3D`      | Key-value separator       |
| `\`    | `\\`        | Escape character itself   |

### Optional

| Byte   | Escaped Form | Reason                            |
|--------|-------------|-----------------------------------|
| `\r`   | `\x0D`      | Windows line-ending safety        |

### What is NOT escaped

Everything else passes through literally:

- CJK characters (東京, 日本語)
- Emoji (🚀, 🎉)
- Accented Latin characters (café, naïve)
- Spaces (literal space, no `+` substitution)
- Punctuation, symbols, all other printable Unicode

This means **most real-world values need zero escaping**, enabling the zero-copy fast path: if no `\` is present in a slice, borrow it directly from the underlying buffer.

---

## 6. File Structure

A `.dov` file consists of two sections separated by a single blank line:

```
<sorted section>

<pending section>
```

### 6.1 Sorted Section

Records sorted lexicographically by UUID (ASCII byte-order comparison of the 12-character base62-Gu string). Because the class prefix is the first character and the timestamp follows, records naturally group by class and sort chronologically within each class.

Properties:

- **O(log n) lookup** via binary search on `mmap`
- **Deterministic ordering** — insert position is defined by UUID, not by insertion time
- **Stable git diffs** — new records appear as single inserted lines in predictable locations

### 6.2 Pending Section

A write-ahead buffer of uncommitted operations, appended after the blank line separator. Each line is prefixed with a single-byte opcode:

```
+<uuid>\t<key=val>\t...\n       insert
-<uuid>\n                       delete
~<uuid>\t<key=newval>\t...\n    patch (changed pairs only)
```

| Prefix | Operation | Payload                  |
|--------|-----------|--------------------------|
| `+`    | Insert    | UUID + all KV pairs      |
| `-`    | Delete    | UUID only                |
| `~`    | Patch     | UUID + changed KV pairs  |

**Read path:** Binary search the sorted section, then linear scan the pending section for overrides. The pending section is expected to be small.

**Write path:** Always append to the pending section — O(1).

**Compaction:** Merge the pending section into the sorted section when it exceeds a threshold (e.g., 100 lines). This is a single sequential read + write pass.

---

## 7. Comments and Blank Lines

- Lines starting with `#` are comments and MUST be ignored by parsers.
- Blank lines within the sorted section are not permitted (the first blank line marks the section boundary).
- Additional blank lines after the section separator are ignored.

---

## 8. In-Place Modification

When modifying a value in the sorted section, two strategies apply:

| Condition                    | Strategy                                   |
|-----------------------------|--------------------------------------------|
| New line ≤ old line length  | Overwrite in place, pad with trailing spaces before `\n` |
| New line > old line length  | Append a `~` patch to the pending section  |

The padding strategy works because the KV parser trims trailing whitespace from the last value. This avoids file rewrites for most edits, since values typically stay similar in length.

---

## 9. Parsing — Zero-Copy Fast Path

The core parse loop in Rust:

```rust
fn parse_record(line: &str) -> (&str, Vec<(&str, &str)>) {
    let uuid = &line[..12];
    let kvs = line[13..]          // skip uuid + \t
        .split('\t')
        .filter_map(|pair| pair.split_once('='))
        .collect();
    (uuid, kvs)
}
```

- `split` and `split_once` are `memchr`-accelerated in Rust's standard library.
- When no `\` appears in a value slice, the slice is a direct borrow from the `mmap` — zero allocation.
- When `\` is detected, only that value is decoded into a `Cow<str>`.

### Binary Search on mmap

```rust
fn find_record<'a>(mmap: &'a [u8], target: &str) -> Option<&'a str> {
    let text = std::str::from_utf8(mmap).ok()?;
    let mut lo = 0usize;
    let mut hi = text.len();

    while lo < hi {
        let mid = (lo + hi) / 2;
        let line_start = match text[..mid].rfind('\n') {
            Some(p) => p + 1,
            None => 0,
        };
        let uuid = &text[line_start..line_start + 12];

        if uuid < target {
            lo = text[mid..].find('\n').map(|p| mid + p + 1).unwrap_or(hi);
        } else if uuid > target {
            hi = line_start;
        } else {
            let line_end = text[line_start..].find('\n')
                .map(|p| line_start + p)
                .unwrap_or(text.len());
            return Some(&text[line_start..line_end]);
        }
    }
    None
}
```

---

## 10. Design Rationale

| Goal                  | Mechanism                                         |
|-----------------------|---------------------------------------------------|
| Fast query            | Sorted UUIDs + `mmap` + binary search = O(log n)  |
| Fast parse            | Tab-split is `memchr`-SIMD accelerated; fixed-width 12-char UUID avoids scanning |
| Low memory            | Zero-copy borrows from `mmap`; no HashMap, no deserialization tree |
| Fast writes           | O(1) append to pending section                    |
| Fast deletes          | O(1) append `-` opcode to pending section         |
| Fast modify           | In-place overwrite when size fits; else O(1) patch append |
| Git-traceable         | Sorted records = one-line insertion diffs; deterministic key order within records |
| Human-readable        | Plain UTF-8 text; minimal escaping; tab-aligned columns in most editors |
| Full Unicode          | Only 4 control bytes escaped; all other Unicode is literal |

---

## 11. Concurrency — Lock File Protocol

DOTSV defines a companion lock file for coordinating concurrent access by multiple `tsdb` instances.

### 11.1 Lock File

For every `.dov` file, a corresponding `.dov.lock` file serves as both a kernel-level lock target and a queue manifest.

```
target.dov          ← data
target.dov.lock     ← flock() target + queue manifest
```

**Why a separate file:** The atomic write strategy replaces the `.dov` via temp-file → rename, which invalidates any `flock()` held on the original fd. The `.lock` file is stable — never renamed, never rewritten during data operations.

### 11.2 Lock File Contents

The lock file contains one line per queued `tsdb` instance:

```
<status>\t<process_id>\t<uuid1>,<uuid2>,...\t<timestamp>\n
```

| Field      | Spec                                                    |
|------------|---------------------------------------------------------|
| Status     | `EXEC` (currently running) or `WAIT` (queued)           |
| Process ID | 16 lowercase hex chars, randomly generated at startup   |
| UUID list  | Comma-separated target UUIDs from the action file       |
| Timestamp  | Unix epoch seconds, used for heartbeat / stale detection |

Example at runtime with three queued instances:

```
EXEC	a1b2c3d4e5f6a7b8	NGk26cHcv001,NGk26cHdn002,EGk26cICK001	1711700000
WAIT	d9e0f1a2b3c4d5e6	NGk26dAa0001,EGk26dBb0001	1711700005
WAIT	f7a8b9c0d1e2f3a4	NGk26eC10001,NGk26eC20001	1711700008
```

### 11.3 Conflict Detection

Before joining the queue, a `tsdb` instance pre-scans its action file to collect all target UUIDs, then checks for set intersection against every existing entry in the lock file.

**The rule:** Your UUID set must have zero intersection with every entry — both `EXEC` and all `WAIT` entries ahead of you.

```
Conflict  =  your_uuids  ∩  any_queued_uuids  ≠  ∅
```

Opcodes are irrelevant to conflict detection. Two `+` appends with the same UUID conflict identically to a `+` and a `~`. The reasoning: any queued operation ahead of you may change the state of that record before your turn arrives.

| Instance A | Instance B (you) | Same UUID? | Result           |
|-----------|------------------|------------|------------------|
| `+` insert | `+` insert       | yes        | Rejected         |
| `+` insert | `~` patch        | yes        | Rejected         |
| `~` patch  | `-` delete       | yes        | Rejected         |
| `+` insert | `+` insert       | no         | Queued normally  |

On conflict, `tsdb` exits immediately without joining the queue and reports the conflicting process ID and overlapping UUIDs.

### 11.4 Crash Safety

`flock()` is released automatically by the kernel when a process exits, even on SIGKILL. However, the dead process's line remains in the lock file.

Resolution: each `EXEC` process periodically refreshes its timestamp. During the poll loop, if the `EXEC` entry's timestamp is older than a configurable threshold (default: 30 seconds), the next `WAIT` process evicts the stale entry and promotes itself to `EXEC`.

### 11.5 Locking Granularity

The `flock()` on `.dov.lock` is held only for microseconds — just long enough to read/write the manifest. Actual `.dov` processing happens entirely outside the lock. This means many instances can queue, check, and poll concurrently without blocking each other.

---

## 12. Limitations

- Not suitable for values larger than ~1 MB (entire record must fit one line).
- No built-in indexing beyond UUID — secondary key lookup requires full scan or an external index.
- Concurrent writers that touch the same UUIDs are rejected — the lock protocol does not provide merge or conflict resolution, only conflict detection.
- The pending section must be periodically compacted to maintain read performance.

---

## 13. Companion Files

| File                 | Purpose                                         |
|---------------------|-------------------------------------------------|
| `target.dov`        | The database file                               |
| `target.dov.lock`   | Queue manifest and `flock()` target             |
| `target.dov.tmp`    | Transient temp file during atomic write/compact |

The `.lock` file persists between runs (zero bytes when idle). The `.tmp` file exists only during write operations and is renamed to `.dov` atomically on completion.

---

## 14. File Extension and Identification

| Property       | Value                                    |
|---------------|------------------------------------------|
| Full name     | Database Oriented Tab Separated Vehicle  |
| Abbreviation  | DOTSV                                    |
| Extensions    | `.dotsv`, `.dov`                         |
| Encoding      | UTF-8 (no BOM)                           |
| Line ending   | `\n` (LF only)                           |

---

## 15. Timestamp Tracking

Every successful write to a `.dov` file appends a timestamp comment as the final line:

```
# YYYYDDMMhhmmss
```

The timestamp is in UTC. The field layout is:

| Segment | Length | Example | Meaning             |
|---------|--------|---------|---------------------|
| `YYYY`  | 4      | `2026`  | Calendar year       |
| `DD`    | 2      | `29`    | Day of month        |
| `MM`    | 2      | `03`    | Month               |
| `hh`    | 2      | `14`    | Hour (24-hour UTC)  |
| `mm`    | 2      | `30`    | Minute              |
| `ss`    | 2      | `22`    | Second              |

Full example: `# 20262903143022`

This comment line is ignored by all existing DOTSV parsers (see §7). The format is human-readable and sortable as a plain string. Appending rather than embedding avoids rewriting the file.

### 15.1 Scope

The timestamp is written after every operation that produces a new `.dov` file, including:

- Normal action file execution (`tsdb <target.dov> <action.atv>`)
- Compaction (`tsdb --compact <target.dov>`)
- After `--relate` completes its implicit compaction

### 15.2 Compaction Behaviour

`--compact` merges the pending section into the sorted section. The resulting file retains exactly the **last timestamp line** from the pre-compaction file as the final line — all earlier timestamp lines accumulated during prior writes are discarded. A new timestamp is then appended reflecting the compaction time.

---

## 16. Related Formats

The DOTSV ecosystem defines three companion file formats that share the same UTF-8 plain-text conventions:

| Format | Extension | Full name                        | Role                                               | Hand-authored? |
|--------|-----------|----------------------------------|----------------------------------------------------|----------------|
| `atsv` | `*.atv`   | Action Tab Separated Vehicle     | Input file for standard `tsdb` write operations   | Yes            |
| `rtsv` | `*.rtv`   | Relation Tab Separated Vehicle   | Generated inverted index over a `.dov` file        | No             |
| `qtsv` | `*.qtv`   | Query Tab Separated Vehicle      | Input file for `tsdb --query` mode                 | Yes            |

### `atsv` (Action TSV)

Formalises the existing action file (previously `action.txt`) as a named format. The format is unchanged: each line is an opcode-prefixed DOTSV record using `+`, `-`, `~`, or `!`. The parser is byte-identical to the DOTSV pending section parser.

### `rtsv` (Relation TSV)

A generated flat three-column index answering: *for a given (key, value) combination, which UUIDs hold it?* Two variants are produced per `.dov` file:

| Variant       | Filename              | Column order          |
|---------------|-----------------------|-----------------------|
| Key-Value     | `<target>.kv.rtv`    | key, value, uuid-list |
| Value-Key     | `<target>.vk.rtv`    | value, key, uuid-list |

Rows are sorted lexicographically by column 1 then column 2, enabling O(log n) binary search. The UUID array in column 3 is `,`-separated (no spaces). The last line is a `# YYYYDDMMhhmmss` timestamp matching the source `.dov` (see §15). Generated by `tsdb --relate`; never hand-authored.

### `qtsv` (Query TSV)

Input format for `tsdb --query`. The optional first line declares `# mode\tunion` or `# mode\tintersect` (default: `intersect`). Each subsequent non-blank, non-comment line is one filter criterion:

| Form           | Syntax            | Lookup                                         |
|----------------|-------------------|------------------------------------------------|
| Bare token     | `<token>`         | Searches both `kv.rtv` col 1 and `vk.rtv` col 1 |
| Key + value    | `<key>\t<value>`  | Exact pair lookup in `kv.rtv`                  |
