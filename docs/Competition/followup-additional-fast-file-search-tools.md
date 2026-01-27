# Follow-up: Additional “fast find” tools (Windows NTFS + CLI focus)

**Date:** 2026-01-27  
**Scope:** Deep-dive analysis of the *additional* tools you listed: **Everything (voidtools), WizFile, UltraSearch, Listary, Locate32, Agent Ransack, FileSeek, FSearch (Linux)** — with emphasis on:  
- **How they achieve speed** on NTFS (MFT / index / scan)  
- **Whether they are truly usable as CLI tools** (stdout, exit codes, automation)  
- **Any published performance metrics** (and what’s missing)  
- **Concrete “how to use” CLI snippets**

> If your north star is “`find`-like CLI that can locate any file on NTFS *very fast*”, the most CLI-native option in this list is **Everything + es.exe**.

---

## 1) At-a-glance matrix (CLI + NTFS performance)

| Tool | Primary speed strategy | “Real CLI” (prints results to stdout) | Typical privileges | Best at |
|---|---|---:|---|---|
| **Everything** (voidtools) | Builds in-memory index from NTFS metadata (incl. MFT) and tracks updates via NTFS change journal / USN; supports “Everything service” so UI can run standard-user | **Yes** via `es.exe` | Service or admin for NTFS indexing on modern Windows | **Fastest filename/path search + automation** |
| **WizFile** (Antibody) | Reads NTFS **MFT** directly; maintains compact in-memory DB; no external DB file | **Not vendor-documented** for search/output | Often runs as admin (MFT access) | Instant GUI search; minimal setup |
| **UltraSearch** (JAM) | Uses NTFS **MFT** directly (no on-disk index file); internal caching | **Semi**: CLI launches app; can copy results to clipboard / run “no GUI”, but **not stdout-first** | Admin for MFT; `/noadmin` loses MFT access and can be slower | Fast GUI search with some script hooks |
| **Listary** | Productivity launcher/search UX; can call command lines (“Commands/Actions”), often paired with Everything | **No** (not a CLI search engine) | Standard user | Interactive navigation/launching (pair with Everything) |
| **Locate32** | “locate/updatedb” style **database** of paths; fast lookup but needs periodic updates; project is old | **Mixed/legacy**: has command-line components, but docs are scattered; GUI-first | Standard user; depends on where DB lives | “locate”-like workflows if you accept stale indexes |
| **Agent Ransack** (Mythicsoft) | Primarily **scan-based** filename/content search (can also use separate indexes); strong content search | **Yes** (console `flpsearch.exe`), plus GUI can output to file | Standard user (admin may help access) | **Content search / regex** across files |
| **FileSeek** (Binary Fortress) | Scan-based filename/content search; supports regex; profiles; some automation | **Yes-ish**: CLI args exist; **Pro** can output to a file and exit | Standard user | Content search + automation via profiles |
| **FSearch (Linux)** | Fast GUI search for Unix-like systems; project explicitly points CLI users to `find`/`locate`/`fzf` | **No** (CLI not the focus) | N/A | Linux desktop search UX |

---

## 2) Everything (voidtools) — the CLI powerhouse

### Why it’s fast (NTFS)
- Everything indexes file/folder names (and some metadata) and can build the initial index very quickly; vendor FAQ gives **concrete indexing times**: ~**1 second** for ~120k files, and ~**1 minute** for **1,000,000** files.  
  Source: voidtools FAQ (“How long will it take to index my files?”) — https://www.voidtools.com/en-us/faq/  
- Everything maintains its database in memory; voidtools forum guidance gives a **RAM rule of thumb**: ~**100 MB per 1 million items** (files+folders).  
  Source: voidtools forum (“Improving Everything Search Performance for Large File Sets”) — https://www.voidtools.com/forum/viewtopicvoid.php?t=15122  
- On modern Windows, **NTFS indexing as a standard user typically requires the Everything Service**, which “helps Everything index NTFS volumes and monitor USN Journals.”  
  Source: Everything Service docs — https://www.voidtools.com/support/everything/everything_service/

### The CLI you actually want: `es.exe`
Everything ships (or can be installed with) **ES**, the official **command-line interface** client.

**Key point:** `es.exe` uses Everything’s IPC; **Everything must be running**, or ES returns an error (e.g., “Everything IPC window not found”).  
Source: ES docs (“Return Codes”, errorlevel 8) — https://www.voidtools.com/support/everything/command_line_interface/

