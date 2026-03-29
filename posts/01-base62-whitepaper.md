<!--break type:header content-->
title = "Base62 Encoding System Whitepaper"
date = "2026-03-29 00:00:00+08:00"
short = ["base62-whitepaper"]
categories = ["Whitepaper", "Reference"]
<!--break type:content format:md content-->
This whitepaper defines a family of eight related encoding formats — Formats A through G and Gu — built on the base62 numeral system, designed to produce compact, human-readable, time-sortable identifiers suitable for use as filenames, record markers, and universal unique identifiers.

<!--excerpt-->

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

Base62 uses exactly the 62 alphanumeric characters (`0-9`, `a-z`, `A-Z`). These characters are filename-safe on all major operating systems, URL-safe without percent-encoding, shell-safe without quoting, and human-typeable without shift-key symbols.

A single base62 digit encodes values 0–61, which is sufficient to represent months (1–12), days (1–31), hours (0–23), minutes (0–59), and seconds (0–59) each in a single character.

### 2.2 Why Multiple Formats?

The six permutation formats (A–F) exist because the ordering of the three character groups (`0-9`, `a-z`, `A-Z`) determines the lexicographic sort order of encoded values. By defining all six permutations explicitly, the system provides a complete catalogue of base62 alphabet orderings. Each format is self-documenting: the format letter (A–F) tells you the alphabet ordering without looking it up.

### 2.3 Why Format-G?

Format-G was designed for human-facing identifiers. The characters `l` (lowercase L) and `O` (uppercase O) are removed because they are visually indistinguishable from `1` and `0` in many fonts. This reduces the alphabet from 62 to 60 characters, which is still sufficient for single-character encoding of all timestamp segments — the largest segment (minutes/seconds) requires exactly 60 values: 0–59.

### 2.4 Why Format-Gu?

Format-Gu wraps Format-G in a UUID structure. The "u" stands for "UUID." It adds a **class prefix** (1 character) identifying the record type, and an **order number** (2 characters) for sub-second collision handling. The result is a 12-character identifier encoding: record type, full timestamp to the second, and a collision-resolution suffix.

---

## 3. Base62 Alphabet Fundamentals

### 3.1 Definition

Base62 is a positional numeral system with 62 symbols drawn from the ASCII alphanumeric characters:

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
| Filename-safe | Yes | Yes | Yes | No (`/`, `=`) |
| URL-safe | Yes | Yes | Yes | No (`+`, `/`, `=`) |
| Visually unambiguous | Mostly | Yes | No | No |
| Bits per character | 4.00 | 5.86 | 5.95 | 6.00 |
| Chars for 64-bit value | 16 | 11 | 11 | 11 |

Base62 offers nearly the same information density as base64 (5.95 bits/char vs. 6.00) while remaining safe for filenames, URLs, and shell arguments without escaping.

---

## 4. The Three-Group Permutation Model (Formats A–F)

### 4.1 All Six Permutations

| Format | Group order | Full alphabet (62 characters) |
|---|---|---|
| **A** | `[0-9][a-z][A-Z]` | `0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ` |
| **B** | `[0-9][A-Z][a-z]` | `0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ abcdefghijklmnopqrstuvwxyz` |
| **C** | `[a-z][0-9][A-Z]` | `abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ` |
| **D** | `[a-z][A-Z][0-9]` | `abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789` |
| **E** | `[A-Z][0-9][a-z]` | `ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdefghijklmnopqrstuvwxyz` |
| **F** | `[A-Z][a-z][0-9]` | `ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789` |

### 4.2 Timestamp Structure (Formats A–F)

All six formats share the same 8-character timestamp structure `{F}{YYY}{M}{D}{h}{m}{s}` where the year segment uses 3 characters: a century symbol (year divided by 100, encoded as a single base62 character) followed by the two-digit year modulo 100. Each single-character segment (month, day, hour, minute, second) is encoded by looking up the numeric value in the format's alphabet.

---

## 5. Format-G — Visual-Ambiguity-Free Encoding

### 5.1 Alphabet

Format-G uses 60 characters in the following order:

```
0123456789abcdefghijkmnopqrstuvwxyzABCDEFGHIJKLMNPQRSTUVWXYZ
```

This is the Format-A ordering with `l` removed from the lowercase group and `O` removed from the uppercase group.

### 5.2 Why 60 Is Sufficient

| Segment | Values needed | Available (60) | Sufficient? |
|---|---|---|---|
| Century-sym | Up to 62 (theoretical) | 60 | Yes (covers years 0–5999) |
| Month | 12 | 60 | Yes |
| Day | 31 (+ 1 reserved) | 60 | Yes |
| Hour | 24 | 60 | Yes |
| Minute | 60 | 60 | Yes (exact fit) |
| Second | 60 | 60 | Yes (exact fit) |

### 5.3 Timestamp Structure

```
G{YYY}{M}{D}{h}{m}{s}
```

Total: 9 characters.

