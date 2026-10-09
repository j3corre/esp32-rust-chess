<#
.SYNOPSIS
    Opens the serial console of the board (firmware logs). Quit with Ctrl+C.

.PARAMETER Port
    COM port of the board, e.g. COM5. If omitted, the script tries to find it.

.EXAMPLE
    .\scripts\monitor.ps1
#>
param(
    [string]$Port
)
. "$PSScriptRoot\common.ps1"

Assert-Command espflash 'Run .\scripts\setup.ps1 first.'

Write-Step 'Looking for the board'
$resolvedPort = Resolve-EspPort $Port

Write-Step 'Serial console (Ctrl+C = quit)'
$monitorArgs = @('monitor', '--chip', $Chip)
if ($resolvedPort) { $monitorArgs += @('--port', $resolvedPort) }
# The built ELF lets espflash translate panic addresses into function names.
if (Test-Path $FirmwareElf) { $monitorArgs += @('--elf', $FirmwareElf) }

Invoke-Native espflash $monitorArgs