#### Core usage patterns
```bat
:: Basic substring search
es.exe report

:: Wildcards
es.exe *.pdf

:: Search only folders
es.exe /ad project*

:: Search only files
es.exe /a-d project*

:: Sort by size and show top 20
es.exe -sort size -n 20

:: Export results
es.exe *.log -export-csv logs.csv
es.exe *.mp3 -export-efu mp3.efu
```

ES supports **multiple export formats** (CSV/JSON/EFU/M3U/TSV/TXT etc.), plus rich switches (columns, sorts, filters).  
Source: ES documentation — https://www.voidtools.com/support/everything/command_line_interface/

#### Everything.exe command line (mostly UI-control)
Everything itself supports a large set of command-line options (install/service/config and search setup). For example, you can set the search text:
- `-search <text>` / `-s <text>`  
Source: Command Line Options docs — https://www.voidtools.com/support/everything/command_line_options/

In practice, for automation **prefer `es.exe`** (it’s built for “CLI piping” use cases).

### Deployment note: standard user vs admin
- On Vista+ with a standard user account, you typically need the **Everything service** for NTFS indexing; alternatively newer versions may support indexing via an elevated helper process.  
Source: Everything Service docs — https://www.voidtools.com/support/everything/everything_service/

### Licensing (quick)
- voidtools publishes an MIT-like license text for Everything.  
Source: https://www.voidtools.com/License.txt  
- voidtools has also publicly stated Everything can be used in commercial environments; see forum pointer to the license.  
Source: https://www.voidtools.com/forum/viewtopic.php?p=25105

---

## 3) WizFile — insanely fast NTFS, but not a true CLI search tool (today)

### Why it’s fast (NTFS)
WizFile’s own documentation is very direct:
- It reads the **NTFS Master File Table (MFT) directly**, bypassing standard filesystem APIs for the initial scan, and maintains an in-memory database.  
Source: WizFile “About” — https://antibody-software.com/wizfile/about  

WizFile also emphasizes:
- **No external database file** (keeps file data in RAM and can swap to pagefile).  
Source: WizFile “About” — https://antibody-software.com/wizfile/about  

### Admin rights
- Antibody’s release notes indicate WizFile “always runs as administrator” historically, and newer versions added options around admin-launched apps.  
Source: WizFile download / changelog text — https://antibody-software.com/wizfile/download  
- PortableApps notes WizFile “requires admin rights to function.”  
Source: https://portableapps.com/apps/utilities/wizfile-portable

### CLI status (important)
As of this writing, Antibody’s official docs (Quick Start / FAQ / About) describe **interactive usage** (wildcards, filters, etc.) but do **not** document a supported “search from CLI and print results to stdout” interface comparable to `es.exe`.  
Sources:  
- Quick Start Guide — https://antibody-software.com/wizfile/quick-start  
- About — https://antibody-software.com/wizfile/about  

**What this means in practice:**  
- WizFile is an excellent *GUI* “instant finder”.  
- If your requirement is “drop-in CLI tool for scripts and pipelines”, WizFile is currently a weaker fit unless you’re willing to do UI automation.

### Licensing (quick)
WizFile’s site states it’s **free for personal use** and has commercial licensing; see its EULA for details.  
Sources:  
- Main page — https://antibody-software.com/wizfile/  
- EULA — https://antibody-software.com/wizfile/eula  
- PortableApps release note summary — https://portableapps.com/news/2025-05-22--wizfile-portable-3.13-released

---

## 4) UltraSearch — MFT speed, but “CLI” mostly means “scriptable launch”

### Why it’s fast (NTFS)
UltraSearch documentation states it uses the NTFS **MFT** to achieve high speed without maintaining an on-disk index file.  
Source: UltraSearch Features — https://manuals.jam-software.com/ultrasearch_free/EN/features.html  

### Admin rights affect MFT access
JAM Software explicitly notes that without admin rights UltraSearch **won’t have access to the MFT**, and “the search may be slower.”  
Source: JAM knowledge base — https://knowledgebase.jam-software.com/7576  

### Command line options (good, but not stdout-first)
UltraSearch supports launching with search paths and a search term, and has switches like:
- `/CLIPBOARD` to copy results to clipboard
- `/NOGUI` to execute in background (results via notification + messages in Windows event log)
- `/CLOSE` to terminate after completing  
Source: UltraSearch Command Line Options — https://manuals.jam-software.com/ultrasearch/EN/commandline.html  

