<#
.SYNOPSIS
  Generate winget, Scoop and Chocolatey package definitions from the real release artifacts.

.DESCRIPTION
  Fills the templates in packaging\ with the version, release URLs, SHA-256 hashes and the MSI's
  actual ProductCode. Output goes to dist\manifests\{winget,scoop,chocolatey}.
  -Validate runs `winget validate` and `choco pack` when those tools are installed.

  Nothing is submitted anywhere; publishing is a manual step (see packaging\README.md).

.PARAMETER Owner    GitHub user/organisation that hosts the release (used in URLs and the winget id).
.PARAMETER Repo     GitHub repository name.
.PARAMETER Msi      The MSI built by build-msi.ps1.
.PARAMETER Zip      The Windows release zip (systui-v<version>-windows-x86_64.zip).
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Owner,
    [Parameter(Mandatory)][string]$Repo,
    [Parameter(Mandatory)][string]$Msi,
    [Parameter(Mandatory)][string]$Zip,
    [string]$Version,
    [string]$Out,
    [switch]$Validate
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not $Version) {
    $m = Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
    $Version = ($m.Matches[0].Groups[1].Value -replace '[-+].*$', '')
}
if (-not $Out) { $Out = Join-Path $root 'dist\manifests' }
foreach ($f in $Msi, $Zip) { if (-not (Test-Path -LiteralPath $f)) { throw "Not found: $f" } }

# The MSI's real ProductCode (winget uses it to detect installed versions).
$wi = New-Object -ComObject WindowsInstaller.Installer
$db = $wi.GetType().InvokeMember('OpenDatabase', 'InvokeMethod', $null, $wi, @((Resolve-Path $Msi).Path, 0))
$view = $db.GetType().InvokeMember('OpenView', 'InvokeMethod', $null, $db, @("SELECT Value FROM Property WHERE Property='ProductCode'"))
$view.GetType().InvokeMember('Execute', 'InvokeMethod', $null, $view, $null) | Out-Null
$rec = $view.GetType().InvokeMember('Fetch', 'InvokeMethod', $null, $view, $null)
$productCode = $rec.GetType().InvokeMember('StringData', 'GetProperty', $null, $rec, @(1))
$view.GetType().InvokeMember('Close', 'InvokeMethod', $null, $view, $null) | Out-Null
if ($productCode -notmatch '^\{[0-9A-Fa-f-]{36}\}$') { throw "Could not read the MSI ProductCode (got '$productCode')" }

$tokens = @{
    '@OWNER@'        = $Owner
    '@REPO@'         = $Repo
    '@VERSION@'      = $Version
    '@MSI_SHA256@'   = (Get-FileHash $Msi -Algorithm SHA256).Hash.ToUpper()
    '@ZIP_SHA256@'   = (Get-FileHash $Zip -Algorithm SHA256).Hash.ToLower()
    '@PRODUCT_CODE@' = $productCode
}
function Render([string]$template, [string]$dest) {
    $t = [IO.File]::ReadAllText($template)
    foreach ($k in $tokens.Keys) { $t = $t.Replace($k, $tokens[$k]) }
    if ($t -match '@[A-Z_0-9]+@') { throw "Unfilled placeholder in ${template}: $($Matches[0])" }
    New-Item -ItemType Directory -Force (Split-Path $dest) | Out-Null
    [IO.File]::WriteAllText($dest, $t, (New-Object Text.UTF8Encoding $false))
    Write-Host "wrote $dest"
}
$p = Join-Path $root 'packaging'
Render "$p\winget\Owner.systui.yaml.in"              "$Out\winget\$Owner.systui.yaml"
Render "$p\winget\Owner.systui.installer.yaml.in"    "$Out\winget\$Owner.systui.installer.yaml"
Render "$p\winget\Owner.systui.locale.en-US.yaml.in" "$Out\winget\$Owner.systui.locale.en-US.yaml"
Render "$p\scoop\systui.json.in"                     "$Out\scoop\systui.json"
Render "$p\chocolatey\systui.nuspec.in"              "$Out\chocolatey\systui.nuspec"
Render "$p\chocolatey\chocolateyinstall.ps1.in"      "$Out\chocolatey\tools\chocolateyinstall.ps1"

if ($Validate) {
    if (Get-Command winget -ErrorAction SilentlyContinue) {
        winget validate --manifest "$Out\winget"
        if ($LASTEXITCODE -ne 0) { throw 'winget validate failed' }
    } else { Write-Warning 'winget not installed: skipped validation' }
    Get-Content "$Out\scoop\systui.json" -Raw | ConvertFrom-Json | Out-Null; Write-Host 'scoop manifest is valid JSON'
    if (Get-Command choco -ErrorAction SilentlyContinue) {
        Push-Location "$Out\chocolatey"
        try { choco pack --out . | Out-Host; if ($LASTEXITCODE -ne 0) { throw 'choco pack failed' } } finally { Pop-Location }
    } else { Write-Warning 'choco not installed: skipped validation' }
}
