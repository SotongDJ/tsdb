# tsdb Extensions — Technical Note

**Version:** 0.3 Draft
**Date:** 2026-04-22
**Scope:** Four new file formats (`atsv`, `rtsv`, `ptsv`, `qtsv`), three new running modes (`--relate`, `--plane`, `--query`), and canonical array values with per-element expansion in `ptsv`.

---

## Naming Note

This note covers four format names. The original working names (`atsv`, `rotsv`, `qtsv`) have been reconciled, and `ptsv` has been added as the flattened counterpart of `rtsv`:

| Working name | Settled name | Extension | Role |
|---|---|---|---|
| `atsv` | `atsv` | `*.atv` | Action file format (formalises current `action.txt`) |
| `rotsv` / `rtsv` | `rtsv` | `*.rtv` | Relation index file (generated, not hand-authored) |
| `ptsv` | `ptsv` | `*.ptv` | Plane (flattened) index file (generated, not hand-authored) |
| `qtsv` | `qtsv` | `*.qtv` | Query input format for `--query` mode |

`rtsv` is settled over `rotsv` because the extension `*.rtv` (three letters, consistent with `*.atv`, `*.ptv`, `*.qtv`) follows the same pattern and the body of the spec uses `rtsv` throughout.

---

## 1. `atsv` — Action TSV

**Full name:** Action Tab Separated Vehicle
**Extension:** `*.atv`
**Role:** Formalises the existing unnamed action file (currently `action.txt`) as a first-class format.

### 1.1 Overview

`atsv` is the input format for standard `tsdb` invocation:

```
tsdb <target.dov> <action.atv>
```

The format is unchanged from the current action file specification — this entry assigns it a name, extension, and MIME type so it can be referenced by other formats and tools.

### 1.2 Format

Identical to the DOTSV pending section. Each line is one operation:

```
+<uuid>\t<key=value>\t...\n    append (insert new record)
-<uuid>\n                      delete (remove by UUID)
~<uuid>\t<key=value>\t...\n    patch (update specific KV pairs)
!<uuid>\t<key=value>\t...\n    upsert (insert or replace)
# <comment>\n                  comment line, ignored
\n                             blank line, ignored
```

Array-valued fields use **repeated keys** on the same line:

```
+PGk26cHcv001\tname=Dave\trole=admin\trole=editor\trole=viewer
```

During parse, repeated keys are combined into a single canonical array value
before the record is committed to the database:

```
PGk26cHcv001\tname=Dave\trole=["admin","editor","viewer"]
```

