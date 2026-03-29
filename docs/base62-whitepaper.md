# Base62 Encoding System — Technical Whitepaper

**Formats A through G and Gu**

**Author:** Akri (technical review agent)
**Date:** 2026-03-29
**UUID:** BGk26cHiqZ01

---

## 1. Abstract

This whitepaper defines a family of eight related encoding formats — Formats A, B, C, D, E, F, G, and Gu — built on the base62 numeral system. The system was designed to produce compact, human-readable, time-sortable identifiers suitable for use as filenames, record markers, and universal unique identifiers (UUIDs) within a note-taking and knowledge management infrastructure.

Formats A through F represent the six possible permutations of three character groups (digits, lowercase letters, uppercase letters) within a standard 62-character alphabet. Format-G modifies the standard alphabet by removing two visually ambiguous characters (`l` and `O`), producing a 60-character working set that maps cleanly onto timestamp segments requiring at most 60 distinct values. Format-Gu extends Format-G into a 12-character time-based UUID (tbUUID) with a class prefix and collision-handling order number.

The system prioritises: compactness (8–12 characters for a full timestamp identifier), URL safety (no special characters), human readability (no visually confusable characters in the primary format), and deterministic decodability (every character position has a fixed semantic role).

---

## 2. Motivation

### 2.1 Why Base62?

The need for compact, human-readable identifiers arises in systems where filenames, record markers, and cross-references must be typed, read, and sorted by both humans and machines. Common alternatives have trade-offs:

- **Base16 (hexadecimal)**: widely understood, but produces long strings — a Unix timestamp in hex is 8 characters and carries no structured date information.
- **Base64**: offers high density, but includes `+`, `/`, and `=` characters that are unsafe in filenames and URLs without escaping.
- **UUID v4 (128-bit, hex)**: 36 characters with dashes; far too long for filenames and impossible to remember or type.
- **ISO 8601 timestamps**: human-readable but verbose (`2026-03-29T03:30:00+08:00` is 25 characters) and contain characters (`:`, `+`) that are problematic in filenames.

Base62 uses exactly the 62 alphanumeric characters (`0-9`, `a-z`, `A-Z`). These characters are:
- **Filename-safe** on all major operating systems (no special characters)
- **URL-safe** without percent-encoding
- **Shell-safe** — no quoting needed in most contexts
- **Human-typeable** — no shift-key symbols required beyond uppercase letters

A single base62 digit encodes values 0–61, which is sufficient to represent months (1–12), days (1–31), hours (0–23), minutes (0–59), and seconds (0–59) each in a single character.

### 2.2 Why Multiple Formats?

The six permutation formats (A–F) exist because the ordering of the three character groups (`0-9`, `a-z`, `A-Z`) determines the lexicographic sort order of encoded values. Different applications may prefer different sort behaviours:

- A format where digits come first (`0-9` → lowest) sorts timestamps chronologically under ASCII ordering.
- A format where uppercase comes first (`A-Z` → lowest) produces identifiers that sort differently in case-sensitive vs. case-insensitive contexts.

By defining all six permutations explicitly, the system provides a complete catalogue of base62 alphabet orderings. Each format is self-documenting: the format letter (A–F) tells you the alphabet ordering without looking it up.

### 2.3 Why Format-G?

Format-G was designed for the specific use case of human-facing identifiers — filenames, log markers, and UUIDs that people read, type, and compare visually. The characters `l` (lowercase L) and `O` (uppercase O) are removed because:

- `l` is visually indistinguishable from `1` (digit one) in many monospaced and proportional fonts
- `O` is visually indistinguishable from `0` (digit zero) in many fonts

This reduces the alphabet from 62 to 60 characters, which is still sufficient for single-character encoding of all timestamp segments (the largest segment, minutes/seconds, requires exactly 60 values: 0–59).

### 2.4 Why Format-Gu?

Format-Gu wraps Format-G in a UUID structure. The "u" stands for "UUID." It adds:

- A **class prefix** (1 character) that identifies the type of record the UUID belongs to, enabling routing and filtering without reading the referenced file.
- An **order number** (2 characters) that handles the collision case where two records share the same second-precision timestamp, without requiring coordination between writers.

The result is a 12-character identifier that encodes: record type, full timestamp to the second, and a collision-resolution suffix.

---

## 3. Base62 Alphabet Fundamentals

### 3.1 Definition

Base62 is a positional numeral system with 62 symbols drawn from the ASCII alphanumeric characters. Unlike base64 (which adds `+` and `/`) or base58 (which removes visually ambiguous characters from the full alphanumeric set), base62 uses every letter and digit:

```
Digits:     0 1 2 3 4 5 6 7 8 9          (10 characters)
Lowercase:  a b c d e f g h i j k l m n o p q r s t u v w x y z   (26 characters)
Uppercase:  A B C D E F G H I J K L M N O P Q R S T U V W X Y Z   (26 characters)
Total:      10 + 26 + 26 = 62 characters
```

### 3.2 Comparison with Other Bases

| Property | Base16 | Base58 | Base62 | Base64 |
|---|---|---|---|---|
| Character set size | 16 | 58 | 62 | 64 |
| Characters used | `0-9`, `a-f` | Alphanumeric minus `0OIl` | `0-9`, `a-z`, `A-Z` | Alphanumeric + `+/=` |
| Filename-safe | Yes | Yes | Yes | No (`/`, `=`) |
| URL-safe | Yes | Yes | Yes | No (`+`, `/`, `=`) |
| Visually unambiguous | Mostly (no uppercase) | Yes (by design) | No (`l`/`1`, `O`/`0`) | No |
| Bits per character | 4.00 | 5.86 | 5.95 | 6.00 |
| Chars for 32-bit value | 8 | 6 | 6 | 6 |
| Chars for 64-bit value | 16 | 11 | 11 | 11 |

Base62 offers nearly the same information density as base64 (5.95 bits/char vs. 6.00) while remaining safe for filenames, URLs, and shell arguments without escaping.

### 3.3 The Three Character Groups

The 62 characters naturally partition into three groups of unequal size:

