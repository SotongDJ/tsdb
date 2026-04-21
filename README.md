# tsdb

A command-line database runner for DOTSV flat files.

---

## Overview

`tsdb` reads an action file line-by-line and applies each operation to a DOTSV (`.dov`) database file. It is designed for high throughput, minimal memory use, and clean git diffs.

```
tsdb <target.dov> <action.atv>          apply actions to database
tsdb <target.dov> --compact             compact pending section
tsdb --relate <target.dov>              generate .kv.rtv and .vk.rtv indexes
tsdb --plane <target.dov>               generate .kv.ptv and .vk.ptv indexes
tsdb --query <query.qtv> <target.dov>   query database, print matching UUIDs
```

---

## DOTSV File Format

Each record occupies exactly one line:

```
<12-char-UUID>\t<key=value>\t<key=value>...\n
```

Files are plain UTF-8 text — readable with `grep`, `awk`, `diff`, or any editor.

### Two-Section Structure

```
<sorted section>          ← binary-searchable, lexicographic UUID order
                          ← blank line separator
<pending section>         ← write-ahead log, O(1) appends
```

The sorted section supports O(log n) lookup. The pending section is a write-ahead log compacted into the sorted section when it exceeds 100 lines. Compaction is a single sequential O(n) pass.

### UUID Column

The first column is always a **12-character base62-Gu UUID** — a time-sortable, class-prefixed identifier:

```
{C}{G}{century}{YY}{M}{D}{h}{m}{s}{XX}
 1   1    1      2   1  1  1  1  1   2   = 12 chars
```

- `C` — uppercase class prefix (user-defined record type)
- `G` — fixed format marker
- Timestamp encoded in Format-G 60-char alphabet (standard base62 minus `l` and `O`)
- `XX` — 2-char order suffix for collision resolution within the same second

UUIDs sort lexicographically by class, then chronologically within each class. `tsdb` validates but never generates UUIDs — they are always user-supplied.

### Escaping

Only four bytes require escaping inside keys or values:

| Byte | Escaped |
|------|---------|
| `\t` | `\x09` |
| `\n` | `\x0A` |
| `=`  | `\x3D` |
| `\`  | `\\`   |

Everything else — CJK, emoji, accented Latin, spaces — passes through unescaped.

---

## Action File

An action file is UTF-8 text. Each line is one operation, using the same format as the DOTSV pending section:

```
# comment (ignored)

+NGk26cHcv001	name=Alice	city=東京	age=30
+NGk26cHdn002	name=Bob	city=大阪

~NGk26cHcv001	city=京都	age=31

-NGk26cHdn002

!EGk26cICK001	name=Carol	city=London
```

### Opcodes

| Opcode | Name   | Behavior                                         |
|--------|--------|--------------------------------------------------|
| `+`    | Append | Insert new record — error if UUID exists         |
| `-`    | Delete | Remove record — error if UUID missing            |
| `~`    | Patch  | Update named fields — error if UUID missing      |
| `!`    | Upsert | Full replace if exists, insert if not — no error |

**Patch semantics:** list only the fields to change. To delete a field, set its value to `\x00`. A patch that would remove all fields is rejected.

**Strict mode (default):** any conflict aborts the run. The `.dov` file is not modified until all operations are validated.

---

## Common Operations

### Append a record

Create `input.txt` with a `+` line — one tab-separated field per column after the UUID:

```
+NGk26cHcv001	name=Alice	city=東京	age=30
```

```bash
tsdb mydb.dov input.txt
```

Errors if `NGk26cHcv001` already exists.

### Modify a record

Use `~` and list only the fields to change. Unlisted fields are left untouched:

```
~NGk26cHcv001	city=京都	age=31
```

To delete a specific field, set its value to `\x00`:

```
~NGk26cHcv001	age=\x00
```

Errors if the UUID does not exist.

### Delete a record

Use `-` with just the UUID:

```
-NGk26cHcv001
```

Errors if the UUID does not exist.

### Combining operations

Multiple operations in one file are validated together, then applied atomically:

```
+PGk26cHcv001	name=Dave	role=admin
~NGk26cHcv001	city=大阪
-NGk26cHdn002
```

```bash
tsdb mydb.dov input.txt
```

---

## `--relate` — Build Inverted Indexes

`--relate` compacts a `.dov` file and generates two companion index files that support fast key/value lookups:

```bash
tsdb --relate users.dov
# creates: users.kv.rtv  (key → value → UUIDs)
#          users.vk.rtv  (value → key → UUIDs)
```

Each index is a plain tab-separated file with exactly three columns:

```
# users.kv.rtv
city	London	EGk26cICK001
city	Tokyo	NGk26cHcv001,NGk26cHdn002
name	Alice	NGk26cHcv001
name	Bob	NGk26cHdn002
# 20262903143022
```

The final line is a timestamp matching the source `.dov`. If both index files are already current (same timestamp), `--relate` is a no-op — safe to call before every query.

---

## `--plane` — Build Flat Inverted Indexes

`--plane` is the flattened counterpart of `--relate`. Instead of packing the UUID list into a single comma-separated cell, each UUID occupies its own row:

```bash
tsdb --plane users.dov
# creates: users.kv.ptv  (one row per key/value/UUID triple)
#          users.vk.ptv  (one row per value/key/UUID triple)
```

Equivalent output for the example above:

```
# users.kv.ptv
city	London	EGk26cICK001
city	Tokyo	NGk26cHcv001
city	Tokyo	NGk26cHdn002
name	Alice	NGk26cHcv001
name	Bob	NGk26cHdn002
# 20262903143022
```

If `col1` has `i` distinct `col2` values and each pair has `j` UUIDs, the file contains `i × j` rows. Choose `--plane` when downstream tools want one record per line (`join`, `sort -u`, `awk`) and `--relate` when compact binary search is the priority. The two modes write to separate files and each has its own skip-if-current check.

---

## `--query` — Filter Records by Key / Value

`--query` reads a `.qtv` query file, auto-runs `--relate`, then prints to stdout the UUIDs of every record that satisfies the criteria.

**Query file format** (`lookup.qtv`):

```
# mode	union
city	Tokyo
name	Alice
```

- First line (optional): `# mode\tunion` or `# mode\tintersect` (default: intersect)
- Criterion lines:
  - `key\tvalue` — records where that exact key=value pair exists
  - bare token — records where the token appears as either a key **or** a value

