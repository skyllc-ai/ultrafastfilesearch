# Windows profiling → Mac analysis (M4) for a Rust low-level NTFS/MFT tool  
*(practical runbook + repeatable workflow)*

## Goals (your constraints, translated into requirements)
- **Profile on native Windows** (real NTFS stack, real caching behavior, real syscalls).
- **Move artifacts by USB** (GBs OK).
- **Analyze on macOS (M4)** with a good UI (flame graphs, timelines), ideally without being “stuck” using Windows-only viewers.
- **Optimize ruthlessly**: you need *repeatability*, *symbols*, and *the ability to compare runs*.

This plan gives you two complementary pipelines:

1. **Fastest “works everywhere” sharing format (recommended):**  
   **PerfView → export SpeedScope JSON → analyze on macOS in speedscope.app**
2. **Best interactive profiler UI + off-CPU insight (great during iteration):**  
   **samply on Windows (ETW) → move `profile.json` (+ symbols) → `samply load` on macOS**  
   *(samply uses the Firefox Profiler UI and works on Windows/macOS/Linux.)*

---

## Why two pipelines?
### Pipeline A — PerfView → SpeedScope (most portable)
- PerfView is Windows-native and ETW-backed; it can capture CPU stacks and more. citeturn2search5  
- PerfView can export to **SpeedScope JSON**, which is **self-contained and cross-platform** (open it on macOS in a browser). citeturn1view2turn2search9turn2search6  
- **Great for moving profiles to your Mac** without worrying about symbol servers or platform-specific analysis tools.

### Pipeline B — samply (best workflow feel, great UI)
- samply is a cross-platform CLI CPU profiler and uses the Firefox Profiler as its UI. citeturn1view0turn0search8  
- On Windows it uses **ETW** and can record both on-CPU and off-CPU samples (locks / waits show up). citeturn1view0  
- You can save the profile to disk (`profile.json`) and later open it via `samply load`. citeturn6search14turn9search8  
- samply’s symbol stack (wholesym / samply-symbols) supports Windows formats (PDB / PE) and symbol servers, across platforms. citeturn4search3turn11view0turn4search20  

**Reality check:** for “ship a profile to another machine and get perfect symbols”, **PerfView→SpeedScope is the least finicky**. samply is awesome, but you must treat symbols as a first-class artifact (see below).

---

# 0) One-time: build outputs that profile well (symbols + stable stacks)

## Rust build profile: “release-like, but with debug info”
samply explicitly recommends compiling release mode *with debug info* for good stacks & source view. citeturn1view0  

Add a dedicated Cargo profile (project-local is easiest):

```toml
# Cargo.toml
[profile.profiling]
inherits = "release"
debug = true
```

*(Alternative per samply docs: put this in `~/.cargo/config.toml`.)* citeturn1view0  

### Extra flags that often improve profiling quality
- Make stacks more reliable (especially in hot loops / LTO-heavy builds):
  - Consider disabling full LTO for profiling builds (LTO can smear stacks and change inlining).
  - Consider `codegen-units = 1` to make profiles less “noisy” across builds.
- If you use MSVC target, ensure `.pdb` is produced and **copied with the `.exe`**.

> You want: **(exe + pdb)** always travel together.

---

# 1) File layout: make “profiling bundles” a habit

On your Mac, create a deterministic staging folder per build:

```
bundles/
  <date>-<gitsha>-<scenario>/
    dist/
      uffs.exe
      uffs.pdb            (or relevant debug info)
      *.dll               (if you ship any)
      inputs/             (the exact dataset used)
      run_args.txt
      build_meta.json     (git sha, rustc version, flags, target)
    profiles/
      perfview.speedscope.json
      samply.profile.json (optional)
    notes.md              (what you changed, what you’re testing)
```

Why it matters:
- When you find a win, you can reproduce it.
- When you regress, you can bisect *with evidence*.

---

# 2) One-time: Windows machine setup

## 2.1 Install tools
### Option A (recommended for cross-platform): PerfView
- PerfView is a Windows performance analysis tool (CPU & memory focused) and produces ETL traces. citeturn2search5turn2search2  

### Option B: samply
- Install samply (either via `cargo install` or prebuilt scripts). citeturn1view0  

## 2.2 Set up symbol resolution (do this once, it pays forever)
Good stacks on Windows require **symbols**.
- PDBs are the symbol files for Windows builds. citeturn4search4  
- Windows debuggers and profiling tools can use **symbol servers** and **symbol stores**; Microsoft documents the concept + SymStore. citeturn6search2  
- Microsoft also documents how to use the public symbol server and configure symbol paths. citeturn6search0turn6search1  

