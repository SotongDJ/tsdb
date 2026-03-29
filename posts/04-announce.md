<!--break type:header content-->
title = "Announcing tsdb v0.1"
date = "2026-03-29 12:00:00+08:00"
short = ["announce"]
categories = ["Announcement"]
<!--break type:content format:md content-->
`tsdb` is a command-line database runner for DOTSV flat files — a plain UTF-8 format designed for high throughput, minimal memory use, git-friendly diffs, and concurrent-safe writes. Today we publish the v0.1 specification along with the whitepapers that define the file format and UUID system.

<!--excerpt-->

## What is tsdb?

`tsdb` reads a plain-text action file line-by-line and applies each operation to a DOTSV (`.dov`) database file:

```
tsdb <target.dov> <action.txt>
tsdb <target.dov> --compact
```

A `.dov` file is just UTF-8 text. Every record occupies exactly one line:

```
<12-char-UUID>	<key=value>	<key=value>...\n
```

Because records are plain text, you can inspect, filter, and diff a `.dov` file with any standard Unix tool: `grep`, `awk`, `diff`, or any text editor. No binary formats, no query engines, no schemas.

## Why DOTSV?

Most lightweight data storage options force a trade-off between human readability and performance. DOTSV sidesteps this by exploiting a simple observation: if records are sorted by UUID and UUIDs embed a timestamp, then:

- **Binary search** on a memory-mapped file gives O(log n) lookup with zero deserialization.
- **Git diffs** are minimal and meaningful — each changed record is a single line change at a deterministic position.
- **Writes are O(1)** by appending to a pending section; compaction merges pending into sorted in a single O(n) sequential pass.

## Key Design Goals

### Same parser everywhere

The action file format is byte-identical to the DOTSV pending section. There is no second grammar to learn, no second tokenizer to write, and no impedance mismatch between what you write to a `.dov` file and what you put in an action file. One function parses both.

### Stream processing

`tsdb` never loads an action file fully into memory. It processes one line at a time, keeping memory usage constant regardless of file size. The target `.dov` file is accessed via `mmap` for zero-copy reads.

### Fail-strict by default

Any conflict — attempting to insert a UUID that already exists, or deleting a UUID that doesn't — aborts the entire run. The `.dov` file is not modified until all operations in the action file are validated. This all-or-nothing approach prevents partial writes. The one exception is the `!` (upsert) opcode, which is explicitly idempotent by design.

### Git-traceable

Because UUIDs sort lexicographically and records are written in that order, `git diff` on a `.dov` file produces one-line insertions and deletions at predictable positions. A database file in version control becomes a readable, auditable change log.

## Action File Opcodes

Four single-byte prefixes cover all operations:

| Opcode | Name | Behavior |
|--------|------|----------|
| `+` | Append | Insert new record — error if UUID exists |
| `-` | Delete | Remove record — error if UUID missing |
| `~` | Patch | Update named fields — error if UUID missing |
| `!` | Upsert | Full replace or insert — never errors |

Comments start with `#`; blank lines are ignored.

## Concurrency

Multiple `tsdb` instances targeting the same `.dov` file are coordinated through a companion lock file (`target.dov.lock`). The key insight: instances with **non-overlapping UUID sets** can run concurrently. Only instances that would modify the same UUIDs are serialized. This allows high write throughput when workloads partition naturally by record type or time range.

Each instance pre-scans its action file, extracts the target UUID set, and checks for intersection against all queued entries. On conflict, it exits immediately without joining the queue. No global lock, no blocking reads.

## Build Instructions

Requires Rust 1.94+.

```bash
source "$HOME/.cargo/env"
cargo build --release
# binary: target/release/tsdb
```

Dependencies: `memmap2` (memory-mapped file I/O), `memchr` (SIMD byte search), `fs2` (cross-platform flock).

## Documentation

The full technical specifications are published as whitepapers on this site:

- [DOTSV Whitepaper](/tsdb/posts/dotsv-whitepaper/) — file format, binary search, concurrency protocol
- [tsdb Whitepaper](/tsdb/posts/tsdb-whitepaper/) — CLI opcodes, execution model, error handling
- [Base62 Encoding System Whitepaper](/tsdb/posts/base62-whitepaper/) — the UUID encoding system

The [Docs](/tsdb/docs/) page has a concise technical reference for quick lookup.
