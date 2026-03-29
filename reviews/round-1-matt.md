# Matt's Review — Round 1

**Date:** 2026-03-29

## Summary

I reviewed the full tsdb codebase against the DOTSV and tsdb specifications. The implementation is a solid start but has significant gaps: dead code from UUID generation that tsdb never needs, spec violations in how upsert and lock PIDs are handled, potential panics in non-test code, unused dependencies, and several correctness issues in escape handling, action validation, and the compaction path. Twenty-seven issues are listed below, including two withdrawn after closer inspection.

---

## Issues

**1.** `Cargo.toml` — `rand` dependency is unused. Remove it (or keep for random PID per issue 2).

**2.** `src/lock.rs` ~line 54 — Process ID uses `std::process::id()` instead of random 16 lowercase hex chars. Spec requires random 128-bit value formatted as 16 hex chars.

**3.** `src/base62.rs` lines 159-251 — Dead code: `generate_uuid`, `unix_to_datetime`, `is_leap_year`, `days_to_year_doy`, `doy_to_month_day`. tsdb only validates UUIDs, never generates them. Remove all generation code, the `SystemTime`/`UNIX_EPOCH` import, and tests `test_generate_uuid_valid`/`test_generate_uuid_class`.

**4.** `src/base62.rs` line 26 — `FORMAT_G`, `MONTH_TABLE`, `DAY_TABLE`, `HOUR_TABLE` are `pub` but only used internally. Reduce to `pub(crate)` or private.

**5.** `src/base62.rs` line 42 — `fg_encode` uses `assert!` (panic) instead of `Result`. Non-test code must not panic on bad input. (Will be removed with issue 3.)

**6.** `src/dotsv.rs` line 101 — Method named `from_str` on a type that doesn't implement `std::str::FromStr`. Rename to `parse_str`.

**7.** `src/dotsv.rs` lines 258-264 — `apply_single_action` writes `!`-prefixed lines to the pending section. Spec says pending only contains `+`, `-`, `~`. Upsert must resolve to `+` or `~` before writing to pending.

**8.** *(Withdrawn — `"\x00"` in Rust is the null byte, comparison is correct.)*

**9.** `src/dotsv.rs` lines 94-121 — Sorted section records not validated on load. Each line must be checked for >=13 bytes and a valid 12-char UUID prefix. Return `TsdbError::Parse` on failure.

**10.** `src/dotsv.rs` line 108 — Edge case: file starting with a blank line is treated as having no sorted section. Minor, worth documenting.

**11.** `src/dotsv.rs` lines 348-356 — In-place patch never shrinks padded width. Not incorrect, but wasteful. Awareness note.

**12.** *(Withdrawn — `dedup()` after `sort()` correctly removes consecutive duplicates.)*

**13.** `src/main.rs` lines 104-121 — Validation then re-application is O(n^2): `uuid_exists` scans pending each time. Refactor to call `apply_all_actions` once with the full slice.

**14.** `src/main.rs` line 109 — `apply_actions` called once per action via `std::slice::from_ref`. Needlessly roundabout. Pass full slice at once.

**15.** `src/main.rs` lines 46-51 — `run_compact_only` bypasses the lock protocol entirely. A concurrent EXEC writer could have its writes lost via temp-rename. Must acquire exclusive lock before compacting.

**16.** `src/escape.rs` lines 17-28 — `escape` escapes `=` in keys too. Spec says `=` only needs escaping in values. Harmless but not strictly correct. Acceptable as-is.

**17.** `src/escape.rs` line 80 — `bytes[i] as char` is unsafe for multi-byte UTF-8. Iterating bytes and casting to `char` produces mojibake for non-ASCII input. Rewrite using `chars()` or byte-slice copying.

**18.** `src/escape.rs` line 49 — `unescape` accepts `\X` (uppercase). Spec only defines `\x` lowercase. Remove `b'X'` arm.

**19.** `src/action.rs` lines 122-127 — Append with zero KV fields is accepted. Spec requires at least one KV pair for Append. Add validation.

**20.** `src/action.rs` line 39 — Redundant `.map_err(|e| TsdbError::Io(e))`. Since `From<io::Error>` is implemented, use `?` directly.

**21.** `src/lock.rs` line 120 — Lock path uses `format!("{}.lock", dov_path.display())`. `Display` may not round-trip on paths with non-UTF-8 bytes. Use `OsString` API: push `.lock` onto the `OsStr`.

**22.** `src/lock.rs` line 235 — `try_promote` blocks on *any* EXEC entry. Spec allows concurrent writers with non-overlapping UUID sets. Fix: only block if existing EXEC has overlapping UUIDs.

**23.** `src/lock.rs` lines 226-231 — Stale WAIT entries never evicted in `try_promote`. Queue can grow unboundedly after crashes. Evict stale WAIT entries too.

**24.** `src/dotsv.rs` — `memmap2` and `memchr` listed in `Cargo.toml` but never used. Implementation uses `fs::read`. Remove unused dependencies.

**25.** `src/main.rs` line 15 — `REFRESH_INTERVAL_SECS` interaction with action loop is fine. No fix needed.

**26.** `src/main.rs` lines 132-222 — Integration test temp dirs use nanosecond timestamp for uniqueness. Parallel tests could collide. Add a random suffix using `rand`.

**27.** `src/dotsv.rs` line 470 — `atomic_write` temp file naming is correct for `.dov` files. Minor awareness note.

**28.** `src/action.rs` lines 186-189 — `find_unescaped_equals` skips 4 bytes for any `\x` escape without validating hex digits. Bad input gives confusing error. Add hex-digit validation with a clear error message.

**29.** `src/dotsv.rs` line 402 — (Same as issue 7) Upsert writes `!` to pending when new line is longer. Must write `+` instead.

**30.** `src/dotsv.rs` line 142 — `write_to` always writes blank separator line even when pending is empty. Omit separator when pending section is empty.

---

REVIEW COMPLETE — 27 issues found.
