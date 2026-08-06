# Deploy midi-engine to a Raspberry Pi over SSH (Windows PowerShell).
# Usage:
#   .\deploy\deploy.ps1 -PiHost pi@192.168.1.50
#   .\deploy\deploy.ps1 -PiHost pi@192.168.1.50 -Target armv7-unknown-linux-gnueabihf
param(
    [Parameter(Mandatory = $true)]
    [string]$PiHost,
    [string]$Target = "",
    [string]$RemoteDir = "/home/pi/pi-midi-toolkit"
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Set-Location $Root

if ($env:Path -notlike "*\.cargo\bin*") {
    $env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
}

if ($Target) {
    Write-Host "cross-building for $Target ..."
    cargo build --release -p midi-engine --target $Target
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
    $Bin = Join-Path $Root "target\$Target\release\midi-engine"
} else {
    Write-Host "building host release (pass -Target for Pi) ..."
    cargo build --release -p midi-engine
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
    $Bin = Join-Path $Root "target\release\midi-engine"
}

if (-not (Test-Path $Bin)) {
    throw "binary not found: $Bin (cross-compile needs a linker; see .cargo/config.toml.example)"
}

ssh $PiHost "mkdir -p '$RemoteDir/bin' '$RemoteDir/presets'"
scp $Bin "${PiHost}:$RemoteDir/bin/midi-engine"
scp (Join-Path $Root "presets\example.json") "${PiHost}:$RemoteDir/presets/"
scp (Join-Path $Root "presets\mpk-mini-ch3.json") "${PiHost}:$RemoteDir/presets/"
scp (Join-Path $Root "deploy\midi-engine.service") "${PiHost}:/tmp/midi-engine.service"

$remoteCmd = @"
set -euo pipefail
sudo mv /tmp/midi-engine.service /etc/systemd/system/midi-engine.service
sudo systemctl daemon-reload
if [ ! -f '$RemoteDir/presets/active.json' ]; then
  if [ -f '$RemoteDir/presets/mpk-mini-ch3.json' ]; then
    cp '$RemoteDir/presets/mpk-mini-ch3.json' '$RemoteDir/presets/active.json'
  else
    cp '$RemoteDir/presets/example.json' '$RemoteDir/presets/active.json'
  fi
fi
chmod +x '$RemoteDir/bin/midi-engine'
sudo systemctl restart midi-engine || true
sudo systemctl status midi-engine --no-pager || true
"@

$remoteCmd | ssh $PiHost "bash -s"
Write-Host "deployed to ${PiHost}:$RemoteDir"
