<!--break type:header content-->
title = "DOTSV Whitepaper"
date = "2026-03-29 04:00:00+08:00"
short = ["dotsv-whitepaper"]
categories = ["Whitepaper", "Reference"]
<!--break type:content format:md content-->
DOTSV (Database Oriented Tab Separated Vehicle) is a single-line-per-record flat-file database format designed for high processing rate, low memory footprint, git-traceable diffs, full Unicode support, and human readability.

<!--excerpt-->

**Version:** 0.1 Draft
**File extensions:** `*.dotsv`, `*.dov`
**MIME type:** `text/dotsv`

---

## 1. Overview

DOTSV is a single-line-per-record, flat-file database format designed for:

- High processing rate via `mmap` and zero-copy parsing
- Low memory footprint — data is borrowed directly from the memory-mapped buffer
- Git-traceable diffs — deterministic record ordering produces minimal, meaningful changesets
- Full Unicode support — CJK, emoji, accented characters pass through unescaped
- Human readability — plain text, editable in any text editor

A `.dov` file is a valid UTF-8 text file. Every tool that operates on text (`grep`, `awk`, `diff`, `cat`) works on DOTSV without modification.

---

## 2. Record Format

Each record occupies exactly one line (`\n`-terminated). A record consists of a fixed-width base62-Gu UUID followed by one or more tab-separated key-value pairs.

```
<12-char-base62-Gu-uuid>	<key=value>	<key=value>	...\n
```

### Example

```
NGk26cHcv001	name=Alice	city=Tokyo	age=30
NGk26cHdn002	name=Bob	city=Osaka	msg=hello world
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

| Position | Length | Segment | Example | Notes |
|----------|--------|---------|---------|-------|
| 1 | 1 | Class prefix | `N` | Record type (uppercase A–Z) |
| 2 | 1 | Format marker | `G` | Always literal `G` |
| 3 | 1 | Century | `k` | `2026 // 100 = 20` — Format-G char |
| 4–5 | 2 | Year (2d) | `26` | Decimal year mod 100 |
| 6 | 1 | Month | `c` | Format-G month table |
| 7 | 1 | Day | `H` | Format-G day table |
| 8 | 1 | Hour | `c` | Format-G hour table |
| 9 | 1 | Minute | `v` | Format-G 60-char alphabet |
| 10 | 1 | Second | `0` | Format-G 60-char alphabet |
| 11–12 | 2 | Order number | `01` | Collision resolution (01–99, then base62) |

### 3.2 Properties

| Property | Specification |
|----------|--------------|
| Width | Exactly 12 bytes (fixed) |
| Character set | `[A-Z]G[0-9a-km-zA-NP-Z]{8}[0-9a-zA-Z]{2}` |
| Byte position | `line[0..12]` |
| Followed by | `\t` at byte 12 |

Fixed width enables direct slicing (`&line[..12]`) without scanning.

### 3.3 Sort Order Semantics

Records naturally group and order as:

1. **By class** — all `E`-class records sort before `N`-class, etc.
2. **By time within class** — the embedded timestamp sorts chronologically within each class prefix.
3. **By order within second** — the 2-char suffix breaks ties for records created in the same second.

### 3.4 Format-G Alphabet

```
0123456789abcdefghijkmnopqrstuvwxyzABCDEFGHIJKLMNPQRSTUVWXYZ
```

This ensures UUIDs are unambiguous when read, typed, or printed in any font.

---

## 4. Key-Value Pairs

After the UUID and its trailing tab, the remainder of the line is a sequence of `key=value` pairs delimited by `\t`.

- Keys and values are UTF-8 strings.
- Keys MUST NOT be empty.
- Values MAY be empty (e.g., `tag=`).
- Key order within a record is not significant for semantics, but implementations SHOULD maintain a consistent order for git diff stability.

---

## 5. Escaping Rules

DOTSV uses backslash-hex escaping. Only four bytes require escaping when they appear inside a key or value:

| Byte | Escaped Form | Reason |
|------|-------------|--------|
| `\n` | `\x0A` | Record delimiter |
| `\t` | `\x09` | Field delimiter |
| `=` | `\x3D` | Key-value separator |
| `\` | `\\` | Escape character itself |

### Optional

| Byte | Escaped Form | Reason |
|------|-------------|--------|
| `\r` | `\x0D` | Windows line-ending safety |

### What is NOT escaped

Everything else passes through literally: CJK characters (Tokyo, Osaka), emoji, accented Latin characters, spaces, punctuation, and all other printable Unicode. Most real-world values need zero escaping, enabling the zero-copy fast path: if no `\` is present in a slice, borrow it directly from the underlying buffer.

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
+<uuid>	<key=val>	...\n       insert
-<uuid>\n                       delete
~<uuid>	<key=newval>	...\n   patch (changed pairs only)
```

| Prefix | Operation | Payload |
|--------|-----------|---------|
| `+` | Insert | UUID + all KV pairs |
| `-` | Delete | UUID only |
| `~` | Patch | UUID + changed KV pairs |

**Read path:** Binary search the sorted section, then linear scan the pending section for overrides.
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

| Condition | Strategy |
|-----------|---------|
| New line length is less than or equal to old line length | Overwrite in place, pad with trailing spaces before `\n` |
| New line length is greater than old line length | Append a `~` patch to the pending section |

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

| Goal | Mechanism |
|------|-----------|
| Fast query | Sorted UUIDs + `mmap` + binary search = O(log n) |
| Fast parse | Tab-split is `memchr`-SIMD accelerated; fixed-width 12-char UUID avoids scanning |
| Low memory | Zero-copy borrows from `mmap`; no HashMap, no deserialization tree |
| Fast writes | O(1) append to pending section |
| Fast deletes | O(1) append `-` opcode to pending section |
| Fast modify | In-place overwrite when size fits; else O(1) patch append |
| Git-traceable | Sorted records = one-line insertion diffs; deterministic key order within records |
| Human-readable | Plain UTF-8 text; minimal escaping; tab-aligned columns in most editors |
| Full Unicode | Only 4 control bytes escaped; all other Unicode is literal |

---

## 11. Concurrency — Lock File Protocol

DOTSV defines a companion lock file for coordinating concurrent access by multiple `tsdb` instances.

### 11.1 Lock File

For every `.dov` file, a corresponding `.dov.lock` file serves as both a kernel-level lock target and a queue manifest.

```
target.dov          data
target.dov.lock     flock() target + queue manifest
```

**Why a separate file:** The atomic write strategy replaces the `.dov` via temp-file to rename, which invalidates any `flock()` held on the original fd. The `.lock` file is stable — never renamed, never rewritten during data operations.

### 11.2 Lock File Contents

The lock file contains one line per queued `tsdb` instance:

```
<status>	<process_id>	<uuid1>,<uuid2>,...	<timestamp>\n
```

| Field | Spec |
|-------|------|
| Status | `EXEC` (currently running) or `WAIT` (queued) |
| Process ID | 16 lowercase hex chars, randomly generated at startup |
| UUID list | Comma-separated target UUIDs extracted from action file |
| Timestamp | Unix epoch seconds, refreshed periodically by `EXEC` |

### 11.3 Conflict Detection

**The rule:** your UUID set intersected with any queued UUID set must be empty, or conflict is detected.

Opcodes are irrelevant — any queued operation ahead of you may alter the record's state before your turn arrives.

### 11.4 Stale Entry Eviction

Any entry whose timestamp exceeds a configurable staleness threshold (default: 30 seconds) is automatically evicted. This prevents a crashed writer from blocking the queue indefinitely.
