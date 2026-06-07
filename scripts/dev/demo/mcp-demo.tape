Set Shell "powershell"
Set FontSize 20
Set Width 1280
Set Height 720
Set Padding 24
Set Theme "Catppuccin Mocha"
Set TypingSpeed 45ms
Set PlaybackSpeed 1.0

Output uffs-mcp-claude.gif

# UltraFastFileSearch (UFFS)
# Benchmark-honest MCP + Claude demo.
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

Sleep 5s

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
Sleep 2s
Type "uffs.exe status"
Enter
Set TypingSpeed 1ms
Sleep 3s
Type "Clear-Host"
Enter

# 2) Add the UFFS MCP service to Claude

Type "# Add UFFS as a Claude MCP service"
Enter
Set TypingSpeed 45ms
Sleep 2s
Type "claude mcp add --transport http uffs http://127.0.0.1:8080/mcp"
Enter
Set TypingSpeed 1ms
Sleep 3s
Type "Clear-Host"
Enter

# 3) Verify Claude can see the MCP service

Type "# Verify Claude sees the local UFFS MCP server"
Enter
Set TypingSpeed 45ms
Sleep 2s
Type "claude mcp list"
Enter
Set TypingSpeed 1ms
Sleep 2s
Type "Clear-Host"
Enter

# 4) Show the MCP server-side view before asking Claude

Type "# UFFS MCP exposes read-only search, aggregate, drive, info and status tools"
Enter
Set TypingSpeed 45ms
Sleep 2s
Type "uffs.exe mcp stats"
Enter
Set TypingSpeed 1ms
Sleep 3s
Type "Clear-Host"
Enter

# 5) Ask Claude for a small, concrete filesystem answer via MCP

Type "# Ask Claude to use UFFS instead of crawling the disk"
Enter
Set TypingSpeed 45ms
Sleep 2s
Type "claude 'Using the uffs MCP server, list the indexed drives and then find 5 invoice files. Keep the answer compact and include counts/timings if the tool returns them.'"
Enter
Set TypingSpeed 1ms
Sleep 3s
Type "Clear-Host"
Enter

# 6) Ask for a storage summary via aggregation

Type "# Ask for a server-side storage summary"
Enter
Set TypingSpeed 45ms
Sleep 2s
Type "claude 'Using the uffs MCP server, summarize storage by file type. Return only the top 8 rows.'"
Enter
Set TypingSpeed 1ms
Sleep 3s
Type "exit"
Enter
Type "Clear-Host"
Enter

# 7) Close with the operational takeaway

Type "# MCP takeaway"
Enter
Type "# Claude gets fast, read-only filesystem tools through UFFS"
Enter
Type "# The daemon keeps the index hot; MCP stays a thin bridge"
Enter
Sleep 5s

Type "Clear-Host"
Enter
