# UFFS demo capture kit

VHS tapes and tooling for the demo clips embedded in the README, the docs, and the
project site. Everything here is built so the clips are **reproducible**, **honest**,
and **re-renderable** each release.

Each clip has a full reel (`*-demo.tape`), a short loop (`*-demo-short.tape`, TUI/CLI),
and a video cut (`*-demo-video.tape` → MP4). The video cuts are identical to the GIF
tapes in shot list, commands, and keystrokes; only the static card holds (banner,
intro, outro) are shorter. Command typing speed and all measured latencies are the
same in every variant.

Capture must run on a Windows box with live NTFS (the only place `uffs` reads the MFT
directly); macOS/Linux can only show offline-MFT analysis.

> **Two binaries gotcha.** Releases ship both the Rust daemon client (`uffs.exe`) and
> the legacy C++ reference (`uffs.com`). On Windows, `PATHEXT` ranks `.COM` before
> `.EXE`, so a bare `uffs` runs the C++ tool — which has no `daemon` subcommand and
> uses `--drives=`/`--columns=` syntax. The prep tool
> (`scripts/windows/record-demo-prep.rs`) resolves `uffs.exe` **by name** and gates on
> a `>= 0.5.0` semver parsed from `uffs --version`, so the C++ tool or an old build is
> refused up front. Override with `--bin <path>`.

---

## TL;DR

```powershell
# 1. Put the machine in a known, honest recording state (Windows, elevated)
rust-script scripts/windows/record-demo-prep.rs --mode hot --drives C,D,E,F,G,M,S

# 2. Record interactive clips on Windows with ScreenToGif.
#    VHS is NOT available natively on Windows (it needs `ttyd`, Linux/macOS only).

# 2-alt. Reproducible GIF/MP4 via VHS — only under WSL/Linux/macOS:
vhs scripts/dev/demo/cli-demo.tape          # -> uffs-cli.gif        (needs ttyd + ffmpeg)
vhs scripts/dev/demo/cli-demo-video.tape    # -> uffs-cli-video.mp4
```

---

## Recorders

| Tool | Use for | Why | Platform |
|---|---|---|---|
| **[ScreenToGif](https://www.screentogif.com/)** | TUI clip **and** CLI clip on Windows (primary) | Free, Windows-native, captures a real interactive session faithfully; built-in crop/trim/optimize + palette reduction. | Windows |
| **[VHS](https://github.com/charmbracelet/vhs)** (`charmbracelet/vhs`) | Reproducible clips from the `.tape` files | Scripted tapes → deterministic, pixel-stable GIF/MP4/WebM you can re-render every release. Drives the **real** binary, so all timings are genuine. | Requires `ttyd`, which has **no native Windows build** — run tapes under **WSL/Linux/macOS** only. |
| `ffmpeg` | post-processing | Trim, scale, palette-optimize any capture (snippets below). | All |

Install on Windows: `winget install NickeManarin.ScreenToGif`. VHS is Linux/macOS-only
for our purposes (`brew install vhs ttyd` / `apt install ttyd` + `go install ...`).

---

## Honesty guardrails (non-negotiable)

These clips demo a **benchmark-honest** project. Do not undermine that.

- **Never fake or speed-edit latency.** VHS runs the real binary; with ScreenToGif, do
  not cut frames to make a query look faster than it is.
- **State the daemon tier.** The "instant" story is a **hot/warm daemon**.
  `record-demo-prep.rs --mode hot` warms it first; the caption must say so. If you
  want to show the cold build, use `--mode cold --confirm-destructive` and label it COLD.
- **Show real counts.** Don't trim the result count or the "N results in X ms" line
  out of frame.
- **Match the published numbers.** Latency on screen should be consistent with
  `docs/benchmarks/`. If it drifts, update the benchmark hub too — don't cherry-pick.
- **No doctored prompt.** Use a clean but real shell; don't hand-edit recorded text.

The bundled `uffs-tui` is the **free demo** edition (capped result counts, exports
disabled — `DEMO-LICENSE.txt`). That's fine and honest for a clip; don't imply
uncapped/export features.

---

## Outputs

Each `.tape` names its output file. GIF outputs live in `assets/demo/` (this repo) and
are the only demo media committed to git. The `*-video.tape` MP4 outputs are upload
artifacts (gitignored, never committed).

| Tape | Output |
|---|---|
| `tui-demo.tape` / `tui-demo-short.tape` / `tui-demo-video.tape` | `uffs-tui.gif` / `uffs-tui-short.gif` / `uffs-tui-video.mp4` |
| `cli-demo.tape` / `cli-demo-short.tape` / `cli-demo-video.tape` | `uffs-cli.gif` / `uffs-cli-short.gif` / `uffs-cli-video.mp4` |
| `mcp-demo.tape` / `mcp-demo-video.tape` | `uffs-mcp-claude.gif` / `uffs-mcp-claude-video.mp4` |

Keep GIFs small: prefer the smallest optimized GIF (< ~3 MB). If a clip won't fit
under ~5 MB as a GIF, ship an MP4/WebM and let GitHub/the site autoplay it.

---

## Post-processing snippets

```bash
# Trim to a clean window (start 00:02, length 18s) from an MP4 capture
ffmpeg -ss 00:00:02 -t 18 -i raw.mp4 -an trimmed.mp4

# MP4 -> optimized GIF (palette pass keeps it crisp + small)
ffmpeg -i trimmed.mp4 -vf "fps=15,scale=1200:-1:flags=lanczos,palettegen" palette.png
ffmpeg -i trimmed.mp4 -i palette.png -vf "fps=15,scale=1200:-1:flags=lanczos,paletteuse" uffs-cli.gif
```

Aim for: 1200px wide, 12–15 fps.