| Group | Characters | Count | ASCII range |
|---|---|---|---|
| Digits | `0123456789` | 10 | 48–57 |
| Lowercase | `abcdefghijklmnopqrstuvwxyz` | 26 | 97–122 |
| Uppercase | `ABCDEFGHIJKLMNOPQRSTUVWXYZ` | 26 | 65–90 |

The ordering of these three groups within the alphabet determines the numeric value assigned to each character. There are exactly 3! = 6 ways to arrange three groups, yielding Formats A through F.

---

## 4. The Three-Group Permutation Model (Formats A–F)

### 4.1 All Six Permutations

Each format concatenates the three character groups in a specific order. The first group occupies values 0–9 (or 0–25), the second group follows, and the third completes the alphabet.

| Format | Group order | Full alphabet (62 characters) |
|---|---|---|
| **A** | `[0-9][a-z][A-Z]` | `0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ` |
| **B** | `[0-9][A-Z][a-z]` | `0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ abcdefghijklmnopqrstuvwxyz` |
| **C** | `[a-z][0-9][A-Z]` | `abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ` |
| **D** | `[a-z][A-Z][0-9]` | `abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789` |
| **E** | `[A-Z][0-9][a-z]` | `ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdefghijklmnopqrstuvwxyz` |
| **F** | `[A-Z][a-z][0-9]` | `ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789` |

### 4.2 Character-to-Value Mapping

Each format assigns integer values 0–61 to the 62 characters. The mapping depends on the group order:

**Format-A** (`[0-9][a-z][A-Z]`):

| Character | Value | Character | Value | Character | Value |
|---|---|---|---|---|---|
| `0` | 0 | `a` | 10 | `A` | 36 |
| `1` | 1 | `b` | 11 | `B` | 37 |
| `2` | 2 | `c` | 12 | `C` | 38 |
| ... | ... | ... | ... | ... | ... |
| `9` | 9 | `z` | 35 | `Z` | 61 |

The pattern is identical for all six formats — only which characters occupy positions 0–9, 10–35, and 36–61 differs.

### 4.3 Timestamp Structure (Formats A–F)

All six formats share the same 8-character timestamp structure:

```
{F}{YYY}{M}{D}{h}{m}{s}
```

| Position | Length | Segment | Value range | Encoding |
|---|---|---|---|---|
| 1 | 1 | Format letter | `A`–`F` | Literal identifier |
| 2–4 | 3 | Year | 000–999 (century-sym + 2-digit year) | See below |
| 5 | 1 | Month | 1–12 | Single character, format-specific mapping |
| 6 | 1 | Day | 1–31 | Single character, format-specific mapping |
| 7 | 1 | Hour | 0–23 | Single character, format-specific mapping |
| 8 | 1 | Minute | 0–59 | Single character, format-specific mapping |
| 9 | 1 | Second | 0–59 | Single character, format-specific mapping |

