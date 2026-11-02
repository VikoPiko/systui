<#
.SYNOPSIS
  Build the per-user Windows installer (MSI) for systui with WiX 5.

.DESCRIPTION
  Uses packaging/windows/systui.wxs. The MSI installs to %LOCALAPPDATA%\Programs\systui without
  administrator rights and can add that folder to the USER Path. Output: dist\systui-<version>-x64.msi

.PARAMETER Exe          Path to the release systui.exe (default: target\release\systui.exe).
.PARAMETER Version      MSI version, three or four numeric parts (default: from Cargo.toml).
.PARAMETER Out          Output file.
.PARAMETER InstallWix   Install the pinned WiX tool (dotnet tool, user level) if it is missing.
#>
[CmdletBinding()]
param(
    [string]$Exe,
    [string]$Version,
    [string]$Out,
    [switch]$InstallWix
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$wixVersion = '5.0.2'   # last release under the free MS-RL licence; v7+ has a maintenance fee

if (-not $Exe) { $Exe = Join-Path $root 'target\release\systui.exe' }
if (-not (Test-Path -LiteralPath $Exe)) { throw "Binary not found: $Exe  (run: cargo build --release --locked)" }
if (-not $Version) {
    $m = Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
    $Version = $m.Matches[0].Groups[1].Value
}
# MSI versions are numeric: strip any -prerelease/+build suffix.
$Version = ($Version -replace '[-+].*$', '')
if ($Version -notmatch '^\d+\.\d+\.\d+(\.\d+)?$') { throw "MSI version must look like 1.2.3 (got '$Version')" }
if (-not $Out) { New-Item -ItemType Directory -Force (Join-Path $root 'dist') | Out-Null; $Out = Join-Path $root "dist\systui-$Version-x64.msi" }

if (-not (Get-Command wix -ErrorAction SilentlyContinue)) {
    if ($InstallWix) { dotnet tool install --global wix --version $wixVersion; $env:PATH += ";$env:USERPROFILE\.dotnet\tools" }
    else { throw "WiX is not installed. Run: dotnet tool install --global wix --version $wixVersion   (or pass -InstallWix)" }
}
$have = (& wix --version) -replace '\+.*$', ''
if ($have -ne $wixVersion) { Write-Warning "Built and tested with WiX $wixVersion; found $have." }

& wix build (Join-Path $root 'packaging\windows\systui.wxs') `
    -arch x64 -o $Out `
    -d "Version=$Version" -d "ExePath=$Exe" -d "DocDir=$root"
if ($LASTEXITCODE -ne 0) { throw "wix build failed" }
Write-Host "Built $Out"
Write-Host ("SHA256 " + (Get-FileHash $Out -Algorithm SHA256).Hash.ToLower())
