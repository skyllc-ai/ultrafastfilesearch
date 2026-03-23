# Prompt: UFFS Search & Filter Pipeline — Comprehensive Test Coverage

## Objective

Achieve **≥90% branch/line coverage** for the **search pattern matching and record filtering pipeline** across the `uffs-core` and `uffs-cli` crates. The pipeline has four stages:

```
User Input → ParsedPattern → IndexPattern → RecordFilter → SearchResult
              (uffs-core)     (uffs-core)    (uffs-core     (uffs-core)
                                              + uffs-cli)
```

## Target Files

### Tier 1 — Pattern Parsing & Compilation (`uffs-core`)

| File | Key Functions | Priority |
|------|---------------|----------|
| `pattern/parse.rs` | `ParsedPattern::parse()` — input classification, drive extraction, path detection | 🔴 Critical |
| `compiled_pattern/glob.rs` | `classify_glob()` — glob → `GlobKind` | 🔴 Critical |
| `compiled_pattern/mod.rs` | `compile_pattern()` — `GlobKind` → `CompiledPattern` | 🔴 Critical |
| `index_search/pattern.rs` | `compile_index_pattern()`, `compile_parsed_pattern()` — `ParsedPattern` → `IndexPattern`, `IndexPattern::matches()` | 🔴 Critical |

### Tier 2 — Query Execution & Filtering (`uffs-core`)

| File | Key Functions | Priority |
|------|---------------|----------|
| `index_search/query/filtering.rs` | `RecordFilter::matches()` — type, size, pattern filters | 🟡 High |
| `index_search/query/planning.rs` | `CollectPlan::build()` — extension index fast path | 🟡 High |
| `index_search/query/execution.rs` | `IndexQuery::collect()`, `IndexQuery::count()` | 🟡 High |
| `index_search/query/builder.rs` | Fluent builder methods | 🟢 Medium |
| `index_search/query/expansion.rs` | Hard link + ADS expansion | 🟡 High |
| `index_search/routing.rs` | `QueryMode`, `QueryComplexity`, `analyze_pattern_complexity()` | 🟢 Medium |
| `index_search/result.rs` | `SearchResult::from_record()` | 🟢 Medium |

### Tier 3 — Extension Index (`uffs-core`)

| File | Key Functions | Priority |
|------|---------------|----------|
| `extensions/mod.rs` | `ExtensionFilter`, `ExtensionIndex`, `extract_trailing_extension()` | 🟡 High |

### Tier 4 — CLI Streaming Filter (`uffs-cli`)

| File | Key Functions | Priority |
|------|---------------|----------|
| `commands/output.rs` | `StreamingRecordFilter` — files_only, dirs_only, hide_system, attr_filters, newer/older, min/max size | 🟡 High |
| `commands/search/mod.rs` | `build_record_filter()`, `write_streaming_output_with_filter()` | 🟢 Medium |

### Existing Test Files (add tests here)

| Test File | Covers |
|-----------|--------|
| `uffs-core/src/compiled_pattern/tests.rs` | GlobKind classification, pattern compilation |
| `uffs-core/src/extensions/tests.rs` | Extension interning and filtering |
| `uffs-core/src/index_search/tests.rs` | IndexQuery end-to-end, SearchResult, hard link/ADS expansion |
| `uffs-core/src/output/tests.rs` | Output formatting |
| `uffs-core/src/query/tests.rs` | Polars MftQuery path |
| `uffs-cli/src/commands/output_tests.rs` | StreamingRecordFilter matching |

## Hard Constraints

- **Rust workspace** with strict Clippy: `unwrap_used` and `expect_used` = deny in prod, allowed in tests via `#[expect]`
- Use `cargo nextest run -p uffs-core` and `cargo nextest run -p uffs-cli` for test execution
- Tests go in existing `**/tests.rs` files listed above
- **No coverage tooling** — use branch matrix + assertion verification
- Preserve existing public API behavior

---

## Pattern Classification Matrix

### Stage 1: ParsedPattern Parsing

File: `crates/uffs-core/src/pattern/parse.rs`

