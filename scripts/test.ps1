<#
.SYNOPSIS
    Runs all automated checks of the project.

.DESCRIPTION
    1. Unit and integration tests of the libraries on the PC (rules, engine, hub, display, web).
    2. Clippy with warnings as errors (libraries, simulator, firmware).
    3. Formatting check (cargo fmt --check).
    4. Playwright tests of the web UI against the board simulator (desktop + mobile).

    The script stops with a non-zero exit code at the first failure.

.PARAMETER SkipWeb
    Skip the Playwright tests (faster check of the Rust code only).

.EXAMPLE
    .\scripts\test.ps1
.EXAMPLE
    .\scripts\test.ps1 -SkipWeb
#>
param(
    [switch]$SkipWeb
)
. "$PSScriptRoot\common.ps1"

Assert-Command cargo 'Run .\scripts\setup.ps1 first.'

Write-Step 'Library tests (release – the engine is slow without optimisations)'
Invoke-Native cargo @('test', '--workspace', '--release')

Write-Step 'Clippy (libraries, simulator, tools)'
Invoke-Native cargo @('clippy', '--workspace', '--all-targets', '--', '-D', 'warnings')

Write-Step 'Clippy (firmware)'
Invoke-Native cargo @('clippy', '--release', '--', '-D', 'warnings') -WorkDir $FirmwareDir

Write-Step 'Formatting check'
Invoke-Native cargo @('fmt', '--all', '--check')
Invoke-Native cargo @('fmt', '--check') -WorkDir $FirmwareDir

if (-not $SkipWeb) {
    Assert-Command npx 'Install Node.js (winget install OpenJS.NodeJS.LTS) and run .\scripts\setup.ps1'
    Write-Step 'Playwright web tests (starts the simulator on port 8765)'
    Invoke-Native npx @('playwright', 'test') -WorkDir $WebDir
}

Write-Step 'All checks passed'
