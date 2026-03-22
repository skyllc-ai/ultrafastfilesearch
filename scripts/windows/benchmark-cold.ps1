# DEPRECATED: Use benchmark.ps1 instead (cold mode is the default)
# This wrapper forwards all arguments to the unified benchmark script.
#
# Usage:
#   .\benchmark.ps1 -N 5 -Drive C,D,E,F,G,M,S           # cold (default)
#   .\benchmark.ps1 -N 5 -Drive C,D,E,F,G,M,S -Cache     # warm / cached

param(
    [int]$N = 3,
    [string]$Pattern = "*",
    [string[]]$Drive = @(),
    [switch]$RustOnly,
    [switch]$CppOnly,
    [switch]$NoAll
)

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$benchArgs = @{
    N       = $N
    Pattern = $Pattern
}
if ($Drive.Count -gt 0)  { $benchArgs['Drive']    = $Drive }
if ($RustOnly)           { $benchArgs['RustOnly'] = $true }
if ($CppOnly)            { $benchArgs['CppOnly']  = $true }
if ($NoAll)              { $benchArgs['NoAll']    = $true }

& "$scriptDir\benchmark.ps1" @benchArgs

