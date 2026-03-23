# Pattern Matching & Search

## Introduction

This document describes UFFS's pattern matching engine and search pipeline. After reading this document, you should be able to:

1. Understand how search patterns are parsed, classified, and compiled
2. Implement the optimized matching strategies for each pattern type
3. Use the extension index for O(matches) `*.ext` queries
4. Build the full search pipeline from user input to matched results

---

## Pattern Parsing

**Source:** `uffs-core/src/pattern.rs`

### ParsedPattern

User input is parsed into a structured `ParsedPattern`:

```rust
pub struct ParsedPattern {
    drive: Option<char>,        // Drive letter if specified ('C')
    pattern: String,            // Pattern without drive prefix
    pattern_type: PatternType,  // Glob, Regex, or Literal
    case_sensitive: bool,       // Default: false (Windows semantics)
    is_path_pattern: bool,      // Contains \ or / → match full path
}

pub enum PatternType {
    Glob,     // *.rs, **/*.txt, foo*bar
    Regex,    // Starts with > prefix
    Literal,  // No wildcards → substring match
}
```

### Parsing Rules

```
Input: "c:/pro*"
  → drive=Some('C'), pattern="/pro*", type=Glob, is_path=true

Input: "*.rs"
  → drive=None, pattern="*.rs", type=Glob, is_path=false

Input: ">C:\\Temp.*\.txt"
  → drive=None, pattern="C:\\Temp.*\.txt", type=Regex, is_path=true

Input: "readme"
  → drive=None, pattern="readme", type=Literal, is_path=false
```

### Path Pattern Detection

A pattern is **path-aware** when it contains directory separators (`\` or `/`). Path patterns match against the **full resolved path** instead of just the filename:

```rust
fn is_path_pattern(pattern: &str) -> bool {
    pattern.contains('\\') || pattern.contains('/')
}
```

---

## Pattern Compilation

**Source:** `uffs-core/src/index_search/pattern.rs`

Patterns are compiled into optimized `IndexPattern` variants for maximum matching speed.

### IndexPattern Variants

| Variant | Pattern Example | Strategy | Complexity |
|---------|----------------|----------|------------|
| `Any` | `*` | Always true | O(1) |
| `Exact` | `readme.txt` | String equality | O(n) |
| `Prefix` | `foo*` | `starts_with` | O(prefix) |
| `Suffix` | `*.txt` | `ends_with` | O(suffix) |
| `Contains` | `*needle*` | Substring search | O(n) |
| `PrefixSuffix` | `foo*bar` | Both ends | O(prefix+suffix) |
| `ExactSet` | `a.txt\|b.txt` | HashSet lookup | O(1) amortized |
| `SuffixSet` | `*.txt\|*.log` | Multi-suffix | O(k×suffix) |
| `ContainsAny` | `*a*\|*b*` | Aho-Corasick | O(n) |
| `Regex` | `>.*\.txt$` | Full regex | O(n) per regex |
| `Or` | `*.rs\|*.toml` | Any sub-pattern | Sum of children |

### Glob Classification

**Source:** `uffs-core/src/compiled_pattern/mod.rs`

Before compilation, glob patterns are classified to select the optimal strategy:

```rust
pub enum GlobKind {
    Any,           // * (matches everything)
    Exact,         // readme.txt (no wildcards)
    Extension,     // *.txt (single extension)
    Prefix,        // foo*
    Suffix,        // *bar
    PrefixSuffix,  // foo*bar
    Complex,       // Multiple wildcards, character classes
}

pub fn classify_glob(pattern: &str) -> GlobKind {
    // Count wildcards, check positions
    // Determine simplest possible matching strategy
}
```

### Case-Insensitive Matching (Zero Allocation)

For case-insensitive matching, UFFS avoids allocating lowercase copies of input strings. Instead, it uses **byte-level comparison**:

```rust
fn starts_with_ignore_ascii_case(input: &str, prefix_lower: &str) -> bool {
    if input.len() < prefix_lower.len() { return false; }
    input.as_bytes()[..prefix_lower.len()]
        .iter()
        .zip(prefix_lower.as_bytes())
        .all(|(a, b)| a.to_ascii_lowercase() == *b)
}

fn ends_with_ignore_ascii_case(input: &str, suffix_lower: &str) -> bool {
    if input.len() < suffix_lower.len() { return false; }
    let start = input.len() - suffix_lower.len();
    input.as_bytes()[start..]
        .iter()
        .zip(suffix_lower.as_bytes())
        .all(|(a, b)| a.to_ascii_lowercase() == *b)
}
```

For 8M records this eliminates **8M heap allocations** that `.to_ascii_lowercase()` would create.

---

## Extension Index

**Source:** `uffs-core/src/extensions/`, `uffs-mft/src/index/extensions.rs`

### The Problem

For `*.txt` patterns, scanning all 2M records and checking `ends_with(".txt")` takes ~100ms. We can do better.

### The Solution: Extension Index

During index building, every filename's extension is **interned** — mapped to a numeric ID:

```rust
pub struct ExtensionTable {
    ext_to_id: HashMap<String, u16>,  // "txt" → 1, "rs" → 2
    id_to_ext: Vec<String>,           // [1] = "txt", [2] = "rs"
}
```

Each `IndexNameRef` stores its `extension_id` in the upper 16 bits of the `meta` field.

After parsing, an `ExtensionIndex` is built:

```rust
pub struct ExtensionIndex {
    // ext_id → sorted Vec<record_index>
    buckets: Vec<Vec<u32>>,
}

impl ExtensionIndex {
    pub fn get(&self, ext_id: u16) -> &[u32] {
        // Returns all record indices with this extension
        // O(1) lookup, O(matches) iteration
    }
}
```

### Query Flow for `*.txt`

```
1. Parse "*.txt" → GlobKind::Extension, extension="txt"
2. Look up ext_id = extensions.get_id("txt") → 1
3. Get records = extension_index.get(1) → [42, 99, 1234, ...]
4. Iterate only matching records (O(matches) instead of O(all))
```

**Performance:** For a 2M-record index with 50K `.txt` files:
- Full scan: 100ms
- Extension index: **2ms** (50× faster)

### Smart Extension Extraction

For complex patterns like `*hallo*.txt`, UFFS extracts the trailing extension and uses the extension index as a **pre-filter**:

```rust
fn extract_trailing_extension(pattern: &str) -> Option<&str> {
    // "*hallo*.txt" → Some("txt")
    // "*.tar.gz" → Some("gz")
    // "*.*" → None (too broad)
}
```

This narrows the candidate set before applying the full pattern match.

---

## Search Pipeline

### IndexQuery

**Source:** `uffs-core/src/index_search/query.rs`

```rust
pub struct IndexQuery<'a> {
    index: &'a MftIndex,
    pattern: Option<IndexPattern>,
    type_filter: TypeFilter,
    min_size: Option<u64>,
    max_size: Option<u64>,
    case_sensitive: bool,
    is_path_pattern: bool,
    limit: Option<usize>,
}

