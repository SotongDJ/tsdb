# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

`tsdb` is a Rust CLI database runner for a custom flat-file database format called DOTSV (Database Oriented Tab Separated Vehicle). The project is currently in the specification/design phase — the full architecture is documented in `docs/` but the Rust implementation does not yet exist.

## Environment Setup

Rust 1.94.1 is installed but requires sourcing before use:

```bash
source "$HOME/.cargo/env"
```

Add `. "$HOME/.cargo/env"` to `~/.bashrc` to make this permanent.

## Build Commands (once Cargo.toml is created)

```bash
cargo build                        # build
cargo test                         # run all tests
cargo test <test_name>             # run a single test
cargo fmt                          # format code
cargo clippy                       # lint
cargo mutants                      # mutation testing (gitignore includes mutants.out/)
```

Planned dependencies: `memmap2` (memory-mapped file I/O), `memchr` (SIMD byte search), `fs2` (cross-platform flock).

## Architecture

### CLI Invocation

```
tsdb <target.dov> <action.txt>
```

Reads an action file line-by-line and applies operations to a DOTSV database file.

### DOTSV File Format (`docs/DOTSV-whitepaper.md`)

Plain UTF-8 flat files (`.dov` extension). Each record is a single line:
```
UUID<TAB>key=value<TAB>key=value...<LF>
```
- First column is always a 12-char base62-Gu UUID
- Files have two sections: a **sorted section** (binary-searchable) and a **pending section** (appended writes, compacted when it exceeds a threshold)
- Only 4 bytes require escaping: `\t`, `\n`, `\r`, `\`
- Zero-copy parsing via tab-splitting; binary search for lookups

### Base62-Gu UUID System (`docs/base62-whitepaper.md`)

12-character time-sortable identifiers using a 60-char alphabet (standard base62 minus ambiguous `l` and `O`). Structure: `{class}{G}{century}{year}{month}{day}{hour}{minute}{second}{order2}`. The class prefix is user-defined; the `G` character is a fixed format marker.

### Action File Opcodes (`docs/tsdb-whitepaper.md`)

The same DOTSV record format is reused for action files, with a leading opcode byte:

| Opcode | Name   | Behavior                                      |
|--------|--------|-----------------------------------------------|
| `+`    | Append | Insert new record; error on duplicate UUID    |
| `-`    | Delete | Remove record by UUID; error if missing       |
| `~`    | Patch  | Update key-value pairs; error if UUID missing |
| `!`    | Upsert | Insert or replace; never errors               |

Lines starting with `#` are comments; blank lines are ignored.

### Concurrency Model

- **Lock file:** `target.dov.lock` used as both `flock(2)` target and queue manifest
- **UUID-level conflict detection:** Action file is pre-scanned to extract the UUID set; writers with non-overlapping UUID sets can run concurrently
- **Queue protocol:** WAIT → EXEC status transitions; stale entries evicted after 30 seconds
- **Atomic writes:** temp file → rename to prevent corruption

### Key Design Principles

- **Same parser everywhere:** Action file format = DOTSV pending section format (no second grammar)
- **Stream processing:** Constant memory regardless of file size
- **Fail-strict by default:** Conflicts produce errors rather than silent data loss
- **Git-traceable:** Sorted records and deterministic ordering make diffs readable
