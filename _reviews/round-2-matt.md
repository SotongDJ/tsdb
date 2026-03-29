# Matt's Review — Round 2

**Date:** 2026-03-29

## Summary

All 27 round-1 issues have been addressed by Sofia. No regressions found on previously-fixed items. Three new actionable issues were identified.

## Round-1 Fix Verification

| R1 Issue | Status |
|----------|--------|
| 1 (rand dep) | Fixed — rand kept and used |
| 2 (random PID) | Fixed — rand::random::<u64>() at lock.rs:55 |
| 3 (dead generation code) | Fixed — removed |
| 4 (pub visibility) | Fixed — pub(crate) |
| 5 (panic in fg_encode) | Fixed — removed with dead code |
| 6 (from_str rename) | Fixed — now parse_str |
| 7+29 (upsert ! in pending) | Fixed — writes + prefix |
| 9 (sorted section validation) | Fixed — UUID validated on load |
| 13+14 (O(n²) apply loop) | Fixed — full slice at once |
| 15 (compact bypasses lock) | Fixed — acquires lock |
| 17 (UTF-8 mojibake) | Fixed — uses chars() |
| 18 (uppercase \X) | Fixed — removed |
| 19 (append zero KV) | Fixed — rejected |
| 20 (redundant map_err) | Fixed |
| 21 (lock path Display) | Fixed — OsString API |
| 22 (try_promote blocks all EXEC) | Fixed — only blocks overlapping |
| 23 (stale WAIT eviction) | Fixed — retain non-stale |
| 24 (unused deps) | Fixed — removed |
| 26 (test temp dir collision) | Fixed — random suffix |
| 28 (find_unescaped_equals hex) | Fixed — hex validation |
| 30 (separator when empty) | Fixed — omitted when empty |
| 8,10,11,12,16,25,27 | Withdrawn/awareness — no change needed |

## New Issues Found

**Issue 1.** `src/main.rs` lines 119–121 — **Dead timestamp refresh block.**
`last_refresh.elapsed()` is checked immediately after `Instant::now()` — elapsed is always ~0ns, so the condition is never true and `lock_mgr.refresh_timestamp()` is unreachable.
Fix: capture `Instant::now()` *before* `apply_actions`, then check elapsed after it returns. Or remove the dead block if refresh is not needed given the single-pass apply.

**Issue 2.** `src/main.rs` line 48 / `src/lock.rs` — **Compact-only empty UUID set bypasses all EXEC blocking.**
Compact-only mode registers with an empty UUID set. Since empty ∩ anything = empty, the overlap check never blocks, and `try_promote` promotes even while another EXEC writer is active. But compaction rewrites the entire file via `atomic_write` — a concurrent EXEC writer's changes will be silently lost.
Fix: treat an empty UUID set as "touches all UUIDs" in both `do_register` conflict detection and `try_promote` blocking logic.

**Issue 3.** `src/dotsv.rs` line 47 — **`Record::parse` strips trailing spaces from entire raw line before field splitting.**
`trim_end_matches(' ')` is applied to the full line before tab-splitting. This strips in-place padding correctly, but also strips legitimate trailing spaces from the last field's value (e.g., `name=Alice   ` loses the spaces). This is a silent data-loss bug for edge cases.
Fix: strip trailing spaces only from the last tab-separated segment after splitting, not from the raw line.

REVIEW COMPLETE — 3 issues found.
