<#
.SYNOPSIS
  Authenticode-sign Windows binaries/installers (systui.exe, the .msi) with signtool.

.DESCRIPTION
  Signs with SHA-256 and an RFC 3161 timestamp, so the signature stays valid after the certificate
  expires. Certificate sources (first match wins):
    -PfxPath <file>            a .pfx/.p12 file (password from -PfxPassword or $env:WINDOWS_CERT_PASSWORD)
    $env:WINDOWS_CERT_PFX_BASE64   the .pfx as base64 (how a CI secret is passed); written to a temp file
                               that is deleted afterwards
    -Thumbprint <sha1>         a certificate already installed in CurrentUser\My or LocalMachine\My

  The private key is never printed. Exit code is non-zero if signing or verification fails.
  -AllowUntrusted accepts a signature whose chain is not trusted on this machine (a SELF-SIGNED test
  certificate); the file's integrity and the signer are still checked.

.EXAMPLE
  .\scripts\sign-windows.ps1 -Files target\release\systui.exe, dist\systui-0.1.0-x64.msi -PfxPath cert.pfx
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string[]]$Files,
    [string]$PfxPath,
    [string]$PfxPassword,
    [string]$Thumbprint,
    [string]$TimestampUrl = 'http://timestamp.digicert.com',
    [string]$Description = 'systui',
    [switch]$NoTimestamp,
    [switch]$AllowUntrusted
)
$ErrorActionPreference = 'Stop'

function Find-SignTool {
    $c = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($c) { return $c.Source }
    $kits = "${env:ProgramFiles(x86)}\Windows Kits\10\bin"
    $found = Get-ChildItem $kits -Recurse -Filter signtool.exe -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -match '\\x64\\' } | Sort-Object FullName -Descending | Select-Object -First 1
    if ($found) { return $found.FullName }
    throw 'signtool.exe not found. Install the Windows SDK (signing tools) or put signtool on PATH.'
}
$signtool = Find-SignTool

$tmpPfx = $null
if (-not $PfxPath -and -not $Thumbprint -and $env:WINDOWS_CERT_PFX_BASE64) {
    $tmpPfx = Join-Path ([IO.Path]::GetTempPath()) ("cert-" + [guid]::NewGuid().ToString('N') + '.pfx')
    [IO.File]::WriteAllBytes($tmpPfx, [Convert]::FromBase64String($env:WINDOWS_CERT_PFX_BASE64))
    $PfxPath = $tmpPfx
}
if (-not $PfxPassword) { $PfxPassword = $env:WINDOWS_CERT_PASSWORD }
if (-not $PfxPath -and -not $Thumbprint) {
    throw 'No certificate: pass -PfxPath or -Thumbprint, or set WINDOWS_CERT_PFX_BASE64 (+ WINDOWS_CERT_PASSWORD).'
}

try {
    foreach ($f in $Files) {
        if (-not (Test-Path -LiteralPath $f)) { throw "File not found: $f" }
        $args2 = @('sign', '/fd', 'SHA256', '/d', $Description)
        if (-not $NoTimestamp) { $args2 += @('/tr', $TimestampUrl, '/td', 'SHA256') }
        if ($PfxPath) { $args2 += @('/f', $PfxPath); if ($PfxPassword) { $args2 += @('/p', $PfxPassword) } }
        else { $args2 += @('/sha1', $Thumbprint) }
        $args2 += (Resolve-Path -LiteralPath $f).Path
        & $signtool @args2 | Out-Host
        if ($LASTEXITCODE -ne 0) { throw "signtool failed for $f (exit $LASTEXITCODE)" }

        $sig = Get-AuthenticodeSignature -LiteralPath $f
        $trusted = $sig.Status -eq 'Valid'
        $integrityOk = $sig.Status -notin 'HashMismatch', 'NotSigned'
        if (-not $sig.SignerCertificate) { throw "$f carries no signature after signing" }
        if (-not $trusted -and -not ($AllowUntrusted -and $integrityOk)) {
            throw "$f signature status is $($sig.Status): $($sig.StatusMessage)"
        }
        $ts = if ($sig.TimeStamperCertificate) { 'timestamped' } else { 'NOT timestamped' }
        Write-Host ("signed {0}  [{1}; {2}; signer: {3}]" -f $f, $sig.Status, $ts, $sig.SignerCertificate.Subject)
        if (-not $NoTimestamp -and -not $sig.TimeStamperCertificate) { throw "$f was not timestamped" }
    }
}
finally {
    if ($tmpPfx -and (Test-Path -LiteralPath $tmpPfx)) { [IO.File]::WriteAllBytes($tmpPfx, [byte[]]::new(0)); Remove-Item -LiteralPath $tmpPfx -Force }
}
