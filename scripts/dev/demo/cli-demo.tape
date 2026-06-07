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

Output uffs-cli.gif

Hide
Type "pwsh -NoLogo -NoProfile"
Enter
Sleep 1500ms
Type "function global:prompt { '>' }"
Enter
Show

# UltraFastFileSearch (UFFS)
# Benchmark-honest CLI demo.
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
Type "# Hot daemon - Real commands - Real timings"
Enter
Type "# Search, filter and aggregate across 25M+ indexed files"
Enter

Sleep 5s

Type "Clear-Host"
Enter

# Result sets are capped for recording readability.
# Commands, result counts and measured timings remain unchanged.
#
# Row searches use a compact custom format to keep output readable
# within the recording frame while preserving the real commands.
#
# Flow:
#   1. Display the workflow being demonstrated.
#   2. Pause briefly for readability.
#   3. Execute the real command.
#   4. Hold results on screen.
#   5. Clear before the next segment.

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

# 3) Developer workflow

Type "# Locate node_modules directories"
Enter
Set TypingSpeed 45ms
Sleep 2s
Type "uffs.exe node_modules --dirs-only --limit 10 --format custom --max-path-length 90 --sep $([char]9) --columns Path"
Enter
Set TypingSpeed 1ms
Sleep 3s
Type "Clear-Host"
Enter

# 4) Recent work

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

# 5) Pattern search

Type "# Date-stamped exports using a regex query"
Enter
Set TypingSpeed 45ms
Sleep 2s
Type "uffs.exe '>[0-9]{4}-[0-9]{2}-[0-9]{2}' --ext log,csv --files-only --limit 10 --format custom --sep $([char]9) --columns Size,Name"
Enter
Set TypingSpeed 1ms
Sleep 3s
Type "Clear-Host"
Enter

# 6) Drive overview

Type "# Aggregate storage footprint by drive"
Enter
Set TypingSpeed 45ms
Sleep 2s
Type "uffs.exe agg by_drive"
Enter
Set TypingSpeed 1ms
Sleep 3s
Type "Clear-Host"
Enter

# 7) Content overview

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

# 8) Size profile

Type "# Distribution of file sizes across the index"
Enter
Set TypingSpeed 45ms
Sleep 2s
Type "uffs.exe agg by_size"
Enter
Set TypingSpeed 1ms
Sleep 3s
Type "Clear-Host"
Enter

# 9) Daemon status

Type "# Drive residency tiers and daemon telemetry"
Enter
Set TypingSpeed 45ms
Sleep 2s
Type "uffs.exe daemon status_drives"
Enter
Set TypingSpeed 1ms
Sleep 3s
Type "Clear-Host"
Enter
