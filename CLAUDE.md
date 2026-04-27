# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

`tsdb` is a Rust CLI database runner for a custom flat-file database format called DOTSV (Database Oriented Tab Separated Vehicle). The implementation is complete and production-built. Architecture is documented in `_ref/`; source is in `src/`.

## Environment Setup

Rust 1.94.1 is installed but requires sourcing before use:

```bash
source "$HOME/.cargo/env"
```

Add `. "$HOME/.cargo/env"` to `~/.bashrc` to make this permanent.

## Build Commands

```bash
cargo build                        # debug build
cargo build --release              # production build → target/release/tsdb
cargo test                         # run all tests (382 tests as of v0.6)
cargo test <test_name>             # run a single test
cargo fmt                          # format code
cargo clippy                       # lint
cargo mutants                      # mutation testing (gitignore includes mutants.out/)
```

Dependencies: `fs2` (cross-platform flock), `rand` (random process ID for lock queue).

## Architecture

### CLI Invocation

```
tsdb <target.dov> <action.txt>
```

Reads an action file line-by-line and applies operations to a DOTSV database file.

### DOTSV File Format (`_ref/DOTSV-whitepaper.md`)

Plain UTF-8 flat files (`.dov` extension). Each record is a single line:
```
UUID<TAB>key=value<TAB>key=value...<LF>
```
- First column is always a 12-char base62-Gu UUID
- Files have two sections: a **sorted section** (binary-searchable) and a **pending section** (appended writes, compacted when it exceeds 100 lines)
- Only 5 bytes require escaping: `\t`, `\n`, `\r`, `=`, `\`
- Zero-copy parsing via tab-splitting; binary search for lookups

### Base62-Gu UUID System (`_ref/base62-whitepaper.md`)

12-character time-sortable identifiers using a 60-char alphabet (standard base62 minus ambiguous `l` and `O`). Structure: `{class}{G}{century}{year}{month}{day}{hour}{minute}{second}{order2}`. The class prefix is user-defined; the `G` character is a fixed format marker.

### Action File Opcodes (`_ref/tsdb-whitepaper.md`)

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

### Source Layout

| File | Responsibility |
|------|---------------|
| `src/main.rs` | CLI entry point, orchestration, lock lifecycle |
| `src/dotsv.rs` | DOTSV file: parse, binary search, apply ops, compact, atomic write |
| `src/action.rs` | Action file parser (`+` / `-` / `~` / `!`) |
| `src/lock.rs` | Lock file queue: register, promote, refresh, release |
| `src/escape.rs` | DOTSV backslash-hex escaping / unescaping |
| `src/base62.rs` | Base62-Gu UUID validation only (12-char, user-supplied) |
| `src/error.rs` | Unified `TsdbError` enum |

### Key Design Principles

- **Same parser everywhere:** Action file format = DOTSV pending section format (no second grammar)
- **Stream processing:** Constant memory regardless of file size
- **Fail-strict by default:** Conflicts produce errors rather than silent data loss
- **Git-traceable:** Sorted records and deterministic ordering make diffs readable
- **UUIDs are user-supplied:** `tsdb` validates 12-char base62-Gu UUIDs but never generates them

### Code Review History

Four rounds of review between Sofia (Sonnet, code) and Matt (Opus, review). All issues resolved. See `reviews/` for full history.

## Shorthand Commands

| Command | Meaning |
|---------|---------|
| `kk` | (1) Find every `?vTimestamp` or `?v{14-digit-timestamp}` in the project (e.g. `include_files/head.html`) and replace with `?v{YYYYDDMMhhmmss}` using current time. (2) `git add -A`. (3) `git commit -S` with a descriptive message. |

## Whitepaper Pages

Whitepaper pages are live mirrors of the `_ref/` sources. Always read the current source before creating or updating:

- `_ref/DOTSV-whitepaper.md` → `page_files/05-dotsv-whitepaper.html` (path `/dotsv-whitepaper/`)
- `_ref/tsdb-whitepaper.md` → `page_files/06-tsdb-whitepaper.html` (path `/tsdb-whitepaper/`)

## Developing Round Workflow

Used for significant new features (new CLI flags, new file formats, semantics changes). Four-agent process; each round records to `_temp/proposal/round-N/` (git-ignored under `_temp/`).

### Roles

| Agent  | Role                  | Output file                 |
|--------|-----------------------|-----------------------------|
| Apple  | Independent designer  | `apple.md`                  |
| Orange | Independent designer  | `orange.md`                 |
| Banana | Consolidator          | `banana.md`                 |
| Pie    | Strict reviewer       | `pie.md` (APPROVE / REJECT) |

Apple and Orange MUST design without seeing each other's work — diverse proposals are the point.

### Flow

1. **Apple + Orange in parallel** — spawn both in a single message (two `Agent` calls, `run_in_background: true`, model: opus). Each gets the same self-contained brief: project context (CLAUDE.md, README.md, relevant `src/`, `_ref/`), the gaps to solve, hard constraints (existing CLI surface and output bytes unchanged; new features get NEW flags AND NEW file extensions; no new third-party deps), and required deliverables (CLI surface, new file-format grammar, algorithm, backward-compat ledger, edge cases, named `#[test] fn ...` test plan).
2. **Banana consolidates** — reads both proposals, resolves conflicts, produces one merged design with a Decisions Ledger (per-divergence justification, what was rejected and why) and a Risk Register.
3. **Pie reviews** — strict structured verdict: Findings (severity + location + required fix), per-gap completeness check, backward-compat audit (verdict per existing artefact), test-plan audit by category. Any genuine defect is REJECT.
4. **REJECT** → Apple and Orange revise per Pie's per-agent task list → Banana re-merges → Pie re-reviews. Repeat.
5. **APPROVE** → a builder agent implements per Pie's commit-order guidance, absorbing minor findings inline. Builder appends progress to `build-log.md` in the round directory and does NOT commit (leaves working tree dirty for user review).

### Conventions

- Track each round with `TaskCreate`/`TaskUpdate` and `blockedBy` chains: Apple+Orange → Banana → Pie → Implementer.
- New requirements arriving mid-round go into the next round's `requirements.md`, not the current round.
- Each approved round typically bumps the project version (e.g. round 1 = v0.4 → v0.5, round 2 = v0.5 → v0.6).
- The user commits at the end. Do NOT commit on their behalf unless asked.
