# Shared functions and paths for the other scripts.
# Not run on its own – the other scripts load it with:  . "$PSScriptRoot\common.ps1"

# Stop the script immediately on any PowerShell command error.
$ErrorActionPreference = 'Stop'

# --- Project paths ------------------------------------------------------------
$ProjectRoot  = Split-Path -Parent $PSScriptRoot
$FirmwareDir  = Join-Path $ProjectRoot 'firmware'
$WebDir       = Join-Path $ProjectRoot 'web'
$PartitionCsv = Join-Path $FirmwareDir 'partitions.csv'
$RustTarget   = 'riscv32imac-unknown-none-elf'
$Chip         = 'esp32c6'
$FirmwareElf  = Join-Path $FirmwareDir "target\$RustTarget\release\esp32-chess"

# Espressif USB Vendor ID – used to recognise the board among the COM ports.
$EspressifVid = '303A'

# --- Helper functions ---------------------------------------------------------

# Prints a step heading (in green) so the output shows what is going on.
function Write-Step([string]$Message) {
    Write-Host ""
    Write-Host "==> $Message" -ForegroundColor Green
}

# Runs an external program (cargo, rustup, espflash…) and checks its exit code.
# PowerShell does not report errors of external programs itself, hence the $LASTEXITCODE check.
function Invoke-Native {
    param(
        [Parameter(Mandatory)] [string]$Exe,
        [string[]]$Arguments = @(),
        [string]$WorkDir = $ProjectRoot
    )
    Write-Host "   > $Exe $($Arguments -join ' ')" -ForegroundColor DarkGray
    Push-Location $WorkDir
    try {
        & $Exe @Arguments
        if ($LASTEXITCODE -ne 0) {
            throw "Command '$Exe $($Arguments -join ' ')' failed (exit code $LASTEXITCODE)."
        }
    }
    finally {
        Pop-Location
    }
}

# Checks that a program is on the PATH; otherwise stops with a hint on what to do.
function Assert-Command([string]$Name, [string]$Hint) {
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "Program '$Name' not found. $Hint"
    }
}

# Finds the COM ports backed by an Espressif chip (by USB VID 303A).
# Returns objects with the properties Port (e.g. COM5) and Name (device description).
function Get-EspPorts {
    Get-CimInstance -ClassName Win32_PnPEntity -ErrorAction SilentlyContinue |
        Where-Object { $_.PNPDeviceID -match "VID_$EspressifVid" -and $_.Name -match '\((COM\d+)\)' } |
        ForEach-Object {
            [void]($_.Name -match '\((COM\d+)\)')
            [pscustomobject]@{ Port = $Matches[1]; Name = $_.Name }
        }
}

# All COM ports in the system (other vendors too) – for listing and fallback attempts.
# Returns objects Port, Name, Espressif (true/false).
function Get-AllComPorts {
    Get-CimInstance -ClassName Win32_PnPEntity -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match '\((COM\d+)\)' } |
        ForEach-Object {
            [void]($_.Name -match '\((COM\d+)\)')
            [pscustomobject]@{
                Port      = $Matches[1]
                Name      = $_.Name
                Espressif = [bool]($_.PNPDeviceID -match "VID_$EspressifVid")
            }
        } |
        Sort-Object -Property @{ Expression = 'Espressif'; Descending = $true }, Port
}

# Ports to try: the one given by the user, otherwise the Espressif ports
# first and then all other COM ports. Also prints them.
function Get-CandidatePorts([string]$Port) {
    if ($Port) { return @($Port) }
    $all = @(Get-AllComPorts)
    if ($all.Count -eq 0) {
        Write-Host '   No COM port found. Is the board connected with a data USB-C cable?' -ForegroundColor Yellow
        return @()
    }
    Write-Host '   COM ports found:'
    foreach ($p in $all) {
        $tag = if ($p.Espressif) { '  <- Espressif (VID 303A)' } else { '' }
        Write-Host "     $($p.Port)  $($p.Name)$tag"
    }
    # If an Espressif port is found, only that one is tried (other ports are e.g. Bluetooth
    # and would just slow things down). Other ports are tried only when no Espressif port exists.
    $esp = @($all | Where-Object { $_.Espressif } | ForEach-Object { $_.Port })
    if ($esp.Count -gt 0) { return $esp }
    return @($all | ForEach-Object { $_.Port })
}

# Returns the port to use: the one given by the user, or the only Espressif port found.
# If none (or several) are found, returns $null and espflash asks by itself.
function Resolve-EspPort([string]$Port) {
    if ($Port) { return $Port }

    $ports = @(Get-EspPorts)
    if ($ports.Count -eq 1) {
        Write-Host "   Board found: $($ports[0].Name)" -ForegroundColor Cyan
        return $ports[0].Port
    }
    if ($ports.Count -gt 1) {
        Write-Host "   Several Espressif boards found – espflash will ask which one to use:" -ForegroundColor Yellow
        $ports | ForEach-Object { Write-Host "     $($_.Port)  $($_.Name)" }
    }
    else {
        Write-Host "   No board with USB VID $EspressifVid found – espflash will try to find the port itself." -ForegroundColor Yellow
        Write-Host "   (Is the board connected with a data USB-C cable?)" -ForegroundColor Yellow
    }
    return $null
}
