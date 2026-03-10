# Pattern Matching Optimization: From Regex-Everything to Specialized Kernels

## Executive Summary

**Current State:** UFFS converts all patterns (glob, literal, regex) to regex and uses `col.str().contains(regex)` for filtering. This works but leaves significant performance on the table.

**Opportunity:** Polars provides specialized string kernels (`starts_with`, `ends_with`, `contains_literal`, `contains_any`) that are **2-10x faster** than regex for common patterns. By implementing a **Pattern IR → Specialized Lowering** architecture, we can achieve C++-level or better performance.

**Goal:** Make pattern matching cutting-edge by using the fastest possible Polars expression for each pattern type.

---

## ⚠️ Polars 2.0 API Breaking Changes

> **Important**: This document reflects the Polars 2.0+ API. Several collection-based functions have changed signatures.
> See [GitHub issue #22149](https://github.com/pola-rs/polars/issues/22149) for details.

### Affected Functions

| Function | Old Signature (Pre-2.0) | New Signature (2.0+) |
|----------|-------------------------|----------------------|
| `is_in` | `col.is_in(series)` | `col.is_in(series.implode(), nulls_equal)` |
| `str.contains_any` | `col.str().contains_any(patterns, case)` | `col.str().contains_any(patterns.implode(), case)` |
| `str.replace_many` | `col.str().replace_many(patterns, ...)` | `col.str().replace_many(patterns.implode(), ...)` |
| `str.find_many` | `col.str().find_many(patterns)` | `col.str().find_many(patterns.implode())` |
| `str.extract_many` | `col.str().extract_many(patterns)` | `col.str().extract_many(patterns.implode())` |
| `list.gather` | `col.list().gather(indices)` | `col.list().gather(indices.implode())` |
| `replace` | `col.replace(old, new)` | `col.replace(old.implode(), new.implode())` |
| `replace_strict` | `col.replace_strict(old, new)` | `col.replace_strict(old.implode(), new.implode())` |

### Why This Change?

The old behavior was ambiguous when both operands had the same datatype:
- `col.is_in(other_col)` where both are `String` could mean:
  - "Is this row's value in the entire `other_col`?" (elementwise vs. column)
  - "Is this row's value equal to the corresponding row in `other_col`?" (elementwise)

The new API requires explicit `.implode()` to wrap values into a `List` type, making the intent clear.

### Migration Pattern

```rust
// OLD (deprecated, ambiguous):
let series = Series::new("", &["a", "b", "c"]);
col("name").is_in(lit(series))

// NEW (Polars 2.0+, explicit):
let series = Series::new("".into(), &["a", "b", "c"]);
col("name").is_in(lit(series).implode(), false)  // false = nulls_equal
```

---

## Part 1: Current Implementation Analysis

### 1.1 What We Have Today

```
User Input → ParsedPattern → to_regex() → col.str().contains(regex)
```

**Current Flow:**
1. `ParsedPattern::parse()` classifies input as `Glob`, `Regex`, or `Literal`
2. `to_regex()` converts everything to a regex string
3. `MftQuery::pattern()` uses `col("name").str().contains(lit(regex), false)`

**Code Locations:**
- `crates/uffs-core/src/pattern.rs` - Pattern parsing
- `crates/uffs-core/src/glob.rs` - Glob to regex conversion
- `crates/uffs-core/src/query.rs` - Query building
- `crates/uffs-core/src/extensions.rs` - Extension filtering

### 1.2 Performance Problems

| Pattern | Current Approach | Optimal Approach (Polars 2.0+) | Speedup |
|---------|------------------|--------------------------------|---------|
| `*.txt` | `contains("^.*\\.txt$")` | `ends_with(".txt")` | **5-10x** |
| `foo*` | `contains("^foo.*$")` | `starts_with("foo")` | **5-10x** |
| `*bar` | `contains("^.*bar$")` | `ends_with("bar")` | **5-10x** |
| `readme` | `contains(".*readme.*")` | `contains_literal("readme")` | **2-5x** |
| `jpg,png,gif` | `contains("\\.(jpg\|png\|gif)$")` | `is_in(lit(series).implode(), false)` | **3-8x** |

### 1.3 Why Regex Is Slower

1. **Compilation overhead**: Regex must be compiled before matching
2. **Generality tax**: Regex engine handles backtracking, captures, etc.
3. **No SIMD for complex patterns**: Simple operations like `ends_with` use SIMD
4. **Per-row overhead**: Regex state machine runs for each string

---

## Part 2: Proposed Architecture

### 2.1 Pattern IR (Intermediate Representation)

```rust
/// Compiled pattern ready for Polars expression lowering.
#[derive(Debug, Clone)]
pub enum CompiledPattern {
    /// Always matches (e.g., "*")
    Any,
    
    /// Exact string match: `col == "value"`
    Exact(String),
    
    /// Prefix match: `col.str().starts_with("prefix")`
    Prefix(String),
    
    /// Suffix match: `col.str().ends_with("suffix")`
    Suffix(String),
    
    /// Literal substring: `col.str().contains_literal("needle")`
    Contains(String),
    
    /// Prefix AND suffix: `starts_with(p) & ends_with(s)`
    PrefixSuffix { prefix: String, suffix: String },
    
    /// Multiple exact matches: `col.is_in([...])`
    ExactSet(Vec<String>),
    
    /// Multiple literal substrings: `col.str().contains_any([...])`
    ContainsAny(Vec<String>),
    
    /// Multiple suffixes (extensions): `col.str().ends_with_any([...])`
    SuffixSet(Vec<String>),
    
    /// Fallback to regex: `col.str().contains(regex, strict)`
    Regex { pattern: String, anchored: bool },
}
```

### 2.2 Glob Classification

```rust
/// Classify a glob pattern into the most efficient form.
#[derive(Debug, Clone)]
pub enum GlobKind {
    /// "*" - matches everything
    Any,
    
    /// No metacharacters - exact match
    Exact(String),
    
    /// "foo*" - prefix only
    Prefix(String),
    
    /// "*bar" - suffix only  
    Suffix(String),
    
    /// "*needle*" - contains
    Contains(String),
    
    /// "foo*bar" - single star in middle
    PrefixSuffix { prefix: String, suffix: String },
    
    /// "*.ext" - extension pattern (special case of suffix)
    Extension(String),
    
    /// Complex: ?, [], **, multiple * not reducible
    Complex(String),
}
```

### 2.3 Lowering to Polars Expressions

```rust
impl CompiledPattern {
    /// Lower to the optimal Polars expression.
    ///
    /// NOTE: Polars 2.0 API changes (see issue #22149):
    /// - `is_in` with same datatype is deprecated; use `.implode()` to wrap values
    /// - `contains_any` expects `List<String>`, not flat `String`; use `.implode()`
    /// - These changes make the functions properly elementwise
    pub fn to_expr(&self, column: &str, case_sensitive: bool) -> Expr {
        let col_expr = if case_sensitive {
            col(column)
        } else {
            col(column).str().to_lowercase()
        };

        match self {
            CompiledPattern::Any => lit(true),

            CompiledPattern::Exact(s) => col_expr.eq(lit(s)),

            CompiledPattern::Prefix(s) => col_expr.str().starts_with(lit(s)),

            CompiledPattern::Suffix(s) => col_expr.str().ends_with(lit(s)),

            CompiledPattern::Contains(s) => col_expr.str().contains_literal(lit(s)),

            CompiledPattern::PrefixSuffix { prefix, suffix } => {
                col_expr.str().starts_with(lit(prefix))
                    .and(col_expr.str().ends_with(lit(suffix)))
            }

            // NOTE: Polars 2.0+ requires .implode() to wrap the series as List
            // Old: col.is_in(lit(series)) - DEPRECATED
            // New: col.is_in(lit(series).implode(), nulls_equal)
            CompiledPattern::ExactSet(values) => {
                let series = Series::new("".into(), values);
                col_expr.is_in(lit(series).implode(), false)
            }

            // NOTE: Polars 2.0+ requires patterns as List<String>
            // Old: col.str().contains_any(lit(series), case_insensitive)
            // New: col.str().contains_any(lit(series).implode(), case_insensitive)
            CompiledPattern::ContainsAny(patterns) => {
                let series = Series::new("".into(), patterns);
                col_expr.str().contains_any(lit(series).implode(), !case_sensitive)
            }

            CompiledPattern::SuffixSet(suffixes) => {
                // Build OR of ends_with for each suffix
                // Alternative: could use list.contains if we had a suffix list column
                suffixes.iter()
                    .map(|s| col_expr.clone().str().ends_with(lit(s)))
                    .reduce(|a, b| a.or(b))
                    .unwrap_or(lit(false))
            }

            CompiledPattern::Regex { pattern, anchored } => {
                let re = if *anchored && !pattern.starts_with('^') {
                    format!("^(?:{})$", pattern)
                } else {
                    pattern.clone()
                };
                col_expr.str().contains(lit(re), true)
            }
        }
    }
}
```

> **⚠️ Polars 2.0 API Breaking Changes**
>
> As of Polars 2.0 (tracked in [issue #22149](https://github.com/pola-rs/polars/issues/22149)), several collection-based functions have changed signatures:
>
> | Function | Old Signature | New Signature (2.0+) |
> |----------|---------------|----------------------|
> | `is_in` | `col.is_in(series)` | `col.is_in(series.implode(), nulls_equal)` |
> | `contains_any` | `col.str().contains_any(patterns, case)` | `col.str().contains_any(patterns.implode(), case)` |
> | `replace_many` | `col.str().replace_many(patterns, ...)` | `col.str().replace_many(patterns.implode(), ...)` |
> | `find_many` | `col.str().find_many(patterns)` | `col.str().find_many(patterns.implode())` |
>
> The `.implode()` call wraps the flat series into a `List` type, making the function properly elementwise.

---

## Part 3: Decision Table

### 3.1 Pattern → Compiled Form

| User Input | Detected As | Compiled To | Polars Expression |
|------------|-------------|-------------|-------------------|
| `*` | Glob Any | `Any` | `lit(true)` |
| `readme` | Literal | `Contains("readme")` | `contains_literal("readme")` |
| `README.md` | Literal (exact) | `Exact("README.md")` | `col == "README.md"` |
| `foo*` | Glob Prefix | `Prefix("foo")` | `starts_with("foo")` |
| `*bar` | Glob Suffix | `Suffix("bar")` | `ends_with("bar")` |
| `*.txt` | Glob Extension | `Suffix(".txt")` | `ends_with(".txt")` |
| `*needle*` | Glob Contains | `Contains("needle")` | `contains_literal("needle")` |
| `foo*bar` | Glob PrefixSuffix | `PrefixSuffix` | `starts_with("foo") & ends_with("bar")` |
| `foo*.txt` | Glob PrefixSuffix | `PrefixSuffix` | `starts_with("foo") & ends_with(".txt")` |
| `a*b*c` | Glob Complex | `Regex` | `contains("^a.*b.*c$")` |
| `file?.txt` | Glob Complex | `Regex` | `contains("^file.\\.txt$")` |
| `[abc]*` | Glob Complex | `Regex` | `contains("^[abc].*$")` |
| `**/*.rs` | Glob Complex | `Regex` | `contains(".*\\.rs$")` |
| `>.*\.log$` | Regex | `Regex` | `contains(".*\\.log$")` |

### 3.2 Multi-Pattern Optimization (Polars 2.0+)

| Scenario | Input | Compiled To | Polars Expression |
|----------|-------|-------------|-------------------|
| Extension list | `--ext jpg,png,gif` | `SuffixSet` | `ends_with(".jpg") \| ends_with(".png") \| ...` |
| Extension list (optimized) | `--ext jpg,png,gif` | Use `ext` column | `col("ext").is_in(lit(series).implode(), false)` |
| Search terms | `foo bar baz` | `ContainsAny` | `contains_any(lit(series).implode(), false)` |
| Exact names | `README.md,LICENSE` | `ExactSet` | `is_in(lit(series).implode(), false)` |

> **Note**: Polars 2.0+ requires `.implode()` for `is_in` and `contains_any` to wrap values as `List` type.

---

## Part 4: Extension Column Optimization

### 4.1 The Problem

Currently, `*.txt` is matched against the full filename using `ends_with(".txt")`. This works but:
- Scans the entire filename string
- No index utilization

### 4.2 The Solution: Pre-computed Extension Column

Add an `ext` column during MFT parsing:

```rust
// In ParsedColumns or DataFrame building
let ext = name.rsplit('.').next()
    .filter(|e| e.len() < name.len())  // Has a dot
    .map(str::to_lowercase);
```

**Benefits:**
- `*.txt` becomes `col("ext").eq(lit("txt"))` - **O(1) comparison**
- Extension lists become `col("ext").is_in([...])` - **hash lookup**
- Can build `ExtensionIndex` for even faster queries

### 4.3 Case Sensitivity

For Windows (case-insensitive by default):
- Store `ext` as lowercase
- Store `name_lc` (lowercase name) for case-insensitive searches
- Only use original `name` for case-sensitive mode

---

## Part 5: Aho-Corasick Multi-Pattern Matching

### 5.1 When to Use

Polars `contains_any` uses Aho-Corasick internally for multi-pattern matching:

```rust
// Instead of:
col("name").str().contains_literal(lit("foo"))
    .or(col("name").str().contains_literal(lit("bar")))
    .or(col("name").str().contains_literal(lit("baz")))

// Use (Polars 2.0+ requires .implode()):
let patterns = Series::new("".into(), &["foo", "bar", "baz"]);
col("name").str().contains_any(lit(patterns).implode(), false)
```

> **⚠️ Polars 2.0 API Change**: `contains_any` now expects a `List<String>` column.
> Use `.implode()` to wrap the patterns series. See [issue #22149](https://github.com/pola-rs/polars/issues/22149).

**Performance:**
- Single pass through each string
- O(n + m) where n = string length, m = total pattern length
- Much faster than N separate `contains` calls

### 5.2 Threshold

Use `contains_any` when:
- 3+ literal patterns to match
- Patterns are substrings (not prefix/suffix)

---

## Part 6: Benchmarking Strategy

Add to `crates/uffs-core/benches/query.rs`:

```rust
fn bench_pattern_matching(c: &mut Criterion) {
    let df = load_test_mft();  // ~1M files

    let mut group = c.benchmark_group("pattern_matching");

    // Suffix patterns
    group.bench_function("suffix_regex", |b| {
        b.iter(|| df.clone().lazy()
            .filter(col("name").str().contains(lit(".*\\.txt$"), true))
            .collect())
    });

    group.bench_function("suffix_ends_with", |b| {
        b.iter(|| df.clone().lazy()
            .filter(col("name").str().ends_with(lit(".txt")))
            .collect())
    });

    group.finish();
}
```

---

## Part 7: Implementation Plan

### Phase 1: Pattern IR & Classification ✅ COMPLETE

**Goal:** Create the Pattern IR and glob classifier without changing existing behavior.

**Deliverables:**
1. ✅ `CompiledPattern` enum in `crates/uffs-core/src/compiled_pattern.rs`
2. ✅ `GlobKind` enum and `classify_glob()` function
3. ✅ `compile_pattern()` function: `ParsedPattern → CompiledPattern`
4. ✅ Unit tests for all pattern classifications (23 tests)

**Files Created/Modified:**
- ✅ NEW: `crates/uffs-core/src/compiled_pattern.rs`
- ✅ MODIFY: `crates/uffs-core/src/lib.rs` (add module)

### Phase 2: Expression Lowering ✅ COMPLETE

**Goal:** Implement `to_expr()` for all `CompiledPattern` variants.

**Deliverables:**
1. ✅ `CompiledPattern::to_expr()` method with SIMD-optimized operations
2. ✅ Integration with `MftQuery::pattern()`
3. ✅ 12 additional tests for expression lowering

**Optimized Operations Implemented:**
- `starts_with` for prefix patterns
- `ends_with` for suffix patterns
- `contains_literal` for substring patterns
- `is_in` for exact set matching (Polars 2.0+ compatible with `.implode()`)
- `contains_any` for multi-pattern matching (Aho-Corasick)

**Files Modified:**
- ✅ `crates/uffs-core/src/compiled_pattern.rs`
- ✅ `crates/uffs-core/src/query.rs`

### Phase 3: Extension Column ✅ COMPLETE

**Goal:** Add pre-computed `ext` column for fast extension queries.

**Deliverables:**
1. ✅ `ext_expr()` function to extract extension via regex
2. ✅ `add_ext_column()` to add `ext` column to DataFrame
3. ✅ `has_ext_column()` to check if `ext` column exists
4. ✅ `MftQuery::extension_filter_fast()` using `is_in()` on `ext` column
5. ✅ `MftQuery::extension_filter()` optimized with `ends_with` chain

**Files Modified:**
- ✅ `crates/uffs-core/src/extensions.rs`
- ✅ `crates/uffs-core/src/query.rs`
- ✅ `crates/uffs-core/src/lib.rs` (exports)

### Phase 4: Multi-Pattern Optimization ✅ COMPLETE

**Goal:** Use Aho-Corasick for multi-pattern queries.

**Deliverables:**
1. ✅ `ContainsAny` variant using `contains_any()` (Aho-Corasick)
2. ✅ `SuffixSet` for multiple extensions
3. ✅ `ExactSet` using `is_in()`
4. ✅ All variants implemented in `to_expr()`

**Files Modified:**
- ✅ `crates/uffs-core/src/compiled_pattern.rs`
- ✅ `crates/uffs-core/src/extensions.rs`

### Phase 5: Case Sensitivity Optimization ✅ COMPLETE

**Goal:** Optimize case-insensitive matching.

**Deliverables:**
1. ✅ Case-insensitive support in `to_expr()` via `to_lowercase()`
2. ✅ Pattern values normalized at compile time
3. ✅ Extension column stores lowercase extensions

**Note:** The `name_lc` pre-computed column is optional and can be added
during MFT parsing if needed for further optimization. Current implementation
uses `to_lowercase()` at query time which is still fast for typical workloads.

**Files Modified:**
- ✅ `crates/uffs-core/src/compiled_pattern.rs`

### Phase 6: Integration & Benchmarking ✅ COMPLETE

**Goal:** Full integration and performance validation.

**Deliverables:**
1. ✅ All query paths use `CompiledPattern`
2. ✅ 121 tests passing in uffs-core
3. ✅ Full workspace clippy clean
4. ✅ All workspace tests passing

**Test Summary:**
- 35 tests for `compiled_pattern` module
- 12 tests for extension column functionality
- 9 tests for optimized query methods
- 121 total tests in uffs-core

---

## Part 8: Expected Performance Gains

### 8.1 Per-Pattern Speedup

| Pattern Type | Current (regex) | Optimized | Expected Speedup |
|--------------|-----------------|-----------|------------------|
| `*.txt` (suffix) | ~50ms | ~5ms | **10x** |
| `foo*` (prefix) | ~50ms | ~5ms | **10x** |
| `*needle*` (contains) | ~50ms | ~15ms | **3x** |
| `readme` (literal) | ~50ms | ~15ms | **3x** |
| `jpg,png,gif` (multi-ext) | ~150ms | ~10ms | **15x** |
| Complex glob | ~50ms | ~50ms | **1x** (no change) |

*Times are illustrative for ~1M files*

### 8.2 Overall Search Pipeline

| Scenario | Current | Optimized | Improvement |
|----------|---------|-----------|-------------|
| `*.txt` on 1M files | ~100ms | ~15ms | **6x** |
| `--ext pictures` (12 exts) | ~600ms | ~20ms | **30x** |
| `*config*` | ~100ms | ~30ms | **3x** |
| Complex regex | ~100ms | ~100ms | **1x** |

### 8.3 Memory Impact

| Optimization | Memory Cost | Benefit |
|--------------|-------------|---------|
| `ext` column | +8 bytes/file | 10-30x faster extension queries |
| `name_lc` column | +20 bytes/file | Avoid per-query lowercase |
| Total for 1M files | +28 MB | Significant speedup |

**Recommendation:** Add `ext` column (low cost, high benefit). Make `name_lc` optional.

---

## Part 9: Comparison with C++

### 9.1 C++ String Matching

The legacy implementation uses:
- **Boyer-Moore-Horspool** for literal substring search
- **Boost.Xpressive** for regex
- **Custom iterators** for case-insensitive matching

### 9.2 Rust/Polars Advantages

| Feature | C++ | Rust/Polars | Winner |
|---------|-----|-------------|--------|
| Literal substring | Boyer-Moore-Horspool | `contains_literal` (SIMD) | **Rust** |
| Prefix/Suffix | Manual | `starts_with`/`ends_with` (SIMD) | **Rust** |
| Multi-pattern | Loop | Aho-Corasick (`contains_any`) | **Rust** |
| Regex | Boost.Xpressive | Rust `regex` crate | **Tie** |
| Parallelism | Manual threading | Polars auto-parallel | **Rust** |
| Extension lookup | Scan all files | `ext` column + `is_in` | **Rust** |

### 9.3 Conclusion

With the proposed optimizations, Rust/Polars should be **faster than the legacy baseline** for pattern matching because:
1. Polars uses SIMD for string operations
2. Aho-Corasick is faster than looping for multi-pattern
3. Pre-computed `ext` column enables O(1) extension lookup
4. Polars auto-parallelizes across CPU cores

---

## Part 10: Milestones

### Milestone 1: Pattern IR Foundation ✅ → ⏳
- [ ] Create `CompiledPattern` enum
- [ ] Create `GlobKind` enum
- [ ] Implement `classify_glob()` function
- [ ] Implement `compile_pattern()` function
- [ ] Unit tests for all classifications

### Milestone 2: Expression Lowering ⏳
- [ ] Implement `to_expr()` for all variants
- [ ] Integrate with `MftQuery::pattern()`
- [ ] Add benchmarks to `query.rs`
- [ ] Verify no regression in existing tests

### Milestone 3: Extension Column ⏳
- [ ] Add `ext` column to `ParsedColumns`
- [ ] Populate during MFT parsing
- [ ] Update `CompiledPattern::Extension` to use `ext` column
- [ ] Update `ExtensionFilter` to use `is_in()`
- [ ] Benchmark extension queries

### Milestone 4: Multi-Pattern Optimization ⏳
- [ ] Implement `ContainsAny` with `contains_any()`
- [ ] Implement `SuffixSet` for multiple extensions
- [ ] Implement `ExactSet` with `is_in()`
- [ ] Add threshold logic for multi-pattern selection
- [ ] Benchmark multi-pattern queries

### Milestone 5: Case Sensitivity ⏳
- [ ] Add optional `name_lc` column
- [ ] Use `ascii_case_insensitive` in `contains_any()`
- [ ] Benchmark case-insensitive queries

### Milestone 6: Integration & Validation ⏳
- [ ] Update all query paths
- [ ] Run full benchmark suite
- [ ] Compare with C++ baseline
- [ ] Update documentation
- [ ] CI integration

---

## References

- [Polars StringNameSpace](https://docs.pola.rs/api/rust/dev/polars_lazy/dsl/string/struct.StringNameSpace.html)
- [Polars String Operations (Aho-Corasick)](https://deepwiki.com/pola-rs/polars/6.3-string-operations)
- [Rust regex crate (prefilters)](https://docs.rs/regex/latest/regex/)
- [globset crate](https://docs.rs/globset/latest/globset/)
- [UFFS Filter Optimization Guide](./uffs_polars_filter_optimization.md)

---

## Appendix A: Quick Reference

### A.1 Pattern Classification Algorithm

```rust
fn classify_glob(pattern: &str) -> GlobKind {
    // Check for metacharacters
    let has_star = pattern.contains('*');
    let has_question = pattern.contains('?');
    let has_bracket = pattern.contains('[');
    let has_double_star = pattern.contains("**");

    // No metacharacters = exact match
    if !has_star && !has_question && !has_bracket {
        return GlobKind::Exact(pattern.to_string());
    }

    // Complex patterns (?, [], **)
    if has_question || has_bracket || has_double_star {
        return GlobKind::Complex(pattern.to_string());
    }

    // Single star patterns
    let star_count = pattern.matches('*').count();

    match star_count {
        1 => {
            if pattern == "*" {
                GlobKind::Any
            } else if pattern.starts_with('*') && pattern.ends_with('*') {
                // *needle* → Contains
                let needle = &pattern[1..pattern.len()-1];
                GlobKind::Contains(needle.to_string())
            } else if pattern.starts_with('*') {
                // *suffix → Suffix
                let suffix = &pattern[1..];
                if suffix.starts_with('.') && !suffix[1..].contains('.') {
                    GlobKind::Extension(suffix[1..].to_string())
                } else {
                    GlobKind::Suffix(suffix.to_string())
                }
            } else if pattern.ends_with('*') {
                // prefix* → Prefix
                GlobKind::Prefix(pattern[..pattern.len()-1].to_string())
            } else {
                // prefix*suffix → PrefixSuffix
                let parts: Vec<&str> = pattern.split('*').collect();
                GlobKind::PrefixSuffix {
                    prefix: parts[0].to_string(),
                    suffix: parts[1].to_string(),
                }
            }
        }
        2 if pattern.starts_with('*') && pattern.ends_with('*') => {
            // *needle* with exactly 2 stars
            let needle = &pattern[1..pattern.len()-1];
            if !needle.contains('*') {
                GlobKind::Contains(needle.to_string())
            } else {
                GlobKind::Complex(pattern.to_string())
            }
        }
        _ => GlobKind::Complex(pattern.to_string()),
    }
}
```

### A.2 Polars Expression Cheat Sheet (Polars 2.0+)

| Operation | Polars Expression | Notes |
|-----------|-------------------|-------|
| Exact match | `col.eq(lit("value"))` | Fastest |
| Prefix | `col.str().starts_with(lit("pre"))` | SIMD |
| Suffix | `col.str().ends_with(lit("suf"))` | SIMD |
| Contains literal | `col.str().contains_literal(lit("needle"))` | SIMD |
| Contains regex | `col.str().contains(lit("pat"), true)` | Slower |
| Multi-literal | `col.str().contains_any(lit(series).implode(), false)` | Aho-Corasick, **requires `.implode()`** |
| In set | `col.is_in(lit(series).implode(), false)` | Hash lookup, **requires `.implode()`** |
| Lowercase | `col.str().to_lowercase()` | Pre-compute if possible |
| Implode | `expr.implode()` | Wraps values into `List` type |

> **Note**: As of Polars 2.0, `is_in`, `contains_any`, `replace_many`, `find_many`, and `extract_many`
> require the collection argument to be a `List` type. Use `.implode()` to convert a flat series/column.

---

## Appendix B: Testing Strategy

### B.1 Unit Tests for Pattern Classification

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_any() {
        assert!(matches!(classify_glob("*"), GlobKind::Any));
    }

    #[test]
    fn test_classify_exact() {
        assert!(matches!(classify_glob("readme.txt"), GlobKind::Exact(_)));
    }

    #[test]
    fn test_classify_prefix() {
        assert!(matches!(classify_glob("foo*"), GlobKind::Prefix(_)));
    }

    #[test]
    fn test_classify_suffix() {
        assert!(matches!(classify_glob("*bar"), GlobKind::Suffix(_)));
    }

    #[test]
    fn test_classify_extension() {
        assert!(matches!(classify_glob("*.txt"), GlobKind::Extension(_)));
    }

    #[test]
    fn test_classify_contains() {
        assert!(matches!(classify_glob("*needle*"), GlobKind::Contains(_)));
    }

    #[test]
    fn test_classify_prefix_suffix() {
        assert!(matches!(classify_glob("foo*bar"), GlobKind::PrefixSuffix { .. }));
    }

    #[test]
    fn test_classify_complex() {
        assert!(matches!(classify_glob("file?.txt"), GlobKind::Complex(_)));
        assert!(matches!(classify_glob("[abc]*"), GlobKind::Complex(_)));
        assert!(matches!(classify_glob("**/*.rs"), GlobKind::Complex(_)));
        assert!(matches!(classify_glob("a*b*c"), GlobKind::Complex(_)));
    }
}
```

### B.2 Integration Tests

```rust
#[test]
fn test_pattern_matching_equivalence() {
    let df = create_test_dataframe();

    // Old approach (regex)
    let old_result = df.clone().lazy()
        .filter(col("name").str().contains(lit(".*\\.txt$"), true))
        .collect().unwrap();

    // New approach (ends_with)
    let new_result = df.clone().lazy()
        .filter(col("name").str().ends_with(lit(".txt")))
        .collect().unwrap();

    assert_eq!(old_result.height(), new_result.height());
}
```

---

*Document Version: 1.0*
*Last Updated: 2026-01-19*
*Author: UFFS Development Team*
