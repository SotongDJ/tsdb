# tsdb — The DOTSV Database Runner

**Version:** 0.1 Draft
**Binary:** `tsdb`
**Usage:** `tsdb <target.dov> <action.txt>`

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
tsdb target.dov action.txt
```

| Argument      | Description                                    |
|---------------|------------------------------------------------|
| `target.dov`  | The DOTSV database file to operate on          |
| `action.txt`  | Plain-text file containing operations to apply |

`tsdb` reads `target.dov` via `mmap`, streams `action.txt` line-by-line, applies each operation, and writes the result back to `target.dov`.

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
| `+` with existing UUID           | Error, abort          |
| `-` with missing UUID            | Error, abort          |
| `~` with missing UUID            | Error, abort          |
| `!` with any UUID                | Always succeeds       |
| Malformed line in action file     | Error, abort          |
| Invalid UUID (not 12-char base62-Gu) | Error, abort          |

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

No additional escaping rules. The action format is a strict subset of DOTSV.

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
