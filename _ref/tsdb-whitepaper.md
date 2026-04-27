# tsdb — The DOTSV Database Runner

**Version:** 0.5
**Binary:** `tsdb`
**Usage:** `tsdb <target.dov> <action.txt>`

**Revision history:**
- 0.1 — initial release
- 0.2 — --relate and --query modes; atsv/rtsv/qtsv format support; timestamp tracking
- 0.3 — --plane mode; ptsv (plane inverted-index) format
- 0.4 — array values via repeated keys; `--plane` expands arrays into per-element rows
- 0.5 — `--relate` / `--plane` also emit a `uuid.rtv` / `uuid.ptv` sorted UUID list; `--version` / `--help` flags; `--show` full-record output (`.dtv`); `--filter` mode with comparison operators (`.ftv`); `.ord.ptv` numeric companion plane index (extends `--plane`); `@present` / `@absent` directives in `.qtv`

---

## 1. Overview

`tsdb` is a command-line database runner for DOTSV (`.dov`) files. It accepts a target database file and a plain-text action file, then executes the requested operations.

Design principles:

- **Same parser everywhere** — the action file format is byte-identical to the DOTSV pending section. No new grammar, no new tokenizer.
- **Stream processing** — action files are read line-by-line, never fully loaded into memory.
- **Fail-strict by default** — conflicting operations (duplicate insert, missing delete target) produce errors, not silent data loss.

---

## 2. Invocation

```
tsdb <target.dov> <action.atv>
tsdb --compact <target.dov>
tsdb --relate <target.dov>
tsdb --plane <target.dov>
tsdb --query <query.qtv> <target.dov>
tsdb --query <query.qtv> <target.dov> --show [<out.dtv>|-]
tsdb --filter <filter.ftv> <target.dov>
tsdb --filter <filter.ftv> <target.dov> --show [<out.dtv>|-]
```

| Form                                                                    | Description                                                                     |
|-------------------------------------------------------------------------|---------------------------------------------------------------------------------|
| `tsdb <target.dov> <action.atv>`                                        | Apply operations from an action file to the database                            |
| `tsdb --compact <target.dov>`                                           | Merge the pending section into the sorted section                               |
| `tsdb --relate <target.dov>`                                            | Generate `kv.rtv`, `vk.rtv`, and `uuid.rtv` indexes                              |
| `tsdb --plane <target.dov>`                                             | Generate `kv.ptv`, `vk.ptv`, `uuid.ptv`, and `ord.ptv` indexes (v0.5)            |
| `tsdb --query <query.qtv> <target.dov>`                                 | Run filter criteria against the indexes; print UUIDs                            |
| `tsdb --query <query.qtv> <target.dov> --show [<out.dtv>\|-]`           | (v0.5) Emit full records (stdout default; `-` alias; `<out.dtv>` writes a file) |
| `tsdb --filter <filter.ftv> <target.dov>`                               | (v0.5) Rich predicate filter; print matching UUIDs                              |
| `tsdb --filter <filter.ftv> <target.dov> --show [<out.dtv>\|-]`         | (v0.5) Rich predicate filter; emit full records                                 |
| `tsdb --version`                                                        | Print the tsdb version and exit                                                  |
| `tsdb --help`                                                           | Print the usage message and exit                                                 |

For standard write mode, `tsdb` reads `target.dov` via `mmap`, streams `action.atv` line-by-line, applies each operation, and writes the result back to `target.dov`. Action files may use the `.atv` extension or any other name; the format is identified by content, not extension.

---

## 3. Action File Format

An action file is a UTF-8 text file. Each line is one operation. The format is identical to the DOTSV pending section.

### Example

```
# Add two records
+NGk26cHcv001	name=Alice	city=東京	age=30
+NGk26cHdn002	name=Bob	city=大阪

# Update Alice's city and age
~NGk26cHcv001	city=京都	age=31

# Remove Bob
-NGk26cHdn002

# Upsert Carol (insert if missing, replace if exists)
!EGk26cICK001	name=Carol	city=London
```

- Lines starting with `#` are comments.
- Blank lines are ignored.

### 3.1 Array Fields

An array-valued field is expressed by repeating the same key on one line:

```
+PGk26cHcv001	name=Dave	role=admin	role=editor	role=viewer
```

`tsdb` combines the repeats into a single canonical array value before writing:

```
PGk26cHcv001	name=Dave	role=["admin","editor","viewer"]
```