#### Examples
```bat
:: Search C:\Windows for readme.txt
UltraSearch.exe "C:\Windows" "readme.txt"

:: Search and copy results to clipboard; run without UI; close when done
UltraSearch.exe "C:\Windows" "*.exe" /CLIPBOARD /NOGUI /CLOSE

:: Run without admin rights (may be slower)
UltraSearch.exe /NOADMIN
```

**Bottom line:** UltraSearch is great for fast interactive searching and some automation, but it’s not as clean as `es.exe` for piping results in CLI workflows.

### Editions / licensing
UltraSearch comes in Free and Professional editions with different capabilities.  
Source: JAM “Compare UltraSearch Editions” — https://www.jam-software.com/ultrasearch/editions.shtml  

---

## 5) Listary — not a “fast filesystem search engine”, but a great UI front-end

Listary is best understood as a **launcher + workflow tool** (search, quick switch, actions). It can run **Commands** that are essentially **command lines** (cmd/PowerShell or other executables).  
Source: Listary “Commands” — https://help.listary.com/options-commands  

It also supports customizable **Actions** that pass file/folder paths as parameters to other tools.  
Source: Listary “Actions” — https://help.listary.com/options-actions  

### CLI reality check
Listary itself is **not** a `find` replacement:
- It doesn’t present itself as a “search CLI that prints results and exits.”
- The best pattern in CLI-heavy environments is: **use Listary as a UX layer** and wire it to **Everything** (or `es.exe`) for actual fast searching.

### Licensing
Listary has a Pro license tier.  
Source: Listary Pro page — https://www.listary.com/pro  

---

## 6) Locate32 — classic “locate/updatedb” on Windows, but aging

### How it works
Locate32 explicitly states it:
- Stores file/folder names in a **database** and then searches that database quickly.  
- Works like Unix `updatedb` and `locate`.  
Source: Locate32 home — https://locate32.cogit.net/  

### Project freshness / risk
The Locate32 site indicates it was “last updated” in **April 2014**.  
Source: Locate32 site — https://locate32.cogit.net/  

So: it can still be useful, but you should treat it as **legacy software** and test thoroughly on Windows 10/11.

### CLI status
Locate32 historically shipped with command-line components (`locate.exe`, `updtdb32.exe`, etc.), and SourceForge release notes mention fixes and additions for command line arguments.  
Source: Locate32 files/release notes — https://sourceforge.net/projects/locate32/files/  

Because the official CLI documentation is not as centralized/clean as Everything’s `es.exe`, plan on:
- verifying supported switches on your target version (e.g., `locate.exe -h`)
- validating output encoding and quoting behavior in your environment

---

## 7) Agent Ransack — best-in-class for content searching (CLI-friendly), not “instant MFT name search”

Agent Ransack is a file name + **content search** tool. Mythicsoft notes it’s free (Lite mode) for personal/commercial use, with optional paid features.  
Source: Mythicsoft product page — https://www.mythicsoft.com/agentransack/

### Command line (Windows app + console app)
Mythicsoft provides extensive documentation:
- GUI app: `AgentRansack.exe ...`
- Console app: `flpsearch.exe ...`
- Indexing utility: `flpidx.exe ...`  
Source: Agent Ransack Command Line docs — https://help.mythicsoft.com/agentransack/en/commandline.htm  

#### A few high-value examples
```bat
:: Filename search under a folder, stream results to a file (no UI)
AgentRansack.exe -d "C:\WINDOWS" -f "*.sys" -o "C:\temp\results.txt"

:: Console search (prints to console) - best for scripts
flpsearch.exe -d "C:\Projects" -f "*.cs" -c "TODO" -s

:: Regex filename search (-fex) + output as CSV
flpsearch.exe -d "C:\Program Files" -fex -f "agen.*\.exe" -ofc -o "C:\temp\out.csv"
```

### Performance expectations
- Agent Ransack shines when you need **content searching**, complex expressions, and reporting.
- For “find file by name across an entire NTFS volume instantly”, MFT/index-based engines (Everything/WizFile/UltraSearch) are usually faster.

---

## 8) FileSeek — solid scripted “search + export”, but Pro gates some automation

FileSeek supports command line parameters including:
- `-d` search directory (supports multiple paths with `|`)
- `-q` query, or `-r` for regex query
- `-start` to execute the search
- `-o` output results to file **and close FileSeek** (**Pro only**)  
Source: FileSeek FAQ (“Command Line Parameters”) — https://www.fileseek.ca/FAQ/  

