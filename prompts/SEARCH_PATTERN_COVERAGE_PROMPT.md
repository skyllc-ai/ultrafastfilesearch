# Prompt: UFFS Search Pattern Pipeline — Comprehensive Test Coverage

## Objective
Achieve **≥90% branch/line coverage** for the **search pattern matching pipeline** documented in `docs/architecture/SEARCH_PATTERN_DEEP_DIVE.md`. Focus on the three-stage flow:

```
User Input → ParsedPattern (parse.rs) → IndexPattern (pattern.rs) → Matching (filtering.rs)
```

## Target Files (uffs-core crate)

| File | Role | Priority |
|------|------|----------|
| `pattern/parse.rs` | `ParsedPattern::parse()` — input classification, path detection | 🔴 Critical |
| `compiled_pattern/glob.rs` | `classify_glob()` — glob → `GlobKind` | 🔴 Critical |
| `index_search/pattern.rs` | `compile_index_pattern()` — `GlobKind` → `IndexPattern` | 🔴 Critical |
| `index_search/query/filtering.rs` | `StreamingRecordFilter::matches()` — inline filtering | 🟡 High |
| `extensions/mod.rs` | `extract_trailing_extension()` — smart decomposer | 🟡 High |

## Hard Constraints
- **Rust workspace** with strict Clippy (`unwrap_used`, `expect_used` = deny in prod, allowed in tests)
- Use `cargo nextest run -p uffs-core` for test execution
- Tests go in existing `**/tests.rs` files or `#[cfg(test)] mod tests` blocks
- **No coverage tooling** — use branch matrix + assertion verification
- Preserve existing public API behavior

## Pattern Classification Matrix (from DEEP_DIVE.md)

### Stage 1: ParsedPattern Parsing

| Input | Expected Drive | Expected Pattern | Expected PatternType | is_path_pattern |
|-------|----------------|------------------|---------------------|-----------------|
| `*.rs` | None | `*.rs` | Glob | false |
| `c:/pro*` | Some(C) | `/pro*` | Glob | false |
| `C:\Windows\*` | Some(C) | `\Windows\*` | Glob | **true** |
| `>C:\\TemP.*\.txt` | Some(C) | `C:\\TemP.*\.txt` | Regex | **true** |
| `nice` | None | `nice` | Literal | **true** (literals always path-aware) |
| `*hallo*.txt` | None | `*hallo*.txt` | Glob | false |
| `**\Users\**\AppData\**` | None | `**\Users\**\AppData\**` | Glob | **true** |
| `/Users/foo/bar` | None | `\Users\foo\bar` | Literal | **true** (normalized) |

### Stage 2: GlobKind Classification

| Pattern | star_count | Expected GlobKind |
|---------|------------|-------------------|
| `*` | 1 | Any |
| `*.rs` | 1 | Extension("rs") |
| `foo*` | 1 | Prefix("foo") |
| `*bar` | 1 | Suffix("bar") |
| `*needle*` | 2 | Contains("needle") |
| `*hallo*.txt` | 2 | Complex (not `*x*` form) |
| `**\Users\**` | 4 | Complex |
| `file?.txt` | 0 | Complex (has `?`) |

### Stage 3: IndexPattern Compilation

| GlobKind | Expected IndexPattern | Match Strategy |
|----------|----------------------|----------------|
| Any | `Any` | Always true |
| Extension("rs") | `Suffix { suffix: ".rs" }` | `ends_with` |
| Prefix("foo") | `Prefix { prefix: "foo" }` | `starts_with` |
| Suffix("bar") | `Suffix { suffix: "bar" }` | `ends_with` |
| Contains("needle") | `Contains { needle }` | substring |
| Complex | `Regex { regex }` | `is_match` |
| Exact("nice") | `Exact { value: "nice" }` | `eq_ignore_ascii_case` |

### Smart Pattern Decomposer (extract_trailing_extension)

| Input Pattern | Extracted Extension | Uses Ext Index? |
|---------------|---------------------|-----------------|
| `*.txt` | Some("txt") | ✅ Yes |
| `*hallo*.txt` | Some("txt") | ✅ Yes |
| `foo*.rs` | Some("rs") | ✅ Yes |
| `>.*\.log` | Some("log") | ✅ Yes |
| `*hallo*` | None | ❌ Full scan |
| `\Windows\*.exe` | Some("exe") | ✅ Yes (path + ext) |

## Branch Matrix Template

