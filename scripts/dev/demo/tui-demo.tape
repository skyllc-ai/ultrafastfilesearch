# UFFS TUI quick-start demo — VHS tape SKELETON (charmbracelet/vhs)
# Render:  vhs scripts/dev/demo/tui-demo.tape   ->  uffs-tui.gif
#
# STATUS: skeleton. The bundled `uffs-tui` is the free demo binary built in the
# separate githubrobbi/uffs-demo repo, so its exact keybindings are not vendored
# here. Fill in the `# TODO` keystrokes ONCE (confirm against the running TUI),
# then this clip is reproducible too.
#
# If filling keybindings is fiddly, just record the TUI interactively with
# ScreenToGif using the shot list in README.md — that is the supported path.
#
# Prereqs:
#   - `uffs-tui` (and `uffs`/`uffsd`) on PATH, ideally from an unzipped release
#   - daemon warmed:  pwsh -File scripts/dev/demo/record_demo_prep.ps1 -Mode hot
#
# Honesty: the demo TUI caps result counts and disables exports (DEMO-LICENSE.txt).
# Don't imply uncapped/export behaviour in this clip.

Output uffs-tui.gif

Set Shell "pwsh"
Set FontSize 18
Set Width 1280
Set Height 720
Set Padding 20
Set Theme "Catppuccin Mocha"
Set TypingSpeed 45ms

# Story: "unzip -> run -> browsing your real drives in seconds"
Hide
Type "clear" Enter
Show
Sleep 600ms

# Launch the TUI; the daemon auto-starts and the UI populates with real drives
Type "uffs-tui"
Sleep 400ms
Enter
Sleep 3500ms          # let the daemon attach + the UI render

# TODO: type a query into the TUI's search field (confirm focus behaviour)
Type "*.pdf"
Sleep 2500ms          # results filter live

# TODO: navigate results — replace with the demo TUI's real keys
Down
Down
Sleep 1500ms

# TODO: optionally toggle a filter / panel here

# TODO: quit cleanly — replace with the demo TUI's real quit key (often q or Ctrl+C)
Type "q"
Sleep 1500ms