Element order is preserved from the action file; duplicates are kept. The
canonical form is a JSON-style array of double-quoted elements with
element-level escaping `"`→`\"` and `\`→`\\` — see DOTSV §4.1 for the grammar.

### 1.3 Shape Validation

The `atsv` parser rejects any single value whose first byte is `[` or `{`
**and** whose last byte is the matching closer (`]` or `}`). This prevents a
caller from smuggling an already-formatted array or object through a single
key=value pair — arrays must be expressed via the repeated-key mechanism, and
objects and nested arrays are not supported at all. Non-closing prefixes
(`[open`, `{open`) pass through as ordinary scalar values.

### 1.4 Properties

| Property | Value |
|---|---|
| Encoding | UTF-8, no BOM |
| Line ending | `\n` (LF only) |
| Escaping | Same as DOTSV (`\x09`, `\x0A`, `\x3D`, `\\`) plus element-level `\"` / `\\` inside canonical arrays |
| Schema | DOTSV pending-section parser + repeated-key coalescing + array/object shape rejection |

---

## 2. `rtsv` — Relation TSV

**Full name:** Relation Tab Separated Vehicle
**Extension:** `*.rtv`
**Role:** A generated inverted index over the key-value content of a `.dov` file. Not hand-authored.

### 2.1 Overview

An `rtsv` file is a flat three-column index derived from a DOTSV database. It answers the question: *for a given (key, value) combination, which UUIDs hold it?*

Two variants are generated from each `.dov`, differentiated by column order:

| Variant | Filename | Column order | Optimised for |
|---|---|---|---|
| Key-Value | `<target>.kv.rtv` | `key`, `value`, `uuids` | Lookup by key or key+value |
| Value-Key | `<target>.vk.rtv` | `value`, `key`, `uuids` | Lookup by value |

### 2.2 Record Format

Each row is a single tab-separated line with exactly three columns:

```
<col1>\t<col2>\t<uuid1>,<uuid2>,...\n
```

| Column | kv.rtv | vk.rtv |
|---|---|---|
| 1 | key | value |
| 2 | value | key |
| 3 | sorted UUID array (`,`-separated) | sorted UUID array (`,`-separated) |

UUIDs in column 3 are sorted lexicographically (same order as the sorted section of the source `.dov`).

### 2.3 Example

Source DOTSV records:

```
NGk26cHcv001	name=Alice	city=Tokyo	age=30
NGk26cHdn002	name=Bob	city=Tokyo
EGk26cICK001	name=Carol	city=London	age=30
```

`target.kv.rtv` (sorted by col 1, then col 2):

```
age	30	EGk26cICK001,NGk26cHcv001
city	London	EGk26cICK001
city	Tokyo	NGk26cHcv001,NGk26cHdn002
name	Alice	NGk26cHcv001
name	Bob	NGk26cHdn002
name	Carol	EGk26cICK001
# 20262903143022
```

`target.vk.rtv` (sorted by col 1, then col 2):

```
30	age	EGk26cICK001,NGk26cHcv001
Alice	name	NGk26cHcv001
Bob	name	NGk26cHdn002
Carol	name	EGk26cICK001
London	city	EGk26cICK001
Tokyo	city	NGk26cHcv001,NGk26cHdn002
# 20262903143022
```

### 2.4 Sorting

Rows are sorted lexicographically by column 1, then by column 2. This enables binary search on the first column for O(log n) key or value lookup.

### 2.5 Timestamp Footer

The last line of every `rtsv` file is a timestamp comment matching the latest timestamp recorded in the source `.dov`:

```
# YYYYDDMMhhmmss
```

This value is used by `--relate` to decide whether regeneration is needed (see §6.3).

### 2.6 Properties

| Property | Value |
|---|---|
| Encoding | UTF-8, no BOM |
| Line ending | `\n` (LF only) |
| Columns | Exactly 3, tab-separated |
| UUID separator | `,` (no spaces) |
| Sort order | Lexicographic on col 1, then col 2 |
| Sections | None — fully compacted, no sorted/pending split |
| Hand-authoring | Not intended; generated by `--relate` only |

---

## 3. `ptsv` — Plane TSV

**Full name:** Plane Tab Separated Vehicle
**Extension:** `*.ptv`
**Role:** A generated flat inverted index — the denormalised counterpart of `rtsv`. Each `(col1, col2, uuid)` triple occupies its own row. Not hand-authored.

### 3.1 Overview

A `ptsv` file is a flat three-column index derived from a DOTSV database. It answers the same question as `rtsv` (*which UUIDs hold this key-value pair?*), but with **two forms of flattening**:

1. No UUID array nesting in column 3 — one UUID per row.
2. No canonical-array values in column 2 — each array element becomes its own row.

A record with `role=["admin","editor","viewer"]` held by UUID `PGk26cHcv001` produces three `ptsv` rows:

```
role	admin	PGk26cHcv001
role	editor	PGk26cHcv001
role	viewer	PGk26cHcv001
```

Two variants are generated from each `.dov`, differentiated by column order:

| Variant | Filename | Column order | Optimised for |
|---|---|---|---|
| Key-Value | `<target>.kv.ptv` | `key`, `value`, `uuid` | Shell pipelines keyed by key/value |
| Value-Key | `<target>.vk.ptv` | `value`, `key`, `uuid` | Shell pipelines keyed by value |

### 3.2 Record Format

Each row is a single tab-separated line with exactly three columns and exactly one UUID:

```
<col1>\t<col2>\t<uuid>\n
```

| Column | kv.ptv | vk.ptv |
|---|---|---|
| 1 | key | value |
| 2 | value | key |
| 3 | single UUID | single UUID |

If the corresponding `rtsv` row would carry *j* UUIDs in col 3, `ptsv` emits *j* rows — one per UUID. Additionally, if the source record's value is a canonical array with *m* elements, each element contributes its own set of rows (so one `(key, array-value, uuid-list)` pair in `rtsv` fans out to `m × j` rows in `ptsv`). Literal commas, brackets, and quotes inside array elements are preserved verbatim (the array codec escapes only `"` and `\` inside each element and decodes them back on expansion).

### 3.3 Example

Using the same source records as §2.3:

```
NGk26cHcv001	name=Alice	city=Tokyo	age=30
NGk26cHdn002	name=Bob	city=Tokyo
EGk26cICK001	name=Carol	city=London	age=30
```

`target.kv.ptv` (sorted by col 1, col 2, col 3):

```
age	30	EGk26cICK001
age	30	NGk26cHcv001
city	London	EGk26cICK001
city	Tokyo	NGk26cHcv001
city	Tokyo	NGk26cHdn002
name	Alice	NGk26cHcv001
name	Bob	NGk26cHdn002
name	Carol	EGk26cICK001
# 20262903143022
```

`target.vk.ptv` (sorted by col 1, col 2, col 3):

```
30	age	EGk26cICK001
30	age	NGk26cHcv001
Alice	name	NGk26cHcv001
Bob	name	NGk26cHdn002
Carol	name	EGk26cICK001
London	city	EGk26cICK001
Tokyo	city	NGk26cHcv001
Tokyo	city	NGk26cHdn002
# 20262903143022
```

### 3.4 Sorting

Rows are sorted lexicographically by column 1, then column 2, then column 3. This is the order `rtsv` would produce if its comma-separated UUID arrays were expanded and split — but with every UUID materialised as its own record.

### 3.5 Timestamp Footer

Identical to `rtsv`: the last line is a `# YYYYDDMMhhmmss` comment matching the source `.dov`. This value drives the `--plane` skip-if-current check (see §7.3).

### 3.6 Properties

| Property | Value |
|---|---|
| Encoding | UTF-8, no BOM |
| Line ending | `\n` (LF only) |
| Columns | Exactly 3, tab-separated |
| UUIDs per row | Exactly 1 |
| Sort order | Lexicographic on col 1, then col 2, then col 3 |
| Sections | None — fully compacted, no sorted/pending split |
| Hand-authoring | Not intended; generated by `--plane` only |

---

## 4. `qtsv` — Query TSV

**Full name:** Query Tab Separated Vehicle
**Extension:** `*.qtv`
**Role:** Input file for `--query` mode. Defines filter criteria; output is matching UUIDs on stdout.

### 4.1 Overview

A `qtsv` file contains one filter criterion per line. Each criterion is matched against the `rtsv` indexes. The first line of the file MAY declare a filter mode.

### 4.2 Filter Mode Declaration

The first line MAY be a mode declaration:

```
# mode\tunion
# mode\tintersect
```

| Mode | Behaviour |
|---|---|
| `union` | A UUID is included if it satisfies **at least one** criterion |
| `intersect` | A UUID is included only if it satisfies **all** criteria |

If the first line is not a mode declaration, `intersect` is used by default.

### 4.3 Criterion Lines

Each subsequent non-blank, non-comment line is one criterion. Three forms are supported:

| Form | Syntax | Lookup path |
|---|---|---|
| Key only | `<key>` | Search `kv.rtv` column 1; collect all UUIDs for that key regardless of value |
| Value only | `<value>` | Search `vk.rtv` column 1; collect all UUIDs for that value regardless of key |
| Key + value | `<key>\t<value>` | Search `kv.rtv` columns 1 and 2; collect UUIDs for that exact (key, value) pair |

**Disambiguation of single-token lines:** A bare token (no tab) is searched in both `kv.rtv` (col 1) and `vk.rtv` (col 1). The UUID sets from both hits are unioned before the criterion set operation is applied. This avoids requiring the author to know whether a token is a key or a value.

### 4.4 Example

```
# mode	union
city
Tokyo
name	Alice
```

With the example data from §2.3:


- `city` → hits `kv.rtv` key=city → `{NGk26cHcv001, NGk26cHdn002, EGk26cICK001}`; also searches `vk.rtv` value=city → no hit
- `Tokyo` → hits `vk.rtv` value=Tokyo → `{NGk26cHcv001, NGk26cHdn002}`; also searches `kv.rtv` key=Tokyo → no hit
- `name\tAlice` → hits `kv.rtv` (name, Alice) → `{NGk26cHcv001}`

Union of all three: `{NGk26cHcv001, NGk26cHdn002, EGk26cICK001}`

If mode were `intersect`: `{NGk26cHcv001, NGk26cHdn002, EGk26cICK001}` ∩ `{NGk26cHcv001, NGk26cHdn002}` ∩ `{NGk26cHcv001}` = `{NGk26cHcv001}`

### 4.5 Properties

| Property | Value |
|---|---|
| Encoding | UTF-8, no BOM |
| Line ending | `\n` (LF only) |
| Default mode | `intersect` |
| Escaping | Same as DOTSV (keys and values may contain `\x09`-escaped tabs) |
| Comments | Lines starting with `#` are ignored (except the mode declaration) |
| Blank lines | Ignored |

---

## 5. Timestamp Tracking

All `tsdb` operations that write to a `.dov` file must append a timestamp comment as the final line:

```
# YYYYDDMMhhmmss
```

| Segment | Length | Example | Meaning |
|---|---|---|---|
| `YYYY` | 4 | `2026` | Calendar year |
| `DD` | 2 | `29` | Day of month |
| `MM` | 2 | `03` | Month |
| `hh` | 2 | `14` | Hour (24-hour clock) |
| `mm` | 2 | `30` | Minute |
| `ss` | 2 | `22` | Second |

Full example: `# 20262903143022`

### 5.1 Scope

The timestamp is appended after every successful write to the `.dov` file, including:

- Normal action file execution (`tsdb <target.dov> <action.atv>`)
- Compaction (`tsdb --compact <target.dov>`)
- After `--relate` completes its implicit compaction
- After `--plane` completes its implicit compaction

### 5.2 Compaction Behaviour

`--compact` merges the pending section into the sorted section. The resulting file retains exactly the **last timestamp line** from the pre-compaction file as the final line. All earlier timestamp lines (if any accumulated during prior writes) are discarded during compaction. A new timestamp is then appended reflecting the compaction time.

---

## 6. `--relate` Mode

### 6.1 Invocation

```
tsdb --relate <target.dov>
```

### 6.2 Behaviour

1. **Compact** — run `--compact` on `<target.dov>` before index generation. This ensures the source is fully merged and its timestamp is current.
2. **Read timestamp** — read the timestamp from the last line of `<target.dov>`.
3. **Check existing index** — if `<target>.kv.rtv` and `<target>.vk.rtv` both exist, read their timestamp footers.
   - If both footers match the `.dov` timestamp exactly → skip regeneration, exit cleanly.
   - Otherwise → regenerate both files.
4. **Generate `<target>.kv.rtv`** — stream all records from the sorted section, emit one row per (key, value) pair, accumulating UUIDs; sort by (col 1, col 2); write.
5. **Generate `<target>.vk.rtv`** — same pass but with columns 1 and 2 swapped; sort by (col 1, col 2); write.
6. **Append timestamp footer** — write `# YYYYDDMMhhmmss` as the final line of each `.rtv` file, using the `.dov` timestamp read in step 2.

### 6.3 Skip Condition

```
skip if:
    kv.rtv exists
    AND vk.rtv exists
    AND kv.rtv last line == dov last line  (exact string match on timestamp comment)
    AND vk.rtv last line == dov last line
```

### 6.4 Output Files

| File | Description |
|---|---|
| `<target>.kv.rtv` | Key-value inverted index |
| `<target>.vk.rtv` | Value-key inverted index |

---

## 7. `--plane` Mode

### 7.1 Invocation

```
tsdb --plane <target.dov>
```

### 7.2 Behaviour

`--plane` mirrors `--relate` but emits `ptsv` files instead of `rtsv` files, and splits canonical array values along the way:

1. **Compact** — run `--compact` on `<target.dov>`.
2. **Read timestamp** — read the timestamp from the last line of `<target.dov>`.
3. **Check existing index** — if `<target>.kv.ptv` and `<target>.vk.ptv` both exist and their footers match the `.dov` timestamp → skip regeneration.
4. **Generate `<target>.kv.ptv`** — stream all records from the sorted section; for each `(key, value)` pair, if the value is in canonical array form decode it and emit one row per `(key, element, uuid)` triple, otherwise emit a single `(key, value, uuid)` row; sort by (col 1, col 2, col 3); write.
5. **Generate `<target>.vk.ptv`** — same pass with the array expansion applied on the col-1 side, emitting `(element, key, uuid)` rows.
6. **Append timestamp footer** — write `# YYYYDDMMhhmmss` as the final line of each `.ptv` file.

A malformed canonical array value in the source `.dov` (unquoted element, trailing backslash, missing closing bracket) aborts generation with a parse error rather than producing a partial or corrupt index.

### 7.3 Skip Condition

```
skip if:
    kv.ptv exists
    AND vk.ptv exists
    AND kv.ptv last line == dov last line
    AND vk.ptv last line == dov last line
```

### 7.4 Output Files

| File | Description |
|---|---|
| `<target>.kv.ptv` | Key-value flat index |
| `<target>.vk.ptv` | Value-key flat index |

### 7.5 Relationship to `--relate`

`--plane` and `--relate` are independent. They write to disjoint file sets (`*.ptv` vs `*.rtv`), each with its own skip-if-current check, and running one does not imply running the other. `--query` consumes `rtsv`; `ptsv` exists for external consumers that prefer one-record-per-line output suitable for shell pipelines (`join`, `sort -u`, `awk`, `comm`).

---

## 8. `--query` Mode

### 8.1 Invocation

```
tsdb --query <input.qtv> <target.dov>
```

### 8.2 Behaviour

1. **Auto-relate** — run `--relate <target.dov>` implicitly. If the index is current (skip condition met), this is a no-op.
2. **Load indexes** — read `<target>.kv.rtv` and `<target>.vk.rtv` into memory (they are expected to be small relative to the `.dov`).
3. **Parse `input.qtv`** — read filter mode (default: `intersect`) and criterion lines.
4. **Resolve each criterion** — look up in the appropriate index:
   - Key-only or value-only: binary search col 1 of the appropriate `.rtv`; collect UUID array from col 3.
   - Single bare token: search both indexes; union the UUID sets from both hits.
   - Key+value: binary search `kv.rtv` on (col 1, col 2); collect UUID array from col 3.
5. **Combine** — apply the filter mode across all resolved UUID sets:
   - `union`: take the union of all sets.
   - `intersect`: take the intersection of all sets.
6. **Output** — print each matching UUID to stdout, one per line, in lexicographic order.

### 8.3 Output

```
NGk26cHcv001
NGk26cHdn002
```

Plain UUID list, one per line, no headers, no opcode prefixes. Suitable for piping into further `tsdb` action file generation or shell processing.

---

## 9. Companion Files Summary

| File | Created by | Purpose |
|---|---|---|
| `target.dov` | user / `tsdb` write | DOTSV database |
| `target.dov.lock` | `tsdb` | Concurrency queue manifest |
| `target.dov.tmp` | `tsdb` | Transient atomic write buffer |
| `target.kv.rtv` | `tsdb --relate` | Key-value inverted index |
| `target.vk.rtv` | `tsdb --relate` | Value-key inverted index |
| `target.kv.ptv` | `tsdb --plane` | Key-value flat index (one row per UUID) |
| `target.vk.ptv` | `tsdb --plane` | Value-key flat index (one row per UUID) |
| `action.atv` | user | Action file (append/delete/patch/upsert) |
| `query.qtv` | user | Query criteria file |

---

## 10. Design Rationale

| Decision | Reason |
|---|---|
| Arrays via repeated keys + canonical on-disk form | Keeps the action file format (`key=value` repeats) trivial to generate by hand or by script. Storing the combined value in canonical JSON-style form on disk gives `--plane` an unambiguous shape to detect — no heuristic comma-splitting that could misclassify literal commas in scalar values. |
| `--relate` keeps arrays packed; `--plane` expands them | `rtsv` is for binary search — expanding arrays would bloat the row count and break the one-row-per-`(key, value)` invariant. `ptsv` is already row-per-UUID, so also splitting on array elements is a natural extension that lets shell tools filter on individual list members. |
| Reject `[...]` / `{...}`-shaped scalar values in `atsv` | Prevents ambiguity between a string that happens to look like an array and an actual array value. Combined with the repeated-key mechanism, this closes the only path by which a nested or object-shaped value could reach the database. |
| `rtsv` is generated, not hand-authored | The index is a deterministic derivative of the `.dov`; hand-editing would cause drift. The skip-if-current check makes regeneration cheap. |
| Two `rtsv` variants (kv and vk) | Separate sort orders enable O(log n) binary search for key-first and value-first lookups without a secondary index or full scan. |
| UUID array in `rtsv` col 3 is `,`-separated (not tab) | Tab is already the column delimiter. Comma avoids a second escaping layer and keeps rows machine-readable without a recursive parser. |
| `ptsv` alongside `rtsv` | `rtsv` optimises for in-process binary search; its comma-joined UUID column requires a second parse step before shell tools can consume it. `ptsv` denormalises the same data into one-record-per-line so `awk`, `sort -u`, `comm`, and `join` work with no extra parser. Both indexes describe the same relation — users pick the shape that matches the consumer. |
| Three-column sort on `ptsv` (col 1, col 2, col 3) | Adding the UUID as the final sort key keeps the total order deterministic across regenerations and matches what `rtsv` would emit if its UUID arrays were expanded. |
| `--plane` separate from `--relate` | The two modes produce different file sets with different consumers. Running them independently lets callers build only the indexes they need; sharing the skip-if-current machinery keeps the CLI surface symmetric. |
| `--relate` compacts before indexing | The index must reflect the final merged state, not a state split across sorted and pending sections. Compaction is O(n) and idempotent. |
| `--query` auto-invokes `--relate` | Callers should not need to know the index lifecycle. The skip condition makes the implicit call free when the index is current. |
| Timestamp in `# YYYYDDMMhhmmss` format | A comment line at the end is ignored by all existing DOTSV parsers. The format is human-readable and sortable as a plain string. Appending rather than embedding avoids rewriting the file. |
| `--compact` keeps only the last timestamp | Accumulated timestamps from incremental writes are noise after compaction. One timestamp per compacted file is sufficient and keeps the file clean for diffs. |
| `qtsv` bare-token searches both indexes | A user writing `Tokyo` should not need to know whether `Tokyo` is a key or a value in the target database. Searching both is safe; the union cost is bounded by the size of the UUID arrays in the index. |
