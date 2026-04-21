<!--break type:header content-->
title = "tsdb Extensions: ptsv — the plane variant"
date = "2026-04-21 12:00:00+08:00"
short = ["extensions-ptsv"]
categories = ["Announcement", "Reference"]
<!--break type:content format:md content-->
`tsdb` now ships with a fourth named format: `ptsv`. It is the plane — fully flattened — variant of `rtsv`, produced by a new `--plane` mode. Each row is a single `(col1, col2, uuid)` triple, so there are no comma-joined arrays anywhere in the file.

<!--excerpt-->

## What changes

`rtsv` stores a UUID array in column 3:

```
city	Tokyo	NGk26cHcv001,NGk26cHdn002
```

`ptsv` expands that array so every UUID appears on its own line:

```
city	Tokyo	NGk26cHcv001
city	Tokyo	NGk26cHdn002
```

If `col1` has `i` distinct col2 values and each pair has `j` UUIDs, the index grows to `i × j` rows. Nothing else changes: same tab-separated columns, same escaping, same `# YYYYDDMMhhmmss` timestamp footer.

**Array-valued fields expand too.** A later revision (DOTSV 0.4) added canonical array values on disk — `role=["admin","editor","viewer"]` in a single record. `--plane` splits that array at generation time, so a record with a three-element `role` contributes three `(role, <elem>, uuid)` rows rather than one `(role, ["admin","editor","viewer"], uuid)` row. The `--relate` variant (`.rtv`) keeps arrays packed; only `ptsv` fans them out.

## Why add it

`rtsv` is optimised for binary search. `ptsv` is optimised for shell pipelines. A `.rtv` row like

```
tag	work	NGk26cHcv001,NGk26cHdn002,EGk26cICK001
```

needs a second parse step — split the comma list — before you can feed it to `join`, `awk`, `sort -u`, or any standard Unix filter. The `.ptv` variant skips that step:

```bash
awk -F'\t' '$1 == "tag" && $2 == "work" { print $3 }' notes.kv.ptv
```

...is a plain tab split. Same story for `sort -u`, `comm`, `join`, and any tool that expects one record per line.

## The `--plane` mode

The new mode mirrors `--relate`:

```
tsdb --plane <target.dov>
```

It compacts the source `.dov`, reads its timestamp footer, and writes both index variants:

| File                 | Col 1  | Col 2  | Col 3 |
|----------------------|--------|--------|-------|
| `<target>.kv.ptv`    | key    | value  | uuid  |
| `<target>.vk.ptv`    | value  | key    | uuid  |

Rows are sorted lexicographically on `(col 1, col 2, col 3)` — the same order `--relate` would produce if its comma lists were expanded and split. The final line of each file is a `# YYYYDDMMhhmmss` comment matching the source `.dov`. When that footer is already current, `--plane` is a no-op.

### Example

Given these records in `users.dov`:

```
NGk26cHcv001	name=Alice	city=Tokyo
NGk26cHdn002	name=Bob	city=Tokyo
EGk26cICK001	name=Carol	city=London
```

After `tsdb --plane users.dov`, `users.kv.ptv` contains:

```
city	London	EGk26cICK001
city	Tokyo	NGk26cHcv001
city	Tokyo	NGk26cHdn002
name	Alice	NGk26cHcv001
name	Bob	NGk26cHdn002
name	Carol	EGk26cICK001
# 20262104155028
```

Six rows for three records of two fields each — exactly what the `i × j` formula predicts.

### With an array field

Add one more record that carries a multi-valued `role` field:

```
+PGk26cHcv001	name=Dave	role=admin	role=editor	role=viewer
```

On disk, that becomes `role=["admin","editor","viewer"]`. After `tsdb --plane users.dov`, `users.kv.ptv` picks up three extra rows — one per role element — alongside Dave's `name`:

```
name	Dave	PGk26cHcv001
role	admin	PGk26cHcv001
role	editor	PGk26cHcv001
role	viewer	PGk26cHcv001
```

The `.rtv` variant keeps that field packed as a single row with the whole array in col 2. For filtering on a specific element (`role = editor`), `ptsv` is the direct hit.

## When to use which

| Question                                    | Choose  |
|---------------------------------------------|---------|
| Binary search by `(key, value)`?            | `rtsv`  |
| Pipe into `awk` / `sort -u` / `join`?       | `ptsv`  |
| Smallest file size?                         | `rtsv`  |
| Uniform one-record-per-line output?         | `ptsv`  |

Run both if both workflows matter. `--plane` and `--relate` write to separate files (`*.ptv` vs `*.rtv`), so they do not overwrite each other and each has its own independent skip-if-current check.

## Companion-file picture

After running both `--relate` and `--plane` on `users.dov`:

| File             | Created by        | Purpose                               |
|------------------|-------------------|---------------------------------------|
| `users.dov`      | user / `tsdb`     | DOTSV database                        |
| `users.dov.lock` | `tsdb`            | Concurrency queue manifest            |
| `users.kv.rtv`   | `tsdb --relate`   | Key-value index (array col 3)         |
| `users.vk.rtv`   | `tsdb --relate`   | Value-key index (array col 3)         |
| `users.kv.ptv`   | `tsdb --plane`    | Key-value index (one row per uuid)    |
| `users.vk.ptv`   | `tsdb --plane`    | Value-key index (one row per uuid)    |

## Documentation

- [DOTSV Whitepaper](/tsdb/dotsv-whitepaper/) — §16 Related Formats
- [tsdb Whitepaper](/tsdb/tsdb-whitepaper/) — §14 `--plane` Mode, §16 Related Formats
- [tsdb Extensions: atsv, rtsv, qtsv](/tsdb/extensions/) — the earlier extensions post
