<#
.SYNOPSIS
    Runs the board simulator on the PC: the same web, game logic and engine as the ESP32.

.DESCRIPTION
    Open http://localhost:8080/ in a browser (the simulator listens on
    localhost only). Quit with Ctrl+C.

.PARAMETER Port
    HTTP server port (default 8080).

.PARAMETER Fast
    The engine thinks 20× shorter (quick testing).

.PARAMETER Uci
    Runs the engine in UCI mode instead of the web (for Arena, cutechess-cli, …).

.EXAMPLE
    .\scripts\sim.ps1
.EXAMPLE
    .\scripts\sim.ps1 -Port 9000 -Fast
#>
param(
    [int]$Port = 8080,
    [switch]$Fast,
    [switch]$Uci
)
. "$PSScriptRoot\common.ps1"

Assert-Command cargo 'Run .\scripts\setup.ps1 first.'

$simArgs = @('run', '--release', '-p', 'chess-sim', '--')
if ($Uci) {
    $simArgs += '--uci'
}
else {
    $simArgs += @('--port', "$Port")
    if ($Fast) { $simArgs += '--fast' }
    Write-Step "Simulator: http://localhost:$Port/"
}
Invoke-Native cargo $simArgs
