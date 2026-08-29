#Requires -Version 5.1
<#
.SYNOPSIS
  Cross-build Pi armv7 bins on this PC with a Bookworm-safe glibc floor.

.DESCRIPTION
  Pi Bookworm has glibc 2.36. Building inside the default Debian/Ubuntu-24.04
  WSL produces bins that need 2.38+ and will not start on the device.

  This wrapper prefers (in order):
    1. WSL distro Ubuntu-22.04 (glibc 2.35 - same as GitHub Actions)
    2. Docker image from deploy/Dockerfile.pi-bins (ubuntu:22.04)

  Usage:
    .\deploy\build-pi-bins.ps1
    .\deploy\build-pi-bins.ps1 -Packages "jambox-engine,pidi-native"
    .\deploy\build-pi-bins.ps1 -InstallDistro   # install Ubuntu-22.04 if missing
#>
param(
    [string]$Packages = "midi-engine,jambox-engine,pidi-native",
    [switch]$InstallDistro,
    [switch]$PreferDocker
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Set-Location $Root

function Write-Step([string]$msg) {
    Write-Host "build-pi-bins: $msg" -ForegroundColor Cyan
}

function Test-Command([string]$Name) {
    return [bool](Get-Command $Name -ErrorAction SilentlyContinue)
}

function Get-WslDistros {
    # wsl -l -q emits UTF-16; normalize to plain lines.
    $raw = & wsl.exe -l -q 2>$null
    if (-not $raw) { return @() }
    $text = if ($raw -is [array]) { ($raw -join "`n") } else { [string]$raw }
    # Strip NULs from UTF-16 mis-decoded as ANSI
    $text = $text -replace "`0", ""
    return @(
        $text -split "(`r`n|`n)" |
            ForEach-Object { $_.Trim() } |
            Where-Object { $_ -ne "" }
    )
}

function Find-Ubuntu2204 {
    $names = Get-WslDistros
    foreach ($n in $names) {
        if ($n -match '^(Ubuntu-22\.04|Ubuntu2204|Ubuntu_22\.04)$') {
            return $n
        }
    }
    # Friendly name variants
    foreach ($n in $names) {
        if ($n -match '22\.04') { return $n }
    }
    return $null
}

function ConvertTo-WslPath([string]$WinPath) {
    $full = (Resolve-Path $WinPath).Path
    if ($full -match '^([A-Za-z]):\\(.*)$') {
        $drive = $Matches[1].ToLowerInvariant()
        $rest = $Matches[2] -replace '\\', '/'
        return "/mnt/$drive/$rest"
    }
    throw "cannot map Windows path to WSL: $WinPath"
}

function Ensure-Ubuntu2204 {
    $existing = Find-Ubuntu2204
    if ($existing) { return $existing }

    if (-not $InstallDistro) {
        Write-Host @"
No Ubuntu-22.04 WSL distro found (needed for Pi-compatible glibc).

Install once, then re-run this script:

  wsl --install -d Ubuntu-22.04

Or re-run with -InstallDistro (may prompt for a new UNIX username on first launch):

  .\deploy\build-pi-bins.ps1 -InstallDistro

Docker alternative (if Docker Desktop is installed):

  .\deploy\build-pi-bins.ps1 -PreferDocker
"@ -ForegroundColor Yellow
        throw "Ubuntu-22.04 WSL missing"
    }

    Write-Step "installing Ubuntu-22.04 WSL (one-time)..."
    & wsl.exe --install -d Ubuntu-22.04 --no-launch
    if ($LASTEXITCODE -ne 0) {
        # Older WSL may not support --no-launch
        & wsl.exe --install -d Ubuntu-22.04
    }
    Start-Sleep -Seconds 3
    $existing = Find-Ubuntu2204
    if (-not $existing) {
        throw "Ubuntu-22.04 install finished but distro not listed. Open 'Ubuntu 22.04' once from the Start menu to finish setup, then re-run."
    }
    return $existing
}

function Invoke-WslBuild([string]$Distro) {
    $wslRoot = ConvertTo-WslPath $Root
    Write-Step "building inside WSL distro '$Distro' (Ubuntu 22.04 / glibc <= 2.36)"
    Write-Step "repo: $wslRoot"

    # Single-quoted here-string so PowerShell does not expand $HOME / $PATH.
    # Run as root so apt in build-pi-bins.sh works without a passwordless sudo user.
    # Keep cargo target dir on the Linux filesystem (/root/...) - /mnt/c is very slow.
    $bash = @'
set -euo pipefail
cd '__ROOT__'
export PI_BINS_OK_GLIBC=1
export PACKAGES='__PACKAGES__'
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/root/.cache/pi-midi-toolkit-target}"
mkdir -p "$CARGO_TARGET_DIR"
if ! command -v arm-linux-gnueabihf-gcc >/dev/null 2>&1; then
  echo 'build-pi-bins: bootstrapping Ubuntu 22.04 toolchain...'
  ./deploy/bootstrap-pi-bins-wsl.sh
fi
if ! command -v rustup >/dev/null 2>&1; then
  echo 'build-pi-bins: installing rustup...'
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal
fi
# shellcheck disable=SC1090
source "$HOME/.cargo/env" 2>/dev/null || true
export PATH="$HOME/.cargo/bin:$PATH"
rustup target add armv7-unknown-linux-gnueabihf
export SKIP_APT=1
./deploy/build-pi-bins.sh
'@
    $bash = $bash.Replace('__ROOT__', $wslRoot).Replace('__PACKAGES__', $Packages)
    # PowerShell here-strings are CRLF on Windows; bash rejects `set -o pipefail\r`.
    $bash = $bash -replace "`r`n", "`n" -replace "`r", "`n"

    $tmp = Join-Path $env:TEMP ("pi-bins-build-{0}.sh" -f [guid]::NewGuid().ToString('N'))
    $utf8 = New-Object System.Text.UTF8Encoding $false
    [IO.File]::WriteAllText($tmp, $bash, $utf8)
    try {
        $wslTmp = ConvertTo-WslPath $tmp
        & wsl.exe -d $Distro -u root -- bash $wslTmp
        if ($LASTEXITCODE -ne 0) { throw "WSL build failed (exit $LASTEXITCODE)" }
    } finally {
        Remove-Item -Force $tmp -ErrorAction SilentlyContinue
    }
}

