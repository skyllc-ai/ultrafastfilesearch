# Intentionally no: Set Shell "pwsh"
# VHS on Windows starts cmd.exe by default.
# We manually enter pwsh below to avoid VHS's PowerShell launcher issue.

Set FontSize 20
Set Width 1280
Set Height 720
Set Padding 24
Set Theme "Catppuccin Mocha"
Set TypingSpeed 45ms
Set PlaybackSpeed 1.0

Output uffs-cli-short.gif

Hide
Type "pwsh -NoLogo -NoProfile"
Enter
Sleep 1500ms
Type "function global:prompt { '>' }"
Enter
Show

# UltraFastFileSearch (UFFS)
# Benchmark-honest CLI hero demo.
#
# Recorded against the real CLI.
# Every command shown is executed as recorded.
#
# Demo environment:
#   - Hot daemon (pre-warmed)
#   - Real filesystem data
#   - Real result counts
#   - Real measured latency
#
# No edited timings. No synthetic datasets.

Set TypingSpeed 1ms
Type "Clear-Host"
Enter

Type "# ##           ##   ###############   ###############    #############"
Enter
Type "# ##           ##   ##                ##               ##             "
Enter
Type "# ##           ##   ##                ##               ##             "
Enter
Type "# ##           ##   ###########       ###########        ###########  "
Enter
Type "# ##           ##   ##                ##                           ## "
Enter
Type "# ##           ##   ##                ##                            ##"
Enter
Type "# ##           ##   ##                ##               ##           ##"
Enter
Type "#   ###########     ##                ##                 ###########  "
Enter
Enter
Type "# UltraFastFileSearch"
Enter
Type "# Benchmark-driven NTFS search for Windows"
Enter
Type "# Hot daemon - Real data - Real timings"
Enter

Sleep 5s

Type "Clear-Host"
Enter

# Hero flow:
#   1. Known-item lookup.
#   2. Storage triage with size filtering and sorting.
#   3. Developer workflow with recency filtering.
#   4. Server-side aggregation.
#   5. Daemon/readiness proof.
#
# Result sets are capped for recording readability.
# Commands, result counts and measured timings remain unchanged.
#
# Row searches use a compact custom format to keep output readable
# within the recording frame while preserving the real commands.

# 1) Known-item lookup

Type "# Find invoice files across the indexed drives"
Enter
Set TypingSpeed 45ms
Sleep 2s
Type "uffs.exe invoice --files-only --limit 10 --format custom --sep $([char]9) --columns Size,Name"
Enter
Set TypingSpeed 1ms
Sleep 3s
Type "Clear-Host"
Enter

# 2) Storage triage

Type "# Surface large log files consuming space"
Enter
Set TypingSpeed 45ms
Sleep 2s
Type "uffs.exe '*.log' --min-size 100MB --files-only --sort -size --limit 10 --format custom --sep $([char]9) --columns Size,Name"
Enter
Set TypingSpeed 1ms
Sleep 3s
Type "Clear-Host"
Enter

# 3) Recent developer work

Type "# Rust files modified during the last week"
Enter
Set TypingSpeed 45ms
Sleep 2s
Type "uffs.exe '*.rs' --newer 7d --files-only --sort -modified --limit 10 --format custom --sep $([char]9) --columns Size,Name"
Enter
Set TypingSpeed 1ms
Sleep 3s
Type "Clear-Host"
Enter

# 4) Server-side aggregation

Type "# Aggregate storage by detected file type"
Enter
Set TypingSpeed 45ms
Sleep 2s
Type "uffs.exe '*' --agg 'terms:type,top=10' --format table"
Enter
Set TypingSpeed 1ms
Sleep 3s
Type "Clear-Host"
Enter

# 5) Hot daemon proof

Type "# Hot daemon status and indexed drive tiers"
Enter
Set TypingSpeed 45ms
Sleep 2s
Type "uffs.exe daemon status_drives"
Enter
Set TypingSpeed 1ms
Sleep 3s
Type "Clear-Host"
Enter

# Close with the product position

Type "# UltraFastFileSearch"
Enter
Type "# Deterministic local search for humans, scripts and AI agents"
Enter
Type "# CLI, TUI, daemon, API and MCP on one Rust engine"
Enter

Sleep 5s

Type "Clear-Host"
Enter
