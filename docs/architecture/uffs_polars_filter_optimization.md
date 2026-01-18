# UFFS filter optimization in Rust + Polars (Regex / Glob / Literal)

Context: UFFS builds an in-memory index of NTFS MFT records and queries it using Polars DataFrames/LazyFrames (see the Rust rewrite README: https://github.com/githubrobbi/UltraFastFileSearch).

Goal: accept user filters in multiple syntaxes (regex / glob / plain text) and run them as fast as possible over (potentially) tens of millions of rows.

---

## Executive recommendation

**Do not normalize everything to “just regex”.** Instead:

1. Parse every user filter into a small, semantic **Pattern IR** (an AST of what the user means: exact match, prefix, suffix, contains, glob, regex, etc.).
2. Lower that IR into the **fastest Polars expression** available for that meaning (e.g. `starts_with`, `ends_with`, `contains_literal`, `contains_any`, `is_in`), and only fall back to regex when necessary.
3. For “advanced” matching that Polars does not natively expose (e.g. large glob sets, fuzzy matching), use a **Polars expression plugin** (Rust-native UDF) rather than `map_elements`/per-row loops.

This gives you:
- the *same* user-facing flexibility,
- maximum speed on the common cases (`*.rs`, `project*`, `foo`, `*bar`),
- and a clean escape hatch for complex patterns.

Why this is the “master move”:
- Polars has **specialized string kernels** in the Rust API: `contains_literal`, `starts_with`, `ends_with`, and multi-pattern `contains_any` (Aho-Corasick). See the Rust `StringNameSpace` methods list and docs.  
  https://docs.pola.rs/api/rust/dev/polars_lazy/dsl/string/struct.StringNameSpace.html  
- Polars `contains` uses the Rust `regex` crate and supports strict/invalid-regex handling.  
  https://docs.pola.rs/api/rust/dev/polars_lazy/dsl/string/struct.StringNameSpace.html  
- Polars also has a **multi-pattern Aho-Corasick system** (feature `find_many`) that can be significantly faster than iterating patterns.  
  https://deepwiki.com/pola-rs/polars/6.3-string-operations  
- The Rust `regex` crate itself does literal extraction/prefilters, and is often fast even when you compile “regex-like” patterns (good news for glob->regex fallback).  
  https://docs.rs/regex/latest/regex/  
  https://deepwiki.com/rust-lang/regex/3.9-prefilters-and-literal-optimization  

---

## Key performance facts to anchor decisions

### 1) Polars gives you multiple matching operators; they are not equivalent
From the Rust API `StringNameSpace` (lazy DSL), you have (relevant subset):
- `starts_with(sub: Expr) -> Expr`
- `ends_with(sub: Expr) -> Expr`
- `contains_literal(pat: Expr) -> Expr`  (literal substring)
- `contains(pat: Expr, strict: bool) -> Expr` (regex)
- `contains_any(patterns: Expr, ascii_case_insensitive: bool) -> Expr` (multi-pattern Aho-Corasick)
Docs: https://docs.pola.rs/api/rust/dev/polars_lazy/dsl/string/struct.StringNameSpace.html

### 2) Regex is powerful, but compilation and “genericity” are real costs
- Even if matching is linear-time, compiling a regex and running a general engine can be more work than a simple prefix/suffix check.
- The Rust `regex` crate mitigates a lot of this via literal extraction/prefilters, but you don’t get “free” wins like `contains_any` unless you choose them explicitly.  
  https://docs.rs/regex/latest/regex/  
  https://deepwiki.com/rust-lang/regex/3.9-prefilters-and-literal-optimization  

### 3) Multi-pattern matching should never be a loop
If the user supplies multiple patterns (OR lists, extension lists, include/exclude lists), don’t do:
- `expr1 | expr2 | expr3 | ...` with N large, or
- per-row UDF loops

Prefer:
- `contains_any` (Aho-Corasick) for a set of literal patterns, or
- `is_in` for exact matches, or
- a single compiled matcher (globset/regex-set) via a Polars expression plugin.

Polars describes its multi-pattern system and Aho-Corasick usage here:  
https://deepwiki.com/pola-rs/polars/6.3-string-operations  

---

## What “normalize to regex” gets right (and what it misses)

### The good
- One unified matcher; easy to implement.
- Converting glob to regex is standard; even the `globset` crate (used by ripgrep) compiles globs to regex and can be competitive or faster due to regex optimizations.  
  https://docs.rs/globset/latest/globset/  
  https://github.com/BurntSushi/ripgrep/blob/master/crates/globset/README.md  

### The bad (for your specific workload)
- You miss very cheap kernels (`starts_with`, `ends_with`, `contains_literal`) on common patterns.
- You miss `contains_any` for multi-literal OR queries.
- You increase complexity around escaping rules and “what syntax am I in?” for users.
- You’re more likely to accept regex DoS-ish patterns (even though Rust regex avoids catastrophic backtracking, user patterns can still be expensive due to size/complexity).  
  Rust regex guarantees worst-case O(m*n) and omits features like look-around/backrefs to keep things efficient.  
  https://github.com/rust-lang/regex  

**Net: a regex-only backend is acceptable as a baseline, but it is not the fastest design.** The fastest design is *semantic IR -> specialized lowering*.

---

## Proposed architecture: Pattern IR -> Polars Expr lowering

### Step 0: normalize the input, not the engine
Before you even classify patterns:
- Normalize path separators if you match path strings (`/` vs `\` on Windows).
- Decide on case semantics up front (Windows is generally case-insensitive for paths). If your default is case-insensitive, consider storing a pre-lowercased column to avoid per-query `to_lowercase()` allocations.

### Step 1: parse into a small Pattern IR
Example:

```rust
enum MatchMode {
    LiteralSubstr,
    LiteralExact,
    Prefix,
    Suffix,
    GlobWhole,   // glob semantics: match the entire string
    RegexSubstr, // regex semantics: match anywhere in string (Polars contains)
    RegexWhole,  // anchored regex
}

struct Pattern {
    mode: MatchMode,
    pattern: String,
    case_sensitive: bool,
}
```

But in practice you want slightly more structure for globs:

```rust
enum GlobKind {
    Any,                // "*"
    Exact(String),      // no meta
    Prefix(String),     // "foo*"
    Suffix(String),     // "*bar"
    PrefixSuffix { prefix: String, suffix: String }, // "foo*bar" (single star)
    Complex(String),    // anything with ?, [], multiple * not reducible
}
```

### Step 2: lower to the cheapest correct Polars Expr
Assume you’re matching against `col("path")` or `col("name")`.

**Literal:**
- exact: `col.eq(lit(s))`
- substring: `col.str().contains_literal(lit(s))`

**Prefix/suffix:**
- `col.str().starts_with(lit(prefix))`
- `col.str().ends_with(lit(suffix))`

**Glob whole-string:**
- `*` -> `lit(true)`
- exact (no meta) -> exact or contains_literal depending on your UX
- `foo*` -> starts_with
- `*bar` -> ends_with
- `foo*bar` (single `*`, no other meta) -> `starts_with(foo) & ends_with(bar)` (this is *exactly* correct for whole-string glob)
- complex -> convert to anchored regex and use `contains(regex, strict=true)`.

**Regex:**
- substring regex -> `col.str().contains(lit(re), /*strict*/ true)`
- whole-string regex -> anchor: `^(?:re)$` and use `contains(...)`.

The Rust Polars DSL entry points for these are all in `StringNameSpace`.  
https://docs.pola.rs/api/rust/dev/polars_lazy/dsl/string/struct.StringNameSpace.html

### Step 3: (optional) use Polars lazy API to get optimizer + parallelism
Even if your “data is in a DataFrame”, build your query as a `LazyFrame` and only `collect` at the end when possible. Polars' lazy model exists specifically to optimize queries.  
https://docs.pola.rs/user-guide/concepts/lazy-api/  
https://docs.pola.rs/user-guide/lazy/execution/  

---

## Decision table (what to do for each incoming filter)

| Incoming filter | Recognize as | Best Polars lowering | Notes |
|---|---|---|---|
| `foo` (no meta) | literal substring (or exact, if you choose) | `contains_literal("foo")` | fast; no regex parsing |
| `"foo"` (quoted) or `=foo` | exact | `col == "foo"` | consider adding explicit exact syntax |
| `foo*` | prefix glob | `starts_with("foo")` | very fast |
| `*bar` | suffix glob | `ends_with("bar")` | very fast |
| `foo*bar` (one `*`) | prefix+suffix glob | `starts_with("foo") & ends_with("bar")` | exact glob semantics |
| `*.rs` | ext glob | prefer `col("ext") == "rs"` | faster than scanning full path/name |
| `*foo*` | contains glob | `contains_literal("foo")` | exact equivalence |
| complex glob (`a*b*c`, `?`, `[...]`, `**`) | complex glob | anchored regex fallback | easiest correct path |
| regex `/foo.*/` | regex | `contains("foo.*", strict=true)` | Polars uses Rust regex |
| regex exact `/^foo$/` | whole regex | `contains("^foo$", strict=true)` | already anchored |
| list of literals (`foo,bar,baz`) | multi literal OR | `contains_any([...])` or `is_in` | depends on exact vs substring |
| list of exact extensions | multi exact OR | `col("ext").is_in([...])` | very fast |

---

## How to implement glob->regex fallback (safely)

If you choose to support “full glob semantics” (including `?` and `[...]`) without writing your own engine, converting to regex is the straightforward approach. The ripgrep `globset` crate does this too.  
https://docs.rs/globset/latest/globset/  
https://github.com/BurntSushi/ripgrep/blob/master/crates/globset/README.md  

Minimal anchored conversion (conceptual):
- Escape regex metacharacters in literals
- Replace:
  - `*` -> `.*`
  - `?` -> `.`
  - `[...]` -> keep as character class (but handle escaping carefully)
- Wrap with `^(?: ... )$` for whole-string match

For UFFS (paths), decide whether `*` matches path separators. Different tools differ. `globset` exposes options (e.g., how separators are treated). If you need ripgrep-like behavior, consider using `globset` semantics directly, but that likely means a custom matcher (plugin) rather than converting yourself.

---

## Multi-pattern optimization (huge win in real UIs)

If your UI supports:
- multiple include patterns
- multiple exclude patterns
- extension include/exclude lists
- “search terms” split by whitespace

Then the right answer is usually:

1) Normalize them to the same *semantic kind* (exact, contains, etc)
2) Use the right multi-pattern kernel

### For many literal substrings: `contains_any` (Aho-Corasick)
Polars exposes `contains_any` and the underlying Aho-Corasick system via the `find_many` feature.  
https://docs.pola.rs/api/rust/dev/polars_lazy/dsl/string/struct.StringNameSpace.html  
https://deepwiki.com/pola-rs/polars/6.3-string-operations  

This avoids N regex evaluations and does a single pass per string.

### For many exact matches: `is_in`
For exact matches (extensions, types, flags), `is_in` is generally the right tool.

### For many globs / many regexes: consider a plugin
For sets of globs, ripgrep’s `globset` compiles many globs and can match them efficiently.  
https://docs.rs/globset/latest/globset/  

Polars expression plugins let you register Rust-native expressions that run “almost as fast as native expressions.”  
https://docs.pola.rs/user-guide/plugins/expr_plugins/  

This is the clean way to do:
- a cached `GlobSet` / `RegexSet`
- complex matching without losing Polars parallelism

---

## Case sensitivity (Windows reality check)

If your default is case-insensitive (common for Windows file search), the fastest strategy is usually:

- store an extra column `name_lc` and/or `path_lc` in the index (ASCII lowercase is often enough for Windows paths; full Unicode casefold is more expensive),
- run case-insensitive queries against that column,
- only do “case-sensitive mode” by switching to the original column.

Aho-Corasick support in Polars explicitly has an `ascii_case_insensitive` option for multi-pattern search.  
https://docs.pola.rs/api/rust/dev/polars_lazy/dsl/string/struct.StringNameSpace.html

---

## LazyFrame, query planning, and why it matters even in-memory

Polars’ lazy API builds a query graph and optimizes it before execution. Even if you already loaded the data, building the filter as an expression (not a Rust loop) gives Polars room to reorder, combine, and parallelize operations.  
https://docs.pola.rs/user-guide/concepts/lazy-api/  
https://docs.pola.rs/user-guide/lazy/execution/  

Rule of thumb:
- if you can express it as a Polars expression, do it as a Polars expression.

Avoid per-row `map_elements` style UDFs unless you are forced to. If you are forced to, use an expression plugin.

---

## Concrete Rust/Polars skeleton

Below is a simplified sketch showing the pattern IR -> expression lowering.

```rust
use polars::prelude::*;
use polars::lazy::dsl::*;

#[derive(Debug, Clone)]
enum InputKind { Literal, Glob, Regex }

#[derive(Debug, Clone)]
enum GlobKind {
    Any,
    Exact(String),
    Prefix(String),
    Suffix(String),
    PrefixSuffix { prefix: String, suffix: String },
    Complex(String),
}

#[derive(Debug, Clone)]
enum Compiled {
    True,
    Exact(String),
    Prefix(String),
    Suffix(String),
    Contains(String),
    Regex { pattern: String, strict: bool }, // pattern is regex text
}

fn classify_glob(p: &str) -> GlobKind {
    // Minimal classifier:
    // - treat '?' '[' ']' or multiple '*' as complex
    // - special-case '*', 'foo*', '*bar', 'foo*bar'
    // (Implement carefully; this is illustrative.)
    if p == "*" { return GlobKind::Any; }

    let stars = p.matches('*').count();
    let has_other_meta = p.contains('?') || p.contains('[');

    if has_other_meta || stars > 1 {
        return GlobKind::Complex(p.to_string());
    }

    match (p.starts_with('*'), p.ends_with('*'), stars) {
        (false, false, 0) => GlobKind::Exact(p.to_string()),
        (false, true, 1)  => GlobKind::Prefix(p.trim_end_matches('*').to_string()),
        (true, false, 1)  => GlobKind::Suffix(p.trim_start_matches('*').to_string()),
        (false, false, 1) => {
            let mut it = p.splitn(2, '*');
            let prefix = it.next().unwrap_or("").to_string();
            let suffix = it.next().unwrap_or("").to_string();
            GlobKind::PrefixSuffix { prefix, suffix }
        }
        _ => GlobKind::Complex(p.to_string()),
    }
}

fn glob_to_anchored_regex(glob: &str) -> String {
    // This is just a stub. If you need full correctness with classes and escaping,
    // consider using the `globset` crate to compile globs (it converts to regex internally).
    // https://docs.rs/globset/latest/globset/
    let mut out = String::from("^");
    for ch in glob.chars() {
        match ch {
            '*' => out.push_str(".*"),
            '?' => out.push('.'),
            // escape common regex metacharacters
            '.' | '+' | '(' | ')' | '|' | '{' | '}' | '^' | '$' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out.push('$');
    out
}

fn compile_filter(input: &str, kind: InputKind, whole_string: bool) -> Compiled {
    match kind {
        InputKind::Literal => {
            if whole_string {
                Compiled::Exact(input.to_string())
            } else {
                Compiled::Contains(input.to_string())
            }
        }
        InputKind::Glob => {
            match classify_glob(input) {
                GlobKind::Any => Compiled::True,
                GlobKind::Exact(s) => Compiled::Exact(s),
                GlobKind::Prefix(s) => Compiled::Prefix(s),
                GlobKind::Suffix(s) => Compiled::Suffix(s),
                GlobKind::PrefixSuffix { prefix, suffix } => {
                    // lowering will AND these
                    // represent as regex-free primitives
                    // (alternatively keep a dedicated variant)
                    // We'll just encode as regex here for brevity.
                    let re = format!("^{}.*{}$",
                        regex::escape(&prefix),
                        regex::escape(&suffix)
                    );
                    Compiled::Regex { pattern: re, strict: true }
                }
                GlobKind::Complex(glob) => {
                    let re = glob_to_anchored_regex(&glob);
                    Compiled::Regex { pattern: re, strict: true }
                }
            }
        }
        InputKind::Regex => {
            let mut re = input.to_string();
            if whole_string && !(re.starts_with('^') && re.ends_with('$')) {
                re = format!("^(?:{})$", re);
            }
            Compiled::Regex { pattern: re, strict: true }
        }
    }
}

fn lower_to_expr(target_col: &str, compiled: Compiled) -> Expr {
    let c = col(target_col);

    match compiled {
        Compiled::True => lit(true),

        Compiled::Exact(s) => c.eq(lit(s)),

        Compiled::Prefix(s) => c.str().starts_with(lit(s)),

        Compiled::Suffix(s) => c.str().ends_with(lit(s)),

        Compiled::Contains(s) => c.str().contains_literal(lit(s)),

        Compiled::Regex { pattern, strict } => c.str().contains(lit(pattern), strict),
    }
}
```

Notes:
- The stub `glob_to_anchored_regex` is intentionally incomplete for full glob features. If you want a truly correct glob implementation, use `globset` and either:
  - precompile to regex strings (and feed those to Polars), or
  - use a Polars expression plugin and run `GlobSet::is_match` inside it.
  `globset` docs: https://docs.rs/globset/latest/globset/
- The Rust Polars DSL already has `contains_any` which is often the best choice for multi-literal OR.  
  https://docs.pola.rs/api/rust/dev/polars_lazy/dsl/string/struct.StringNameSpace.html

---

## Benchmarking plan (so you can prove it)

Do not guess; measure on your actual workload (millions of file paths).

1. Create a representative `Series` of paths/names (real data from a saved parquet index is ideal).
2. Bench these pattern classes:
   - exact: `README.md`
   - prefix: `c:\\pro*`
   - suffix: `*.rs`
   - contains: `node_modules`
   - complex glob: `src/**/test_*.rs`
   - regex: `(?i)readme\\.(md|txt)$`
   - multi-pattern: 10, 100, 1000 literals
3. Compare:
   - regex-everything approach
   - IR-lowered approach (specialized ops)
   - (optional) plugin-based globset/regex-set approach for multi-pattern

Also measure memory overhead for extra columns (like `path_lc`) and decide if it is worth it (usually yes for interactive search).

---

## Bottom line

**Best speed with least complexity:**
- Keep a semantic Pattern IR.
- Lower to specialized Polars kernels for the common cases.
- Fallback to regex for complex cases.
- Use `contains_any` / `is_in` for multi-pattern queries.
- If you outgrow built-ins (large glob sets, fuzzy matching), use Polars expression plugins.

This design lets you keep your CLI flexible while hitting the metal for the common patterns UFFS users type all day.

---

## References (primary)

- UFFS Rust rewrite repo README: https://github.com/githubrobbi/UltraFastFileSearch
- Polars Rust API: `StringNameSpace` (contains_literal / contains / contains_any / starts_with / ends_with):  
  https://docs.pola.rs/api/rust/dev/polars_lazy/dsl/string/struct.StringNameSpace.html
- Polars string operations architecture + multi-pattern Aho-Corasick notes:  
  https://deepwiki.com/pola-rs/polars/6.3-string-operations
- Polars expression plugins (Rust-native, near-native speed):  
  https://docs.pola.rs/user-guide/plugins/expr_plugins/
- Polars lazy execution model and query optimization:  
  https://docs.pola.rs/user-guide/concepts/lazy-api/  
  https://docs.pola.rs/user-guide/lazy/execution/
- Rust `regex` crate docs (literal extraction/prefilters; linear-time guarantees):  
  https://docs.rs/regex/latest/regex/  
  https://github.com/rust-lang/regex  
  https://deepwiki.com/rust-lang/regex/3.9-prefilters-and-literal-optimization
- `globset` crate (glob sets; converts to regex; perf notes):  
  https://docs.rs/globset/latest/globset/  
  https://github.com/BurntSushi/ripgrep/blob/master/crates/globset/README.md
