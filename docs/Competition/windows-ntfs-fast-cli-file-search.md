# High‑Performance CLI File Search on NTFS (Windows)
*Top 5 fastest options (practical), performance notes, and “how to use” reference — January 2026*

> Goal: **find files by name/path extremely fast on Windows/NTFS**, in a way that feels like Linux `find` / `locate`, but scriptable from CMD/PowerShell.

---

## 0) Executive summary (what to shortlist)
If you can run **one background indexer service**, you almost always want:

1) **Everything + `es.exe`** — fastest “type-and-get-results” experience, full-path output, rich query language, and easy scripting. Everything builds its NTFS index by reading the **MFT** and keeps it current via the **USN Journal**.
2) **Windows Search Index (PowerShell via OLE DB)** — very fast *if* the locations you care about are indexed; excellent in enterprise environments where the Windows Search service is already managed.

If you **can’t** rely on an index (or want a `find`-style walk), shortlist:

3) **`fd`** (sharkdp) — among the fastest cross-platform “scan the tree” tools, with parallel traversal and clean defaults.

If you want “locate-style” queries with periodic refresh:

4) **GNU `locate` + `updatedb`** (via MSYS2/Cygwin) — query is fast; freshness depends on how often you rebuild the database.

If you want “fast enumeration” without a long-lived index (niche but powerful):

5) **UltraSearch** (JAM Software) — can run searches from the command line and even in the background; geared more like a GUI tool with CLI switches, but can be automated.

> Honorable mentions are at the end (including a very fast MFT-based open-source CLI with a **non‑commercial** license).

---

## 1) Why some tools are *much* faster on NTFS
There are three fundamentally different strategies:

### A. **Index-first (persistent DB)**
- Build an index once; queries are near-instant.
- Examples: **Everything**, **Windows Search**, **GNU locate**.

### B. **MFT/USN enumeration (metadata-first)**
- Leverage NTFS metadata structures to enumerate names quickly (often faster than recursive API traversal).
- Some tools keep a persistent DB (Everything); others do a “load & scan” each run.

### C. **Traditional directory walk (scan-first)**
- Walk directories using Win32 APIs; performance depends heavily on depth, disk type, AV hooks, network shares, etc.
- Examples: **fd**, **where /r**, PowerShell `Get-ChildItem`.

---

## 2) Performance data points (what we can cite)
### Everything (index build + footprint)
Voidtools’ FAQ provides ballpark numbers:
- ~**120,000 files**: about **1 second** to index
- **1,000,000 files**: about **1 minute** to index
- **1,000,000 files**: about **75 MB RAM** and **45 MB disk** for the index
- Everything keeps NTFS indexes up-to-date via the **USN Journal**, so changes aren’t missed when Everything isn’t running (the system maintains the USN journal).
Sources: <https://mail.voidtools.com/en-us/faq/>

### fd (scan-first) vs GNU find (scan-first)
The `fd` project reports a benchmark where **fd is ~23× faster** than `find -iregex` and **~13× faster** than `find -iname` for a particular workload (both found the same 546 files).
Source: <https://github.com/sharkdp/fd/blob/master/README.md>

### A third-party benchmark comparing MFT-based tools
The “Ultra Fast File Search” (UFFS) project includes a benchmark table (author-run) showing **load & sort times** on ~19M records:
- UFFS: **121s** (19M records, all disks)
- Everything: **178s** (19M records, all disks)
- WizFile: **299s** (6.5M records, 1 hard drive)
Source: <https://github.com/githubrobbi/Ultra-Fast-File-Search>
⚠️ This is a single author’s benchmark; treat it as **one data point**, not a neutral lab result.

### UltraSearch CLI automation
UltraSearch supports command line parameters, including **/NOGUI**, **/CLIPBOARD**, and **/CLOSE**, enabling background operation and scripting-style flows.
Source: <https://manuals.jam-software.com/ultrasearch/EN/commandline.html>

