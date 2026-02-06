# UFFS Parity “Total Regression” (0% Path Match) — Root Cause + Concrete Fixes

**Date:** 2026-02-05  
**Version observed:** `uffs 0.2.197` (from `uffs_version.log`)  
**Affected runs:** `h_disk` (live + offline), likely all runs produced with the new output header prelude.

---

## 1) What you’re seeing

Your parity analyzer output shows a catastrophic mismatch:

- C++ rows: 20  
- Rust rows: 21  
- **Common paths: 0**  
- **Match rate: 0%**  
- All boolean flags match 0% (because the analyzer can’t even read the correct columns)

Yet it also claims “Tree metrics ✅ ALL MATCH”, which is a big hint that parsing is broken, not the tree code.

---

## 2) The real root cause

### 2.1 Rust output is no longer pure CSV: it now starts with a comment line

Your `rust_live_h.txt` begins with:

```
# TRIPWIRE: UFFS cpp_tree FIXED v0.2.197 tree_metrics_parity
"Path","Name","Path Only", ...
...
```

Your C++ output begins immediately with the CSV header:

```
"Path","Name","Path Only", ...
...
```

This one-line prelude is **enough to break any CSV reader that expects the first record to be the header**.

### 2.2 Why that produces exactly “Common paths: 0, Rust only: 1”

Most CSV parsing code in Rust looks like:

```rust
let mut rdr = csv::Reader::from_reader(file);
let headers = rdr.headers()?;                 // <-- assumes first record is the header
let path_idx = headers.iter().position(|h| h == "Path").unwrap();
for rec in rdr.records() {
    let r = rec?;
    let path = r.get(path_idx).unwrap().to_string();
    ...
}
```

If the first line is `# TRIPWIRE: ...`, then:

- `headers()` becomes **one column**: `["# TRIPWIRE: UFFS ..."]`
- `"Path"` is not found
- Many implementations then default path to `""` or `"UNKNOWN"`, or they fall back to the first column in each record incorrectly.

Result:
- every parsed row ends up with the **same “path”** (empty string or a constant)
- the analyzer sees **1 unique Rust path**, and 20 C++ paths
- **intersection = 0**

That explains:
- `Rust only: 1`
- `Common: 0`
- `C++ only: 20`

It also explains why your boolean flags show 0%: the analyzer is reading the wrong columns.

---

## 3) This is NOT a filesystem / NTFS / tree-metrics regression

If you ignore the tripwire comment line, the underlying CSV data rows are fine.

When parsed correctly, `cpp_h.txt` and `rust_live_h.txt` have the same 20 paths and differ only on the root tree metrics (the earlier issue), not on path enumeration.

So: **fix the CSV prelude / parsing first**. Otherwise you can’t trust the parity tool’s results.

---

## 4) Fix options (pick one)

### Option A (recommended): Remove the tripwire prelude from the CSV output

**Why:** You want 100% output parity vs C++ and a stable CSV format.  
**Where to put tripwire instead:** stderr, trace log, or a separate sidecar file.

#### A1) Print tripwire to stderr instead of stdout

Wherever you currently do something like:

```rust
println!("# TRIPWIRE: ...");
println!("{}", csv_header_line);
```

Change it to:

```rust
eprintln!("[TRIPWIRE] UFFS cpp_tree FIXED v0.2.197 tree_metrics_parity");
println!("{}", csv_header_line);
```

This keeps the CSV identical to C++ while still giving you a reliable tripwire in captured logs.

#### A2) Or write a sidecar metadata file
For trial runs, write:

```
docs/trial_runs/h_disk/rust_meta.txt
```

Contents:

```
TRIPWIRE=cpp_tree_fixed
VERSION=0.2.197
GIT_SHA=...
```

Then update the analyzer to read this file if present, instead of grepping random logs.

---

### Option B: Make the parity analyzer ignore comment lines

If you *insist* on keeping the tripwire prelude in the CSV file, then the analyzer must explicitly skip comment lines before reading headers.

#### B1) Easiest: `csv::ReaderBuilder` with `.comment(Some(b'#'))`

In `scripts/analyze_trial_parity.rs`, change CSV reader construction to:

```rust
let mut rdr = csv::ReaderBuilder::new()
    .comment(Some(b'#'))   // <-- ignore lines starting with '#'
    .flexible(true)        // tolerates minor row-length issues
    .from_reader(file);
```

This tells the `csv` crate to ignore `# ...` lines **including before headers**, so the `"Path",...` line becomes the real header.

#### B2) More robust: scan until the header line
This avoids surprises from other non-CSV text (your output also includes interactive prompt lines like `Drives? ...`).

Pseudo-code:

```rust
fn extract_csv_block(raw: &str) -> String {
    let lines: Vec<&str> = raw.lines().collect();

    // 1) find the real header
    let start = lines.iter().position(|l| l.starts_with("\"Path\""))
        .ok_or("missing CSV header")?;

    // 2) collect CSV rows until the first non-CSV line
    let mut out = Vec::new();
    out.push(lines[start]);

    for l in &lines[start+1..] {
        if l.starts_with('"') {
            out.push(*l);
        } else if l.trim().is_empty() {
            // optionally keep scanning (or break)
            continue;
        } else {
            break; // stop at interactive prompt
        }
    }

    Ok(out.join("\n"))
}
```

Then parse the returned CSV text normally.

This is the safest approach for your environment because both C++ and Rust outputs contain trailing interactive prompt text.

---

### Option C: Add the exact same prelude to C++ output too
This is generally the worst option because:
- it requires touching the C++ tool/output,
- it doesn’t actually improve correctness,
- and it still isn’t “pure CSV”.

I’m listing it for completeness only.

---

## 5) What to do immediately (fast checklist)

1) **Undo the tripwire comment line in the CSV file** (Option A)  
   or update analyzer to ignore comment lines (Option B).

2) Regenerate:
   - `docs/trial_runs/h_disk/rust_live_h.txt`
   - `docs/trial_runs/h_disk/rust_offline_h.txt`

3) Re-run:
   ```bash
   rust-script scripts/analyze_trial_parity.rs docs/trial_runs/h_disk
   ```

Expected result:
- path match rate returns to **100%**
- boolean flags return to **100%**
- you again see the real remaining mismatch (likely just root metrics in live).

---

## 6) After you fix parsing: confirm the *real* remaining issue

After stripping the comment line, the actual data comparison for `h_disk` shows:

- Only `H:\` differs in live mode:
  - `Size`: C++ `42,168,722` vs Rust live `31,065,729,314`
  - `Desc`: C++ `57` vs Rust live `54`

Offline matches perfectly.

That means: your older “live root row wiring” issue still exists, and should be addressed separately.
(But don’t try to do that until the analyzer is trustworthy again.)

---

## 7) Notes about expired uploads

Some earlier uploaded files from prior turns are no longer accessible in this session.  
If you want me to cross-reference an older run/log/code snapshot that isn’t in your latest uploads, please re-upload it.

---

## Appendix: Evidence from current artifacts

- `uffs_version.log` contains: `uffs 0.2.197`
- `rust_live_h.txt` and `rust_offline_h.txt` both begin with: `# TRIPWIRE: ...`
- `cpp_h.txt` begins directly with the CSV header line (`"Path","Name",...`)