| Position | Segment | Description |
|---|---|---|
| 1 | Prefix | Literal `G` |
| 2 | Century-sym | `year // 100` — character from the Format-G alphabet |
| 3–4 | 2-digit year | `year % 100`, zero-padded decimal |
| 5 | Month | 1-character encoding (months 1–6 map to `a`–`f`; months 7–12 map to `A`–`F`) |
| 6 | Day | 1-character encoding (days 1–10 map to `0`–`9`; 11–21 map to `a`–`k`; 22–31 map to `A`–`J`) |
| 7 | Hour | 1-character encoding (hour 0 maps to `0`; 1–12 map to `a`–`l`; 13–23 map to `A`–`K`) |
| 8 | Minute | Format-G 60-char alphabet positional lookup |
| 9 | Second | Format-G 60-char alphabet positional lookup |

### 5.4 Verified Example

**Input timestamp:** `2026-03-10 04:00:45`

| Segment | Value | Result |
|---|---|---|
| Prefix | — | `G` |
| Century-sym | `2026 // 100 = 20` — alphabet position 20 — `k` | `k` |
| 2-digit year | `2026 % 100 = 26` | `26` |
| Month | 3 (March) maps to `c` | `c` |
| Day | 10 maps to `9` | `9` |
| Hour | 4 maps to `d` | `d` |
| Minute | 0 maps to `0` | `0` |
| Second | 45 maps to `K` | `K` |

**Result:** `Gk26c9d0K` (9 characters)

---

## 6. Format-Gu — Time-Based UUID (tbUUID)

### 6.1 Structure

```
{C}G{YYY}{M}{D}{h}{m}{s}{XX}
```

**Total length: 12 characters.**

| Position | Length | Segment | Description |
|---|---|---|---|
| 1 | 1 | Class indicator (C) | Single uppercase letter identifying the record type |
| 2 | 1 | Format marker | Literal `G` |
| 3–5 | 3 | Year (YYY) | Century-sym + 2-digit year |
| 6 | 1 | Month (M) | Format-G month encoding |
| 7 | 1 | Day (D) | Format-G day encoding |
| 8 | 1 | Hour (h) | Format-G hour encoding |
| 9 | 1 | Minute (m) | Format-G minute/second encoding |
| 10 | 1 | Second (s) | Format-G minute/second encoding |
| 11–12 | 2 | Order number (XX) | Collision-handling suffix, default `01` |

### 6.2 Class Indicator

The class indicator is a single character (typically an uppercase letter) that identifies the type of record the UUID refers to. This enables routing, filtering, and display logic to operate on the UUID alone without reading the referenced file. All Format-Gu UUIDs match the regex pattern `[A-Z]G[0-9a-zA-Z]{10}`.

### 6.3 Order Number

The order number (positions 11–12) handles the case where multiple records are created within the same second. The default is `01`. On collision, it increments through the sequence `01`–`09`, `0a`–`0z`, `0A`–`0Z`, `10`–..., providing 3,844 possible values per class per second.

### 6.4 UUID Immutability

Once a Format-Gu UUID is assigned to a record, it is **permanent**. The UUID must never be changed, reassigned, or reused, even when the associated file is moved or renamed. This preserves git history integrity and cross-reference stability.

---

## 7. Comparison Table — All Eight Formats

| Property | A–F | Format-G | Format-Gu |
|---|---|---|---|
| **Alphabet size** | 62 | 60 | 60 |
| **Excluded chars** | None | `l`, `O` | `l`, `O` |
| **Total length** | 9 chars | 9 chars | 12 chars |
| **Order number** | No | No | Yes (2 chars) |
| **Visual ambiguity** | Possible | Eliminated | Eliminated |
| **Use case** | General encoding | Human-facing IDs | Record UUIDs |

---

## 8. Encoding and Decoding Algorithms

### 8.1 Format-G Encoding (Pseudocode)

```
function encode_format_g(datetime):
    ALPHABET = "0123456789abcdefghijkmnopqrstuvwxyzABCDEFGHIJKLMNPQRSTUVWXYZ"
    MONTH_MAP = {1:'a',2:'b',3:'c',4:'d',5:'e',6:'f',
                 7:'A',8:'B',9:'C',10:'D',11:'E',12:'F'}
    // DAY: 1-10 -> '0'-'9'; 11-21 -> 'a'-'k'; 22-31 -> 'A'-'J'
    // HOUR: 0->'0'; 1-12->'a'-'l'; 13-23->'A'-'K'
    // MINUTE/SECOND: ALPHABET[value]

    century_char = ALPHABET[datetime.year // 100]
    year_2digit = zero_pad(datetime.year % 100, 2)
    return "G" + century_char + year_2digit + MONTH_MAP[month]
           + DAY_MAP[day] + HOUR_MAP[hour]
           + ALPHABET[minute] + ALPHABET[second]
```

### 8.2 Format-Gu Encoding (Pseudocode)

```
function encode_format_gu(datetime, class_char, existing_uuids):
    prefix = class_char + encode_format_g(datetime)  // 10 chars
    order = 1
    while (prefix + format_order(order)) in existing_uuids:
        order += 1
    return prefix + format_order(order)  // 12 chars

function format_order(n):
    ORDER_CHARS = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ"
    return ORDER_CHARS[n // 62] + ORDER_CHARS[n % 62]
    // n=1 -> "01", n=9 -> "09", n=10 -> "0a", n=36 -> "0A"
```