### Windows Search programmatic querying
Microsoft documents querying Windows Search via **SQL and AQS**, including use via the **Windows Search OLE DB provider** and AQS reference docs.
Sources:
- <https://learn.microsoft.com/en-us/windows/win32/search/using-sql-and-aqs-to-query-the-index>
- <https://learn.microsoft.com/en-us/windows/win32/lwef/-search-2x-wds-aqsreference>
- Microsoft scripting example using **Search.CollatorDSO** provider: <https://devblogs.microsoft.com/scripting/hey-scripting-guy-weekend-scripter-querying-the-windows-search-index/>

### GNU locate / updatedb docs
- `locate` options and semantics: <https://www.gnu.org/software/findutils/locate>
- `updatedb` creates/updates the locate database: <https://www.gnu.org/software/findutils/updatedb>
- MSYS2 `findutils` package includes `find.exe`, `locate.exe`, and `updatedb`: <https://packages.msys2.org/package/findutils?repo=msys&variant=x86_64>

---

## 3) The Top 5 fastest tools (practical ranking)
> “Fastest” depends on whether you allow a background indexer and what you consider “complete” (local volumes only vs network shares vs removable drives).

| Rank | Tool | Strategy | Best at | What “fast” looks like | Primary caveat |
|---:|---|---|---|---|---|
| 1 | **Everything + `es.exe`** | Persistent MFT-based index + USN updates | Instant filename/path search across NTFS volumes | Query is against an in-RAM DB; results stream immediately | Needs Everything installed/running (or service); network shares require folder indexing |
| 2 | **Windows Search index (PowerShell)** | Persistent content+metadata index | Enterprise-managed search across selected locations | SQL/AQS queries return quickly when the scope is indexed | Only finds what is indexed; tuning Indexing Options matters |
| 3 | **`fd`** | Parallel directory traversal | Fast “`find`-like” scans without an index | Multi-thread traversal; often dramatically faster than `find`-style scans | Still has to walk the tree; network shares can be slow |
| 4 | **GNU `locate` + `updatedb`** | Periodic DB build + instant query | `locate`-style “search by name” queries | Lookup is quick; DB can be compressed for speed/size | DB is stale between updates; initial build is a full scan |
| 5 | **UltraSearch** | NTFS metadata-based search + GUI/CLI hybrid | Quick local search with command-line automation hooks | Can run without GUI and copy results to clipboard | Output is not native stdout-first; product/licensing is commercial for many orgs |

---

# 4) Tool-by-tool: installation + CLI usage
## 4.1 Everything + ES (`es.exe`) — **best overall**
### How it works (why it’s fast)
- Everything reads the NTFS **Master File Table (MFT)** to index filenames quickly, and uses the NTFS **USN Journal** to keep the index current.
  Sources: voidtools forum explanation <https://www.voidtools.com/forum/viewtopic.php?t=12371> and voidtools DB notes <https://www.voidtools.com/support/everything/db/>
- Everything’s FAQ provides practical index-time and RAM numbers (see §2).
  Source: <https://mail.voidtools.com/en-us/faq/>

### Install & deployment notes
- **Install Everything (64-bit)** and consider enabling the **Everything service** if you want NTFS indexing without running the UI as admin (and to avoid UAC prompts).
  The FAQ notes that “NTFS indexing requires the Everything service or running Everything as administrator.”
  Source: <https://mail.voidtools.com/en-us/faq/>
- Download **ES (Everything Command Line Interface)** from voidtools and place `es.exe` somewhere on `PATH`.

### ES basics
ES requires Everything to be installed and running.
Source: <https://www.voidtools.com/support/everything/command_line_interface/>

**General syntax**
```bat
es.exe [options] [search text]
```

**Common options (high value)**
From the official ES CLI docs:
- Regex search: `-r` / `-regex`
- Case sensitive: `-i` / `-case`
- Whole word(s): `-w` / `-whole-word`, `-ww` / `-whole-words`
- Match full path: `-p` / `-match-path`
- Limit output rows: `-n <num>` / `-max-results <num>`
- Offset: `-o <offset>` / `-offset <offset>`
- Columns: `-name`, `-path-column`, `-size`, `-dm` (date modified), etc.
- Sort: `-sort size`, `-sort dm`, …
- Output formats: `-csv`, `-txt`, `-efu`, plus `-export-csv <file>` etc.
Source: <https://www.voidtools.com/support/everything/command_line_interface/>

