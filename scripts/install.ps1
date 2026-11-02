<#
.SYNOPSIS
  Per-user install of systui on Windows. No admin rights, no machine-wide changes.

.DESCRIPTION
  Builds the release binary with cargo (unless -Binary is given), copies it to
  $InstallDir (default: %USERPROFILE%\.local\bin) and, only with your consent,
  appends that directory to your USER PATH. Open a new terminal afterwards.

.PARAMETER InstallDir  Target directory.
.PARAMETER Binary      Use this prebuilt systui.exe instead of building.
.PARAMETER AddToPath   Add InstallDir to the user PATH without asking.
.PARAMETER NoPath      Never touch PATH (just copy the file).
#>
[CmdletBinding()]
param(
    [string]$InstallDir = (Join-Path $env:USERPROFILE '.local\bin'),
    [string]$Binary,
    [switch]$AddToPath,
    [switch]$NoPath
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

if (-not $Binary) {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        throw "cargo not found. Install Rust from https://rustup.rs, or pass -Binary <path to systui.exe>."
    }
    Write-Host "Building release binary..."
    Push-Location $root
    try { cargo build --release --locked; if ($LASTEXITCODE -ne 0) { throw "cargo build failed" } }
    finally { Pop-Location }
    $Binary = Join-Path $root 'target\release\systui.exe'
}
if (-not (Test-Path -LiteralPath $Binary)) { throw "Binary not found: $Binary" }

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
$dest = Join-Path $InstallDir 'systui.exe'
Copy-Item -LiteralPath $Binary -Destination $dest -Force
Write-Host "Installed: $dest"

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$entries = @($userPath -split ';' | Where-Object { $_ })
$already = $entries | Where-Object { $_.TrimEnd('\') -ieq $InstallDir.TrimEnd('\') }
if ($already) {
    Write-Host "$InstallDir is already on your user PATH."
} elseif ($NoPath) {
    Write-Host "Skipped PATH change (-NoPath). Add $InstallDir to PATH yourself, or run $dest directly."
} else {
    $go = $AddToPath.IsPresent
    if (-not $go -and [Environment]::UserInteractive -and -not [Console]::IsInputRedirected) {
        $answer = Read-Host "Add $InstallDir to your USER PATH? [y/N]"
        $go = $answer -match '^(y|yes)$'
    }
    if ($go) {
        $new = if ($userPath) { "$userPath;$InstallDir" } else { $InstallDir }
        [Environment]::SetEnvironmentVariable('Path', $new, 'User')
        Write-Host "Added to user PATH. Open a NEW terminal, then run: systui"
    } else {
        Write-Host "PATH not changed. Run $dest, or add $InstallDir to PATH (re-run with -AddToPath)."
    }
}
& $dest --version