| Input | Drive | Pattern | PatternType | is_path_pattern |
|-------|-------|---------|-------------|-----------------|
| `*.rs` | None | `*.rs` | Glob | false |
| `c:/pro*` | Some('C') | `/pro*` | Glob | true |
| `C:\Windows\*` | Some('C') | `\Windows\*` | Glob | true |
| `>C:\\TemP.*\.txt` | None | `C:\\TemP.*\.txt` | Regex | true |
| `nice` | None | `nice` | Literal | false |
| `*hallo*.txt` | None | `*hallo*.txt` | Glob | false |
| `**\Users\**\AppData\**` | None | `**\Users\**\AppData\**` | Glob | true |
| `/Users/foo/bar` | None | `/Users/foo/bar` | Literal | true |
| `*.txt\|*.log` | None | `*.txt\|*.log` | Glob | false |
| (empty) | None | `` | Glob (match-all) | false |

### Stage 2: GlobKind Classification

File: `crates/uffs-core/src/compiled_pattern/glob.rs`

| Pattern | Expected GlobKind |
|---------|-------------------|
| `*` | `Any` |
| `*.rs` | `Extension("rs")` |
| `foo*` | `Prefix("foo")` |
| `*bar` | `Suffix("bar")` |
| `*needle*` | `Contains("needle")` |
| `foo*bar` | `PrefixSuffix("foo", "bar")` |
| `*hallo*.txt` | `Complex` |
| `**\Users\**` | `Complex` |
| `file?.txt` | `Complex` (has `?`) |
| `[abc]*` | `Complex` (character class) |
| `readme.txt` | `Exact("readme.txt")` |

### Stage 3: IndexPattern Compilation

File: `crates/uffs-core/src/index_search/pattern.rs`

| Input | Expected IndexPattern Variant | Match Strategy |
|-------|-------------------------------|----------------|
| `*` | `Any` | Always true |
| `*.rs` | `Suffix { suffix: ".rs" }` | `ends_with` |
| `foo*` | `Prefix { prefix: "foo" }` | `starts_with` |
| `*bar` | `Suffix { suffix: "bar" }` | `ends_with` |
| `*needle*` | `Contains { needle: "needle" }` | Substring search |
| `foo*bar` | `PrefixSuffix { prefix, suffix }` | Both ends |
| `nice` (literal) | `Contains { needle: "nice" }` | Substring (literals → contains) |
| `>.*\.txt$` | `Regex { regex }` | `is_match` |
| `*.txt\|*.log` | `Or { patterns: [Suffix, Suffix] }` | Any sub-pattern |
| Complex glob | `Regex { regex }` | Compiled regex fallback |

### Extension Index Pre-Filter

File: `crates/uffs-core/src/extensions/mod.rs`

| Input Pattern | Extracted Extension | Uses Ext Index? |
|---------------|---------------------|-----------------|
| `*.txt` | Some("txt") | ✅ O(matches) |
| `*hallo*.txt` | Some("txt") | ✅ Pre-filter then pattern |
| `foo*.rs` | Some("rs") | ✅ Pre-filter then pattern |
| `*hallo*` | None | ❌ Full scan |
| `*.txt\|*.log` | None (OR pattern) | ❌ Full scan |

---

## Branch Matrix: RecordFilter (`uffs-core`)

File: `crates/uffs-core/src/index_search/query/filtering.rs`
Struct: `RecordFilter`

| ID | Filter Config | Record State | Expected | Test Name |
|----|--------------|--------------|----------|-----------|
| RF1 | type_filter=FilesOnly | is_dir=true | false | test_files_only_skips_dirs |
| RF2 | type_filter=FilesOnly | is_dir=false | true | test_files_only_passes_files |
| RF3 | type_filter=DirsOnly | is_dir=false | false | test_dirs_only_skips_files |
| RF4 | type_filter=DirsOnly | is_dir=true | true | test_dirs_only_passes_dirs |
| RF5 | type_filter=All | any | true | test_all_passes_everything |
| RF6 | min_size=1024 | size=512 | false | test_min_size_filter |
| RF7 | min_size=1024 | size=2048 | true | test_min_size_passes |
| RF8 | max_size=1024 | size=2048 | false | test_max_size_filter |
| RF9 | max_size=1024 | size=512 | true | test_max_size_passes |
| RF10 | pattern=Suffix(".rs") | name="foo.rs" | true | test_pattern_suffix_match |
| RF11 | pattern=Suffix(".rs") | name="foo.txt" | false | test_pattern_suffix_mismatch |
| RF12 | pattern=Contains("hello") | name="hello_world.txt" | true | test_pattern_contains_match |
| RF13 | all filters combined | passes all | true | test_combined_filters_pass |
| RF14 | all filters combined | fails one | false | test_combined_filters_fail_early |

