# Deploy midi-tone to the Pi over SSH (key auth — no password in scripts).
# Usage:
#   .\tools\midi-tone\deploy-to-pi.ps1
#   .\tools\midi-tone\deploy-to-pi.ps1 -PiHost ray@192.168.1.225
#   .\tools\midi-tone\deploy-to-pi.ps1 -Restart

param(
    [string]$PiHost = "ray@192.168.1.225",
    [switch]$Restart
)

$ErrorActionPreference = "Stop"
$Here = Split-Path -Parent $MyInvocation.MyCommand.Path

Write-Host "Deploying midi-tone -> ${PiHost}:~/midi-tone/"
scp -r `
    "$Here\midi_tone.py" `
    "$Here\requirements.txt" `
    "$Here\run.sh" `
    "$Here\setup-venv.sh" `
    "$Here\install-desktop-shortcut.sh" `
    "$Here\midi-tone.desktop" `
    "$Here\README.md" `
    "${PiHost}:~/midi-tone/"

ssh $PiHost "sed -i 's/\r`$//' ~/midi-tone/*.sh ~/midi-tone/*.desktop 2>/dev/null; chmod +x ~/midi-tone/*.sh"

if ($Restart) {
    Write-Host "Restarting midi-tone on Pi display..."
    # Kill previous instance; start fresh on :0
    ssh $PiHost @"
pkill -f 'midi_tone.py' 2>/dev/null || true
export DISPLAY=:0
export XDG_RUNTIME_DIR=/run/user/\$(id -u)
cd ~/midi-tone
nohup ./run.sh --input MPK > /tmp/midi-tone.log 2>&1 &
echo started pid \$!
"@
}

Write-Host "Done."
