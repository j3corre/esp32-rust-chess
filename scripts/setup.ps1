<#
.SYNOPSIS
    One-time PC setup (Windows 11) for building and flashing the firmware.

.DESCRIPTION
    1. Checks that Rust is installed (rustup + cargo). If not, tries to
       install it with winget.
    2. Adds the Rust target for the ESP32-C6 (riscv32imac-unknown-none-elf) and rust-src.
    3. Installs espflash for flashing over USB.
    4. Installs Playwright (npm) and the Chromium browser for the web tests.

    The script can safely be run repeatedly – whatever is already installed is skipped.

.EXAMPLE
    .\scripts\setup.ps1
#>
. "$PSScriptRoot\common.ps1"

# --- 1) Rust ------------------------------------------------------------------
Write-Step 'Checking the Rust installation'
if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
    if (Get-Command winget -ErrorAction SilentlyContinue) {
        Write-Host '   Rust is not installed – installing via winget (Rustlang.Rustup)...'
        Invoke-Native winget @('install', '--id', 'Rustlang.Rustup', '-e', '--accept-source-agreements', '--accept-package-agreements')
        # Newly installed programs are not on the PATH of the current window – add them.
        $env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
    }
    else {
        throw 'Rust is not installed. Download the installer from https://rustup.rs and run the script again.'
    }
}
Assert-Command cargo 'Open a new PowerShell window (for the PATH) and run the script again.'
Invoke-Native rustc @('--version')
Invoke-Native cargo @('--version')

# Helper programs (build.rs, macros) are compiled for Windows and need the
# linker from the Visual Studio Build Tools. If it is missing, the build fails with
# a message about 'link.exe' – then install the "Visual Studio Build Tools" with the
# "Desktop development with C++" workload (winget install Microsoft.VisualStudio.2022.BuildTools).

# --- 2) Target for the ESP32-C6 -----------------------------------------------
Write-Step "Adding Rust target $RustTarget"
Invoke-Native rustup @('target', 'add', $RustTarget)
Invoke-Native rustup @('component', 'add', 'rust-src', 'clippy', 'rustfmt')

# --- 3) espflash --------------------------------------------------------------
Write-Step 'Installing espflash (flashing firmware over USB)'
if (Get-Command espflash -ErrorAction SilentlyContinue) {
    Write-Host '   espflash is already installed.'
}
elseif (Get-Command cargo-binstall -ErrorAction SilentlyContinue) {
    # cargo-binstall downloads a prebuilt binary – much faster than compiling.
    Invoke-Native cargo @('binstall', '-y', 'espflash')
}
else {
    # Build from source – may take several minutes.
    Invoke-Native cargo @('install', 'espflash', '--locked')
}
Invoke-Native espflash @('--version')

# --- 4) Playwright (web UI tests) ---------------------------------------------
Write-Step 'Playwright for the web tests'
if (Get-Command npm -ErrorAction SilentlyContinue) {
    Invoke-Native npm @('install', '--no-audit', '--no-fund') -WorkDir $WebDir
    Invoke-Native npx @('playwright', 'install', 'chromium') -WorkDir $WebDir
}
else {
    Write-Host '   Node.js/npm not found – the Playwright tests will not run.' -ForegroundColor Yellow
    Write-Host '   Install: winget install OpenJS.NodeJS.LTS  (then run setup.ps1 again)' -ForegroundColor Yellow
}

Write-Step 'Done'
Write-Host '   Next steps:'
Write-Host '     .\scripts\test.ps1     – all tests (Rust + Playwright)'
Write-Host '     .\scripts\sim.ps1      – development server on the PC (http://localhost:8080)'
Write-Host '     .\scripts\flash.ps1    – build and flash to the board'
