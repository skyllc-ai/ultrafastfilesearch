#requires -Version 5.1
<#
.SYNOPSIS
    Put a Windows box into a known, HONEST state before recording a UFFS demo clip.

.DESCRIPTION
    The launch demo GIFs (see scripts/dev/demo/README.md) must show real,
    reproducible behaviour. This script warms (or, on request, cools) the UFFS
    daemon and prints the exact recorder settings so every capture is consistent
    and the on-screen latency matches docs/benchmarks/.

    It only calls DOCUMENTED `uffs` commands. It is non-destructive by default;
    the cold path (which deletes on-disk caches) is gated behind -ConfirmDestructive.

.PARAMETER Mode
    hot   (default) Restart the daemon and warm it so targeted queries answer from
                    memory. This is the "instant" story — caption your clip
                    "hot daemon, <N> records".
    cold            Show the cold MFT build path. DESTRUCTIVE: evicts caches for the
                    target drives so the next search rebuilds from raw MFT
                    (tens of seconds). Requires -ConfirmDestructive.

.PARAMETER Drives
    Drive letters to warm/preload (hot) or forget (cold). Default: C.

.PARAMETER ConfirmDestructive
    Required to actually run Mode=cold (which deletes on-disk caches).

.EXAMPLE
    pwsh -File scripts/dev/demo/record_demo_prep.ps1 -Mode hot -Drives C,D

.EXAMPLE
    pwsh -File scripts/dev/demo/record_demo_prep.ps1 -Mode cold -Drives D -ConfirmDestructive
#>
[CmdletBinding()]
param(
    [ValidateSet('hot', 'cold')]
    [string]$Mode = 'hot',

    [string[]]$Drives = @('C'),

    [switch]$ConfirmDestructive
)

$ErrorActionPreference = 'Stop'

function Write-Step($msg) { Write-Host "==> $msg" -ForegroundColor Cyan }
function Write-Warn($msg) { Write-Host "!!  $msg" -ForegroundColor Yellow }

# --- locate uffs ------------------------------------------------------------
$uffs = Get-Command uffs -ErrorAction SilentlyContinue
if (-not $uffs) {
    throw "'uffs' is not on PATH. Unzip a release and add its folder to PATH, then re-run."
}
Write-Step "Using uffs: $($uffs.Source)"
& uffs --version

# --- recorder settings (print, don't enforce) -------------------------------
Write-Host ""
Write-Step "Recommended recorder settings (keep every clip consistent):"
Write-Host @"
  Terminal      : Windows Terminal, single tab, no split panes
  Window size   : 1200 x 640 (CLI)  /  1280 x 720 (TUI)
  Font          : Cascadia Mono / Cascadia Code, size 18-20
  Theme         : dark, high-contrast (e.g. One Half Dark / Catppuccin Mocha)
  FPS / width   : 12-15 fps, export at 1200px wide, target < 3 MB GIF
  Recorder      : ScreenToGif (TUI + fallback) | VHS tapes (CLI, reproducible)
"@ -ForegroundColor DarkGray

# --- mode -------------------------------------------------------------------
switch ($Mode) {
    'hot' {
        Write-Host ""
        Write-Step "Mode=hot — warming the daemon so targeted queries answer from memory."
        Write-Step "Restarting daemon for a clean, known state..."
        & uffs daemon restart

        foreach ($d in $Drives) {
            Write-Step "Preloading drive $d and pinning it for the recording window..."
            & uffs daemon preload $d --pin-minutes 60
        }

        Write-Step "Priming the query path (these warm-up results are NOT part of the clip)..."
        & uffs "*.rs"            | Out-Null
        & uffs "*.dll" --drive ($Drives[0]) | Out-Null

        Write-Host ""
        Write-Step "Current tier / telemetry (this is the table the CLI clip shows):"
        & uffs daemon status_drives

        Write-Host ""
        Write-Host "READY (hot). Caption the clip as a HOT daemon over your real record count." -ForegroundColor Green
        Write-Warn "Honesty: do not edit frames to alter latency; the numbers on screen must stand."
    }

    'cold' {
        Write-Host ""
        Write-Warn "Mode=cold is DESTRUCTIVE: it evicts on-disk caches for: $($Drives -join ', ')"
        Write-Warn "The next search rebuilds from raw MFT (tens of seconds). Indexes are re-buildable, but this is not instant."
        if (-not $ConfirmDestructive) {
            throw "Refusing to run cold mode without -ConfirmDestructive. Re-run with that switch if you really want the cold-build clip."
        }
        foreach ($d in $Drives) {
            Write-Step "Forgetting drive $d (evict + delete on-disk caches)..."
            & uffs daemon forget $d --force
        }
        Write-Host ""
        Write-Host "READY (cold). Your next 'uffs' query will now show the COLD build path." -ForegroundColor Green
        Write-Warn "Label the clip COLD and show the full build time honestly."
    }
}

Write-Host ""
Write-Step "Next: render the CLI clip with  vhs scripts/dev/demo/cli-demo.tape"
Write-Step "      record the TUI clip per scripts/dev/demo/README.md (ScreenToGif)."
