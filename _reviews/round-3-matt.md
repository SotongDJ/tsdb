# Matt's Review — Round 3

**Date:** 2026-03-29

## Summary

All 3 round-2 fixes are correct and complete. One new edge-case correctness bug found.

## Round-2 Fix Verification

| R2 Issue | Status |
|----------|--------|
| 1 (dead timestamp refresh) | Fixed — Instant::now() captured before apply_actions |
| 2 (empty UUID set bypasses blocking) | Fixed — empty set conflicts with all entries |
| 3 (trim_end on full line) | Fixed — only last segment trimmed |

## New Issue Found

**Issue 1.** `src/dotsv.rs` lines 33 / 133 — **Record with all fields deleted becomes unloadable after compaction.**

If every field in a record is removed via `~uuid\tkey=\x00` patches, `Record::serialize()` produces a 12-char string (UUID only, no tab). After `compact()` + `write_to()` + reload, the sorted section validator at line 133 rejects that line with "sorted section line too short (need >=13 bytes)". The database cannot reload its own output — a correctness bug.

Fix options:
- **(a) Preferred:** Reject a patch that would remove the last remaining field. Return `TsdbError::Parse` if applying the patch would leave `fields` empty.
- **(b) Alternative:** Change the sorted line length check to `< 12` and handle the no-tab case in `Record::parse` (return a record with empty fields).

Option (a) is cleaner — it enforces the spec invariant that every record has at least one KV pair.

REVIEW COMPLETE — 1 issue found.