```
File: crates/uffs-core/src/pattern/parse.rs
Function: ParsedPattern::parse()

| ID | Input | Branch | Expected Outcome | Test Name |
|----|-------|--------|------------------|-----------|
| P1 | `>regex` | starts_with('>') = true | PatternType::Regex | test_regex_prefix |
| P2 | `*.rs` | has_glob_chars = true | PatternType::Glob | test_glob_detection |
| P3 | `nice` | no special chars | PatternType::Literal | test_literal_fallback |
| P4 | `c:/foo` | drive_prefix = Some | drive extracted | test_drive_extraction_slash |
| P5 | `C:\foo` | drive_prefix = Some | drive extracted | test_drive_extraction_backslash |
| P6 | `\Users\*` | contains `\` | is_path_pattern = true | test_path_separator_detection |
| P7 | `/Users/*` | contains `/` | is_path_pattern = true, normalized | test_forward_slash_normalization |
| P8 | `nice` (literal) | always | is_path_pattern = true | test_literal_always_path_aware |
```

## Test Design Guidelines

1. **Parameterized tests** — Use `#[rstest]` or manual iteration over test cases
2. **Golden outputs** — Pattern → expected classification chain
3. **Boundary conditions** — Empty pattern, single char, max length
4. **Case sensitivity** — `Nice` vs `nice` with `--case` / `--smart-case`
5. **Error paths** — Invalid regex syntax, malformed patterns
6. **Integration paths** — Full flow from CLI input to match result

## Deliverables

1. **Branch Matrix** per target file (table format)
2. **Test implementations** in existing test modules
3. **Coverage estimate** per file: branches covered / total, lines covered / total
4. **Notes** on any unreachable branches or intentional gaps

## Execution

```bash
# Run all uffs-core tests
cargo nextest run -p uffs-core

# Run specific test module
cargo nextest run -p uffs-core -- pattern

# Run with output
cargo test -p uffs-core -- --nocapture
```

After each file's tests, update the coverage estimate and proceed to the next file.

---

## Appendix A: Record Filter Branch Matrix

File: `crates/uffs-core/src/index_search/query/filtering.rs`
Function: `StreamingRecordFilter::matches()`

| ID | Filter Config | Record State | Expected Result | Test Name |
|----|--------------|--------------|-----------------|-----------|
| F1 | files_only=true | is_dir=true | false (skip) | test_files_only_skips_dirs |
| F2 | files_only=true | is_dir=false | true (pass) | test_files_only_passes_files |
| F3 | dirs_only=true | is_dir=false | false (skip) | test_dirs_only_skips_files |
| F4 | dirs_only=true | is_dir=true | true (pass) | test_dirs_only_passes_dirs |
| F5 | hide_system=true | flags=SYSTEM | false (skip) | test_hide_system |
| F6 | hide_system=true | flags=HIDDEN | false (skip) | test_hide_hidden |
| F7 | min_size=1024 | size=512 | false (skip) | test_min_size_filter |
| F8 | min_size=1024 | size=2048 | true (pass) | test_min_size_passes |
| F9 | max_size=1024 | size=2048 | false (skip) | test_max_size_filter |
| F10 | attr_include=COMPRESSED | flags=COMPRESSED | true (pass) | test_attr_include |
| F11 | attr_exclude=HIDDEN | flags=HIDDEN | false (skip) | test_attr_exclude |
| F12 | newer_than=7d | mtime=3d_ago | true (pass) | test_newer_than_passes |
| F13 | newer_than=7d | mtime=10d_ago | false (skip) | test_newer_than_fails |

## Appendix B: Case Sensitivity Branch Matrix

| ID | Pattern | Flag | Input | Expected Match | Test Name |
|----|---------|------|-------|----------------|-----------|
| C1 | `nice` | default | `Nice` | true | test_case_insensitive_default |
| C2 | `nice` | default | `NICE` | true | test_case_insensitive_upper |
| C3 | `nice` | --case | `Nice` | false | test_case_sensitive_mismatch |
| C4 | `nice` | --case | `nice` | true | test_case_sensitive_exact |
| C5 | `DuaLipa` | --smart-case | `DuaLipa` | true | test_smart_case_uppercase_exact |
| C6 | `DuaLipa` | --smart-case | `dualipa` | false | test_smart_case_uppercase_strict |
| C7 | `dualipa` | --smart-case | `DuaLipa` | true | test_smart_case_lowercase_insensitive |

## Appendix C: Path-Aware Matching Matrix

| ID | Pattern | Match Against | Input Path | Expected | Test Name |
|----|---------|---------------|------------|----------|-----------|
| PA1 | `\Users\*` | full path | `C:\Users\john` | true | test_path_pattern_matches_path |
| PA2 | `\Users\*` | filename only | `john` | false | test_path_pattern_needs_path |
| PA3 | `nice` | full path | `C:\nice_project\file.txt` | true | test_literal_matches_dir |
| PA4 | `*.txt` | filename | `readme.txt` | true | test_glob_matches_filename |
| PA5 | `*.txt` | filename | `C:\docs\readme.txt` | true (ext match) | test_glob_ignores_path |
| PA6 | `**/Users/**` | full path | `D:\Users\john\Desktop` | true | test_doublestar_glob |

## Appendix D: OR Pattern (Pipe) Matrix

| ID | Pattern | Input | Expected | Test Name |
|----|---------|-------|----------|-----------|
| OR1 | `*.txt\|*.log` | `file.txt` | true | test_or_first_match |
| OR2 | `*.txt\|*.log` | `file.log` | true | test_or_second_match |
| OR3 | `*.txt\|*.log` | `file.rs` | false | test_or_no_match |
| OR4 | `foo*\|*bar` | `foobar` | true | test_or_both_match |
| OR5 | `nice\|cool\|awesome` | `cool` | true | test_or_multi |

## Appendix E: Error Handling Matrix

| ID | Input | Expected Error | Test Name |
|----|-------|----------------|-----------|
| E1 | `>[invalid(regex` | Regex syntax error | test_invalid_regex_syntax |
| E2 | `""` (empty) | Empty pattern error or match-all | test_empty_pattern |
| E3 | `[unclosed` | Glob bracket error | test_unclosed_bracket |

---

## Summary Checklist

- [ ] `pattern/parse.rs` — ParsedPattern::parse() branches covered
- [ ] `compiled_pattern/glob.rs` — classify_glob() all GlobKind variants
- [ ] `index_search/pattern.rs` — compile_index_pattern() all IndexPattern variants
- [ ] `index_search/query/filtering.rs` — StreamingRecordFilter all filter combinations
- [ ] `extensions/mod.rs` — extract_trailing_extension() edge cases
- [ ] Case sensitivity (default, --case, --smart-case)
- [ ] Path-aware vs filename-only matching
- [ ] OR patterns (pipe separator)
- [ ] Error handling (invalid patterns)

**Target: ≥90% branch and line coverage per file, verified via branch matrix assertions.**

