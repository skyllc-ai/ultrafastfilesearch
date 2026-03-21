# Search Pattern Deep Dive: How Patterns Flow Through the Code

> **Status**: All patterns now use the unified streaming output path.
> Zero `SearchResult` allocation for every pattern type.

## Pattern Types and Classification

When a user types a search pattern, it goes through three stages:

```
User Input → ParsedPattern (parse.rs) → IndexPattern (pattern.rs) → Matching (filtering.rs)
```

### Stage 1: ParsedPattern Parsing

`ParsedPattern::parse()` in `pattern/parse.rs` classifies the raw input:

| Input | Drive | Pattern | PatternType |
|-------|-------|---------|-------------|
| `*.rs` | None | `*.rs` | Glob |
| `c:/pro*` | C | `/pro*` | Glob |
| `>C:\\TemP.*\.txt` | C | `C:\\TemP.*\.txt` | Regex |
| `nice` | None | `nice` | Literal |
| `*hallo*.txt` | None | `*hallo*.txt` | Glob |
| `**\Users\**\AppData\**` | None | `**\Users\**\AppData\**` | Glob |

**Rules:**
- Starts with `>` → Regex (the `>` is stripped)
- Contains `*`, `?`, or `[` → Glob
- Everything else → **Literal**
- Drive prefix `c:` or `C:\` is extracted and removed
- **Default: case_sensitive = false** (all patterns are case-insensitive by default)

### Stage 2: IndexPattern Compilation

`compile_parsed_pattern()` / `compile_index_pattern()` converts to optimized match form:

| PatternType | Pattern | GlobKind | IndexPattern | Strategy |
|---|---|---|---|---|
| Glob | `*` | Any | `Any` | Always true |
| Glob | `*.rs` | Extension("rs") | `Suffix { suffix: ".rs" }` | `name.ends_with(".rs")` |
| Glob | `*.txt` | Extension("txt") | `Suffix { suffix: ".txt" }` | `name.ends_with(".txt")` |
| Glob | `*hallo*.txt` | PrefixSuffix ❌ → Complex | `Regex` | Glob→regex conversion |
| Glob | `foo*` | Prefix("foo") | `Prefix { prefix: "foo" }` | `name.starts_with("foo")` |
| Glob | `*bar` | Suffix("bar") | `Suffix { suffix: "bar" }` | `name.ends_with("bar")` |
| Glob | `*needle*` | Contains("needle") | `Contains { needle: "needle" }` | Substring search |
| Glob | `**\Users\**` | Complex | `Regex` | Glob→regex |
| Regex | `C:\\TemP.*\.txt` | N/A | `Regex { regex }` | `regex.is_match(name)` |
| Literal | `nice` | Exact("nice") | `Exact { value: "nice" }` | `name == "nice"` |

**Key detail about `*hallo*.txt`:**  
This has TWO `*` characters → `star_count >= 2` in `classify_single_star_pattern` → BUT
the inner logic checks: `*hallo*.txt` → strip leading `*` and trailing `*` → no, it doesn't
end with `*`. Actually let me trace more carefully:

- `*hallo*.txt` has 2 stars
- `star_count == 2`: check if `*needle*` form → strip `*` prefix and `*` suffix
- `"*hallo*.txt".strip_prefix('*')` = `"hallo*.txt"`
- `"hallo*.txt".strip_suffix('*')` = None (doesn't end with `*`)
- Falls to `GlobKind::Complex` → converted to regex

So `*hallo*.txt` becomes a **regex** pattern. This is slower than specialized matchers.

### Stage 3: Matching — What Gets Matched Against?

**Critical: patterns match against the FILENAME only, not the full path.**

From `filtering.rs` line 67:
```rust
let name = self.index.record_name(record);  // ← filename only!
if !pat.matches(name, self.case_sensitive) {
    return false;
}
```

`record_name()` returns the primary filename (e.g., `"document.txt"`, not `"C:\Users\foo\document.txt"`).

## Answering Your Specific Questions

### 1. `*hallo*.txt` — How does it flow?

```
Input: "*hallo*.txt"
→ ParsedPattern: Glob, pattern="*hallo*.txt"
→ classify_glob: 2 stars, not *needle* form → Complex
→ compile_index_pattern: glob_to_regex("*hallo*.txt") → Regex
→ IndexPattern::Regex { regex, regex_lower }
→ Matching: regex.is_match(filename) — matches "hallo_world.txt", "say_hallo.txt"
```

**Performance:** Regex matching, BUT the smart pattern decomposer extracts `.txt`
from the trailing extension → uses the extension index for O(matches) pre-filtering.
Only `.txt` files are scanned, then the full regex is applied to those candidates.
This turns a 3M-record full scan into a ~50K-record filtered scan.

### 2. `**\Users\**\AppData\**` — How does it flow?

```
Input: "**\Users\**\AppData\**"
→ ParsedPattern::parse():
    contains_path_separator() = true → is_path_pattern = true
    normalize: already Windows separators
    detect_pattern_type: has * → Glob
