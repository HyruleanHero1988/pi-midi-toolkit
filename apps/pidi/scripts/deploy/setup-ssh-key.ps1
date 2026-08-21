# One-time: copy your Windows SSH public key to the Pi (password once).
# After this, scp/ssh and deploy-to-pi.ps1 work without typing a password.
#
# Usage (PowerShell on PC):
#   .\tools\midi-tone\setup-ssh-key.ps1
#   .\tools\midi-tone\setup-ssh-key.ps1 -PiHost ray@192.168.1.225

param(
    [string]$PiHost = "ray@192.168.1.225"
)

$ErrorActionPreference = "Stop"
$Key = Join-Path $env:USERPROFILE ".ssh\id_ed25519"
$Pub = "$Key.pub"

if (-not (Test-Path $Pub)) {
    Write-Host "Creating ed25519 key at $Key ..."
    ssh-keygen -t ed25519 -f $Key -N '""'
}

Write-Host "Copying public key to $PiHost (enter Pi password once)..."
type $Pub | ssh $PiHost "mkdir -p ~/.ssh && chmod 700 ~/.ssh && cat >> ~/.ssh/authorized_keys && chmod 600 ~/.ssh/authorized_keys"
Write-Host "Testing passwordless SSH..."
ssh -o BatchMode=yes $PiHost "echo OK from $(hostname)"
Write-Host "All set. Deploy with: .\tools\midi-tone\deploy-to-pi.ps1 -Restart"
