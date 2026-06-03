# UFFS CLI speed demo — VHS tape (charmbracelet/vhs)
# Render:  vhs scripts/dev/demo/cli-demo.tape   ->  uffs-cli.gif
#
# Runs the REAL `uffs` binary, so every latency on screen is genuine.
# Prereqs:
#   - `uffs` (and `uffsd`) on PATH
#   - daemon WARMED before recording:  pwsh -File scripts/dev/demo/record_demo_prep.ps1 -Mode hot
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

# 1) Whole-machine search across every drive — returns immediately
Type "uffs ""*.rs"""
Sleep 400ms
Enter
Sleep 2500ms

# 2) Filtered hunt: big, recent log files only
Type "uffs ""*.log"" --min-size 100MB --newer 7d --files-only"
Sleep 400ms
Enter
Sleep 2800ms

# 3) Single-drive scope
Type "uffs ""*.dll"" --drive C"
Sleep 400ms
Enter
Sleep 2500ms

# 4) Prove the architecture: per-drive tier + telemetry table (the hot daemon)
Type "uffs daemon status_drives"
Sleep 400ms
Enter
Sleep 3500ms

Sleep 1s