→ classify_glob: has "**" → Complex → glob_to_regex → Regex
→ IndexPattern::Regex { regex, regex_lower }
→ Matching: regex.is_match(FULL PATH)  ← path-aware!
    e.g. "D:\Users\john\AppData\Local\config.ini" ✅ matches
```

**Path-aware matching** is auto-detected because the pattern contains `\`.
The compiled regex is matched against the full materialized path (e.g.,
`D:\Users\john\AppData\Local\`) instead of just the filename.

Forward slashes also work: `**/Users/**/AppData/**` is normalized to
`**\Users\**\AppData\**` automatically.

### 3. `>C:\\TemP.*\.txt` — Regex pattern

```
Input: ">C:\\TemP.*\.txt"
→ ParsedPattern::parse():
    starts with > → Regex
    extract_drive_from_regex: drive=C, remaining="C:\\TemP.*\.txt"
    contains_path_separator() = true → is_path_pattern = true
→ compile_parsed_pattern: PatternType::Regex
→ Regex::new("C:\\TemP.*\.txt") for case-sensitive
→ Regex::new("(?i)C:\\TemP.*\.txt") for case-insensitive
→ IndexPattern::Regex { regex, regex_lower }
→ Smart decomposer: extract ".txt" → extension index pre-filter!
→ Matching: regex_lower.is_match(FULL PATH) — path-aware + case-insensitive
    e.g. "C:\Temp\report.txt" ✅ matches
    e.g. "C:\Temporary\notes.txt" ✅ matches
```

**Two optimizations combined:**
1. **Extension index**: only `.txt` files are scanned (O(matches), not O(all records))
2. **Path-aware**: regex matched against full path `C:\Temp\report.txt`,
   not just filename `report.txt`

### 4. `nice` — Literal/Basic pattern (substring + full path)

```
Input: "nice"
→ ParsedPattern::parse():
    no wildcards → Literal
    Literal always path-aware → is_path_pattern = true
→ compile_parsed_pattern: PatternType::Literal
→ IndexPattern::Contains { needle: "nice", needle_lower: "nice" }
→ Matching: full_path.contains("nice") — substring + case-insensitive
    e.g. "D:\Projects\nice_project\readme.txt" ✅ matches (path)
    e.g. "D:\nicehouse.doc" ✅ matches (filename)
    e.g. "D:\Venice\photo.jpg" ✅ matches (directory name)
```

**Behavior (matches Everything, WizFile, C++ UFFS):**
- `nice` is a **substring** match, not exact — finds `nice` anywhere in the full path
- Finds `nicehouse`, `venice.jpg`, `my_nice_file.txt`, `C:\nice\readme.md`
- **Case-insensitive by default**: `nice` matches `Nice`, `NICE`, `nIcE`
- Matches against **full path** (directories + filename), not just filename
- Use `--case` for case-sensitive: `uffs nice --case`

### 5. Case Sensitivity

**Default: case-INsensitive** — matches NTFS semantics and all major file
search tools (Everything, WizFile, C++ UFFS).

| Tool | Default | Toggle |
|---|---|---|
| Everything | insensitive | Ctrl+I / `case:` prefix |
| WizFile | insensitive | checkbox |
| C++ UFFS | insensitive | — |
| fd (Rust) | smart case | `--case-sensitive` / `--ignore-case` |
| **UFFS Rust** | **insensitive** | **`--case`** |

**Usage:**
```
uffs nice              # case-insensitive (default)
uffs nice --case       # force case-sensitive
uffs DuaLipa --smart-case   # auto case-sensitive (has uppercase)
uffs dualipa --smart-case   # stays case-insensitive (all lowercase)
```

**Precedence:** `--case` always wins > `--smart-case` auto-detection > default (insensitive)

**Smart case rule:** When `--smart-case` is enabled, if the pattern contains
ANY uppercase ASCII letter, matching automatically becomes case-sensitive.
All-lowercase patterns stay case-insensitive.  This matches `fd` and
`ripgrep` behavior.

**Implementation (zero-alloc for most patterns):**
- `Contains`: sliding window with per-byte `to_ascii_lowercase` — no heap alloc
- `Prefix`/`Suffix`: zero-alloc byte comparison per byte
- `Exact`: `eq_ignore_ascii_case` — zero alloc
- `Regex`: pre-compiled `(?i)` regex — no per-record allocation
- `ExactSet`/`ContainsAny`: unavoidable `.to_ascii_lowercase()` for hash/automaton

## Record-Level Filters (beyond pattern matching)

All filters are applied inline during the streaming scan via
`StreamingRecordFilter::matches()` — sub-nanosecond per record.
**All filters combine with AND logic in a single command.**

### Available Filters

| Flag | Effect | Example |
|---|---|---|
| `--files-only` | Skip directories | `uffs *.txt --files-only` |
| `--dirs-only` | Skip files | `uffs AppData --dirs-only` |
| `--hide-system` | Skip system+hidden | `uffs * --hide-system` |
| `--min-size N` | Minimum file size (bytes) | `uffs *.log --min-size 1048576` |
| `--max-size N` | Maximum file size (bytes) | `uffs * --max-size 1024` |
| `--attr LIST` | NTFS attribute filter | `uffs * --attr hidden,compressed` |
| `--newer AGE` | Modified after date/duration | `uffs *.log --newer 7d` |
| `--older AGE` | Modified before date/duration | `uffs * --older 2026-01-01` |
| `--case` | Force case-sensitive | `uffs DuaLipa --case` |
| `--smart-case` | Auto case-sensitive if uppercase | `uffs DuaLipa --smart-case` |

### `--attr` Generic Attribute Filter

Filters on ANY of the 17 NTFS file attributes.  Prefix `!` to exclude.
Comma-separated for multiple (AND logic).

```
uffs * --attr hidden                   # only hidden files
uffs * --attr !hidden                  # exclude hidden files
uffs * --attr compressed,encrypted     # compressed AND encrypted
uffs * --attr !system,!hidden          # same as --hide-system
uffs * --attr directory                # only directories (same as --dirs-only)
uffs * --attr readonly,!archive        # readonly AND not archive
```

**Available attributes:** hidden, system, archive, readonly, compressed,
encrypted, sparse, reparse, offline, notindexed, temporary, virtual,
pinned, unpinned, integrity, noscrub, directory

### `--newer` / `--older` Date Range Filter

Filters on the **modified** timestamp.  Supports durations and dates:

```
uffs *.log --newer 7d                  # modified in last 7 days
uffs *.log --newer 24h                 # modified in last 24 hours
uffs *.log --newer 30m                 # modified in last 30 minutes
uffs * --older 2026-01-01              # modified before Jan 1, 2026
uffs * --newer 2026-01-01 --older 2026-02-01  # modified in January 2026
```

### Combining All Filters

ALL filters are combinable in a single command.  They are applied with
AND logic inline during the streaming scan — zero overhead for unset filters.

```
uffs *.txt --files-only --min-size 1024 --attr hidden --newer 7d --case
```

This command:
1. Extension index → only `.txt` files scanned
2. `--files-only` → skip directories
3. `--min-size 1024` → skip files < 1KB
4. `--attr hidden` → must have hidden attribute
5. `--newer 7d` → modified in last 7 days
6. `--case` → case-sensitive `.txt` match
7. Pattern match → filename ends with `.txt`

All checks are simple bitwise/comparison operations — sub-nanosecond per record.

## Pattern Classification Performance Tiers

**All patterns now use the unified streaming path** via
`write_index_streaming_with_filter()`.  Zero `SearchResult` allocation
for every pattern type.  The **smart pattern decomposer** extracts
trailing extensions from complex patterns for O(matches) pre-filtering.

### Tier 1: ⚡⚡⚡ Instant — Extension Index + Streaming

These patterns extract a literal extension → use the extension index
for O(matches) lookup → iterate ONLY matching records → write directly.

| Pattern | Example Matches | IndexPattern | Records Scanned | Optimizations |
|---|---|---|---|---|
| `*.rs` | `main.rs` | Suffix | Only .rs files (~1K) | ext index + streaming |
| `*.txt` | `readme.txt` | Suffix | Only .txt files (~50K) | ext index + streaming |
| `*hallo*.txt` | `hallo_world.txt` | Regex | Only .txt files (~50K) | **decomposer** + ext index + regex |
| `foo*.rs` | `foobar.rs` | PrefixSuffix | Only .rs files (~1K) | **decomposer** + ext index + prefix check |
| `report*.csv` | `report_2026.csv` | PrefixSuffix | Only .csv files | **decomposer** + ext index |
| `>.*\.log` | `app.log` | Regex | Only .log files | **decomposer** + ext index + regex |

**Estimated speed on C: (3M records):** ~2-3s (index build) + ~0.01-0.1s (scan+output)

### Tier 2: ⚡⚡ Fast — Full Scan + Streaming (no ext index)

These patterns have no extractable extension → must scan ALL records →
but still use the fast streaming writer (zero alloc, itoa, BufWriter).

| Pattern | Example Matches | IndexPattern | Records Scanned | Optimizations |
|---|---|---|---|---|
| `*` | everything | Any | All (skip filter) | streaming, no filter overhead |
| `nice` | `nice` (exact only) | Exact | All 3M | `eq_ignore_ascii_case` (fast) |
| `foo*` | `foobar`, `foo.txt` | Prefix | All 3M | `starts_with` (fast) |
| `*bar` | `foobar`, `toolbar` | Suffix | All 3M | `ends_with` (fast) |
| `*needle*` | `my_needle_file` | Contains | All 3M | sliding window (fast) |

**Estimated speed on C: (3M records):** ~2-3s (index build) + ~1-2s (scan+output)

### Tier 3: ⚡ Good — Full Scan + Regex Streaming

These have no extension AND need regex — slowest per-record cost, but
still use streaming output (no SearchResult allocation).

| Pattern | Example Matches | IndexPattern | Records Scanned | Optimizations |
|---|---|---|---|---|
| `*hallo*` | `hallo_world` | Contains | All 3M | substring (not regex) |
| `**\Users\**` | ⚠️ filename only! | Regex | All 3M | regex.is_match per record |
| `>C:\\TemP.*` | ⚠️ filename only! | Regex | All 3M | pre-compiled (?i) regex |
| `file?.txt` | `file1.txt` | Regex | Only .txt (**decomposer**!) | ext index + regex |

**Estimated speed on C: (3M records):** ~2-3s (index build) + ~2-4s (scan+output)

### Gaps and Future Optimization Opportunities

#### ✅ DONE — What We've Implemented

| Optimization | Benefit | Patterns Helped |
|---|---|---|
| **Unified streaming writer** | Zero SearchResult alloc for ALL patterns | All |
| **Extension index pre-filter** | O(matches) instead of O(all records) | `*.ext` patterns |
| **Smart pattern decomposer** | Extract ext from complex patterns | `*hallo*.txt`, `foo*.rs`, `>.*\.log` |
| **itoa integers** | 2-3× faster than Display trait | All |
| **1MB BufWriter** | Fewer WriteFile syscalls | All |
| **Zero-alloc case matching** | No .to_lowercase() per record | All case-insensitive |
| **materialize_path_into** | Reusable buffer, no String alloc | All |
| **Multi-drive channel output** | Overlap I/O with output | All drives parallel |

#### 🔲 Remaining Gaps (with memory/ROI analysis)

1. **Name index for Exact patterns** — `nice` scans all 3M records.
   - ❌ **Memory: +620 MB** for HashMap.  NOT WORTH IT.

2. **Prefix/suffix index** — `foo*` and `*bar` scan all records.
   - ⚠️ **Memory: +66 MB** + 2s build.  MARGINAL.

3. ~~**Path-aware matching**~~ — ✅ **DONE!** Inline path check,
   zero extra memory.  Patterns with `\` or `/` match against full path.

4. **Parallel scan for filtered queries** — Single-threaded streaming.
   - **0 MB** if inline.  MAYBE LATER — I/O is the bottleneck, not CPU.

5. ~~**Multi-drive filtered streaming**~~ — ✅ **DONE!** Channel-based
   architecture extended to ALL patterns.  Zero memory overhead.

### Unified Streaming Architecture (Implemented)

```
ALL patterns → write_index_streaming_with_filter()
  │
  ├── pattern = None (full scan *):
  │     iterate all records, no filter
  │
  ├── pattern + is_path_pattern = false + ext_indices = Some:
  │     iterate ONLY extension-index records, match filename
  │     (smart decomposer: *.rs, *hallo*.txt, \Windows\*.log)
  │
  ├── pattern + is_path_pattern = false + ext_indices = None:
  │     iterate all records, match filename inline
  │     (nice, foo*, *needle*)
  │
  └── pattern + is_path_pattern = true:
        iterate all records, match against FULL PATH
        (\Users\*\AppData, *\Desktop\*.doc)
        + optional ext_indices pre-filter for path+ext patterns
```

One function, one output path, zero `SearchResult` allocation.

**Smart Pattern Decomposer** (`extract_trailing_extension`):
- Extracts literal extension from ANY pattern ending with `.ext`
- Works for both name and path patterns
- `*hallo*.txt` → extracts `txt` → extension index → O(.txt files)
- `foo*.rs` → extracts `rs` → extension index → O(.rs files)
- `\Windows\*.log` → extracts `log` → extension index → then path check
- `*hallo*` → no extension → falls back to full scan

**Path-Aware Matching** (`is_path_pattern`):
- Auto-detected: pattern contains `\` or `/` → path mode
- Normalizes `/` → `\` for Windows compatibility
- Matches against `path_buffer` (full materialized path) instead of filename
- Zero extra memory — path is already computed for output
- Combined with extension decomposer for `\Windows\*.log` patterns

### Files Involved

| File | Role |
|---|---|
| `pattern/parse.rs` | `ParsedPattern::parse()` — classifies input, detects path patterns, normalizes separators |
| `pattern.rs` | `ParsedPattern.is_path_pattern` — path vs name flag |
| `compiled_pattern/glob.rs` | `classify_glob()` — glob → `GlobKind` |
| `index_search/pattern.rs` | `compile_index_pattern()` — `GlobKind` → `IndexPattern` |
| `output.rs` | `write_index_streaming_with_filter()` — unified streaming writer with path/name matching |
| `search/mod.rs` | `write_streaming_output_with_filter()` — console/file routing |
| `search/mod.rs` | `extract_trailing_extension()` — smart pattern decomposer |
| `search/mod.rs` | `try_get_extension_indices()` — extension index lookup |
| `search/mod.rs` | `build_record_filter()` — builds StreamingRecordFilter from CLI flags |

## Competitive Analysis: UFFS vs Major File Search Tools

### Feature Comparison Matrix

| Feature | Everything | WizFile | fd | ripgrep | C++ UFFS | **Rust UFFS** |
|---|---|---|---|---|---|---|
| **Pattern Matching** |
| Glob (`*.txt`) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Regex | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ |
| Substring (bare text) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Path-aware matching | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ |
| Smart case | ❌ | ❌ | ✅ | ✅ | ❌ | ✅ |
| Whole word (`--word`) | ✅ | ❌ | ❌ | ✅ | ❌ | ✅ |
| OR operator (`\|`) | ✅ | ❌ | ❌ | ✅ | ❌ | ✅ |
| Exclude pattern | ✅ | ❌ | ✅ | ✅ | ❌ | ✅ |
| Extension index | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ **unique** |
| Smart decomposer | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ **unique** |
| **Filtering** |
| Files only | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ |
| Dirs only | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ |
| Size range | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ |
| Date: modified | ✅ | ✅ | ✅ | ❌ | ❌ | ✅ |
| Date: created | ✅ | ✅ | ❌ | ❌ | ❌ | ✅ |
| Date: accessed | ✅ | ✅ | ❌ | ❌ | ❌ | ✅ |
| Attr filter (generic) | ✅ | ❌ | ❌ | ❌ | ❌ | ✅ |
| Hide system/hidden | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ |
| Extension filter | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ |
| Limit results | ✅ | ❌ | ✅ | ❌ | ❌ | ✅ |
| **Output** |
| CSV/custom columns | ❌ | ❌ | ❌ | ❌ | ✅ | ✅ |
| Tree metrics | ❌ | ❌ | ❌ | ❌ | ✅ | ✅ |
| Multi-drive parallel | ❌ | ❌ | ❌ | ❌ | ✅ | ✅ |
| **Performance** |
| MFT direct read | ❌ | ✅ | ❌ | ❌ | ✅ | ✅ |
| Streaming output | ❌ | ❌ | ✅ | ✅ | ✅ | ✅ |
| Zero-alloc matching | ❌ | ❌ | ❌ | ✅ | ❌ | ✅ |

### What We Have That Nobody Else Does

1. **Extension index with smart decomposer** — `*hallo*.txt` extracts
   `.txt` for O(matches) pre-filtering.  No other tool does this.
2. **Tree metrics** (treesize, tree_allocated, descendants) — unique to UFFS.
3. **Generic `--attr` filter on ALL 17 NTFS attributes** — Everything has
   `attrib:` but WizFile, fd, ripgrep don't.
4. **Multi-drive parallel with output-as-ready** — channel architecture
   overlapping I/O with output.
5. **CSV output with configurable columns** — no other file search tool
   does this.

### Remaining Gaps vs Everything (the gold standard)

| Feature | Everything | UFFS Rust | Status |
|---|---|---|---|
| Boolean operators (OR) | ✅ | ✅ | **DONE** (`*.txt\|*.log`) |
| Exclude pattern | ✅ | ✅ | **DONE** (`--exclude`) |
| Whole word | ✅ | ✅ | **DONE** (`--word`) |
| Date: all timestamps | ✅ | ✅ | **DONE** (`--newer-created`, etc.) |
| Column-specific syntax | `size:>1mb` | Separate flags | 🔲 Future |
| Real-time monitoring | ✅ (USN journal) | ❌ | 🔲 Different arch |
| Diacritics matching | ✅ (`café`=`cafe`) | ❌ | 🔲 Unicode |
| Parent/child search | `parent:` / `child:` | ❌ | 🔲 Path decompose |
| Duplicate finder | ✅ | ❌ | 🔲 Post-scan |
| Content search | ✅ | ❌ | 🔲 Different scope |
| Sort results | ✅ | ❌ | 🔲 Requires buffer |
| Bookmarks | ✅ | ❌ | 🔲 GUI feature |

### Complete CLI Reference

```
uffs PATTERN [OPTIONS]

Pattern:
  *.txt                     Glob pattern (filename match)
  nice                      Substring match (full path, case-insensitive)
  "*.txt|*.log"             OR: match either pattern
  >C:\\Temp.*\.txt          Regex (prefix with >)
  \Users\*\AppData\*        Path-aware (auto-detected by \ or /)

Options:
  --case                    Force case-sensitive matching
  --smart-case              Auto case-sensitive if pattern has uppercase
  --word                    Whole word matching (\b...\b regex)
  --exclude PATTERN         Exclude files matching this pattern
  --files-only              Skip directories
  --dirs-only               Skip files
  --hide-system             Skip system and hidden files
  --min-size BYTES          Minimum file size
  --max-size BYTES          Maximum file size
  --attr LIST               NTFS attribute filter (hidden,!system,compressed)
  --newer DURATION|DATE     Modified after (7d, 24h, 2026-01-01)
  --older DURATION|DATE     Modified before
  --newer-created ...       Created after
  --older-created ...       Created before
  --newer-accessed ...      Accessed after
  --older-accessed ...      Accessed before
  --ext EXTENSIONS          Filter by extension
  --limit N                 Maximum results
  --drive LETTER            Single drive
  --format csv|custom       Output format
  --out FILE|console        Output destination
  --columns LIST|all        Columns to include

All flags combine with AND logic.  All applied inline during streaming scan.
```
