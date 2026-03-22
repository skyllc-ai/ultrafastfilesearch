# DEPRECATED: Use benchmark.ps1 -Cache instead
# This wrapper forwards all arguments to the unified benchmark script.
#
# Usage:
#   .\benchmark.ps1 -N 5 -Drive C,D,E,F,G,M,S -Cache     # warm / cached
#   .\benchmark.ps1 -N 5 -Drive C,D,E,F,G,M,S             # cold (default)

param(
    [int]$N = 5,
    [string]$Pattern = "*",
    [string[]]$Drive = @(),
    [switch]$RustOnly,
    [switch]$CppOnly,
    [switch]$NoAll
)

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$args = @('-N', $N, '-Pattern', $Pattern, '-Cache')
if ($Drive.Count -gt 0)  { $args += @('-Drive') + $Drive }
if ($RustOnly)           { $args += '-RustOnly' }
if ($CppOnly)            { $args += '-CppOnly' }
if ($NoAll)              { $args += '-NoAll' }

& "$scriptDir\benchmark.ps1" @args

