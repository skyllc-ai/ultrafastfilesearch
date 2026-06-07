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

Output uffs-mcp-claude.gif

Hide
Type "pwsh -NoLogo -NoProfile"
Enter
Sleep 1500ms
Type "function global:prompt { '>' }"
Enter
Show

# UltraFastFileSearch (UFFS)
# Crisp MCP + Claude demo.
#
# Story:
#   1. UFFS is already hot.
#   2. Add it to Claude over local MCP HTTP.
#   3. Ask Claude real filesystem questions.
#
# Recorded against real CLI, real MCP endpoint, real filesystem data.
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
Type "# UltraFastFileSearch MCP"
Enter
Type "# Search NTFS at machine speed"
Enter
Type "# Talk to your filesystem through Claude"
Enter

Sleep 5s

Type "Clear-Host"
Enter

# 3) Ask one concrete agent workflow question

Type "# Ask Claude a filesystem question"
Enter
Sleep 2s
Set TypingSpeed 45ms
Type "claude 'Using the uffs MCP server, find the 5 largest video files modified between 2020 and 2024. Keep the answer compact.'"
Enter
Set TypingSpeed 1ms
Sleep 20s

# 4) Show aggregation, not just search

Set TypingSpeed 45ms
Type "Using the uffs MCP server, summarize storage by file type. Return only the top 4 rows."
Enter
Set TypingSpeed 1ms
Sleep 15s
Type "exit"
Enter
Sleep 3s
Type "Clear-Host"
Enter

# 5) Close with the positioning

Type "# UFFS MCP"
Enter
Type "# Fast local retrieval for humans, scripts and AI agents"
Enter
Type "# Deterministic search underneath. MCP on top."
Enter

Sleep 5s

Type "Clear-Host"
Enter