### Search syntax (query language)
ES uses the **Everything search syntax**.
Sources:
- ES docs: <https://www.voidtools.com/support/everything/command_line_interface/>
- Searching primer: <https://www.voidtools.com/support/everything/searching/>

You’ll use operators like:
- `ext:pdf` (extension)
- `path:C:\Windows\` or `C:\Windows\` (location scoping)
- Wildcards like `*.log`

### Practical examples
**Find by name (anywhere)**
```bat
es.exe readme.txt
```

**Find all `.log` files under `C:\Temp`**
```bat
es.exe -p "C:\Temp\" ext:log
```

**Regex match**
```bat
es.exe -regex ".*\\(error|fail).*\.log$"
```

**Top 20 largest `.zip` files on D:**
```bat
es.exe "D:\" ext:zip -size -sort size -n 20
```

**Export results to CSV**
```bat
es.exe "*.dll" -path-column -size -dm -export-csv "C:\out\dlls.csv"
```

### Operational gotchas
- If ES can’t connect, you’ll see an error like “Everything IPC window not found” (Everything not running).
  Source: ES return code docs <https://www.voidtools.com/support/everything/command_line_interface/>
- The **Lite** build of Everything does **not** include IPC/ES support.
  Source: <https://mail.voidtools.com/en-us/faq/>

---

## 4.2 Windows Search Index via PowerShell (OLE DB) — **best if you already manage indexing**
### When it’s fast
Windows Search queries can be very fast *when* the target paths are indexed. Microsoft’s troubleshooting docs note that indexing performance depends heavily on the number of items indexed and index size.
Source: <https://learn.microsoft.com/en-us/troubleshoot/windows-client/shell-experience/windows-search-performance-issues>

### How to query the index (SQL / AQS)
Microsoft documents querying via **Windows Search SQL** and **AQS**:
- SQL/AQS overview: <https://learn.microsoft.com/en-us/windows/win32/search/using-sql-and-aqs-to-query-the-index>
- AQS reference: <https://learn.microsoft.com/en-us/windows/win32/lwef/-search-2x-wds-aqsreference>

A classic provider string:
- `Provider=Search.CollatorDSO;Extended Properties='Application=Windows';`
Source: <https://devblogs.microsoft.com/scripting/hey-scripting-guy-weekend-scripter-querying-the-windows-search-index/>

### Drop-in PowerShell function (copy/paste)
This returns results as objects (so you can pipe to CSV/JSON).

```powershell
function Invoke-WindowsSearchIndex {
  [CmdletBinding()]
  param(
    # Example: 'file:C:\' or 'file:C:\Users\alice\Documents\'
    [Parameter(Mandatory)]
    [string]$Scope,

    # Windows Search SQL WHERE clause fragment (excluding "WHERE")
    # Example: "System.FileName LIKE '%report%'"
    [Parameter(Mandatory)]
    [string]$Where,

    [int]$Top = 200
  )

  $provider = "Provider=Search.CollatorDSO;Extended Properties='Application=Windows';"
  $sql = @"
SELECT TOP $Top
  System.ItemName,
  System.ItemPathDisplay,
  System.Size,
  System.DateModified
FROM SYSTEMINDEX
WHERE SCOPE='$Scope' AND ($Where)
ORDER BY System.DateModified DESC
"@

  $da = New-Object System.Data.OleDb.OleDbDataAdapter($sql, $provider)
  $ds = New-Object System.Data.DataSet
  [void]$da.Fill($ds)
  $ds.Tables[0]
}
```

**Examples**
```powershell
# Find “report” in filename under C:\Projects (indexed)
Invoke-WindowsSearchIndex -Scope "file:C:\Projects" -Where "System.FileName LIKE '%report%'" -Top 200 |
  Select-Object System.ItemName, System.ItemPathDisplay, System.DateModified

