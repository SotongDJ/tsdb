<!--break type:header content-->
title = "tsdb Extensions: atsv, rtsv, qtsv and the relate/query pipeline"
date = "2026-03-29 13:30:24+08:00"
short = ["extensions"]
categories = ["Announcement", "Reference"]
<!--break type:content format:md content-->
`tsdb` v0.2 introduces three named file formats and two new operating modes that extend the core write pipeline with an inverted-index lookup layer. This post explains what each format does and how the `--relate` and `--query` modes fit into a typical workflow.

<!--excerpt-->

## Motivation

The v0.1 design gave you a fast, git-friendly database for write operations. What it did not give you was a way to answer the question: *which records have `city=Tokyo`?* Answering that in v0.1 required a full scan of the `.dov` file. For small databases that is fine; for larger ones it is not.

v0.2 adds a generated inverted index and a query interface built on top of it, without changing the core write path or the `.dov` file format. All new capabilities are additive.

## Three new format names

### `atsv` — Action TSV (`*.atv`)

The existing action file format now has a proper name. `atsv` formalises the `action.txt` convention as a first-class format with a defined extension (`*.atv`) and MIME type. The format itself is unchanged — it remains byte-identical to the DOTSV pending section. The practical effect is that tools and documentation can now refer to action files by name rather than by example.

```
tsdb mydata.dov changes.atv
```

### `rtsv` — Relation TSV (`*.rtv`)

`rtsv` is a generated flat three-column inverted index. You never write one by hand. Two variants are produced from each `.dov` file:

| File                 | Col 1  | Col 2  | Col 3          |
|----------------------|--------|--------|----------------|
| `<target>.kv.rtv`   | key    | value  | sorted UUIDs   |
| `<target>.vk.rtv`   | value  | key    | sorted UUIDs   |

Rows are sorted lexicographically on col 1 then col 2, so a binary search on either index is O(log n). The UUID list in col 3 is comma-separated with no spaces.

The last line of every `.rtv` file is a timestamp comment in the format `# YYYYDDMMhhmmss`, matching the current timestamp of the source `.dov`. This footer is what allows `--relate` to skip regeneration when the indexes are already up to date.

Example — given these records in `users.dov`:

```
NGk26cHcv001	name=Alice	city=Tokyo	age=30
NGk26cHdn002	name=Bob	city=Tokyo
EGk26cICK001	name=Carol	city=London	age=30
```

`users.kv.rtv` looks like this:

```
age	30	EGk26cICK001,NGk26cHcv001
city	London	EGk26cICK001
city	Tokyo	NGk26cHcv001,NGk26cHdn002
name	Alice	NGk26cHcv001
name	Bob	NGk26cHdn002
name	Carol	EGk26cICK001
# 20262903143022
```

### `qtsv` — Query TSV (`*.qtv`)

`qtsv` is the input format for `--query` mode. Each line is one filter criterion. An optional first line declares the combination mode.

```
# mode	intersect
city	Tokyo
age	30
```

Criterion lines take two forms:

- **Key + value** (`key\tvalue`) — exact pair lookup in `kv.rtv`.
- **Bare token** — searched in col 1 of both `kv.rtv` and `vk.rtv`; the UUID sets from both hits are unioned before the mode operation is applied. This means you do not need to know whether a token is a key or a value in the target database.

Mode is either `intersect` (default — all criteria must match) or `union` (any criterion may match).

## Two new CLI modes

### `tsdb --relate <target.dov>`

Generates or refreshes the `kv.rtv` and `vk.rtv` index files for a given database.

```bash
tsdb --relate users.dov
# produces: users.kv.rtv, users.vk.rtv
```

The workflow:

1. Compact `users.dov` so the sorted section is fully merged and the timestamp is current.
2. Check whether both `.rtv` files exist and their timestamp footers match the `.dov` timestamp. If so, exit immediately — no work needed.
3. Stream all records, build the key-value and value-key indexes, sort, and write both files.
4. Append the `.dov` timestamp as the footer of each `.rtv`.

The skip condition means calling `--relate` on an unchanged database is essentially free. You can add it to scripts without worrying about redundant work.

### `tsdb --query <query.qtv> <target.dov>`

Runs filter criteria against the index and prints matching UUIDs to stdout.

```bash
tsdb --query find-tokyo.qtv users.dov
```

Where `find-tokyo.qtv` contains:

```
city	Tokyo
```

Output:

```
NGk26cHcv001
NGk26cHdn002
```

`--query` automatically invokes `--relate` first, so the index is always current. If the index is already up to date the implicit `--relate` is a no-op. You do not need to call `--relate` separately before running a query.

Output is a plain list of UUIDs, one per line, in lexicographic order. No headers, no opcodes. This makes it straightforward to pipe into further processing:

```bash
# fetch the full records for all Tokyo users
tsdb --query find-tokyo.qtv users.dov | while read uuid; do
    grep "^$uuid" users.dov
done
```

Or build an action file from the results:

```bash
# generate a patch action for every matched record
tsdb --query find-tokyo.qtv users.dov \
  | awk '{print "~" $1 "\tstatus=archived"}' \
  > archive.atv
tsdb users.dov archive.atv
```

## Timestamp tracking

v0.2 also introduces a timestamp footer on `.dov` files. Every write operation — action file execution, compaction, and the implicit compact inside `--relate` — appends a `# YYYYDDMMhhmmss` comment as the final line of the `.dov`. This is a UTC timestamp; the field order is year, day, month, hour, minute, second (example: `# 20262903143022` for 2026-03-29 14:30:22 UTC).

The timestamp serves one concrete purpose: it is the value that `--relate` compares against the `.rtv` footer to decide whether regeneration is needed. It also happens to be useful for auditing — a quick `tail -1 users.dov` tells you the last time the file was modified.

Compaction keeps only the latest timestamp: all accumulated timestamp lines from prior writes are discarded during the compaction merge, and a fresh one is appended at the end.

## Full companion file picture

After running `--relate` on `users.dov`, the working directory contains:

| File              | Created by          | Purpose                               |
|-------------------|---------------------|---------------------------------------|
| `users.dov`       | `tsdb` write / user | DOTSV database                        |
| `users.dov.lock`  | `tsdb`              | Concurrency queue manifest            |
| `users.kv.rtv`    | `tsdb --relate`     | Key-value inverted index              |
| `users.vk.rtv`    | `tsdb --relate`     | Value-key inverted index              |
| `changes.atv`     | user                | Action file (write operations)        |
| `find-tokyo.qtv`  | user                | Query criteria                        |

## Documentation

Full specifications for the new formats and modes are in the updated whitepapers:

- [DOTSV Whitepaper](/tsdb/posts/dotsv-whitepaper/) — §15 Timestamp Tracking, §16 Related Formats
- [tsdb Whitepaper](/tsdb/posts/tsdb-whitepaper/) — §13 `--relate` Mode, §14 `--query` Mode, §15 Related Formats