The on-disk form is a JSON-style array with `"` and `\` element-level escaping;
see DOTSV §4.1 for the formal grammar. Element order is preserved from the
action file — if the same key appears with the same value twice, it appears
twice in the array.

Because the element separator inside the canonical form is `,`, literal commas
inside an element are carried unescaped: `tag=Baker St, London` in a single-key
field and `tag=Baker St, London\ttag=London, UK` as a two-element field both
round-trip through `--plane` without loss.

### 3.2 Shape Validation

A single action-file value MUST NOT look like an array or object literal.
Any value where the first byte is `[` or `{` **and** the last byte is the
matching closer (`]` or `}`) is rejected with an error:

```
+PGk26cHcv001	roles=["admin","editor"]        # rejected — looks like array
+PGk26cHcv001	profile={"city":"Tokyo"}         # rejected — looks like object
```

This ensures arrays enter the database only through the repeated-key mechanism,
and that objects and nested arrays cannot appear at all. Scalar values that
start with `[` or `{` but do not close (`[not-an-array`, `{open`) are allowed.

---

## 4. Opcodes

Four single-byte prefixes define all operations:

| Prefix | Name   | Behavior                               | On Conflict         |
|--------|--------|----------------------------------------|---------------------|
| `+`    | Append | Insert a new record                    | Error if UUID exists |
| `-`    | Delete | Remove a record by UUID                | Error if UUID missing |
| `~`    | Patch  | Update specific KV pairs in a record   | Error if UUID missing |
| `!`    | Upsert | Insert if absent, full replace if present | Never errors      |

### 4.1 Append (`+`)

```
+<uuid>\t<key=value>\t<key=value>\t...\n
```

Inserts a new record. The full set of KV pairs must be provided. If the UUID already exists in the database, `tsdb` reports an error and aborts (or skips, depending on mode).

### 4.2 Delete (`-`)

```
-<uuid>\n
```

Removes the record with the given UUID. No payload beyond the UUID. If the UUID does not exist, `tsdb` reports an error.

### 4.3 Patch (`~`)

```
~<uuid>\t<key=newvalue>\t<key=newvalue>\t...\n
```

Modifies specific key-value pairs in an existing record. Only the changed pairs are listed. Existing pairs not mentioned are preserved unchanged.

Rules:

- To update a value: include the key with the new value.
- To add a new key: include the key with its value (it will be appended to the record).
- To delete a key: include the key with a special tombstone value `\x00` (the null byte, escaped).

If the UUID does not exist, `tsdb` reports an error.

### 4.4 Upsert (`!`)

```
!<uuid>\t<key=value>\t<key=value>\t...\n
```

If the UUID exists, the entire record is replaced with the provided KV pairs. If the UUID does not exist, the record is inserted. This operation never fails due to presence/absence conflicts.

---

## 5. Parsing

The action file parser is a single function — the same one used for the DOTSV pending section:

```rust
enum Action<'a> {
    Append(&'a str, Vec<(&'a str, &'a str)>),
    Delete(&'a str),
    Patch(&'a str, Vec<(&'a str, &'a str)>),
    Upsert(&'a str, Vec<(&'a str, &'a str)>),
    Comment,
    Blank,
}

fn parse_action(line: &str) -> Action<'_> {
    if line.is_empty() {
        return Action::Blank;
    }

    match line.as_bytes()[0] {
        b'#' => Action::Comment,
        b'+' => {
            let rest = &line[1..];
            let mut fields = rest.split('\t');
            let uuid = fields.next().unwrap();
            Action::Append(uuid, parse_kv(fields))
        }
        b'-' => Action::Delete(&line[1..].trim_end()),
        b'~' => {
            let rest = &line[1..];
            let mut fields = rest.split('\t');
            let uuid = fields.next().unwrap();
            Action::Patch(uuid, parse_kv(fields))
        }
        b'!' => {
            let rest = &line[1..];
            let mut fields = rest.split('\t');
            let uuid = fields.next().unwrap();
            Action::Upsert(uuid, parse_kv(fields))
        }
        _ => Action::Blank,  // unknown lines ignored
    }
}

fn parse_kv<'a>(fields: impl Iterator<Item = &'a str>) -> Vec<(&'a str, &'a str)> {
    fields
        .filter_map(|pair| pair.split_once('='))
        .collect()
}
```

**One byte dispatch, then the same `split('\t')` path as record parsing.** No tokenizer, no lookahead, no state machine.

---

## 6. Execution Model

### 6.1 Processing Pipeline

```
                    ┌──────────────┐
action.txt ────────►│  line-by-line │
                    │   streaming   │
                    └──────┬───────┘
                           │
                    ┌──────▼───────┐
                    │  parse opcode │  ◄── 1 byte check
                    │  + split KV   │  ◄── memchr-accelerated
                    └──────┬───────┘
                           │
                    ┌──────▼───────┐
target.dov ◄───────│    apply op   │
  (mmap)           │  to .dov file │
                    └──────────────┘
```

### 6.2 Operation Strategies

| Operation | Strategy                                                       |
|-----------|----------------------------------------------------------------|
| Append    | Binary search for insert position → write to pending section   |
| Delete    | Binary search → mark in pending section                        |
| Patch     | Binary search → in-place overwrite if fits, else pending patch |
| Upsert    | Binary search → overwrite or append depending on existence     |

### 6.3 Compaction

After processing all actions, `tsdb` checks whether the pending section exceeds a configurable threshold (default: 100 lines). If so, it performs a compaction pass:

1. Read sorted section sequentially.
2. Merge pending operations in UUID order.
3. Write the new sorted section.
4. Clear the pending section.

This is a single O(n) sequential pass over the file.

---

## 7. Error Handling

`tsdb` operates in **strict mode** by default:

| Condition                         | Behavior              |
|-----------------------------------|-----------------------|
| `+` with existing UUID                | Error, abort          |
| `-` with missing UUID                 | Error, abort          |
| `~` with missing UUID                 | Error, abort          |
| `!` with any UUID                     | Always succeeds       |
| Malformed line in action file         | Error, abort          |
| Invalid UUID (not 12-char base62-Gu)  | Error, abort          |
| Value shaped like `[...]` or `{...}`  | Error, abort (§3.2)   |

On error, `tsdb` reports the line number in the action file and the offending content. The target `.dov` file is not modified until all actions are validated (or the operation is atomic via a write-to-temp + rename strategy).

---

## 8. Concurrency and Queue Management

Multiple `tsdb` instances can target the same `.dov` file simultaneously. Coordination uses a lock file that acts as both a kernel-level lock and a human-readable queue manifest.

### 8.1 Lock File

```
target.dov.lock
```

The lock file uses `flock()` for atomic metadata access. The lock is held only for microseconds — just long enough to read or update the manifest. All actual `.dov` processing happens outside the lock, so many instances can queue and poll without blocking each other.

**Why not lock the `.dov` directly:** The atomic write strategy does temp → rename, which replaces the file descriptor. A lock on the original fd would be lost. The `.lock` file is stable — never renamed, never rewritten during data operations.

### 8.2 Queue Manifest Format

Each line in the lock file represents one queued `tsdb` instance:

```
<status>\t<process_id>\t<uuid1>,<uuid2>,...\t<timestamp>\n
```

| Field      | Spec                                                    |
|------------|---------------------------------------------------------|
| Status     | `EXEC` (currently running) or `WAIT` (queued)           |
| Process ID | 16 lowercase hex chars, randomly generated at startup   |
| UUID list  | Comma-separated target UUIDs extracted from action file  |
| Timestamp  | Unix epoch seconds, refreshed periodically by `EXEC`    |

Example with three instances:

```
EXEC	a1b2c3d4e5f6a7b8	NGk26cHcv001,NGk26cHdn002,EGk26cICK001	1711700000
WAIT	d9e0f1a2b3c4d5e6	NGk26dAa0001,EGk26dBb0001	1711700005
WAIT	f7a8b9c0d1e2f3a4	NGk26eC10001,NGk26eC20001	1711700008
```

### 8.3 Conflict Detection

Before joining the queue, `tsdb` pre-scans the action file to collect all target UUIDs into a set. It then checks for set intersection against every existing entry in the lock file — both `EXEC` and all `WAIT` entries.

**The rule:**

```
Conflict  =  your_uuids  ∩  any_queued_uuids  ≠  ∅
```

**Opcodes are irrelevant.** Two `+` appends targeting the same UUID conflict identically to a `+` and a `~`, or any other combination. The reasoning: any queued operation ahead of you may alter the record's state before your turn arrives, making your assumptions invalid.

| Instance A   | Instance B (you) | Same UUID? | Result          |
|-------------|------------------|------------|-----------------|
| `+` insert  | `+` insert       | yes        | B rejected      |
| `+` insert  | `~` patch        | yes        | B rejected      |
| `~` patch   | `~` patch        | yes        | B rejected      |
| `~` patch   | `-` delete       | yes        | B rejected      |
| `!` upsert  | `+` insert       | yes        | B rejected      |
| `+` insert  | `+` insert       | no         | Both queue fine |

On conflict, `tsdb` exits immediately without joining the queue.

### 8.4 Two Layers of Validation

Conflict detection and data validation are cleanly separated:

```
Phase 1 — Queue level (before execution):
  Pre-scan action.txt → collect {uuid1, uuid2, ...}
  flock(.lock) → read manifest → check UUID set intersection
  ├── overlap found → error, exit, do not queue
  └── no overlap   → append WAIT line, release lock

Phase 2 — Data level (during execution):
  mmap target.dov → apply each opcode
  + on existing UUID  → error
  - on missing UUID   → error
  ~ on missing UUID   → error
  ! on any UUID       → always ok
```

Queue-level catches cross-process conflicts. Data-level catches logical errors against the actual database state.

### 8.5 Execution Flow

```
tsdb target.dov action.txt

 1. Generate random 16-hex process ID
 2. Pre-scan action.txt → collect all target UUIDs
 3. flock(LOCK_EX) on .lock              ← microseconds
 4. Read .lock manifest
 5. Conflict check (UUID set intersection)
    ├─ overlap → release lock, report error, exit
    └─ clean  → append WAIT line, release lock
 6. Poll loop:
    │  flock(LOCK_EX) briefly
    │  Am I first WAIT and no EXEC?
    │  ├─ yes → change my line to EXEC, release lock, proceed
    │  └─ no  → release lock, sleep, retry
 7. Execute: mmap .dov, apply all actions
 8. Write target.dov.tmp → rename to target.dov
 9. flock(LOCK_EX) briefly
10. Remove my line from .lock
11. Release lock
```

### 8.6 Error Reporting

When a conflict is detected at queue level:

```
error: conflict with process d9e0f1a2b3c4d5e6
  overlapping UUIDs: NGk26dAa0001, EGk26dBb0001
  status: EXEC (running)
  action: aborted, not queued
```

The caller knows exactly which process is in the way, which UUIDs overlap, and whether the conflicting process is running or waiting.

### 8.7 Crash Recovery

`flock()` is released automatically by the kernel when a process exits, including on SIGKILL. However, the dead process's line persists in the manifest.

Resolution: the `EXEC` process refreshes its timestamp periodically (background thread or between action lines). During the poll loop, if the `EXEC` entry's timestamp exceeds a configurable staleness threshold (default: 30 seconds), the next `WAIT` process evicts the stale entry and promotes itself.

```rust
fn is_stale(entry: &QueueEntry, threshold_secs: u64) -> bool {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    now - entry.timestamp > threshold_secs
}
```

### 8.8 Rust Implementation Sketch

```rust
use fs2::FileExt;
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::path::Path;

struct QueueEntry {
    status: String,         // "EXEC" or "WAIT"
    process_id: String,     // 16 hex chars
    uuids: HashSet<String>,
    timestamp: u64,
}

fn enqueue(
    lock_path: &Path,
    my_id: &str,
    my_uuids: &HashSet<String>,
) -> Result<(), ConflictError> {
    let lock_file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(lock_path)?;

    lock_file.lock_exclusive()?;  // brief hold

    let entries = read_manifest(lock_path)?;

    // conflict check against every existing entry
    for entry in &entries {
        let overlap: Vec<_> = entry.uuids
            .intersection(my_uuids)
            .cloned()
            .collect();
        if !overlap.is_empty() {
            lock_file.unlock()?;
            return Err(ConflictError {
                with_process: entry.process_id.clone(),
                with_status: entry.status.clone(),
                overlapping_uuids: overlap,
            });
        }
    }

    // no conflict — join queue
    append_to_manifest(lock_path, "WAIT", my_id, my_uuids)?;
    lock_file.unlock()?;
    Ok(())
}
```

---

## 9. Escaping

The action file uses the same escaping rules as DOTSV:

| Byte   | Escaped Form | Reason                    |
|--------|-------------|---------------------------|
| `\n`   | `\x0A`      | Record/line delimiter     |
| `\t`   | `\x09`      | Field delimiter           |
| `=`    | `\x3D`      | Key-value separator       |
| `\`    | `\\`        | Escape character itself   |

No additional escaping rules at the DOTSV layer. Array elements (§3.1) use a second, independent escape layer — inside an element, `"` → `\"` and `\` → `\\` — which composes cleanly with the outer DOTSV escaping.

---

## 10. Workflow Examples

### 10.1 Bulk Import

```bash
# Generate action file from CSV
awk -F',' '{printf "+%s\tname=%s\tcity=%s\n", $1, $2, $3}' data.csv > import.txt
tsdb mydata.dov import.txt
```

### 10.2 Targeted Update

```bash
# action.txt — update one field on one record
echo '~NGk26cHcv001	status=active' > action.txt
tsdb mydata.dov action.txt
```

### 10.3 Batch Delete

```bash
# Remove multiple records
cat > cleanup.txt << 'EOF'
-NGk26cHcv001
-NGk26cHdn002
-EGk26cICK001
EOF
tsdb mydata.dov cleanup.txt
```

### 10.4 Git-Friendly Workflow

```bash
# Make changes
tsdb users.dov changes.txt

# Compact for clean diff
tsdb users.dov --compact

# Commit
git add users.dov
git commit -m "update user records"
```

### 10.5 Concurrent Access

```bash
# Terminal 1 — modifies records A, B
tsdb data.dov batch1.txt &

# Terminal 2 — modifies records C, D (no UUID overlap → queued behind T1)
tsdb data.dov batch2.txt &

# Terminal 3 — modifies record A (overlaps with T1 → rejected immediately)
tsdb data.dov batch3.txt
# error: conflict with process a1b2c3d4e5f6a7b8
#   overlapping UUIDs: NGk26cHcv001
#   status: EXEC (running)
#   action: aborted, not queued
```

---

## 11. Design Rationale

| Goal                  | Mechanism                                                         |
|-----------------------|-------------------------------------------------------------------|
| Fast parsing          | 1-byte opcode dispatch + `memchr`-accelerated tab split           |
| Zero new syntax       | Action format = DOTSV pending section; one parser for everything  |
| Stream processing     | Line-by-line read; constant memory regardless of action file size |
| Safe by default       | Strict mode catches conflicts; atomic write prevents corruption   |
| Concurrent-safe       | UUID-level conflict detection; flock-based queue; no global lock  |
| Human-authorable      | Plain text, writable by hand, by `echo`, by `awk`, by any tool   |
| Composable            | Action files can be concatenated, diffed, version-controlled      |

---

## 12. Dependencies

| Crate       | Purpose                              |
|-------------|--------------------------------------|
| `memmap2`   | Memory-mapped file I/O               |
| `memchr`    | SIMD-accelerated byte search         |
| `fs2`       | Cross-platform `flock()` wrapper     |

Minimal dependency surface. No serde, no async runtime, no allocation-heavy parsing frameworks.

---

## 13. `--relate` Mode

```
tsdb --relate <target.dov>
```

`--relate` generates a triple of inverted-index files (`rtsv` format) from a `.dov` database. These indexes allow O(log n) lookup of UUIDs by key, value, or key+value pair — without a full scan of the `.dov` file.

### 13.1 Output Files

| File                  | Description                                                   |
|-----------------------|---------------------------------------------------------------|
| `<target>.kv.rtv`    | Key-value index — sorted by (key, value)                       |
| `<target>.vk.rtv`    | Value-key index — sorted by (value, key)                       |
| `<target>.uuid.rtv`  | Sorted list of all UUIDs in the database, one per line         |

The `kv` / `vk` files are flat three-column `rtsv` files: the first two columns are the lookup key, and the third column is a `,`-separated sorted list of UUIDs that hold that pair. The `uuid` file is a single-column list — the sorted set of UUIDs present in the sorted section.

### 13.2 Execution Steps

1. **Compact** — run `--compact` on `<target.dov>`. This ensures the source reflects all pending writes and has a current timestamp footer.
2. **Read timestamp** — read the `# YYYYDDMMhhmmss` comment from the last line of `<target.dov>`.
3. **Check existing indexes** — if all three `.rtv` files exist and their timestamp footers match the `.dov` timestamp exactly, skip regeneration and exit cleanly.
4. **Generate `<target>.kv.rtv`** — stream all sorted-section records; emit one row per (key, value) pair, accumulating UUIDs; sort by (col 1, col 2); write.
5. **Generate `<target>.vk.rtv`** — same pass with columns 1 and 2 swapped; sort by (col 1, col 2); write.
6. **Generate `<target>.uuid.rtv`** — emit each UUID from the sorted section exactly once, in ascending order.
7. **Append timestamp footer** — write `# YYYYDDMMhhmmss` as the final line of each `.rtv` file, using the value read from the `.dov` in step 2.

### 13.3 Skip Condition

```
skip if:
    kv.rtv exists
    AND vk.rtv exists
    AND uuid.rtv exists
    AND kv.rtv last line == dov last line   (exact string match)
    AND vk.rtv last line == dov last line
    AND uuid.rtv last line == dov last line
```

This makes repeated calls to `--relate` on an unchanged database effectively free.

---

## 14. `--plane` Mode

```
tsdb --plane <target.dov>
```

`--plane` generates a triple of fully flattened inverted-index files (`ptsv` format) from a `.dov` database. It is the denormalised counterpart to `--relate`: each `(key, value, uuid)` triple occupies its own row, so there is no array nesting in column 3. In addition, canonical array values in the source record (see §3.1 / DOTSV §4.1) are split at this stage — each element becomes its own col-2 entry and its own row.

### 14.1 Output Files

| File                  | Description                                              |
|-----------------------|----------------------------------------------------------|
| `<target>.kv.ptv`    | Key-value flat index — sorted by (key, value, uuid)      |
| `<target>.vk.ptv`    | Value-key flat index — sorted by (value, key, uuid)      |
| `<target>.uuid.ptv`  | Sorted list of all UUIDs in the database, one per line   |

The `kv` / `vk` files are three-column `ptsv` files with exactly one UUID per row. For a `.rtv` row whose column 3 contains *j* UUIDs, the corresponding `.ptv` produces *j* rows. The `uuid.ptv` file has identical content to `uuid.rtv` — a sorted single-column UUID list — and is emitted from `--plane` so consumers working purely in `ptsv` space do not need to cross formats.

### 14.2 Execution Steps

Identical to `--relate` except the index schema is denormalised and canonical array values are split:

1. **Compact** — run `--compact` on `<target.dov>` so the sorted section reflects all pending writes and the timestamp is current.
2. **Read timestamp** — read the `# YYYYDDMMhhmmss` comment from the last line of `<target.dov>`.
3. **Check existing indexes** — if all three `.ptv` files exist and their timestamp footers match the `.dov` timestamp exactly, skip regeneration and exit cleanly.
4. **Generate `<target>.kv.ptv`** — stream all sorted-section records; for each `(key, value)` pair, if `value` is in canonical array form decode it and emit one row per `(key, element, uuid)` triple, otherwise emit a single `(key, value, uuid)` row; sort by (col 1, col 2, col 3); write.
5. **Generate `<target>.vk.ptv`** — same pass with the array expansion applied in col 1, emitting `(element, key, uuid)` rows.
6. **Generate `<target>.uuid.ptv`** — emit each UUID from the sorted section exactly once, in ascending order.
7. **Append timestamp footer** — write `# YYYYDDMMhhmmss` as the final line of each `.ptv` file, using the value read from the `.dov` in step 2.

A malformed canonical array value in the source `.dov` (e.g. unquoted element, trailing backslash, missing closing bracket) aborts generation with a parse error rather than producing a corrupt or partial index.

### 14.3 Skip Condition

```
skip if:
    kv.ptv exists
    AND vk.ptv exists
    AND uuid.ptv exists
    AND kv.ptv last line == dov last line   (exact string match)
    AND vk.ptv last line == dov last line
    AND uuid.ptv last line == dov last line
```

### 14.4 Relationship to `--relate`

`--plane` and `--relate` are independent. They write to separate files (`*.ptv` vs `*.rtv`) and each maintains its own skip-if-current check. A `.dov` with both commands run will have six index files (`kv`/`vk`/`uuid` in each format). `--query` currently consumes `rtsv`; `ptsv` is provided for external consumers that prefer one-record-per-line output.

---

## 15. `--query` Mode

```
tsdb --query <query.qtv> <target.dov>
```

`--query` executes filter criteria defined in a `qtsv` file against the `rtsv` indexes of a `.dov` database, printing matching UUIDs to stdout.

### 15.1 Execution Steps

1. **Auto-relate** — implicitly run `--relate <target.dov>`. If the skip condition is met the indexes are already current and this is a no-op.
2. **Load indexes** — read `<target>.kv.rtv` and `<target>.vk.rtv` into memory.
3. **Parse `<query.qtv>`** — read the optional mode declaration (default: `intersect`) and each criterion line.
4. **Resolve each criterion**:
   - **Bare token** — search col 1 of both `kv.rtv` and `vk.rtv`; union the resulting UUID sets.
   - **Key + value** — binary search `kv.rtv` on (col 1, col 2); collect UUID array from col 3.
5. **Combine** — apply the declared mode across all resolved UUID sets:
   - `union`: a UUID is included if it satisfies at least one criterion.
   - `intersect`: a UUID is included only if it satisfies all criteria.
6. **Output** — print each matching UUID to stdout, one per line, in lexicographic order.

### 15.2 Query File Format (`qtsv`)

```
# mode	intersect
name	Alice
city
Tokyo
```

- The optional first line declares `# mode\tunion` or `# mode\tintersect`. Default is `intersect`.
- Criterion lines are either a bare token (tab-free) or a key-tab-value pair.
- Comment lines (`#`) and blank lines are ignored.

### 15.3 Output

Plain UUID list, one per line, no headers, no opcode prefixes:

```
NGk26cHcv001
EGk26cICK001
```

Suitable for piping into shell processing or as the basis for generating a new action file.

---

## 16. Related Formats

`tsdb` defines four named input and output formats, all sharing the same UTF-8 plain-text conventions as DOTSV:

| Format | Extension | Full name                        | Role                                          | Created by      |
|--------|-----------|----------------------------------|-----------------------------------------------|-----------------|
| `atsv` | `*.atv`   | Action Tab Separated Vehicle     | Action file input for write operations        | User            |
| `rtsv` | `*.rtv`   | Relation Tab Separated Vehicle   | Inverted index (UUID array in col 3)          | `tsdb --relate` |
| `ptsv` | `*.ptv`   | Plane Tab Separated Vehicle      | Flattened inverted index (one row per UUID)   | `tsdb --plane`  |
| `qtsv` | `*.qtv`   | Query Tab Separated Vehicle      | Query criteria input for `--query` mode       | User            |

### `atsv` (Action TSV)

Formalises the existing action file as a first-class named format. Each line is an opcode-prefixed record using `+`, `-`, `~`, or `!`. Array-valued fields are expressed by repeating a key on the same line (see §3.1); the `atsv` parser combines the repeats into the canonical array form before writing. `atsv` adds one validation pass on top of the DOTSV pending-section grammar: a single value shaped like `[...]` or `{...}` is rejected (§3.2).

### `rtsv` (Relation TSV)

A generated flat three-column inverted index. Two variants are produced per `.dov` file:

- `<target>.kv.rtv` — sorted by (key, value); UUID list in col 3
- `<target>.vk.rtv` — sorted by (value, key); UUID list in col 3

Rows are sorted lexicographically on col 1, then col 2, enabling O(log n) binary search. Canonical array values from the source `.dov` are kept packed in column 2; use `ptsv` if per-element rows are needed. The last line is a `# YYYYDDMMhhmmss` timestamp matching the source `.dov`. Not hand-authored.

### `ptsv` (Plane TSV)

The fully flattened (denormalised) sibling of `rtsv`. Two dimensions are expanded:

- The UUID array in col 3 becomes one row per UUID.
- Canonical array values (DOTSV §4.1) in col 2 become one row per element.

Files:

- `<target>.kv.ptv` — sorted by (key, value, uuid); single UUID in col 3, single element in col 2
- `<target>.vk.ptv` — sorted by (value, key, uuid); single UUID in col 3, single element in col 1

Sort order matches `rtsv` for the first two columns; the uuid becomes the tiebreaker in column 3. The last line is a `# YYYYDDMMhhmmss` timestamp matching the source `.dov`. Generated by `tsdb --plane`; never hand-authored. Designed for shell pipelines (`awk`, `sort -u`, `join`) that expect one record per line, and for per-element filtering on array-valued fields. See §14 for full generation semantics.

### `qtsv` (Query TSV)

Input format for `--query` mode. The optional mode declaration on the first line selects `union` or `intersect` semantics. Criterion lines are bare tokens (searched in both indexes) or `key\tvalue` pairs (exact lookup in `kv.rtv`). v0.5 also accepts `@present\t<key>`, `@absent\t<key>`, and `@absent\t<key>\t<value>` directives — see §20. The leading `@` is a reserved sigil. See §15 for full execution semantics.

---

## 17. `--filter` Mode

```
tsdb --filter <filter.ftv> <target.dov>
tsdb --filter <filter.ftv> <target.dov> --show [<out.dtv>|-]
```

`--filter` (introduced in v0.5) is a rich predicate runner. It auto-runs `--relate` and `--plane`, then evaluates the predicates in `<filter.ftv>` against the resulting indexes and prints matching UUIDs to stdout. Add `--show` to emit full records instead — see §19.

### 17.1 `ftsv` (Filter TSV)

| Property         | Value                          |
|------------------|--------------------------------|
| Extension        | `*.ftv`                        |
| Encoding         | UTF-8, no BOM                  |
| Line ending      | `\n`                           |
| Hand-authored    | Yes                            |
| Default mode     | `intersect`                    |

Each non-blank, non-comment line is either a flat predicate, an `and` / `or` combinator opener, an `end` closer, or a mode declaration (`# mode\tunion` / `# mode\tintersect`).

Operator vocabulary (19 tokens):

| Op                       | Args        | Meaning                                                     | Required index    |
|--------------------------|-------------|-------------------------------------------------------------|-------------------|
| `has`                    | key         | Record has the key (any value)                              | `kv.rtv`          |
| `nohas`                  | key         | Record lacks the key                                        | `kv.rtv` + `uuid.rtv` |
| `eq`                     | key, value  | Record has key=value (lex)                                  | `kv.rtv` (or `kv.ptv` for per-element on arrays) |
| `ne`                     | key, value  | Record has key with value ≠ given (lex; record-level)       | `kv.rtv`          |
| `lt` `le` `gt` `ge`      | key, value  | Lex comparison on stored string                             | `kv.ptv`          |
| `pre` `suf` `sub`        | key, value  | Prefix / suffix / substring match on string value           | `kv.ptv`          |
| `neq` `nne`              | key, value  | Numeric equality / inequality (uses normal-form encoding)   | `ord.ptv`         |
| `nlt` `nle` `ngt` `nge`  | key, value  | Numeric comparison                                          | `ord.ptv`         |

Combinators `and ... end` and `or ... end` group sub-predicates. Children may be predicates or further combinators (depth ≤ 4; deeper nesting is a parse error).

Example:

```
# mode	intersect
has	city
ngt	age	30
or
eq	city	Tokyo
eq	city	Osaka
end
nohas	deleted_at
```

Reads as: city-key set AND age numerically > 30 AND (city=Tokyo OR city=Osaka) AND no `deleted_at` field.

### 17.2 Numeric vs lex semantics

Numeric ops (`n`-prefixed) use `.ord.ptv` and operate on a normal-form encoding of decimal values matching `^-?\d+(\.\d+)?$`. Lex ops (`lt`, `gt`, etc.) operate on the stored string column 2 of `.kv.ptv` — so `"30" < "5"` lex (which catches the user the first time they want a numeric range and reach for `lt`). The explicit prefix split is intentional.

### 17.3 Mixed-type columns

A numeric op against a column where some records hold non-numeric values silently excludes those records and emits one summary line on stderr per offending op (e.g. `warning: 'nlt age 5' skipped 3 record(s) with non-numeric value`). Exit code stays 0.

### 17.4 Per-element semantics on array values

Because `.kv.ptv` and `.ord.ptv` already expand canonical array values (one row per element, see §14 / §18), `--filter` predicates are element-level for array fields. `eq role admin` matches a record holding `role=["admin","editor"]`; `ngt score 50` matches a record holding `score=["10","60","70"]` via the 60 and 70 elements.

`ne` is record-level (set difference): `ne done true` returns records that have `done` AND whose value differs from `true`, so it does NOT include records lacking the key entirely. To get the asymmetric "lacks-key OR differs" semantics, compose `or\nnohas\tdone\nne\tdone\ttrue\nend` — or use `@absent\tdone\ttrue` from `.qtv`.

---

## 18. `*.ord.ptv` Numeric Companion Plane Index

```
<target>.ord.ptv
```

Generated by `--plane` alongside the existing three `.ptv` files (v0.5). One row per (numeric value, key, uuid) triple, sorted by `(norm, key, raw-value, uuid)`.

| Column | Meaning                                                         |
|--------|-----------------------------------------------------------------|
| 1      | `norm` — sortable numeric-normal-form encoding (P/N + magnitude) |
| 2      | key                                                              |
| 3      | raw-value — the original string from `.dov` (NOT normalised)     |
| 4      | uuid                                                             |

The last line is a `# YYYYDDMMhhmmss` footer matching the source `.dov`.

### 18.1 Numeric-normal form (`norm`)

```
norm     = sign , magnitude
sign     = "P" for non-negative, "N" for negative
magnitude (non-negative): <4-digit int-len> "_" <int-part> [ "." <fraction-trimmed> ]
magnitude (negative):     same shape, but each digit (and the int-len digits) digit-complemented (0↔9, 1↔8, …),
                          with a trailing "~" terminator so that shorter fractional strings sort AFTER longer ones lex.
```

Worked examples:

| Raw | norm |
|-----|------|
| `0` | `P0001_0` |
| `5` | `P0001_5` |
| `30` | `P0002_30` |
| `100` | `P0003_100` |
| `3.14` | `P0001_3.14` |
| `-5` | `N9998_4~` |
| `-30` | `N9997_69~` |
| `-3.14` | `N9998_6.85~` |

The `~` terminator on the negative side fixes a subtle lex-ordering hazard with variable-length fractional parts (e.g. `-3.1` vs `-3.14`); a property test exercises 1000 random pairs against the `f64` reference.

### 18.2 Skip-if-current rule (extended for v0.5)

`--plane` now skips regeneration only when **all four** companion files (`kv.ptv`, `vk.ptv`, `uuid.ptv`, `ord.ptv`) exist and carry footers matching the source `.dov`. The first run after upgrading a v0.5 install rewrites all four — the three legacy outputs are byte-identical (modulo a refreshed footer if `.dov` itself changed in the meantime).

### 18.3 What is excluded

Values that don't match `^-?\d+(\.\d+)?$` (scientific notation, hex, leading zeros except a lone `0`, leading whitespace, `+`-prefixed, etc.) are not represented in `.ord.ptv`. Numeric ops cannot match such records — see §17.3 for the warning behaviour.

---

## 19. `--show` Modifier and `*.dtv`

`--show` is a modifier on `--query` and `--filter` that emits full DOTSV records instead of bare UUIDs.

### 19.1 Forms

```
... --show               → records to stdout
... --show -             → records to stdout (alias; lets scripts always pass an arg)
... --show <out.dtv>     → atomic write to file
```

A path argument that begins with `-` other than the lone `-` alias is rejected at argv-parse time with exit code 2. To use such a path, pass it as `./<path>` or place it in a directory.

### 19.2 `dtsv` (Display TSV)

| Property | Value |
|----------|-------|
| Extension | `*.dtv` |
| Encoding | UTF-8 |
| Line ending | `\n` |
| Hand-authored | No — output of `--query --show` / `--filter --show` only |

Each non-footer line is a complete DOTSV record: `<uuid>\t<key=value>\t...`. Records sort by UUID (matching the `.dov` sorted section). KV pairs within a record are key-sorted (matching `Record::serialize`). Canonical array values stay packed (`role=["admin","editor"]`); `--show` is record-level, never per-element.

Footer: `# YYYYDDMMhhmmss`, exact-byte copy of the `.dov` footer at execution time.

### 19.3 Skip-if-current (file mode)

`--show <out.dtv>` skips regeneration iff:
- `<out.dtv>` exists,
- its last line equals the `.dov` footer,
- AND `mtime(criterion-file)` < `mtime(<out.dtv>)`.

Stdout mode is never skipped. Skipped runs print `skipped: <out.dtv> already current` to stderr.

### 19.4 Argument ordering

`--show` and its optional path argument always trail the second positional (the `.dov`). This keeps the existing 4-arg `--query <qtv> <dov>` invocation byte-identical.

---

## 20. `.qtv` Field-Absence Directives (`@present` / `@absent`)

In v0.5 the `.qtv` grammar gains three reserved directives:

| Form              | Syntax                       | Semantics                                                                          |
|-------------------|------------------------------|------------------------------------------------------------------------------------|
| Present           | `@present\t<key>`            | UUIDs that have at least one binding for `<key>` (any value)                       |
| Absent            | `@absent\t<key>`             | UUIDs that have **no** binding for `<key>`                                         |
| Absent (key+val)  | `@absent\t<key>\t<value>`    | UUIDs lacking the exact `key=value` pair (lacks key entirely OR has a different value) |

The leading `@` is reserved as the directive sigil at the start of a line. An unknown `@xxx` line is rejected with `unknown qtv directive '@xxx'`. To use a value beginning with `@` in a regular two-column criterion, place it after the tab — only the first column is checked for the sigil.

The universe of UUIDs for absence resolution is read from `<stem>.uuid.rtv` (already produced by `--relate` since v0.5). It is loaded lazily — only when at least one absence criterion exists.

### 20.1 Semantic asymmetry vs `.ftv`'s `ne k v`

`@absent\tdone\ttrue` (in `.qtv`) returns: records without `done` ∪ records with `done` ≠ `true`.

`ne done true` (in `.ftv`) returns only the second set — records that have `done` AND whose value differs. To get `@absent` semantics inside `.ftv`, compose `or\nnohas\tdone\nne\tdone\ttrue\nend`. The asymmetry is documented and pinned by tests.
