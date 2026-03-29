# tsdb

A command-line database runner for DOTSV flat files.

---

## Overview

`tsdb` reads an action file line-by-line and applies each operation to a DOTSV (`.dov`) database file. It is designed for high throughput, minimal memory use, and clean git diffs.

```
tsdb <target.dov> <action.txt>
tsdb <target.dov> --compact
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

## Whitepapers

Full format specifications are in `docs/`:

- `docs/DOTSV-whitepaper.md` — file format, escaping, binary search, concurrency protocol
- `docs/tsdb-whitepaper.md` — CLI opcodes, execution model, error handling
- `docs/base62-whitepaper.md` — base62-Gu UUID encoding system
