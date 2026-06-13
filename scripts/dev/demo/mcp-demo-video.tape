# Intentionally no: Set Shell "pwsh"
# VHS on Windows starts cmd.exe by default.
# We manually enter pwsh below to avoid VHS's PowerShell launcher issue.

Set FontSize 20
Set Width 1920
Set Height 1080
Set Padding 24
Set Theme "Catppuccin Mocha"
Set TypingSpeed 45ms
Set PlaybackSpeed 1.0

Output uffs-mcp-claude-video.mp4

Hide
Type "pwsh -NoLogo -NoProfile"
Enter
Sleep 1500ms
Type "function global:prompt { '>' }"
Enter
Show


# UltraFastFileSearch (UFFS)
# Benchmark-honest MCP + Claude demo — VIDEO cut.
#
# Same shot list and commands as mcp-demo.tape; only the static card
# holds are tightened for feed video (YouTube / LinkedIn / X), where
# the first payoff must land within ~4 seconds.  The GIF variant keeps
# the longer holds for page embeds.
#
# NOTE: the two `claude` invocations keep their full 3 s post-Enter
# holds — that time covers Claude's real response latency, which is
# part of the honest demo, not a static card.
#
# Recorded against the real CLI and real MCP HTTP endpoint.
# Every command shown is executed as recorded.
#
# Demo environment:
#   - Hot daemon (pre-warmed)
#   - MCP HTTP server on 127.0.0.1:8080
#   - Claude connected through MCP
#   - Real filesystem data
#   - Real result counts
#   - Real measured latency
#
# No edited timings. No synthetic datasets.
#
# Prereqs:
#   - uffs.exe and uffsd.exe on PATH
#   - claude CLI installed and signed in
#   - daemon warmed before recording:
#       rust-script scripts/windows/record-demo-prep.rs --mode hot
#   - MCP HTTP server available:
#       uffs mcp start
#   - VHS + ffmpeg installed

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
Type "# Claude + UFFS over local HTTP"
Enter
Type "# Hot daemon - Real tools - Real timings"
Enter

Sleep 2s

Type "Clear-Host"
Enter

# Keep this clip short and readable.
# The goal is to show:
#   1. MCP is local and explicit.
#   2. Claude can connect with one command.
#   3. The server exposes read-only filesystem search tools.
#   4. Claude can ask UFFS questions instead of crawling the disk.
#
# Result volume is kept small for recording readability.
# Commands, counts, and measured timings remain real.

# 1) Start from a clean, explicit MCP status check

Type "# Local MCP gateway: hot daemon, HTTP transport"
Enter
Set TypingSpeed 45ms
Sleep 1200ms
Type "uffs.exe status"
Enter
Set TypingSpeed 1ms
Sleep 1200ms
Type "Clear-Host"
Enter

# 2) Add the UFFS MCP service to Claude

Type "# Add UFFS as a Claude MCP service"
Enter
Set TypingSpeed 45ms
Sleep 1200ms
Type "claude mcp add --transport http uffs http://127.0.0.1:8080/mcp"
Enter
Set TypingSpeed 1ms
Sleep 1200ms
Type "Clear-Host"
Enter

# 3) Verify Claude can see the MCP service

Type "# Verify Claude sees the local UFFS MCP server"
Enter
Set TypingSpeed 45ms
Sleep 1200ms
Type "claude mcp list"
Enter
Set TypingSpeed 1ms
Sleep 1200ms
Type "Clear-Host"
Enter

# 4) Show the MCP server-side view before asking Claude

Type "# UFFS MCP exposes read-only search, aggregate, drive, info and status tools"
Enter
Set TypingSpeed 45ms
Sleep 1200ms
Type "uffs.exe mcp stats"
Enter
Set TypingSpeed 1ms
Sleep 1200ms
Type "Clear-Host"
Enter

# 5) Ask Claude for a small, concrete filesystem answer via MCP

Type "# Ask Claude to use UFFS instead of crawling the disk"
Enter
Set TypingSpeed 45ms
Sleep 2s
Type "claude 'Using the uffs tools, find the 8 largest files on my machine that I have not modified in over a year. Keep it compact, and cite counts/query time the tool reports.'"
Enter
Set TypingSpeed 1ms
Sleep 35s

Type "exit"
Enter
Sleep 1s
Type "Clear-Host"
Enter
Sleep 1s

# 7) Close with the operational takeaway
Set TypingSpeed 20ms
Type "# MCP takeaway"
Enter
Type "# Claude gets fast, read-only filesystem tools through UFFS"
Enter
Type "# The daemon keeps the index hot; MCP stays a thin bridge"
Enter
Sleep 2500ms

Type "Clear-Host"
Enter
