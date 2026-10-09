<#
.SYNOPSIS
    Builds the firmware and flashes it over USB to the ESP32-C6-LCD-1.3 board.

.DESCRIPTION
    The board is connected with a USB-C cable. The ESP32-C6 has a built-in USB-Serial-JTAG,
    Windows 11 has a driver for it, nothing else needs to be installed.

    After flashing (unless -NoMonitor is given) a serial console with the board's logs
    opens. Quit with Ctrl+C.

.PARAMETER Port
    COM port of the board, e.g. COM5. If omitted, the script lists all COM
    ports and tries the Espressif ports (USB VID 303A); other ports only
    when no Espressif port is found.

.PARAMETER NoMonitor
    Do not open the serial console after flashing.

.PARAMETER SkipBuild
    Do not build; flash the last built firmware.

.PARAMETER FlashMode
    SPI flash mode written to the image header: qio, qout, dio, dout.
    Defaults to the espflash default (dio). Note: the ESP32-C6-LCD-1.3 board does not
    support QIO (the firmware will not start) – use dio only.

.PARAMETER FlashFreq
    SPI flash frequency, e.g. 80mhz. Defaults to the espflash default (40mhz).

.EXAMPLE
    .\scripts\flash.ps1
.EXAMPLE
    .\scripts\flash.ps1 -Port COM5 -NoMonitor
#>
param(
    [string]$Port,
    [switch]$NoMonitor,
    [switch]$SkipBuild,
    [string]$FlashMode,
    [string]$FlashFreq
)
. "$PSScriptRoot\common.ps1"

Assert-Command espflash 'Run .\scripts\setup.ps1 first.'

if (-not $SkipBuild) {
    & "$PSScriptRoot\build.ps1"
}
if (-not (Test-Path $FirmwareElf)) {
    throw "Firmware $FirmwareElf does not exist – run the script without -SkipBuild."
}

Write-Step 'Looking for the board'
$candidates = @(Get-CandidatePorts $Port)

# Custom partition table: the application (with NNUE weights) is larger than the default 1 MB partition.
# Flash without the monitor – the console is opened later on the port where flashing succeeded.
$baseArgs = @('flash', '--chip', $Chip, '--partition-table', $PartitionCsv)
if ($FlashMode) { $baseArgs += @('--flash-mode', $FlashMode) }
if ($FlashFreq) { $baseArgs += @('--flash-freq', $FlashFreq) }

$flashedPort = $null
if ($candidates.Count -eq 0) {
    # No port in the list – let espflash search by itself.
    Write-Step 'Flashing firmware (espflash picks the port)'
    try {
        Invoke-Native espflash ($baseArgs + $FirmwareElf)
        $flashedPort = ''
    }
    catch { Write-Host "   Failed: $($_.Exception.Message)" -ForegroundColor Red }
}
foreach ($candidate in $candidates) {
    Write-Step "Flashing firmware via $candidate"
    try {
        Invoke-Native espflash ($baseArgs + @('--port', $candidate, $FirmwareElf))
        $flashedPort = $candidate
        break
    }
    catch {
        Write-Host "   Failed via ${candidate}: $($_.Exception.Message)" -ForegroundColor Yellow
    }
}

if ($null -eq $flashedPort) {
    Write-Host ''
    Write-Host 'Flashing failed on all ports. Try:' -ForegroundColor Red
    Write-Host '  1) bootloader mode: hold BOOT, press and release RST, release BOOT – and run the script again'
    Write-Host '     (needed when the running firmware crashes and USB keeps disconnecting),'
    Write-Host '  2) close programs that have the COM port open (monitor.ps1, Arduino IDE, PuTTY, ...),'
    Write-Host '  3) another USB-C cable (it must carry data) or another USB port on the computer,'
    Write-Host '  4) a specific port: .\scripts\flash.ps1 -Port COM7'
    throw 'Flashing the firmware failed.'
}

Write-Step 'Flashed'
if (-not $NoMonitor) {
    Write-Step 'Serial console (Ctrl+C = quit)'
    $monitorArgs = @('monitor', '--chip', $Chip, '--elf', $FirmwareElf)
    if ($flashedPort) { $monitorArgs += @('--port', $flashedPort) }
    Invoke-Native espflash $monitorArgs
}