function Invoke-DockerBuild {
    if (-not (Test-Command docker)) {
        throw "Docker not found on PATH. Install Docker Desktop or use Ubuntu-22.04 WSL."
    }
    $image = "pi-midi-toolkit-pi-bins:local"
    Write-Step "building Docker image $image from deploy/Dockerfile.pi-bins"
    & docker build -f deploy/Dockerfile.pi-bins -t $image .
    if ($LASTEXITCODE -ne 0) { throw "docker build failed" }

    # Map Windows path; Docker Desktop accepts Windows paths for -v.
    Write-Step "running cross-build in container"
    $vol = "${Root}:/src"
    & docker run --rm `
        -e "PACKAGES=$Packages" `
        -e "PI_BINS_OK_GLIBC=1" `
        -e "SKIP_APT=1" `
        -v $vol `
        -w /src `
        $image
    if ($LASTEXITCODE -ne 0) { throw "docker run build failed" }
}

# --- main ---
Write-Step "Pi Bookworm needs glibc <= 2.36; staging bins via Ubuntu 22.04 toolchain"

if ($PreferDocker) {
    Invoke-DockerBuild
} elseif (Test-Command wsl) {
    $distro = Ensure-Ubuntu2204
    Invoke-WslBuild -Distro $distro
} elseif (Test-Command docker) {
    Write-Step "WSL unavailable - falling back to Docker"
    Invoke-DockerBuild
} else {
    throw "Need either WSL (Ubuntu-22.04) or Docker Desktop to build Pi-compatible bins on this PC."
}

$stage = Join-Path $Root "dist\armv7"
Write-Step "staged:"
Get-ChildItem $stage -File | ForEach-Object {
    $size = $_.Length
    Write-Host ("  {0}  ({1} bytes)" -f $_.Name, $size)
}
Write-Host ""
Write-Host "Next: deploy with your usual script, or commit dist/armv7 for SET->UPDATE." -ForegroundColor Green
