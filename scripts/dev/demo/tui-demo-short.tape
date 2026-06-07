Set Shell "powershell"
Set FontSize 20
Set Width 1280
Set Height 720
Set Padding 24
Set Theme "Catppuccin Mocha"
Set TypingSpeed 45ms
Set PlaybackSpeed 1.0

Output uffs-tui-short.gif

# UltraFastFileSearch (UFFS)
# Benchmark-honest demo.
#
# Recorded against the real binary.
# No edited timings, no synthetic results.
#
# Demo environment:
#   - Hot daemon (pre-warmed)
#   - Real filesystem data
#   - Demo edition TUI
#
# The demo edition limits result counts and disables exports.

Set TypingSpeed 1ms
Type "Clear-Host"
Enter

Set TypingSpeed 1ms
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
Type "# Hot daemon • Real drives • Real timings"
Enter
Type "# 25M+ files searchable in seconds"
Enter

Sleep 5s

Type "Clear-Host"
Enter

Type "# Story: unzip, run, search"
Enter
Type "# No indexing wait. No pre-generated demo data."
Enter
Enter

Sleep 2s

Type "# Launching the TUI against a hot daemon"
Enter
Type "# Real result counts and measured latency remain visible"
Enter

Sleep 4s

Set TypingSpeed 45ms
Type "uffs_tui.exe --keys windows --reset-history"
Enter

Sleep 3500ms

# Search interaction

Set TypingSpeed 300ms
Type "Invoice"
Sleep 2s
Tab
Sleep 2s
Tab

Set TypingSpeed 1s
Escape
PageDown 2
Down 2
PageUp 2
Escape

# Browse previously executed searches

Set TypingSpeed 2s
Down 5

# Exit the application

Ctrl+Q