### Practical setup (works for many tools)
Create:
- `C:\symcache` (download/cache)
- `C:\mysymbols` (your own symbol store, optional but recommended)

Then use a symbol path like:

```text
srv*C:\symcache*https://msdl.microsoft.com/download/symbols
```

If you maintain your own symbol store too:

```text
srv*C:\symcache*C:\mysymbols*https://msdl.microsoft.com/download/symbols
```

> This massively improves call stacks inside Windows libraries and lets you correlate time spent in kernel / filesystem / memory manager.

---

# 3) Pipeline A (recommended): PerfView → SpeedScope JSON → macOS analysis

## 3.1 Collect CPU trace on Windows
### The “simple & reliable” way (GUI)
1. Launch PerfView.
2. Use its collection workflow to record your run (CPU sampling / thread time).
3. Stop collection after the operation of interest (e.g., “scan disk + parse MFT + build results”).

*(PerfView is a well-known ETW-based collector, including in Microsoft’s own guidance.)* citeturn2search2  

## 3.2 Export to SpeedScope JSON (portable)
PerfView has a **SpeedScope export** feature; it generates a JSON file you can load in speedscope.app. citeturn1view2turn2search6  

In PerfView:
- Open the trace
- Go to CPU stack view / stack viewer
- Export using SpeedScope export (often via “Save View As” → SpeedScope)

Microsoft also calls out SpeedScope as a cross-platform analysis target. citeturn2search9  

## 3.3 Move + analyze on macOS
1. Copy `*.speedscope.json` to your Mac (USB is perfect).
2. Open it in a browser using SpeedScope.
3. Use:
   - flame graph
   - time order view
   - left/right compare (great for before/after changes)

### What you gain
- **Single file** artifact that’s easy to attach to PRs or drop into a “perf-results” folder.
- No dependency on Windows-only viewers for day-to-day iteration.

---

# 4) Pipeline B: samply on Windows → analyze on macOS with `samply load`

## 4.1 Record on Windows
Baseline command:

```powershell
# From the folder that contains uffs.exe (+ uffs.pdb)
samply record --save-only -o profiles\samply.profile.json -- .\uffs.exe <args>
```

- samply records and uses the Firefox Profiler UI. citeturn1view0turn0search8  
- `--save-only` is used in real samply workflows to write the JSON profile to disk. citeturn3search2turn9search5  

### If you want OS symbols (Windows libs) via Microsoft’s symbol server
samply explicitly supports using the Microsoft Symbol Server on Windows. citeturn1view0  

Example:

```powershell
samply record --save-only -o profiles\samply.profile.json `
  --windows-symbol-server https://msdl.microsoft.com/download/symbols `
  -- .\uffs.exe <args>
```

*(If you’re profiling the whole system / many processes, samply also supports `-a`.)* citeturn1view0  

## 4.2 Transfer to macOS (what to copy)
Copy **both**:
- `profiles/samply.profile.json`
- `dist/` containing **your exe + pdb** (or equivalent debug files)

## 4.3 Open on macOS with symbols
On macOS:

```bash
# install once, if needed
cargo install --locked samply

