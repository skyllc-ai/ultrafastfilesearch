# CLI Feature Parity with Reference UFFS ✅ COMPLETE

This document describes the CLI features implemented to achieve parity with the reference
`UltraFastFileSearch` (SwiftSearch Robert Nio's version) as documented in `The Manual.md`.

**Status: 100% Feature Parity Achieved**

---

## Current State

### Existing CLI Commands

| Command | Description | Status |
|---------|-------------|--------|
| `uffs <pattern>` | Search with glob/regex pattern | 🟢 Complete |
| `uffs search <pattern>` | Search (explicit subcommand) | 🟢 Complete |
| `uffs index --drive C` | Build index from single drive | 🟢 Complete |
| `uffs index --drives C,D,E` | Build index from multiple drives | 🟢 Complete |
| `uffs info <path>` | Show index file info | 🟢 Complete |
| `uffs stats --index <path>` | Show file statistics | 🟢 Complete |
| `uffs save-raw --drive C` | Save raw MFT bytes | 🟢 Complete |
| `uffs load-raw <input>` | Load and export raw MFT | 🟢 Complete |

### Current Search Options

| Option | Description | Status |
|--------|-------------|--------|
| `--drive` | Single drive letter | 🟢 |
| `--drives` | Multiple drives (C,D,E) | 🟢 |
| `--index` | Use pre-built index | 🟢 |
| `--files-only` | Exclude directories | 🟢 |
| `--dirs-only` | Only directories | 🟢 |
| `--min-size` | Minimum file size | 🟢 |
| `--max-size` | Maximum file size | 🟢 |
| `--limit` | Max results | 🟢 |
| `--format` | Output format (table/json/csv) | 🟢 |
| `--out` | Output destination (file or console) | 🟢 |
| `--columns` | Custom column selection | 🟢 |
| `--sep` | Column separator | 🟢 |
| `--quotes` | Quote string values | 🟢 |
| `--header` | Include header row | 🟢 |
| `--ext` | Extension filter (pictures,mp4,pdf) | 🟢 |
| `--case-sensitive` | Case-sensitive matching | 🟢 |
| `--pos` | Positive highlight color | 🟢 |
| `--neg` | Negative highlight color | 🟢 |

---

## Feature Reference (All Implemented ✅)

### 1. Search Pattern Syntax ✅

**Supported Patterns:**
```bash
uffs c:/pro*           # Drive prefix in pattern ✅
uffs /pro*.txt         # All drives (no prefix) ✅
uffs **\Users\**       # ** matches backslashes (recursive) ✅
uffs ">C:\\Temp.*"     # REGEX (starts with >) ✅
```

### 2. Multi-Drive Search (`--drives`) ✅

```bash
uffs /pro*.txt --drives C,D,M    # Search specific drives
uffs "*.txt"                      # Auto-detect all NTFS drives
```

### 3. Extension Filtering (`--ext`) ✅

```bash
uffs /pro** --ext jpg,mp4,documents
```

**Extension Collections:**
| Collection | Extensions |
|------------|------------|
| `pictures` | jpg, png, tiff, gif, bmp, webp, svg, ico, heic, raw |
| `documents` | doc, docx, txt, pdf, rtf, odt, xls, xlsx, ppt, pptx |
| `videos` | mpeg, mp4, avi, mkv, mov, wmv, flv, webm |
| `music` | mp3, wav, flac, aac, ogg, wma, m4a |

### 4. Output Customization ✅

| Option | Default | Description |
|--------|---------|-------------|
| `--out` | console | Output location (file or console) |
| `--columns` | all | Column selection |
| `--header` | true | Include header line |
| `--sep` | `,` | Column separator |
| `--quotes` | `"` | Quote character for paths |
| `--pos` | `1` | Representation for active attributes |
| `--neg` | `0` | Representation for inactive attributes |

**Available Columns:**
- `path`, `name`, `pathonly`, `type`, `size`, `sizeondisk`
- `created`, `written`/`modified`, `accessed`
- `descendants`, `treesize`, `tree_allocated`, `bulkiness`
- `hidden`, `system`, `archive`, `readonly`, `compressed`
- `encrypted`, `sparse`, `reparse`, `offline`, `not_indexed`
- `temporary`, `virtual`, `pinned`, `unpinned`
- `attributevalue`

### 5. Case Sensitivity (`--case-sensitive`) ✅

```bash
uffs DuaLippa --case-sensitive    # Case-sensitive matching
```

### 6. Default Command Behavior ✅

```bash
uffs c:/pro*           # Direct search (no subcommand needed)
uffs search "*.txt"    # Explicit subcommand also works
uffs --help            # Help
```

---

## Implementation Plan

### Phase CLI-1: Search Pattern Syntax 🟢 COMPLETE

**Goal:** Parse drive prefix from pattern and support REGEX

| ID | Deliverable | Status | Notes |
|----|-------------|--------|-------|
| CLI-1.1 | Pattern parser module | ✅ | `crates/uffs-core/src/pattern.rs` |
| CLI-1.2 | Drive prefix parsing | ✅ | `c:/pro*` → drive=C, pattern=`/pro*` |
| CLI-1.3 | REGEX detection | ✅ | Patterns starting with `>` |
| CLI-1.4 | REGEX search implementation | ✅ | Integrated with Polars regex |
| CLI-1.5 | `**` recursive glob | ✅ | Converts to `.*` in regex |
| CLI-1.6 | Unit tests | ✅ | 14 pattern parsing tests |

### Phase CLI-2: Multi-Drive Search 🟢 COMPLETE

**Goal:** Add `--drives` flag to search command

| ID | Deliverable | Status | Notes |
|----|-------------|--------|-------|
| CLI-2.1 | Add `--drives` to search | ✅ | Comma-separated drive letters |
| CLI-2.2 | Concurrent multi-drive search | ✅ | Uses `MultiDriveMftReader` |
| CLI-2.3 | "All drives" default | ✅ | Auto-detects all NTFS drives when no drive specified |
| CLI-2.4 | Unit tests | ✅ | Multi-drive reader tests |

### Phase CLI-3: Extension Filtering 🟢 COMPLETE

**Goal:** Filter by file extension with collection aliases

| ID | Deliverable | Status | Notes |
|----|-------------|--------|-------|
| CLI-3.1 | `--ext` flag | ✅ | Comma-separated extensions |
| CLI-3.2 | Extension collections | ✅ | pictures, documents, videos, music, archives, code |
| CLI-3.3 | Extension normalization | ✅ | Handles with/without dot |
| CLI-3.4 | DataFrame filtering | ✅ | `MftQuery::extension_filter()` |
| CLI-3.5 | Unit tests | ✅ | 10 extension filtering tests |

### Phase CLI-4: Output Customization 🟢 COMPLETE

**Goal:** Full control over output format, columns, and separators

| ID | Deliverable | Status | Notes |
|----|-------------|--------|-------|
| CLI-4.1 | `--out` flag | ✅ | Output to file or console |
| CLI-4.2 | `--columns` flag | ✅ | Select specific columns |
| CLI-4.3 | Column aliases | ✅ | `all`, `path`, `name`, etc. |
| CLI-4.4 | `--header` flag | ✅ | Include/exclude header line |
| CLI-4.5 | `--sep` flag | ✅ | Custom column separator |
| CLI-4.6 | Special separators | ✅ | TAB, NEWLINE |
| CLI-4.7 | `--quotes` flag | ✅ | Quote character for paths |
| CLI-4.8 | `--pos` / `--neg` flags | ✅ | Attribute representation |
| CLI-4.9 | Output writer module | ✅ | `crates/uffs-core/src/output.rs` |
| CLI-4.10 | Unit tests | ✅ | 7 output formatting tests |

### Phase CLI-5: Case Sensitivity 🟢 COMPLETE

**Goal:** Optional case-sensitive matching

| ID | Deliverable | Status | Notes |
|----|-------------|--------|-------|
| CLI-5.1 | `--case` flag | ✅ | `--case` boolean flag |
| CLI-5.2 | Case-sensitive glob | ✅ | Via `ParsedPattern::with_case_sensitive()` |
| CLI-5.3 | Case-sensitive regex | ✅ | Removes `(?i)` prefix when case-sensitive |
| CLI-5.4 | Unit tests | ✅ | Pattern tests include case sensitivity |

### Phase CLI-6: Default Search Command ✅ COMPLETE

**Goal:** Make search the default action without subcommand

| ID | Deliverable | Status | Notes |
|----|-------------|--------|-------|
| CLI-6.1 | Default subcommand | ✅ | `uffs *.txt` works without `search` subcommand |
| CLI-6.2 | Backward compatibility | ✅ | `uffs search *.txt` still works |
| CLI-6.3 | Help text updates | ✅ | Pattern argument documented at top level |

### Phase CLI-7: Additional Columns ✅ COMPLETE

**Goal:** Support all columns from reference implementation

| ID | Deliverable | Status | Notes |
|----|-------------|--------|-------|
| CLI-7.1 | `pathonly` column | ✅ | Maps to `path_only` (derived from path) |
| CLI-7.2 | `type` column | ✅ | Maps to `type` (derived from extension) |
| CLI-7.3 | `sizeondisk` column | ✅ | Maps to `allocated_size` from MFT |
| CLI-7.4 | Tree columns | ✅ | `descendants`, `treesize`, `tree_allocated`, `bulkiness` - computed on-demand via `uffs-core/tree.rs` |
| CLI-7.5 | Extended attributes | ✅ | All boolean flags: hidden, system, archive, readonly, compressed, encrypted, sparse, reparse, offline, not_indexed, temporary |
| CLI-7.6 | `attributevalue` column | ✅ | Maps to `flags` column |
| CLI-7.7 | Unit tests | ✅ | Column mapping tests in output.rs |

---

## Milestone Summary

| Phase | Name | Priority | Status | Progress |
|-------|------|----------|--------|----------|
| CLI-1 | Search Pattern Syntax | High | 🟢 Complete | 100% |
| CLI-2 | Multi-Drive Search | High | 🟢 Complete | 100% |
| CLI-3 | Extension Filtering | High | 🟢 Complete | 100% |
| CLI-4 | Output Customization | Medium | 🟢 Complete | 100% |
| CLI-5 | Case Sensitivity | Medium | 🟢 Complete | 100% |
| CLI-6 | Default Search Command | Low | 🟢 Complete | 100% |
| CLI-7 | Additional Columns | Low | 🟢 Complete | 100% |

---

## Reference Examples

### Target CLI Behavior

```bash
# Simple search (all drives)
uffs /pro*.txt

# Drive-specific search
uffs c:/pro*

# Multi-drive search
uffs /pro*.txt --drives=c,d,m

# Extension filtering
uffs /pro** --ext=jpg,mp4,documents

# REGEX search
uffs ">C:\\Temp.*\.txt"

# Custom output
uffs c:/Music** --out=bigfile.csv --header=true --sep=; --columns=path,size,created

# Case-sensitive search
uffs DuaLippa --case=on
```

### Column Reference

| Column | Description | MFT Source |
|--------|-------------|------------|
| `path` | Full path + filename | Reconstructed from parent_frs |
| `name` | Filename only | $FILE_NAME attribute |
| `pathonly` | Directory path only | Derived from path |
| `type` | File extension | Derived from name |
| `size` | Actual file size | $DATA attribute |
| `sizeondisk` | Allocated size | $DATA attribute (allocated) |
| `created` | Creation time | $STANDARD_INFORMATION |
| `written` | Last write time | $STANDARD_INFORMATION |
| `accessed` | Last access time | $STANDARD_INFORMATION |
| `decendents` | Descendant count | Requires tree traversal |
| `r` | Read-only flag | $STANDARD_INFORMATION flags |
| `a` | Archive flag | $STANDARD_INFORMATION flags |
| `s` | System flag | $STANDARD_INFORMATION flags |
| `h` | Hidden flag | $STANDARD_INFORMATION flags |
| `o` | Offline flag | $STANDARD_INFORMATION flags |
| `directory` | Is directory | $FILE_NAME flags |
| `compressed` | Is compressed | $STANDARD_INFORMATION flags |
| `encrypted` | Is encrypted | $STANDARD_INFORMATION flags |
| `sparse` | Is sparse | $STANDARD_INFORMATION flags |
| `reparse` | Is reparse point | $STANDARD_INFORMATION flags |
| `attributevalue` | Raw flags value | $STANDARD_INFORMATION flags |

### Special Separator Values

| Keyword | Character |
|---------|-----------|
| `TAB` | `\t` |
| `NEWLINE` / `NEW LINE` | `\n` |
| `SPACE` | ` ` |
| `RETURN` | `\r` |
| `DOUBLE` | `"` |
| `SINGLE` | `'` |
| `NULL` | `\0` |

---

## Dependencies

- **Phase CLI-1** is foundational - pattern parsing needed for all search features
- **Phase CLI-2** depends on `MultiDriveMftReader` (already implemented in Phase 2.6)
- **Phase CLI-4** can be implemented independently
- **Phase CLI-7** (tree columns) ✅ Complete - `descendants`, `treesize`, `tree_allocated`, `bulkiness` computed on-demand in `uffs-core/tree.rs`

---

## Notes

1. The reference implementation uses `--drives=c,d,m` syntax (equals sign). Our current
   implementation uses `--drives C,D,E` (space). Consider supporting both.

2. REGEX patterns in reference start with `>` and must be quoted. This is a unique syntax
   that needs careful parsing.

3. The `decendents` (sic - typo in original) column requires tree traversal which is
   planned for Phase 3.5 in the main milestones.

4. Some features marked as "NOT implemented yet" in the reference (like `--case`) should
   still be implemented for completeness.