#### Examples
```bat
:: Start FileSeek with a directory and query
FileSeek.exe -d "C:\Logs" -q "error" -start

:: Regex content search
FileSeek.exe -d "C:\Projects" -r "(?i)\bpassword\b" -start

:: Export results (Pro only) and exit
FileSeek.exe -d "C:\Projects" -q "TODO" -o "C:\temp\todo.csv" -start
```

**Reality check:** FileSeek is more “grep-like” (content-aware) than “MFT-instant filename finder.”

---

## 9) FSearch (Linux) — great desktop search; CLI users should use `find`/`locate`/`fzf`

FSearch is a fast file search utility for Unix-like systems (GTK-based).  
Source: GitHub repo — https://github.com/cboxdoerfer/fsearch  

The project’s own site explicitly says: if you want a **command line interface**, it recommends `fzf` and the obvious tools `find` and `(m)locate`.  
Source: FSearch site — https://cboxdoerfer.github.io/fsearch/  

There are also third-party efforts claiming Windows support, but that’s outside the “core” Linux FSearch project and should be evaluated separately.  
Source (example): https://github.com/cygmris/fsearch_windows

---

## 10) Practical recommendation (based on your requirements)

### If you must have **very fast** + **CLI-native**:
1) **Everything + `es.exe`** — best “Windows `find`” experience in scripts and terminals.

### If you want “fastest interactive UI” and can compromise on CLI:
- **WizFile** (fast NTFS MFT scanning, low friction)  
- **UltraSearch** (MFT-based and scriptable launch, but not stdout-first)

### If you primarily need **content search** (grep-like) with automation:
- **Agent Ransack** (`flpsearch.exe` is quite capable)
- **FileSeek** (especially Pro if you want `-o` output + exit)

### If you want “Unix locate/updatedb” semantics on Windows:
- **Locate32**, but treat as legacy; validate on Windows 11, and validate update cadence + CLI options.

---

## Appendix A — quick benchmark plan (useful if you need “top 5 fastest” proof in your environment)

Even when vendors don’t publish numbers, you can generate defensible internal metrics. A simple approach:

**Metrics to collect**
- **Cold start** time (first run after reboot)
- **Time-to-first-result**
- **Time-to-N-results** (e.g., first 100, first 10k)
- CPU time / peak RSS during query (optional)
- For indexers: **index build time** and **index size**

**Workload**
- Pick 10–20 realistic queries (short substrings, wildcards, full path matches, regex where supported)
- Run each query 5–10 times; report median + p95.

**Tip:** For Everything specifically, include its published baselines (120k→~1s, 1M→~1m) as a sanity check against your own results.  
Source: https://www.voidtools.com/en-us/faq/

---

## Sources (most important)
- Everything FAQ (indexing time): https://www.voidtools.com/en-us/faq/  
- Everything Service (standard user + NTFS indexing): https://www.voidtools.com/support/everything/everything_service/  
- Everything ES CLI docs: https://www.voidtools.com/support/everything/command_line_interface/  
- Everything command line options: https://www.voidtools.com/support/everything/command_line_options/  
- Everything memory guidance (forum): https://www.voidtools.com/forum/viewtopicvoid.php?t=15122  
- WizFile About (MFT + in-memory DB): https://antibody-software.com/wizfile/about  
- WizFile Quick Start: https://antibody-software.com/wizfile/quick-start  
- UltraSearch Features (MFT): https://manuals.jam-software.com/ultrasearch_free/EN/features.html  
- UltraSearch Command Line Options: https://manuals.jam-software.com/ultrasearch/EN/commandline.html  
- UltraSearch no-admin/MFT note: https://knowledgebase.jam-software.com/7576  
- Listary Commands: https://help.listary.com/options-commands  
- Locate32 home (updatedb/locate model + last updated): https://locate32.cogit.net/  
- Locate32 files/release notes: https://sourceforge.net/projects/locate32/files/  
- Agent Ransack CLI: https://help.mythicsoft.com/agentransack/en/commandline.htm  
- FileSeek CLI parameters: https://www.fileseek.ca/FAQ/  
- FSearch site (CLI recommendation): https://cboxdoerfer.github.io/fsearch/  
- FSearch GitHub: https://github.com/cboxdoerfer/fsearch