# in the bundle directory
samply load profiles/samply.profile.json
```

Using `samply load` is the documented way to open a saved profile with working symbolication. citeturn6search14turn9search8  

### Why this can work cross-platform
samply’s symbol stack supports:
- Windows symbols (PDB/PE)
- symbol servers
- local symbol directories
…across platforms. citeturn11view0turn4search3turn4search20  

### If symbols don’t resolve on macOS (what to do)
This is usually because the symbol resolver can’t find your PDB/binary by its identifiers.

Most robust fix:
1. Create a **symbol store** on Windows (using SymStore). citeturn6search2  
2. Put it on the USB drive (e.g., `USB:\symbols\...`).
3. Point your tooling at that symbol store + Microsoft symbol server.

If you don’t want to fight this today:
- fall back to **Pipeline A** (PerfView → SpeedScope JSON), which is specifically designed for portable viewing. citeturn1view2turn2search9  

---

# 5) Capturing I/O + CPU together (NTFS/MFT work is often I/O-bound)

For low-level NTFS scanning, a pure CPU profile can be misleading:
- page cache effects
- synchronous reads
- readahead behavior
- file metadata calls
- kernel time

## 5.1 ETL tracing and WPA (deep Windows system view)
Windows Performance Analyzer can open ETL traces produced by WPR / Xperf. citeturn0search2  

Workflow:
1. Record an ETL trace that includes CPU sampling + disk/file I/O providers (WPR scenario / custom profile).
2. Analyze disk I/O, file I/O, CPU usage, context switches, etc.
3. Export tables/charts from WPA as CSV for archiving and later analysis on macOS.

> You won’t get WPA itself on macOS, but you *can* export data products (CSV) and correlate them with your CPU profile findings.

---

# 6) Repeatability: the part most people skip (but it’s where wins come from)

## 6.1 Freeze your “scenario”
Define 3–5 fixed scenarios (and keep them forever), e.g.
- `mft_small` (tiny image)
- `mft_medium` (realistic)
- `mft_large` (worst-case)
- `cold_cache` vs `warm_cache` (explicitly note which)

Store the exact dataset hash in `build_meta.json`.

## 6.2 Run protocol (so your profiles compare cleanly)
- Pick one:
  - **Warm cache** (run once, discard, then profile)
  - **Cold cache** (reboot or flush file cache; harder)
- Keep sampling rate stable (don’t change it between A/B runs).
- Use the same binary flags (profiling build).
- Use stable CPU power settings (“High Performance” plan) if possible.

## 6.3 Artifact naming (so you can diff without thinking)
Example:

```
2026-01-20_a1b2c3d_mft_large_readonly.perfview.speedscope.json
2026-01-20_a1b2c3d_mft_large_readonly.samply.profile.json
```

---

# 7) What I would do for your project (my “world-class engineer” default)
If you want the highest ROI with minimal tool pain:

1. **Make PerfView→SpeedScope your canonical, shareable artifact.**  
   That becomes your “performance PR evidence” on macOS. citeturn1view2turn2search9turn2search6  

2. **Use samply when you’re actively iterating**, especially if you care about:
   - off-CPU waiting / lock contention
   - fast visual scanning (Firefox Profiler UI) citeturn1view0turn0search8  

3. **Treat symbols as build artifacts, not optional files.**  
   Always ship exe + pdb together; optionally maintain a symbol store. citeturn6search2turn4search4  

4. **Add a `profiling` Cargo profile** and keep it consistent across the team. citeturn1view0  

---

# Appendix: Minimal command snippets

## A) Build on macOS (example; adapt target/toolchain)
```bash
cargo build --profile profiling --target x86_64-pc-windows-gnu
# or: x86_64-pc-windows-msvc (if you have that toolchain working)
```

## B) Record with samply on Windows (save-only)
```powershell
samply record --save-only -o profile.json -- .\uffs.exe <args>
```
`--save-only` usage is referenced in samply issues and practice. citeturn3search2turn9search5  

## C) Record all processes (if needed)
```powershell
samply record -a --windows-symbol-server https://msdl.microsoft.com/download/symbols
```
Windows symbol server usage is shown in samply docs. citeturn1view0  

## D) Load on macOS
```bash
samply load profile.json
```
Loading saved profiles via `samply load` is the recommended way to get symbolication. citeturn6search14turn9search8  

---

# References (key sources used)
*(URLs in code blocks so they’re easy to copy/paste.)*

- samply (cross-platform, uses Firefox Profiler, Windows ETW, symbol servers)  
```text
https://github.com/mstange/samply
```

- “Need `samply load` to view saved profile with symbols” (discussion / rough edges)  
```text
https://github.com/mstange/samply/issues/83
```

- PerfView (Windows ETW tool)  
```text
https://github.com/microsoft/perfview
```

- PerfView SpeedScope export overview  
```text
https://deepwiki.com/microsoft/perfview/7.1-speedscope-export
```

- Microsoft: Symbol servers / symbol stores (SymStore)  
```text
https://learn.microsoft.com/en-us/windows/win32/debug/symbol-servers-and-symbol-stores
```

- Microsoft: public symbol server + symbol path configuration  
```text
https://learn.microsoft.com/en-us/windows-hardware/drivers/debugger/microsoft-public-symbols
https://learn.microsoft.com/en-us/windows-hardware/drivers/debugger/symbol-path
```

- Microsoft: WPA can open ETL traces produced by WPR/Xperf  
```text
https://learn.microsoft.com/en-us/windows-hardware/test/wpt/opening-and-analyzing-etl-files-in-wpa
```

- samply / wholesym cross-platform symbolication support (PDB/PE etc.)  
```text
https://docs.rs/wholesym
```