pub enum TypeFilter {
    All,
    FilesOnly,
    DirsOnly,
}
```

### Query Execution

```
IndexQuery::collect()
  │
  ├─► Extension index fast path (if *.ext pattern):
  │   Get record indices from ExtensionIndex
  │   Apply remaining filters (size, type, case)
  │   Resolve paths if needed
  │
  └─► Full scan path (all other patterns):
      Iterate all records
      For each record:
        ├─► Type filter (files/dirs only)
        ├─► Pattern match (filename or full path)
        ├─► Size filter (min/max)
        ├─► Attribute filters (hidden, system, etc.)
        └─► If all pass → add to results
```

### SearchResult

```rust
pub struct SearchResult {
    pub record_index: u32,   // Index into MftIndex::records
    pub frs: u64,            // FRS number
    pub name: String,        // Filename
    pub path: String,        // Full resolved path
    pub size: u64,           // File size
    pub is_directory: bool,
    // ... additional fields ...
}
```

---

## Query Routing

**Source:** `uffs-core/src/index_search/routing.rs`

UFFS analyzes query complexity to choose the optimal execution path:

```rust
pub enum QueryMode {
    IndexDirect,   // Fast: search MftIndex directly
    DataFrame,     // Slow: convert to DataFrame for complex queries
}

pub struct QueryFeatures {
    pub has_pattern: bool,
    pub has_size_filter: bool,
    pub has_type_filter: bool,
    pub has_date_filter: bool,
    pub has_attribute_filter: bool,
    pub needs_aggregation: bool,
}

pub fn analyze_pattern_complexity(features: &QueryFeatures) -> QueryMode {
    if features.needs_aggregation {
        QueryMode::DataFrame  // Need Polars for GROUP BY, etc.
    } else {
        QueryMode::IndexDirect  // Fast path handles everything else
    }
}
```

---

## Smart Case

When `--smart-case` is enabled (default in CLI):
- Pattern has uppercase letters → case-sensitive
- Pattern is all lowercase → case-insensitive

```rust
fn is_smart_case_sensitive(pattern: &str) -> bool {
    pattern.chars().any(|c| c.is_uppercase())
}
```

This matches the behavior of tools like ripgrep and provides intuitive search semantics.

---

## Polars Query Path

**Source:** `uffs-core/src/query/`

For complex analytics queries, UFFS provides a fluent Polars-based API:

```rust
pub struct MftQuery {
    lazy_frame: LazyFrame,
}

impl MftQuery {
    pub fn new(df: DataFrame) -> Self { ... }

    pub fn glob(self, pattern: &str) -> Self {
        // Adds Polars filter expression for glob matching
    }

    pub fn files_only(self) -> Self {
        // Filters to non-directory records
    }

    pub fn min_size(self, size: u64) -> Self {
        // Adds size >= filter
    }

    pub fn sort_by_size(self, descending: bool) -> Self {
        // Adds ORDER BY size
    }

    pub fn limit(self, n: usize) -> Self {
        // Adds LIMIT n
    }

    pub fn collect(self) -> Result<DataFrame> {
        self.lazy_frame.collect()
    }
}
```

This path is ~10-50× slower than `IndexQuery` but supports:
- Complex aggregations (GROUP BY extension, size buckets)
- JOIN operations
- Custom Polars expressions
- Export to Parquet/CSV

---

## Performance Comparison

### Search Latency (2M records)

| Query | IndexQuery | DataFrame | Speedup |
|-------|-----------|-----------|---------|
| `*` (all) | 50ms | 3s | 60× |
| `*.rs` (ext index) | 2ms | 500ms | 250× |
| `*hello*` (contains) | 80ms | 2s | 25× |
| `>regex` | 200ms | 3s | 15× |
| Size filter only | 60ms | 1s | 17× |

The `IndexQuery` path is always preferred for interactive search. The DataFrame path is used only for analytics and export.

---

*Document Version: 1.0*
*Last Updated: 2026-03-23*
*UFFS Version: 0.3.62*
