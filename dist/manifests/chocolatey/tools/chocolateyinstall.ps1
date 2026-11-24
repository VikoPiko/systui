$ErrorActionPreference = 'Stop'
$toolsDir = Split-Path -Parent $MyInvocation.MyCommand.Definition

# Unzips systui.exe into the package's tools folder; Chocolatey creates a shim on PATH for it.
Install-ChocolateyZipPackage `
    -PackageName 'systui' `
    -Url64bit 'https://github.com/example/systui/releases/download/v0.1.0/systui-v0.1.0-windows-x86_64.zip' `
    -Checksum64 '5e8709b1b296ef419eccf232cb01460cd2198145d456fe050c88024506cffd8f' `
    -ChecksumType64 'sha256' `
    -UnzipLocation $toolsDir