## Branch Matrix: StreamingRecordFilter (`uffs-cli`)

File: `crates/uffs-cli/src/commands/output.rs`
Struct: `StreamingRecordFilter`

| ID | Filter Config | Record State | Expected | Test Name |
|----|--------------|--------------|----------|-----------|
| SF1 | hide_system=true | flags has SYSTEM | false | test_hide_system_files |
| SF2 | hide_system=true | name starts with "$" | false | test_hide_dollar_prefix |
| SF3 | attr_require=COMPRESSED | flags has COMPRESSED | true | test_attr_require_match |
| SF4 | attr_require=COMPRESSED | flags no COMPRESSED | false | test_attr_require_miss |
| SF5 | attr_exclude=HIDDEN | flags has HIDDEN | false | test_attr_exclude_match |
| SF6 | attr_exclude=HIDDEN | flags no HIDDEN | true | test_attr_exclude_miss |
| SF7 | newer_than=timestamp | modified > threshold | true | test_newer_than_passes |
| SF8 | newer_than=timestamp | modified < threshold | false | test_newer_than_fails |
| SF9 | older_than=timestamp | modified < threshold | true | test_older_than_passes |
| SF10 | older_than=timestamp | modified > threshold | false | test_older_than_fails |

## Branch Matrix: Case Sensitivity

File: `crates/uffs-core/src/index_search/pattern.rs` — `IndexPattern::matches()`

| ID | Pattern | case_sensitive | Input | Expected | Test Name |
|----|---------|---------------|-------|----------|-----------|
| C1 | `nice` (Contains) | false | `Nice` | true | test_case_insensitive_default |
| C2 | `nice` (Contains) | false | `NICE` | true | test_case_insensitive_upper |
| C3 | `nice` (Contains) | true | `Nice` | false | test_case_sensitive_mismatch |
| C4 | `nice` (Contains) | true | `nice` | true | test_case_sensitive_exact |
| C5 | `*.RS` (Suffix) | false | `foo.rs` | true | test_suffix_case_insensitive |
| C6 | `*.RS` (Suffix) | true | `foo.rs` | false | test_suffix_case_sensitive |
| C7 | `FOO*` (Prefix) | false | `foobar` | true | test_prefix_case_insensitive |
| C8 | `FOO*BAR` (PrefixSuffix) | false | `fooXbar` | true | test_prefixsuffix_case_insensitive |

## Branch Matrix: OR Patterns (Pipe Separator)

| ID | Pattern | Input | Expected | Test Name |
|----|---------|-------|----------|-----------|
| OR1 | `*.txt\|*.log` | `file.txt` | true | test_or_first_match |
| OR2 | `*.txt\|*.log` | `file.log` | true | test_or_second_match |
| OR3 | `*.txt\|*.log` | `file.rs` | false | test_or_no_match |
| OR4 | `foo*\|*bar` | `foobar` | true | test_or_both_match |
| OR5 | `nice\|cool\|awesome` | `cool` | true | test_or_multi_literal |

## Branch Matrix: Query Routing

File: `crates/uffs-core/src/index_search/routing.rs`

| ID | Input | Expected | Test Name |
|----|-------|----------|-----------|
| QR1 | `QueryMode::from_str_opt("auto")` | Some(Auto) | test_mode_parse_auto |
| QR2 | `QueryMode::from_str_opt("index")` | Some(ForceIndex) | test_mode_parse_index |
| QR3 | `QueryMode::from_str_opt("dataframe")` | Some(ForceDataFrame) | test_mode_parse_df |
| QR4 | `QueryMode::from_str_opt("nonsense")` | None | test_mode_parse_invalid |
| QR5 | Simple pattern, no sort | QueryComplexity::Simple | test_complexity_simple |
| QR6 | Needs aggregation | QueryComplexity::Complex | test_complexity_complex |

## Branch Matrix: Error Handling