```bash
tsdb --query lookup.qtv users.dov
# stdout (union mode — Tokyo OR Alice):
NGk26cHcv001
NGk26cHdn002
```

```bash
# intersect mode — must satisfy ALL criteria
cat > exact.qtv << 'EOF'
# mode	intersect
city	Tokyo
name	Alice
EOF
tsdb --query exact.qtv users.dov
# stdout:
NGk26cHcv001
```

### Typical workflow

```bash
# 1. Build / update indexes once after any writes
tsdb --relate users.dov

# 2. Run queries (--relate is also auto-invoked, but skipped when current)
tsdb --query find-admins.qtv users.dov | while read uuid; do
    grep "^$uuid" users.dov
done
```

---

## Concurrency

Multiple `tsdb` instances targeting the same `.dov` file are coordinated through a companion lock file (`target.dov.lock`).

### Protocol

1. Pre-scan action file → collect all target UUIDs
2. `flock` the `.lock` file exclusively (held for microseconds)
3. Check UUID set intersection against all queued entries — **reject immediately on any overlap**
4. Append a `WAIT` entry to the queue manifest; release lock
5. Poll until promoted to `EXEC` (no blocking EXEC with overlapping UUIDs ahead)
6. Execute; refresh timestamp periodically
7. Atomic write: `target.dov.tmp` → rename → `target.dov`
8. Remove own entry from queue manifest

### Lock File Format

```
EXEC\t<16-hex-pid>\t<uuid1,uuid2,...>\t<unix-timestamp>\n
WAIT\t<16-hex-pid>\t<uuid1,uuid2,...>\t<unix-timestamp>\n
```

Instances with **non-overlapping UUID sets can run concurrently**. Stale entries (timestamp > 30 seconds old) are evicted automatically. Compact-only mode (`--compact`) treats its UUID set as "all records" and waits for all other writers to finish.

---

## Companion Files

| File | Purpose |
|------|---------|
| `target.dov` | Database |
| `target.dov.lock` | Queue manifest and `flock` target |
| `target.dov.tmp` | Transient during atomic write (auto-cleaned) |
| `target.kv.rtv` | Key-value inverted index (generated by `--relate`) |
| `target.vk.rtv` | Value-key inverted index (generated by `--relate`) |
| `target.kv.ptv` | Key-value flat index (generated by `--plane`) |
| `target.vk.ptv` | Value-key flat index (generated by `--plane`) |

---

## Building

Requires Rust 1.94+.

```bash
source "$HOME/.cargo/env"
cargo build --release
# binary: target/release/tsdb
```

Dependencies: `fs2` (cross-platform flock), `rand` (random process ID).

---

## Whitepapers and References

Full format specifications are in `_ref/`:

- `_ref/DOTSV-whitepaper.md` — file format, escaping, binary search, concurrency protocol, timestamp tracking
- `_ref/tsdb-whitepaper.md` — CLI opcodes, execution model, `--relate`, `--plane`, `--query`
- `_ref/base62-whitepaper.md` — base62-Gu UUID encoding system
- `_ref/extensions-technote.md` — technical note for atsv, rtsv, ptsv, qtsv formats