# Find all .pdf under D:\Legal
Invoke-WindowsSearchIndex -Scope "file:D:\Legal" -Where "System.ItemType = '.pdf'"
```

### Notes & caveats
- **Index coverage** is the whole ballgame. If `D:\` isn’t indexed, results will be incomplete or empty.
- Property names are the Windows Search property system names (e.g., `System.ItemPathDisplay`). Microsoft’s docs are the authoritative reference.

---

## 4.3 `fd` — fastest “find-like” scan-first CLI
### Why it’s fast
- Parallel directory traversal; benchmark claims large speedups vs `find` in at least one workload.
Source: <https://github.com/sharkdp/fd/blob/master/README.md>

### Install (common enterprise-friendly options)
- `winget install --id=sharkdp.fd -e`
- or `choco install fd`
- or `scoop install fd`

### Core usage patterns
**General**
```powershell
fd [OPTIONS] [PATTERN] [PATH]
```

**High-value options**
- Include hidden: `-H`
- Don’t respect ignore files: `-I`
- Unrestricted (hidden + ignored): `-u`
- Use glob instead of regex: `-g` / `--glob`
- Filter by type: `-t f` (file), `-t d` (dir)
- Filter by extension: `-e log` (repeatable)
- Exclude pattern: `-E <glob>`
- Exec: `-x <cmd>` or batch exec: `-X <cmd>`
Docs: <https://github.com/sharkdp/fd>

**Examples**
```powershell
# Find “hosts” under C:\Windows
fd hosts C:\Windows

# Glob for all .dll under C:\Program Files (include hidden)
fd -H -g "*.dll" "C:\Program Files"

# Only directories containing “node_modules”
fd -t d node_modules C:\Projects

# Find big logs, then delete (batch exec is safer/faster)
fd -e log -x powershell -NoProfile -Command "Get-Item '{}' | Select FullName,Length"
```

### Important defaults to know
`fd` ignores hidden files and `.gitignore` patterns by default; use `-H` / `-I` / `-u` when you want “search literally everything.”
Source: <https://github.com/sharkdp/fd>

---

## 4.4 GNU `locate` + `updatedb` (MSYS2/Cygwin) — locate-style speed
### Install via MSYS2 (recommended for clean packaging)
MSYS2’s `findutils` package provides `locate.exe` and `updatedb`.
Source: <https://packages.msys2.org/package/findutils?repo=msys&variant=x86_64>

**Install**
```bash
pacman -S findutils
```

### Build the database (scheduled or manual)
`updatedb` creates/updates the filename database used by `locate`.
Source: <https://www.gnu.org/software/findutils/updatedb>

```bash
# Simple (default location depends on the build)
updatedb
```

In practice, you’ll want to control:
- What roots are indexed
- What paths are excluded
- Where the database is stored
(see your MSYS2/Cygwin man pages; behavior varies by distribution packaging)

### Query (fast)
`locate` searches the database for patterns.
Source: <https://www.gnu.org/software/findutils/locate>

```bash
# Find anything with “invoice” in the path/name
locate invoice

# Case-insensitive
locate -i invoice

# Match only basename
locate -b -i invoice.pdf

