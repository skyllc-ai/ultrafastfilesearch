# UFFS demo capture kit

Tooling and shot lists for the two Phase 1 launch GIFs:

1. **TUI quick-start** — the zero-friction "front door" (hero on README + site).
2. **CLI speed** — raw query latency across a real NTFS estate (proof clip for HN / Reddit / Rust).

Everything here is built so the clips are **reproducible**, **honest**, and **re-renderable** each release. Capture must run on the Windows box with live NTFS (the only place `uffs` reads the MFT directly); macOS/Linux can only show offline-MFT analysis.

---

## TL;DR

```powershell
# 1. Put the machine in a known, honest recording state (Windows, elevated)
pwsh -File scripts/dev/demo/record_demo_prep.ps1 -Mode hot

# 2a. Scripted, reproducible CLI GIF (recommended — re-render every release):
vhs scripts/dev/demo/cli-demo.tape      # -> uffs-cli.gif

# 2b. TUI GIF — record interactively with ScreenToGif (see "TUI capture") ,
#     or try the VHS skeleton once keybindings are filled in:
vhs scripts/dev/demo/tui-demo.tape       # -> uffs-tui.gif

# 3. Drop outputs into both repos and wire them in (see "Where the GIFs go")
```

---

## Recorders

| Tool | Use for | Why | Platform |
|---|---|---|---|
| **[VHS](https://github.com/charmbracelet/vhs)** (`charmbracelet/vhs`) | CLI clip (primary) | Scripted `.tape` files → deterministic, pixel-stable GIF/MP4/WebM you can re-render every release. Drives the **real** binary, so all timings are genuine. | Best on Linux/macOS; Windows support is best-effort (needs `ttyd` + `ffmpeg`). If VHS misbehaves on Windows, record the CLI with ScreenToGif using the same shot list. |
| **[ScreenToGif](https://www.screentogif.com/)** | TUI clip (primary), CLI fallback | Free, Windows-native, captures a real interactive TUI session faithfully; built-in crop/trim/optimize + palette reduction. | Windows |
| `ffmpeg` | post-processing | Trim, scale, palette-optimize any capture (snippets below). | All |

Install on Windows: `winget install charmbracelet.vhs` (if available) and `winget install NickeManarin.ScreenToGif`.

---

## Honesty guardrails (non-negotiable)

These clips are marketing for a **benchmark-honest** project. Do not undermine that.

- **Never fake or speed-edit latency.** VHS runs the real binary; with ScreenToGif, do not cut frames to make a query look faster than it is.
- **State the daemon tier.** The "instant" story is a **hot/warm daemon**. `record_demo_prep.ps1 -Mode hot` warms it first; the caption must say so (e.g. "hot daemon, 25.9M records"). If you want to show the cold build, use `-Mode cold` and label it COLD.
- **Show real counts.** Don't trim the result count or the "N results in X ms" line out of frame.
- **Match the published numbers.** Latency on screen should be consistent with `docs/benchmarks/`. If it drifts, update the benchmark hub too — don't cherry-pick.
- **No doctored prompt.** Use a clean but real shell; don't hand-edit the recorded text.

---

## CLI shot list (`cli-demo.tape`)

Target ~18–22 s. Commands mirror the README Quick Start so the clip and docs agree.

1. `uffs "*.rs"` — whole-machine search, all drives, returns immediately.
2. `uffs "*.log" --min-size 100MB --newer 7d --files-only` — filtered hunt for big recent logs.
3. `uffs "*.dll" --drive C` — single-drive scope.
4. `uffs daemon status_drives` — show the per-drive tier/telemetry table (proves the hot-daemon architecture).

Caption to burn in or use as alt text: **"UFFS — targeted NTFS queries in single-digit ms on a hot daemon (25.9M records, 7 drives)."**

---

## TUI capture (`tui-demo.tape` / ScreenToGif)

Target ~15–20 s. The story is **"unzip → run → browsing your own drives in seconds."**

Shot list:
1. Terminal in an unzipped release folder. Type `uffs-tui` and Enter.
2. Daemon auto-starts; the TUI comes up populated with the real drives.
3. Type a query (e.g. `*.pdf`), show the results list filtering live.
4. Arrow through a couple of results / toggle a filter.
5. Quit cleanly.

> The bundled `uffs-tui` is the **free demo** (capped result counts, exports disabled — `DEMO-LICENSE.txt`). That's fine and honest for the front-door clip; don't imply uncapped/export features.

**Why ScreenToGif for the TUI:** interactive navigation reads more authentically when a human drives it, and the demo TUI's keybindings live in the separate [`githubrobbi/uffs-demo`](https://github.com/githubrobbi/uffs-demo) repo (not vendored here), so the VHS tape ships as a **skeleton** — fill in the real keystrokes once, then it's reproducible too.

---

## Where the GIFs go

Keep the heavy binaries out of git history where possible; prefer the smallest optimized GIF (< ~3 MB) or an MP4/WebM.

| Output | Location | Wires into |
|---|---|---|
| `uffs-tui.gif` | `assets/demo/uffs-tui.gif` (this repo) **and** `skyllc-ai.github.io/assets/demo/uffs-tui.gif` | README hero block; site hero/section |
| `uffs-cli.gif` | `assets/demo/uffs-cli.gif` (this repo) | README "Benchmark snapshot" area; HN/Reddit posts |

README wiring (hero):

```markdown
<p align="center">
  <img src="assets/demo/uffs-tui.gif" alt="UFFS TUI: unzip, run uffs-tui, and browse your real NTFS drives in seconds (hot daemon, 25.9M records).">
</p>
```

Site wiring: add an `<img>` in the hero or a dedicated demo section of `index.html`, served from `assets/demo/`.

---

## Post-processing snippets

```bash
# Trim to a clean window (start 00:02, length 18s) from an MP4 capture
ffmpeg -ss 00:00:02 -t 18 -i raw.mp4 -an trimmed.mp4

# MP4 -> optimized GIF (palette pass keeps it crisp + small)
ffmpeg -i trimmed.mp4 -vf "fps=15,scale=1200:-1:flags=lanczos,palettegen" palette.png
ffmpeg -i trimmed.mp4 -i palette.png -vf "fps=15,scale=1200:-1:flags=lanczos,paletteuse" uffs-cli.gif
```

Aim for: 1200px wide, 12–15 fps, < 3 MB. If a clip won't fit under ~5 MB as a GIF, ship an MP4/WebM and let GitHub/the site autoplay it.