| ID | Input | Expected Error | Test Name |
|----|-------|----------------|-----------|
| E1 | `>[invalid(regex` | CoreError (regex syntax) | test_invalid_regex_syntax |
| E2 | `""` (empty string) | Match-all (`Any`) | test_empty_pattern_is_match_all |
| E3 | `[unclosed` | CoreError (glob syntax) | test_unclosed_bracket |

## Branch Matrix: IndexQuery End-to-End

File: `crates/uffs-core/src/index_search/tests.rs` (existing fixture: `build_index_query_fixture`)

| ID | Query | Expected Results | Test Name |
|----|-------|-----------------|-----------|
| IQ1 | `Any` pattern, no filters | All records | test_query_any_returns_all |
| IQ2 | `*.txt` pattern | Only .txt files | test_query_extension_filter |
| IQ3 | FilesOnly filter | Skip directories | test_query_files_only |
| IQ4 | DirsOnly filter | Skip files | test_query_dirs_only |
| IQ5 | min_size=100 | Skip small files | test_query_min_size |
| IQ6 | limit=1 | At most 1 result | test_query_limit |
| IQ7 | expand_names=true | Hard links expanded | test_query_expand_hardlinks |
| IQ8 | expand_streams=true | ADS expanded | test_query_expand_ads |
| IQ9 | expand_names=false | No hard link rows | test_query_no_expand_hardlinks |
| IQ10 | include_system_metafiles=false | Skip FRS < 16 | test_query_skip_system_metafiles |

---

## Test Design Guidelines

1. **Parameterized tests** — Manual iteration over test case vectors (no external dep needed)
2. **Golden outputs** — Pattern → expected classification → expected match/no-match chain
3. **Boundary conditions** — Empty pattern, single char, very long names
4. **Case sensitivity** — Test all three modes: default (insensitive), `--case`, `--smart-case`
5. **Error paths** — Invalid regex, unclosed brackets, malformed drive prefixes
6. **Integration paths** — Full `IndexQuery::collect()` against the test fixture
7. **Extension index** — Test `CollectPlan::build()` with and without extension index present
8. **Zero allocations** — Verify `IndexPattern::matches()` doesn't allocate (benchmark or doc-test)

## Execution

```bash
# Run all uffs-core tests
cargo nextest run -p uffs-core

# Run specific test module
cargo nextest run -p uffs-core -- pattern
cargo nextest run -p uffs-core -- index_search
cargo nextest run -p uffs-core -- compiled_pattern
cargo nextest run -p uffs-core -- extensions

# Run CLI filter tests
cargo nextest run -p uffs-cli -- output_tests

# Run with stdout
cargo test -p uffs-core -- --nocapture
```

## Deliverables

1. **Branch Matrix** per target file (table format, as above)
2. **Test implementations** in existing test modules (listed in "Existing Test Files")
3. **Coverage estimate** per file: branches covered / total, lines covered / total
4. **Notes** on any unreachable branches or intentional gaps

---

## Summary Checklist

- [ ] `pattern/parse.rs` — `ParsedPattern::parse()` all branches (drive, type, path detection)
- [ ] `compiled_pattern/glob.rs` — `classify_glob()` all `GlobKind` variants
- [ ] `compiled_pattern/mod.rs` — `compile_pattern()` all pattern types
- [ ] `index_search/pattern.rs` — `compile_index_pattern()` all `IndexPattern` variants + `matches()` for each
- [ ] `index_search/query/filtering.rs` — `RecordFilter::matches()` all filter combinations
- [ ] `index_search/query/planning.rs` — Extension index fast path vs full scan
- [ ] `index_search/query/execution.rs` — `collect()` and `count()` end-to-end
- [ ] `index_search/query/expansion.rs` — Hard link + ADS expansion
- [ ] `index_search/routing.rs` — `QueryMode` parsing, complexity analysis
- [ ] `extensions/mod.rs` — `extract_trailing_extension()` edge cases
- [ ] `commands/output.rs` — `StreamingRecordFilter` all filter branches (CLI crate)
- [ ] Case sensitivity — default, `--case`, `--smart-case` across all pattern types
- [ ] OR patterns — pipe separator with 2+ sub-patterns
- [ ] Error handling — invalid regex, empty pattern, unclosed brackets

**Target: ≥90% branch and line coverage per file, verified via branch matrix assertions.**