# Regex mode
locate -r '.*\\.(log|txt)$'
```

### Windows integration notes
- Paths returned in MSYS2 are often POSIX-like (e.g., `/c/Users/...`). Convert with `cygpath -w` if needed.
- Freshness is limited by your `updatedb` cadence.

---

## 4.5 UltraSearch (JAM Software) — CLI-driven quick search (GUI heritage)
UltraSearch supports command line parameters to start with a search already defined, and can run without UI and copy results to clipboard.
Source: <https://manuals.jam-software.com/ultrasearch/EN/commandline.html>

### Usage patterns
**General**
```bat
UltraSearch.exe <search path(s)> <search term> [switches]
```

**Examples (from vendor docs)**
```bat
ultrasearch.exe "C:\Windows" "readme.txt"
ultrasearch.exe "C:\Windows" "ext:txt"
ultrasearch.exe "C:\Windows" "*.exe" /CLIPBOARD /NOGUI /CLOSE
```

### Practical automation tip
Because UltraSearch can copy results to the clipboard, common automation patterns are:
1) run UltraSearch with `/NOGUI /CLIPBOARD /CLOSE`
2) read clipboard contents in PowerShell
3) parse into objects / write to CSV

That is clunkier than stdout, but workable when you need UltraSearch’s engine.

---

# 5) Honorable mentions (useful depending on constraints)
## 5.1 `where.exe` (built-in) — quick & dirty
`where` is built-in and supports recursive search with `/r`, but it’s fundamentally scan-first.
Source: <https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/where>

```bat
where /r C:\ *.dll
where /r D:\Projects *.csproj
```

## 5.2 Ultra Fast File Search (UFFS) — extremely fast, **non-commercial license**
The UFFS project provides a powerful CLI, MFT-based scanning, and publishes benchmark numbers — but it is explicitly **non-commercial use only** (Creative Commons BY-NC 2.0).
Sources:
- Project + benchmark table: <https://github.com/githubrobbi/Ultra-Fast-File-Search>
- License callout in README: same page

If you’re evaluating for an enterprise deployment, treat UFFS as a *reference implementation* unless you can satisfy licensing.

## 5.3 UsnParser (MIT) — developer/forensics-grade CLI for MFT/USN
UsnParser is a command-line utility to monitor USN changes and **search the MFT** (fast enumeration).
Source: <https://github.com/wangfu91/UsnParser>

Basic usage:
```bat
UsnParser monitor C:
UsnParser read D:
UsnParser search E: -f *.xlsx
```

It’s not as “friendly” as Everything for end users, but it’s scriptable and open-source.

---

# 6) How to benchmark candidates on *your* fleet (recommended)
Published benchmarks rarely match enterprise realities (EDR, network drives, folder exclusions, etc.). A simple harness:

## 6.1 Choose realistic workloads
- **Narrow**: “find `*.dll` under `C:\Windows\System32`”
- **Wide**: “find `*.pdf` anywhere on `C:\`”
- **Worst case**: include deep trees + many small folders

## 6.2 Measure cold vs warm runs
- Cold: first run after reboot (cache cold, indexer starting)
- Warm: second run (cache warm, index loaded)

## 6.3 Suggested tooling
- Use `hyperfine` on Windows (via Scoop/Chocolatey) if allowed.
- Or PowerShell `Measure-Command`:

```powershell
Measure-Command { es.exe "*.dll" -path "C:\Windows\" -n 200 | Out-Null }
Measure-Command { fd -g "*.dll" "C:\Windows" | Out-Null }
```

## 6.4 Capture “completeness” metrics
Speed is meaningless if tools disagree on results due to defaults (hidden/ignored) or index scope.
- For `fd`, standardize flags (`-H -I` for a fairer “search everything” run).
- For Windows Search, ensure Indexing Options include the test scope.
- For Everything, ensure all target volumes/folders are indexed.

---

# 7) Recommendation matrix (quick decision)
- **Need the fastest name/path search across NTFS + great CLI output:** **Everything + ES**
- **Already standardizing Windows Search indexing in your org:** **Windows Search (PowerShell SQL/AQS)**
- **Need a `find`-like walk with good performance and clean UX:** **fd**
- **Want `locate`-style instant queries with a scheduled rebuild:** **GNU locate**
- **Need an alternate engine & can live with clipboard-based output:** **UltraSearch**

---

# 8) UFFS Rust CLI Compatibility Analysis

This section documents the compatibility status of the **UFFS Rust CLI** (`uffs`) with popular file search tools. UFFS supports a **multi-personality CLI** (BusyBox pattern) where the same binary can behave differently based on how it's invoked:

```bash
uffs *.txt           # Modern mode (default)
ln -s uffs es        # Create symlink
es *.txt             # Everything-compatible mode
ln -s uffs uffs-cpp  # Create symlink
uffs-cpp *.txt       # C++ UFFS compatible mode
```

## 8.1 C++ UFFS Compatibility: ✅ 100% Complete

The Rust CLI is a **drop-in replacement** for the original C++ UFFS implementation.

| C++ Feature | Rust Equivalent | Status |
|-------------|-----------------|--------|
| `uffs c:/pro*` | `uffs c:/pro*` | ✅ |
| `--drives=c,d,m` | `--drives c,d,m` | ✅ |
| `--ext=jpg,mp4,documents` | `--ext jpg,mp4,documents` | ✅ (collections supported) |
| `--case=on` | `--case` | ✅ |
| `--out=bigfile.csv` | `--out bigfile.csv` | ✅ |
| `--header=true` | `--header` | ✅ |
| `--columns=path,size,created` | `--columns path,size,created` | ✅ |
| `--sep=TAB` | `--sep TAB` | ✅ (TAB, NEWLINE, SPACE, etc.) |
| `--quotes='` | `--quotes '` | ✅ |
| `--pos=1 --neg=0` | `--pos 1 --neg 0` | ✅ |
| Column aliases: `r,a,s,h,o` | Same | ✅ |
| `decendents` (typo) | Maps to `descendants` | ✅ |
| `sizeondisk` | Maps to `allocated_size` | ✅ |
| `written` | Maps to `modified` | ✅ |
| REGEX: `">pattern"` | Same | ✅ |

**Extension Collections:**
- `pictures` → jpg, jpeg, png, gif, bmp, tiff, webp, svg, ico, raw, heic
- `documents` → doc, docx, pdf, txt, rtf, odt, xls, xlsx, ppt, pptx, csv, md
- `videos` → mp4, avi, mkv, mov, wmv, flv, webm, mpeg, mpg, m4v, 3gp
- `music` → mp3, wav, flac, aac, ogg, wma, m4a, opus, aiff

## 8.2 Everything ES.exe Compatibility: 🟡 ~70% Complete

UFFS supports most ES.exe features but is missing some commonly-used options.

### ✅ Supported Features

| ES Flag | UFFS Equivalent | Notes |
|---------|-----------------|-------|
| `-r` / `-regex` | `">pattern"` | Regex via `>` prefix |
| `-i` / `-case` | `--case` | Case sensitive matching |
| `-n` / `-max-results` | `-n` / `--limit` | Limit results |
| `-h` / `-help` | `--help` | Help |
| `-name`, `-size`, `-dc`, `-dm`, `-da` | `--columns` | Column selection |
| `-csv` | `--format csv` | CSV output |
| `-export-csv <file>` | `--out <file>` | Export to file |
| `-no-header` | `--header=false` | No header row |
| `/ad` | `--dirs-only` | Folders only |
| `/a-d` | `--files-only` | Files only |
| `-version` | `--version` | Version info |
| `-s` (sort by path) | Default behavior | Results sorted by path |

### ❌ Missing Features (Planned)

| ES Flag | Priority | Description | Status |
|---------|----------|-------------|--------|
| `-sort <column>` | 🔴 HIGH | Sort by size, date, etc. | Planned |
| `-sort-ascending/-descending` | 🔴 HIGH | Sort direction | Planned |
| `-w` / `-whole-word` | 🔴 HIGH | Match whole word only | Planned |
| `-o` / `-offset` | 🟡 MEDIUM | Skip first N results | Planned |
| `-get-result-count` | 🟡 MEDIUM | Show count only | Planned |
| `-path <path>` | 🟡 MEDIUM | Scope to path (ES style) | Use pattern prefix |
| `/a[RHSDA...]` | 🟢 LOW | DIR-style attribute filter | Planned |
| `-highlight` | 🟢 LOW | Highlight matches in terminal | Planned |
| `-efu` | 🟢 LOW | Everything File List format | Not planned |

### ⚪ Not Applicable (ES-specific)

These features are specific to Everything's architecture and don't apply to UFFS:

| ES Feature | Reason |
|------------|--------|
| `-instance <name>` | ES uses IPC to Everything; UFFS reads MFT directly |
| `-timeout` | ES waits for Everything DB; UFFS has no external dependency |
| `-save-db`, `-reindex` | Everything-specific database operations |
| `-set-run-count`, `-get-run-count` | Everything run history tracking |

## 8.3 fd Compatibility: 🟡 ~60% Complete

UFFS shares some concepts with fd but has a different paradigm (MFT-based vs scan-first).

### ✅ Supported Features

| fd Flag | UFFS Equivalent | Notes |
|---------|-----------------|-------|
| `-g` / `--glob` | Default mode | Glob patterns are default |
| `-t f` | `--files-only` | Files only |
| `-t d` | `--dirs-only` | Directories only |
| `-e <ext>` | `--ext <ext>` | Extension filter |
| `-i` | Default | Case-insensitive by default |

### ❌ Missing Features

| fd Flag | Priority | Description | Status |
|---------|----------|-------------|--------|
| `-E` / `--exclude` | 🟡 MEDIUM | Exclude pattern | Planned |
| `-x` / `-X` | 🟢 LOW | Execute command on results | Not planned |
| `-H` | N/A | Include hidden | UFFS shows all by default |
| `-I` | N/A | Ignore .gitignore | Not applicable (MFT-based) |
| `-u` | N/A | Unrestricted | UFFS shows all by default |

### Fundamental Differences

| Aspect | fd | UFFS |
|--------|-----|------|
| **Strategy** | Scan-first (walks directories) | Index-first (reads MFT) |
| **Speed** | Fast for small trees | Fast for entire volumes |
| **Hidden files** | Excluded by default | Included by default |
| **gitignore** | Respected by default | Not applicable |
| **Cross-platform** | Yes | Windows NTFS only |

## 8.4 Personality Priority Roadmap

| Personality | Binary Name | Status | Priority |
|-------------|-------------|--------|----------|
| Modern (default) | `uffs` | ✅ Complete | - |
| C++ UFFS | `uffs-cpp` | ✅ Complete | - |
| Everything | `es` | 🟡 70% | HIGH |
| fd | `fd` | 🟡 60% | MEDIUM |

**To achieve 100% ES compatibility**, implement:
1. `--sort <column>` with ascending/descending
2. `--whole-word` / `-w` flag
3. `--offset` / `-o` flag
4. `--count` flag (result count only)

---

## Appendix A: Quick reference cheat-sheets
### Everything / ES
- Help: `es.exe -help`
- Limit: `-n 200`
- Match full path: `-p`
- Regex: `-regex "..."`
- Export CSV: `-export-csv out.csv`
Source: <https://www.voidtools.com/support/everything/command_line_interface/>

### fd
- Help: `fd -h` or `fd --help`
- Include hidden: `-H`
- Ignore ignore-files: `-I`
- Glob patterns: `-g "*.log"`
Source: <https://github.com/sharkdp/fd>

### where.exe
- Recursive: `where /r C:\ *.exe`
Source: <https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/where>

### Windows Search (OLE DB)
- Provider: `Search.CollatorDSO`
- Table: `SYSTEMINDEX`
- Scope: `SCOPE='file:C:\Path'`
Sources:
- <https://learn.microsoft.com/en-us/windows/win32/search/using-sql-and-aqs-to-query-the-index>
- <https://devblogs.microsoft.com/scripting/hey-scripting-guy-weekend-scripter-querying-the-windows-search-index/>

### UFFS (Ultra Fast File Search) - Rust CLI
- Help: `uffs --help`
- Search all drives: `uffs "*.txt"`
- Search specific drive: `uffs "*.txt" --drive C`
- Multi-drive: `uffs "*.txt" --drives C,D,E`
- Regex: `uffs ">.*\.log$"`
- Extension filter: `uffs "*" --ext jpg,png,pictures`
- Files only: `uffs "*" --files-only`
- Dirs only: `uffs "*" --dirs-only`
- Limit results: `uffs "*.dll" -n 100`
- Case sensitive: `uffs "README" --case`
- Export CSV: `uffs "*.rs" --out results.csv`
- Custom columns: `uffs "*" --columns path,size,modified`
- Fresh MFT read: `uffs "*.txt" --no-cache`
- Build index: `uffs index output.parquet`
Source: <https://github.com/rniow/UltraFastFileSearch>

---

*Document prepared January 27, 2026. Updated with UFFS Rust CLI compatibility analysis.*
