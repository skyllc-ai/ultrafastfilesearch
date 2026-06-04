# UFFS CLI speed demo — VHS tape (charmbracelet/vhs)
# Render:  vhs scripts/dev/demo/cli-demo.tape   ->  uffs-cli.gif
#
# Runs the REAL `uffs` binary, so every latency on screen is genuine.
# Prereqs:
#   - `uffs` (and `uffsd`) on PATH
#   - daemon WARMED before recording:  rust-script scripts/windows/record-demo-prep.rs --mode hot
#   - VHS + ffmpeg installed
#
# Honesty: this clip shows the HOT-DAEMON path. Keep the "hot daemon" caption
# in README/site alt text. Do not edit frames to alter timings.

Output uffs-cli.gif

Set Shell "pwsh"
Set FontSize 20
Set Width 1200
Set Height 640
Set Padding 24
Set Theme "Catppuccin Mocha"
Set TypingSpeed 45ms
Set PlaybackSpeed 1.0

# Settle on a clean prompt
Hide
Type "clear" Enter
Show
Sleep 600ms

# Every row search is capped with --limit so VHS never scroll-captures
# thousands of lines (default limit=0 = unlimited). Aggregations below
# return compact summaries, so they need no cap.

# 1) Known-item lookup: "find that invoice, anywhere on the machine"
#    Literal pattern → full-path substring match across every drive.
Type "uffs.exe invoice --files-only --limit 12 --format table"
Sleep 400ms
Enter
Sleep 2600ms

# 2) Storage triage: the giant logs eating your disk (size filter + sort)
Type "uffs.exe '*.log' --min-size 100MB --files-only --sort -size --limit 12 --format table"
Sleep 400ms
Enter
Sleep 2600ms

# 3) Developer reflex: every node_modules folder on the box, instantly
Type "uffs.exe node_modules --dirs-only --limit 12 --format table"
Sleep 400ms
Enter
Sleep 2600ms

# 4) Recent work: Rust files touched this week (relatable recency filter).
#    NOTE: --newer 7d is data-age-dependent — empty on old offline MFT
#    snapshots, but lights up against live drives (the Windows record run).
Type "uffs.exe '*.rs' --newer 7d --files-only --sort -modified --limit 12 --format table"
Sleep 400ms
Enter
Sleep 2600ms

# 5) Flex the engine: regex (the slowest mode) for date-stamped exports —
#    still returned in milliseconds.
Type "uffs.exe '>[0-9]{4}-[0-9]{2}-[0-9]{2}' --ext log,csv --files-only --limit 12 --format table"
Sleep 400ms
Enter
Sleep 2800ms

# 6) Answers, not rows: every drive's footprint in one server-side pass
Type "uffs.exe agg by_drive"
Sleep 400ms
Enter
Sleep 3000ms

# 7) Where is my space going? Top-10 semantic types via --agg power syntax
Type "uffs.exe '*' --agg 'terms:type,top=10' --format table"
Sleep 400ms
Enter
Sleep 3000ms

# 8) Size distribution — tiny vs huge, across the whole index
Type "uffs.exe agg by_size"
Sleep 400ms
Enter
Sleep 3000ms

# 9) Prove the architecture: per-drive Hot/Warm/Parked/Cold tier telemetry
Type "uffs.exe daemon status_drives"
Sleep 400ms
Enter
Sleep 3500ms

Sleep 1s