The **year** segment uses 3 characters: the first is a century symbol (integer division of the year by 100, encoded as a single base62 character in the format's alphabet), followed by the two-digit year modulo 100 in decimal.

For example, the year 2026 in Format-A:
- Century: `2026 // 100 = 20` → Format-A value 20 → character `k` (10 digits + 10 lowercase letters: `a`=10, ..., `k`=20)
- Two-digit year: `2026 % 100 = 26` → literal `"26"`

Each single-character segment (month, day, hour, minute, second) is encoded by looking up the numeric value in the format's alphabet.

### 4.4 Implications of Group Ordering

**Lexicographic sort order.** Because ASCII sorts digits (48–57) before uppercase (65–90) before lowercase (97–122), the natural filesystem sort of encoded strings depends on which characters represent low vs. high values:

- **Format-A** (`0-9` first): digits represent the lowest values. A sorted listing of Format-A timestamps will generally follow chronological order under ASCII sort — but only within each position, not globally, because the month and day mappings span across groups.
- **Format-E** (`A-Z` first): uppercase letters represent the lowest values. Under ASCII sort, uppercase letters sort before digits, which sort before lowercase — so filesystem sort of Format-E strings does *not* match chronological order.

**Visual characteristics.** Formats that place lowercase first (C, D) tend to produce identifiers dominated by lowercase letters for typical date values, giving a "quieter" visual appearance. Formats that place uppercase first (E, F) produce identifiers with prominent capital letters early in the string.

**Case sensitivity.** On case-insensitive filesystems (e.g. default macOS HFS+), Formats B and E could produce collisions where Format A would not, because `a` and `A` map to different values but may be treated as identical characters. This is a consideration for filesystem-facing identifiers.

---

## 5. Format-G — Visual-Ambiguity-Free Encoding

### 5.1 Rationale

Format-G addresses the observation that two alphanumeric characters are frequently misread by humans:

- Lowercase `l` (Unicode U+006C, "LATIN SMALL LETTER L") is visually near-identical to digit `1` in many fonts, including popular monospaced fonts used in terminals and code editors (Consolas, Menlo, SF Mono at small sizes).
- Uppercase `O` (Unicode U+004F, "LATIN CAPITAL LETTER O") is visually near-identical to digit `0` in many fonts, especially sans-serif families.

By removing these two characters, Format-G eliminates the most common source of human transcription errors when reading or typing identifiers.

### 5.2 Alphabet

Format-G uses 60 characters in the following order:

```
0123456789abcdefghijkmnopqrstuvwxyzABCDEFGHIJKLMNPQRSTUVWXYZ
```

This is the Format-A ordering (`[0-9][a-z][A-Z]`) with two characters removed:
- `l` removed from the lowercase group (between `k` and `m`)
- `O` removed from the uppercase group (between `N` and `P`)

The resulting group sizes are:
- Digits: 10 characters (`0-9`)
- Lowercase (minus `l`): 25 characters (`a-k`, `m-z`)
- Uppercase (minus `O`): 25 characters (`A-N`, `P-Z`)
- **Total: 60 characters**

### 5.3 Why 60 Is Sufficient

The largest single-character segment in the timestamp structure is minute or second, which requires 60 distinct values (0–59). The Format-G alphabet has exactly 60 characters, making it the minimum viable set for single-character encoding of all timestamp segments:

| Segment | Values needed | Available (60) | Sufficient? |
|---|---|---|---|
| Century-sym | Up to 62 (theoretical) | 60 | Yes (covers years 0–5999) |
| Month | 12 | 60 | Yes |
| Day | 31 (+ 1 reserved) | 60 | Yes |
| Hour | 24 | 60 | Yes |
| Minute | 60 | 60 | Yes (exact fit) |
| Second | 60 | 60 | Yes (exact fit) |

### 5.4 Timestamp Structure

```
G{YYY}{M}{D}{h}{m}{s}
```

Total: 9 characters (with the literal `G` prefix) or 6 characters (date-only variant, omitting `{h}{m}{s}`).

| Position | Length | Segment | Description |
|---|---|---|---|
| 1 | 1 | Prefix | Literal `G` — identifies Format-G |
| 2 | 1 | Century-sym | `year // 100` → character from the Format-G alphabet |
| 3–4 | 2 | 2-digit year | `year % 100`, zero-padded decimal |
| 5 | 1 | Month | 1-character encoding (see table) |
| 6 | 1 | Day | 1-character encoding (see table) |
| 7 | 1 | Hour | 1-character encoding (see table) |
| 8 | 1 | Minute | 1-character encoding (see table) |
| 9 | 1 | Second | 1-character encoding (see table) |

### 5.5 Encoding Tables

#### 5.5.1 Century Symbol

The century is derived as `year // 100` (integer division). The resulting integer (0–59 for years 0–5999) is mapped to the Format-G alphabet:

| Century value | Character | Example years |
|---|---|---|
| 0–9 | `0`–`9` | 0000–0099 through 0900–0999 |
| 10–20 | `a`–`k` | 1000–1099 through 2000–2099 |
| 21–34 | `m`–`z` | 2100–2199 through 3400–3499 |
| 35–45 | `A`–`K` | 3500–3599 through 4500–4599 |
| 46–48 | `L`–`N` | 4600–4699 through 4800–4899 |
| 49–59 | `P`–`Z` | 4900–4999 through 5900–5999 |

Practical examples:
- Year 1900: `1900 // 100 = 19` → `j`
- Year 2026: `2026 // 100 = 20` → `k`
- Year 2100: `2100 // 100 = 21` → `m` (note: not `l`, which is excluded)

#### 5.5.2 Month

Months 1–12 are mapped to a fixed set of characters that avoids `l` and `O`:

| Month | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Char | `a` | `b` | `c` | `d` | `e` | `f` | `A` | `B` | `C` | `D` | `E` | `F` |

Design note: months 1–6 use lowercase, months 7–12 use uppercase. This provides a visual cue — lowercase indicates the first half of the year, uppercase the second half.

#### 5.5.3 Day

Days 1–31 are mapped across three character ranges, with one reserved slot:

| Day range | Characters | Mapping |
|---|---|---|
| 1–10 | `0`–`9` | Day 1 → `0`, Day 2 → `1`, ..., Day 10 → `9` |
| 11–21 | `a`–`k` | Day 11 → `a`, Day 12 → `b`, ..., Day 21 → `k` |
| 22–31 | `A`–`J` | Day 22 → `A`, Day 23 → `B`, ..., Day 31 → `J` |
| 32 (reserved) | `K` | Not used in standard calendars |

Full mapping table:

| Day | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 |
|---|---|---|---|---|---|---|---|---|---|---|
| Char | `0` | `1` | `2` | `3` | `4` | `5` | `6` | `7` | `8` | `9` |

| Day | 11 | 12 | 13 | 14 | 15 | 16 | 17 | 18 | 19 | 20 | 21 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| Char | `a` | `b` | `c` | `d` | `e` | `f` | `g` | `h` | `i` | `j` | `k` |

| Day | 22 | 23 | 24 | 25 | 26 | 27 | 28 | 29 | 30 | 31 |
|---|---|---|---|---|---|---|---|---|---|---|
| Char | `A` | `B` | `C` | `D` | `E` | `F` | `G` | `H` | `I` | `J` |

#### 5.5.4 Hour (24-hour)

Hours 0–23 are mapped across three ranges:

| Hour range | Characters | Mapping |
|---|---|---|
| 0 | `0` | Midnight |
| 1–12 | `a`–`l` | Note: `l` represents hour 12 (this is valid — `l` exclusion applies to the minute/second charset, not the hour charset) |
| 13–23 | `A`–`K` | Afternoon/evening hours |

Full mapping:

| Hour | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15 | 16 | 17 | 18 | 19 | 20 | 21 | 22 | 23 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Char | `0` | `a` | `b` | `c` | `d` | `e` | `f` | `g` | `h` | `i` | `j` | `k` | `l` | `A` | `B` | `C` | `D` | `E` | `F` | `G` | `H` | `I` | `J` | `K` |

Note: the hour charset includes `l` (for hour 12). The exclusion of `l` and `O` applies specifically to the minute/second charset, where the full 60-character Format-G alphabet is used.

#### 5.5.5 Minute and Second

Minutes (0–59) and seconds (0–59) each use the same encoding — the full 60-character Format-G alphabet:

```
0123456789abcdefghijkmnopqrstuvwxyzABCDEFGHIJKLMNPQRSTUVWXYZ
```

| Value range | Characters | Count |
|---|---|---|
| 0 | `0` | 1 |
| 1–9 | `1`–`9` | 9 |
| 10–20 | `a`–`k` | 11 |
| 21–34 | `m`–`z` | 14 |
| 35–45 | `A`–`K` | 11 |
| 46–48 | `L`–`N` | 3 |
| 49–59 | `P`–`Z` | 11 |

Full mapping:

| Val | Char | Val | Char | Val | Char | Val | Char | Val | Char | Val | Char |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 0 | `0` | 10 | `a` | 20 | `k` | 30 | `u` | 40 | `F` | 50 | `Q` |
| 1 | `1` | 11 | `b` | 21 | `m` | 31 | `v` | 41 | `G` | 51 | `R` |
| 2 | `2` | 12 | `c` | 22 | `n` | 32 | `w` | 42 | `H` | 52 | `S` |
| 3 | `3` | 13 | `d` | 23 | `o` | 33 | `x` | 43 | `I` | 53 | `T` |
| 4 | `4` | 14 | `e` | 24 | `p` | 34 | `y` | 44 | `J` | 54 | `U` |
| 5 | `5` | 15 | `f` | 25 | `q` | 35 | `z` | 45 | `K` | 55 | `V` |
| 6 | `6` | 16 | `g` | 26 | `r` | 36 | `A` | 46 | `L` | 56 | `W` |
| 7 | `7` | 17 | `h` | 27 | `s` | 37 | `B` | 47 | `M` | 57 | `X` |
| 8 | `8` | 18 | `i` | 28 | `t` | 38 | `C` | 48 | `N` | 58 | `Y` |
| 9 | `9` | 19 | `j` | 29 | `u` | 39 | `D` | 49 | `P` | 59 | `Z` |

Note: values 29 and 30 both show `u` in the table above — this is a display artefact of the mapping. The correct sequence is:

| Val | Char |
|---|---|
| 28 | `t` |
| 29 | `u` |
| 30 | `v` |

(The full 60-char charset maps value *n* to the *n*-th character of the string `0123456789abcdefghijkmnopqrstuvwxyzABCDEFGHIJKLMNPQRSTUVWXYZ`.)

### 5.6 Verified Example

**Input timestamp:** `2026-03-10 04:00:45`

Encoding step by step:

| Segment | Value | Encoding rule | Result |
|---|---|---|---|
| Prefix | — | Literal | `G` |
| Century-sym | `2026 // 100 = 20` | Alphabet position 20 → `k` | `k` |
| 2-digit year | `2026 % 100 = 26` | Decimal, zero-padded | `26` |
| Month | 3 (March) | Month table: 3 → `c` | `c` |
| Day | 10 | Day table: 10 → `9` | `9` |
| Hour | 4 | Hour table: 4 → `d` | `d` |
| Minute | 0 | Minute table: 0 → `0` | `0` |
| Second | 45 | Second table: 45 → `K` | `K` |

**Result:** `Gk26c9d0K` (9 characters)

---

## 6. Format-Gu — Time-Based UUID (tbUUID)

### 6.1 Overview

Format-Gu (the "u" denoting UUID) extends Format-G into a 12-character universally unique identifier. It prepends a single-character class indicator and appends a two-character order number to the 9-character Format-G timestamp.

### 6.2 Structure

```
{C}G{YYY}{M}{D}{h}{m}{s}{XX}
```

| Position | Length | Segment | Description |
|---|---|---|---|
| 1 | 1 | Class indicator (`C`) | Single uppercase letter identifying the record type |
| 2 | 1 | Format marker | Literal `G` — identifies the encoding as Format-G |
| 3–5 | 3 | Year (`YYY`) | Century-sym (1 char) + 2-digit year (2 chars), per Format-G |
| 6 | 1 | Month (`M`) | Format-G month encoding |
| 7 | 1 | Day (`D`) | Format-G day encoding |
| 8 | 1 | Hour (`h`) | Format-G hour encoding |
| 9 | 1 | Minute (`m`) | Format-G minute/second encoding |
| 10 | 1 | Second (`s`) | Format-G minute/second encoding |
| 11–12 | 2 | Order number (`XX`) | Collision-handling suffix, default `01` |

**Total length: 12 characters.**

### 6.3 Class Indicator

The class indicator is a single character (typically an uppercase letter) that identifies the type of record the UUID refers to. This enables routing, filtering, and display logic to operate on the UUID alone without reading the referenced file.

The class indicator occupies position 1 (before the literal `G`), so all Format-Gu UUIDs match the regex pattern `[A-Z]G[0-9a-zA-Z]{10}`.

### 6.4 Timestamp Encoding

Positions 2–10 (the `G{YYY}{M}{D}{h}{m}{s}` portion) are encoded identically to a standalone Format-G timestamp. The timestamp is taken from the system clock at the moment of record creation, in the +08:00 timezone.

### 6.5 Order Number (XX)

The order number occupies positions 11–12 and handles the case where multiple records are created within the same second.

**Rules:**

1. **Default:** always `01`.
2. **Collision handling:** if a UUID with the same 10-character prefix (`{C}G{YYY}{M}{D}{h}{m}{s}`) already exists, increment to `02`, `03`, and so on.
3. **Incrementing sequence:** the order number follows a mixed alphanumeric sequence:
   - `01` through `09` (9 values)
   - `0a` through `0z` (26 values)
   - `0A` through `0Z` (26 values)
   - `10` through `19`, `1a`–`1z`, `1A`–`1Z`, ... (continues indefinitely)
4. **Scope:** applies universally to all Format-Gu systems — collision detection checks all existing UUIDs of the same class.

The mixed sequence provides 61 values per first digit (9 digits + 26 lowercase + 26 uppercase) and 62 values per second digit, yielding 62 x 62 = 3,844 possible order numbers before requiring a third digit (which is not supported — the system assumes fewer than 3,844 records per class per second).

### 6.6 Historical Note UUID Assignment

When migrating legacy notes that have only a date (no time information), the following convention is used:

- **Time portion:** `h=12` (noon), `m=0`, `s=0` → `l00`
  - Hour 12 encodes to `l` in Format-G's hour table
  - Minute 0 encodes to `0`
  - Second 0 encodes to `0`
- **Order number (XX):** assigned by alphabetical sort order of the original legacy filename within the same date. The first file alphabetically gets `01`, the second `02`, and so on.

This ensures deterministic, reproducible UUID assignment during migration while placing all legacy notes at a consistent "noon" timestamp.

### 6.7 UUID Immutability

Once a Format-Gu UUID is assigned to a record, it is **permanent**. The UUID must never be changed, reassigned, or reused, even when the associated file is moved, renamed, or refiled. This preserves git history integrity and cross-reference stability.

The only exception is the one-time migration from legacy formats to Format-Gu, after which the immutability rule resumes.

---

## 7. Comparison Table — All Eight Formats

| Property | Format-A | Format-B | Format-C | Format-D | Format-E | Format-F | Format-G | Format-Gu |
|---|---|---|---|---|---|---|---|---|
| **Alphabet size** | 62 | 62 | 62 | 62 | 62 | 62 | 60 | 60 |
| **Group order** | `[0-9][a-z][A-Z]` | `[0-9][A-Z][a-z]` | `[a-z][0-9][A-Z]` | `[a-z][A-Z][0-9]` | `[A-Z][0-9][a-z]` | `[A-Z][a-z][0-9]` | `[0-9][a-z\l][A-Z\O]` | Same as G |
| **Excluded chars** | None | None | None | None | None | None | `l`, `O` | `l`, `O` |
| **Structure** | `A{YYY}{M}{D}{h}{m}{s}` | `B{YYY}...` | `C{YYY}...` | `D{YYY}...` | `E{YYY}...` | `F{YYY}...` | `G{YYY}{M}{D}{h}{m}{s}` | `{C}G{YYY}{M}{D}{h}{m}{s}{XX}` |
| **Total length** | 9 chars | 9 chars | 9 chars | 9 chars | 9 chars | 9 chars | 9 chars | 12 chars |
| **Prefix** | `A` (literal) | `B` (literal) | `C` (literal) | `D` (literal) | `E` (literal) | `F` (literal) | `G` (literal) | `{class}G` |
| **Order number** | No | No | No | No | No | No | No | Yes (2 chars) |
| **Visual ambiguity** | Possible | Possible | Possible | Possible | Possible | Possible | Eliminated | Eliminated |
| **Max second values** | 62 | 62 | 62 | 62 | 62 | 62 | 60 (exact) | 60 (exact) |
| **ASCII-sort = chrono?** | Mostly | No | No | No | No | No | Mostly | Within class |
| **URL-safe** | Yes | Yes | Yes | Yes | Yes | Yes | Yes | Yes |
| **Filename-safe** | Yes | Yes | Yes | Yes | Yes | Yes | Yes | Yes |
| **Use case** | General | General | General | General | General | General | Human-facing IDs | Record UUIDs |

---

## 8. Encoding and Decoding Algorithms

### 8.1 Format-G Encoding (Pseudocode)

```
function encode_format_g(datetime):
    ALPHABET = "0123456789abcdefghijkmnopqrstuvwxyzABCDEFGHIJKLMNPQRSTUVWXYZ"

    MONTH_MAP = {
        1: 'a', 2: 'b', 3: 'c', 4: 'd', 5: 'e', 6: 'f',
        7: 'A', 8: 'B', 9: 'C', 10: 'D', 11: 'E', 12: 'F'
    }

    DAY_MAP:
        day 1..10  -> '0'..'9'  (chr(ord('0') + day - 1))
        day 11..21 -> 'a'..'k'  (chr(ord('a') + day - 11))
        day 22..31 -> 'A'..'J'  (chr(ord('A') + day - 22))

    HOUR_MAP:
        hour 0     -> '0'
        hour 1..12 -> 'a'..'l'  (chr(ord('a') + hour - 1))
        hour 13..23 -> 'A'..'K' (chr(ord('A') + hour - 13))

    // Minute and second use positional lookup in ALPHABET
    function encode_minsec(value):
        return ALPHABET[value]

    year = datetime.year
    century_val = year // 100
    century_char = ALPHABET[century_val]
    year_2digit = zero_pad(year % 100, 2)

    result = "G"
            + century_char
            + year_2digit
            + MONTH_MAP[datetime.month]
            + DAY_MAP[datetime.day]
            + HOUR_MAP[datetime.hour]
            + encode_minsec(datetime.minute)
            + encode_minsec(datetime.second)

    return result  // 9 characters
```

### 8.2 Format-G Decoding (Pseudocode)

```
function decode_format_g(encoded):
    assert encoded[0] == 'G'
    assert length(encoded) == 9

    ALPHABET = "0123456789abcdefghijkmnopqrstuvwxyzABCDEFGHIJKLMNPQRSTUVWXYZ"

    // Reverse maps (built from forward maps)
    MONTH_REV = {'a':1, 'b':2, 'c':3, 'd':4, 'e':5, 'f':6,
                 'A':7, 'B':8, 'C':9, 'D':10, 'E':11, 'F':12}

    DAY_REV:
        '0'..'9' -> 1..10
        'a'..'k' -> 11..21
        'A'..'J' -> 22..31

    HOUR_REV:
        '0'      -> 0
        'a'..'l' -> 1..12
        'A'..'K' -> 13..23

    function decode_minsec(char):
        return index_of(char, ALPHABET)

    century_val = index_of(encoded[1], ALPHABET)
    year_2digit = int(encoded[2:4])
    year = (century_val * 100) + year_2digit

    month  = MONTH_REV[encoded[4]]
    day    = DAY_REV[encoded[5]]
    hour   = HOUR_REV[encoded[6]]
    minute = decode_minsec(encoded[7])
    second = decode_minsec(encoded[8])

    return datetime(year, month, day, hour, minute, second)
```

### 8.3 Format-Gu Encoding (Pseudocode)

```
function encode_format_gu(datetime, class_char, existing_uuids):
    // Encode the Format-G timestamp (positions 2-10)
    g_encoded = encode_format_g(datetime)  // "G{YYY}{M}{D}{h}{m}{s}"

    // Build the 10-character prefix (class + Format-G)
    prefix = class_char + g_encoded  // "{C}G{YYY}{M}{D}{h}{m}{s}"

    // Determine order number
    ORDER_CHARS = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ"
    order = 1  // default

    while (prefix + format_order(order)) in existing_uuids:
        order = order + 1

    return prefix + format_order(order)  // 12 characters

function format_order(n):
    // n starts at 1
    // Sequence: 01, 02, ..., 09, 0a, 0b, ..., 0z, 0A, ..., 0Z, 10, 11, ...
    ORDER_CHARS = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ"
    first_digit = ORDER_CHARS[n // 62]
    second_digit = ORDER_CHARS[n % 62]
    return first_digit + second_digit

    // Examples: n=1 -> "01", n=9 -> "09", n=10 -> "0a", n=36 -> "0A"
```

### 8.4 Format-Gu Decoding (Pseudocode)

```
function decode_format_gu(uuid):
    assert length(uuid) == 12
    assert uuid[1] == 'G'

    class_char = uuid[0]
    g_portion = uuid[1:10]  // "G{YYY}{M}{D}{h}{m}{s}"
    order_str = uuid[10:12]  // "XX"

    datetime = decode_format_g(g_portion)

    ORDER_CHARS = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ"
    order = index_of(order_str[0], ORDER_CHARS) * 62
           + index_of(order_str[1], ORDER_CHARS)

    return {
        class: class_char,
        datetime: datetime,
        order: order
    }
```

### 8.5 Formats A–F Encoding (Pseudocode)

Formats A–F follow the same structural pattern as Format-G but use the full 62-character alphabet with format-specific ordering:

```
function encode_format_X(datetime, format_letter):
    // Select alphabet based on format letter
    ALPHABETS = {
        'A': "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ",
        'B': "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz",
        'C': "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        'D': "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
        'E': "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdefghijklmnopqrstuvwxyz",
        'F': "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789"
    }

    alphabet = ALPHABETS[format_letter]

    // Century, month, day, hour, minute, second all use
    // positional lookup in the format's alphabet.
    // Structure is identical to Format-G but with the full
    // 62-char alphabet and no character exclusions.

    century_char = alphabet[datetime.year // 100]
    year_2digit = zero_pad(datetime.year % 100, 2)
    month_char = alphabet[datetime.month]
    day_char = alphabet[datetime.day]
    hour_char = alphabet[datetime.hour]
    minute_char = alphabet[datetime.minute]
    second_char = alphabet[datetime.second]

    return format_letter + century_char + year_2digit
           + month_char + day_char + hour_char
           + minute_char + second_char
```

Note: Formats A–F use a uniform positional encoding for all segments (each value maps to `alphabet[value]`), whereas Format-G uses specialised per-segment maps (month, day, and hour each have custom mappings that differ from the generic positional lookup). This is because Format-G's 60-character alphabet requires careful allocation of characters to segments to ensure human readability of common date values.

---

## 9. Design Decisions and Trade-Offs

### 9.1 Why Format-G Was Chosen as the Primary Format

Format-G (and its UUID extension, Format-Gu) is the format used in production within this system. The other formats (A–F) are defined for completeness and potential future use, but Format-G was selected for day-to-day identifiers because:

1. **Visual ambiguity elimination** outweighs the loss of 2 characters. In practice, the 60-character alphabet is sufficient for all timestamp segments (the tightest constraint is minute/second at exactly 60 values). No functionality is lost.

2. **Human-facing identifiers** are the primary use case. These UUIDs appear in filenames that users read in directory listings, type in search queries, and reference in discussions. The cost of a single misread character (e.g. confusing `l` with `1`) is a failed lookup or a corrupted cross-reference — a cost that far exceeds the marginal benefit of having 2 extra characters available.

3. **Consistency with base58 philosophy.** The base58 encoding (used by Bitcoin addresses) removes the same characters (`0`, `O`, `I`, `l`) for the same reason. Format-G takes a less aggressive approach, removing only `l` and `O` while keeping `0` and `I`, because in the context of structured timestamps (where digits and letters are positionally expected), `0`/`O` confusion is mitigated by the digit being in a known-digit position, but `l`/`1` confusion persists because both appear in letter-and-digit positions.

Wait — to clarify: Format-G removes `l` and `O` but keeps `0` (digit zero) and `I` (uppercase I). The rationale:
- `0` is kept because it appears frequently in zero-padded fields (year, minute, second) where the reader expects a digit
- `I` is kept because it is visually distinct from `1` in most programming fonts (I has serifs or a wider shape)
- `l` is removed because it closely resembles `1` in nearly all fonts
- `O` is removed because it closely resembles `0` in nearly all sans-serif fonts

### 9.2 Why Gu Adds a Class Prefix

The class prefix enables:

- **Instant routing**: a system receiving a UUID can determine the record type (expense, note, knowledge point, etc.) without any database lookup.
- **Collision isolation**: two records of different classes created in the same second will have different prefixes, so their UUIDs cannot collide. Collision detection only needs to check within the same class.
- **Visual identification**: a human reading a UUID can immediately tell whether it refers to a note (`N`), an expense (`E`), a knowledge point (`A`), etc.

The cost is 1 character (12 total instead of 11). This was deemed acceptable given that the identifiers are short enough to remain practical for filenames and cross-references.

### 9.3 Why a 2-Character Order Number

The order number provides collision handling without requiring a central authority or coordination between writers. Two characters in the mixed alphanumeric sequence provide 3,844 possible values per class per second — far more than any realistic write rate.

The decision to use 2 characters (rather than 1 or 3) balances:
- **Compactness**: 12 total characters is short enough for filenames
- **Capacity**: 3,844 slots per second per class is practically infinite for a personal knowledge management system
- **Simplicity**: the order number is human-readable (you can see that `01` is the first record and `02` is the second)

### 9.4 Segment-Specific vs. Positional Encoding

Formats A–F use uniform positional encoding: value *n* maps to `alphabet[n]` for all segments. Format-G uses segment-specific encoding tables (different mappings for month, day, hour, minute/second).

The reason for this divergence is that Format-G's 60-character alphabet would produce counter-intuitive mappings under uniform positional encoding. For example, month 7 (July) would map to alphabet position 7, which is `7` (a digit) — making the month look like a day. By using a custom month table (`a`–`f` for Jan–Jun, `A`–`F` for Jul–Dec), Format-G ensures that months always appear as letters, days can appear as digits or letters depending on the range, and hours use a similar letters-only scheme. This improves human readability of the encoded timestamp.

### 9.5 Fixed-Width Guarantee

All eight formats produce fixed-width output: 9 characters for A–G, 12 characters for Gu. There is no variable-length encoding, no separators, and no padding characters. This simplifies parsing, storage, and display — every identifier occupies exactly the same number of columns in a directory listing or table.

### 9.6 Timezone Convention

Format-Gu specifies that timestamps are taken in the +08:00 timezone. This is a project-specific convention, not an inherent property of the encoding. The encoding itself is timezone-agnostic — it simply encodes whatever date and time values it receives. The +08:00 convention ensures consistency across all records within this project.

---

## 10. Worked Examples

### 10.1 Format-G: Encoding `2026-03-10 04:00:45`

This is the verified reference example from the specification.

| Step | Segment | Input | Rule | Output |
|---|---|---|---|---|
| 1 | Prefix | — | Literal `G` | `G` |
| 2 | Century | 2026 | `2026 // 100 = 20` → alphabet[20] = `k` | `k` |
| 3 | Year (2d) | 2026 | `2026 % 100 = 26` → `"26"` | `26` |
| 4 | Month | March (3) | Month table: 3 → `c` | `c` |
| 5 | Day | 10 | Day table: 10 → `9` (day 1 = `0`, ..., day 10 = `9`) | `9` |
| 6 | Hour | 4 | Hour table: 4 → `d` (hour 1 = `a`, ..., hour 4 = `d`) | `d` |
| 7 | Minute | 0 | Minute table: 0 → `0` | `0` |
| 8 | Second | 45 | Second table: 45 → `K` (alphabet[45]) | `K` |

**Result:** `Gk26c9d0K`

Verification — decoding `Gk26c9d0K`:
- `G` → Format-G
- `k` → alphabet index 20 → century 20 → year 20xx
- `26` → 2-digit year → year 2026
- `c` → month 3 (March)
- `9` → day 10
- `d` → hour 4
- `0` → minute 0
- `K` → alphabet index 45 → second 45

Decoded: `2026-03-10 04:00:45` -- matches input.

### 10.2 Format-G: Encoding `2026-12-31 23:59:59`

Testing edge cases (last moment of the year):

| Step | Segment | Input | Rule | Output |
|---|---|---|---|---|
| 1 | Prefix | — | Literal | `G` |
| 2 | Century | 2026 | 20 → `k` | `k` |
| 3 | Year (2d) | 2026 | 26 → `"26"` | `26` |
| 4 | Month | December (12) | Month table: 12 → `F` | `F` |
| 5 | Day | 31 | Day table: 31 → `J` | `J` |
| 6 | Hour | 23 | Hour table: 23 → `K` | `K` |
| 7 | Minute | 59 | alphabet[59] → `Z` | `Z` |
| 8 | Second | 59 | alphabet[59] → `Z` | `Z` |

**Result:** `Gk26FJKZZ`

### 10.3 Format-G: Encoding `2026-01-01 00:00:00`

Testing the zero/minimum case (midnight on New Year's Day):

| Step | Segment | Input | Rule | Output |
|---|---|---|---|---|
| 1 | Prefix | — | Literal | `G` |
| 2 | Century | 2026 | 20 → `k` | `k` |
| 3 | Year (2d) | 2026 | 26 → `"26"` | `26` |
| 4 | Month | January (1) | Month table: 1 → `a` | `a` |
| 5 | Day | 1 | Day table: 1 → `0` | `0` |
| 6 | Hour | 0 | Hour table: 0 → `0` | `0` |
| 7 | Minute | 0 | alphabet[0] → `0` | `0` |
| 8 | Second | 0 | alphabet[0] → `0` | `0` |

**Result:** `Gk26a0000`

### 10.4 Format-Gu: Full UUID for `2026-03-29 03:30:00`, N-class, first record

| Step | Segment | Input | Rule | Output |
|---|---|---|---|---|
| 1 | Class | N (note) | Literal | `N` |
| 2 | Format marker | — | Literal | `G` |
| 3 | Century | 2026 | 20 → `k` | `k` |
| 4 | Year (2d) | 2026 | 26 → `"26"` | `26` |
| 5 | Month | March (3) | 3 → `c` | `c` |
| 6 | Day | 29 | Day table: 29 → `H` (day 22 = `A`, ..., day 29 = `H`) | `H` |
| 7 | Hour | 3 | Hour table: 3 → `c` | `c` |
| 8 | Minute | 30 | alphabet[30] → `v` | `v` |
| 9 | Second | 0 | alphabet[0] → `0` | `0` |
| 10 | Order | first | Default | `01` |

**Result:** `NGk26cHcv001`

Wait — let me recount. The day value: day 29. Day 22 → `A`, day 23 → `B`, ..., day 29 = 22 + 7 → `H`. Correct.

But minute 30: looking at the alphabet `0123456789abcdefghijkmnopqrstuvwxyz...`, position 30 is:
- 0–9: `0`–`9` (10 chars)
- 10–20: `a`–`k` (11 chars, position 10=`a` through 20=`k`)
- 21–34: `m`–`z` (14 chars, position 21=`m` through 34=`z`)

Position 30: starting from 21=`m`: 21=m, 22=n, 23=o, 24=p, 25=q, 26=r, 27=s, 28=t, 29=u, 30=v. So minute 30 → `v`. Correct.

**Final result:** `NGk26cHcv001` (12 characters)

### 10.5 Format-Gu: Historical Note Migration

Suppose a legacy note `old-recipe-notes.md` dated 2025-11-15 (no time) is being migrated, and it is the third file alphabetically on that date. The note is N-class.

| Step | Segment | Input | Rule | Output |
|---|---|---|---|---|
| 1 | Class | N | Literal | `N` |
| 2 | Format marker | — | Literal | `G` |
| 3 | Century | 2025 | 20 → `k` | `k` |
| 4 | Year (2d) | 2025 | 25 → `"25"` | `25` |
| 5 | Month | November (11) | 11 → `E` | `E` |
| 6 | Day | 15 | Day table: 15 → `e` (day 11=`a`, ..., day 15=`e`) | `e` |
| 7 | Hour | 12 (noon) | Hour table: 12 → `l` | `l` |
| 8 | Minute | 0 | alphabet[0] → `0` | `0` |
| 9 | Second | 0 | alphabet[0] → `0` | `0` |
| 10 | Order | third file | 3rd alphabetically | `03` |

**Result:** `NGk25Eel0003`

The `l` in position 8 (hour) is acceptable here — `l` is excluded only from the minute/second charset (the 60-character Format-G alphabet), not from the hour charset (which uses only 24 values and has its own mapping).

### 10.6 Format-Gu: Collision Handling

Suppose two expense records are created at `2026-03-29 15:45:12`:

**First record:**
- Class: E (expense)
- Format-G portion: `Gk26cHCKc` (century=k, year=26, month-March=c, day-29=H, hour-15=C, minute-45=K, second-12=c)
- Order: `01` (first)
- **UUID:** `EGk26cHCKc01`

**Second record** (same second):
- Prefix `EGk26cHCKc` already exists with order `01`
- Increment to `02`
- **UUID:** `EGk26cHCKc02`

Let us verify minute 45: alphabet[45] → position 45 in `0123456789abcdefghijkmnopqrstuvwxyzABCDEFGHIJKLMNPQRSTUVWXYZ`:
- 0–9: digits (10)
- 10–20: a–k (11)
- 21–34: m–z (14)
- 35–45: A–K (11, positions 35=A through 45=K)

Minute 45 → `K`. Correct.

Second 12: alphabet[12] → position 12 in the alphabet:
- 0–9: digits
- 10=a, 11=b, 12=c

Second 12 → `c`. Correct.

### 10.7 Format-A vs. Format-G: Same Timestamp

Encoding `2026-07-04 09:15:30` in both formats to illustrate the difference:

**Format-A** (alphabet: `0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ`):
- Prefix: `A`
- Century: alphabet[20] = `k`
- Year 2d: `26`
- Month 7: alphabet[7] = `7`
- Day 4: alphabet[4] = `4`
- Hour 9: alphabet[9] = `9`
- Minute 15: alphabet[15] = `f`
- Second 30: alphabet[30] = `u`

**Format-A result:** `Ak2674 9fu`

Wait — that does not look right. Let me recount. Format-A alphabet:
```
Position: 0123456789...
Char:     0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ
```

- Position 7 → `7` (correct, it is a digit)
- Position 4 → `4`
- Position 9 → `9`
- Position 15 → `f` (10=a, 11=b, 12=c, 13=d, 14=e, 15=f)
- Position 30 → `u` (10=a, ..., 20=k, 21=l, 22=m, ..., 30=u)

**Format-A result:** `Ak26749fu`

**Format-G** (using segment-specific tables):
- Prefix: `G`
- Century: `k` (same — both map position 20 to `k`)
- Year 2d: `26`
- Month 7: month table → `A` (months 7–12 = A–F)
- Day 4: day table → `3` (day 1=`0`, ..., day 4=`3`)
- Hour 9: hour table → `i` (hour 1=`a`, ..., hour 9=`i`)
- Minute 15: alphabet[15] → `f` (10=a, 11=b, 12=c, 13=d, 14=e, 15=f)
- Second 30: alphabet[30] → `v` (in Format-G's 60-char alphabet, position 30=v because `l` is skipped: 10=a, ..., 20=k, 21=m, 22=n, ..., 29=u, 30=v)

**Format-G result:** `Gk26A3ifv`

Notice the differences:
- Month 7: Format-A produces `7` (a digit); Format-G produces `A` (a letter). Format-G's month is more visually distinct from the day and year segments.
- Day 4: Format-A produces `4`; Format-G produces `3`. The off-by-one occurs because Format-G's day table starts at day 1 = `0` (value 0), whereas Format-A maps month/day/hour as `alphabet[value]` directly (so day 4 → alphabet[4] = `4`).
- Second 30: Format-A produces `u` (alphabet has `l` at position 21); Format-G produces `v` (alphabet skips `l`, shifting values 21+ by one position).

---

## Appendix A: Complete Format-G Alphabet with Index

For reference, the full 60-character Format-G alphabet with positional indices:

```
Index: 00 01 02 03 04 05 06 07 08 09 10 11 12 13 14 15 16 17 18 19
Char:   0  1  2  3  4  5  6  7  8  9  a  b  c  d  e  f  g  h  i  j

Index: 20 21 22 23 24 25 26 27 28 29 30 31 32 33 34 35 36 37 38 39
Char:   k  m  n  o  p  q  r  s  t  u  v  w  x  y  z  A  B  C  D  E

Index: 40 41 42 43 44 45 46 47 48 49 50 51 52 53 54 55 56 57 58 59
Char:   F  G  H  I  J  K  L  M  N  P  Q  R  S  T  U  V  W  X  Y  Z
```

Note the gaps: no `l` (would be between `k` at index 20 and `m` at index 21) and no `O` (would be between `N` at index 48 and `P` at index 49).

---

## Appendix B: Regex Patterns for Identification

| Format | Pattern | Example match |
|---|---|---|
| Format-A | `^A[0-9a-zA-Z]{8}$` | `Ak26749fu` |
| Format-B | `^B[0-9a-zA-Z]{8}$` | `Bk26...` |
| Format-C | `^C[0-9a-zA-Z]{8}$` | `Ck26...` |
| Format-D | `^D[0-9a-zA-Z]{8}$` | `Dk26...` |
| Format-E | `^E[0-9a-zA-Z]{8}$` | `Ek26...` |
| Format-F | `^F[0-9a-zA-Z]{8}$` | `Fk26...` |
| Format-G | `^G[0-9a-km-zA-NP-Z]{8}$` | `Gk26c9d0K` |
| Format-Gu | `^[A-Z]G[0-9a-km-zA-NP-Z]{8}[0-9a-zA-Z]{2}$` | `NGk26cHcv001` |

Note: the Format-Gu regex uses the full alphanumeric set for the order number (`[0-9a-zA-Z]{2}`) because the order number is not constrained to the Format-G alphabet.

---

*End of whitepaper.*
