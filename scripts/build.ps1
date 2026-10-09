<#
.SYNOPSIS
    Builds the firmware for the ESP32-C6 (release = optimised build).

.EXAMPLE
    .\scripts\build.ps1
#>
. "$PSScriptRoot\common.ps1"

Assert-Command cargo 'Run .\scripts\setup.ps1 first.'

Write-Step 'Building firmware (release)'
# The target (riscv32imac-unknown-none-elf) is set in firmware\.cargo\config.toml,
# so cargo must be run from the firmware directory.
Invoke-Native cargo @('build', '--release') -WorkDir $FirmwareDir

if (-not (Test-Path $FirmwareElf)) {
    throw "The build succeeded, but $FirmwareElf was not found."
}
# Memory check: the main-thread stack must be large enough for Wi-Fi initialisation (tools/elf-size).
Push-Location $ProjectRoot
try { $sizeReport = & cargo run -q -p elf-size -- $FirmwareElf 2>&1 } finally { Pop-Location }
$sizeReport | Select-Object -Last 3 | ForEach-Object { Write-Host "   $_" }
if ($sizeReport -match 'WARNING') {
    throw 'The main-thread stack is too small – the firmware would crash when Wi-Fi starts (see the report above).'
}
Write-Step "Done: $FirmwareElf"
