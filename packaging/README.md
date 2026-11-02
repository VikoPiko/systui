# Packaging, signing and publishing systui

Everything here is reproducible from this repository. Each item says **what was actually verified** and what
still needs something only the maintainer has (a certificate, a published release, a GitHub repository).

## Artifacts

| Artifact | Built by | Verified here |
| --- | --- | --- |
| `systui-<ver>-x64.msi` (per-user, no admin) | `scripts/build-msi.ps1` (WiX **5.0.2**, MS-RL) | Built; **installed, upgraded, downgrade refused, repaired, uninstalled** on Windows 11 without elevation; `PATH` entry added exactly once and removed again; Apps & features entry created and removed |
| `systui-v<ver>-windows-x86_64.zip` | `release.yml` | Same layout built and hashed locally |
| winget manifests | `scripts/make-manifests.ps1` | `winget validate` → *Manifest validation succeeded* (real ProductCode and hashes filled in). **Not installed from winget**: needs a published URL |
| Scoop manifest | `scripts/make-manifests.ps1` | Parses as JSON. **Not installed with Scoop** (not installed here) |
| Chocolatey package | `scripts/make-manifests.ps1` | `choco pack` produced a real `.nupkg`. **Not installed**: needs a published URL and admin |
| `systui_<ver>-1_amd64.deb` | `scripts/build-deb.sh` (cargo-deb) | Built on Ubuntu 24.04; contents, man page and bash completion checked after extracting. **Not `dpkg -i`'d** (no root in the test environment) and **`lintian` was not available** |
| `systui-v<ver>-linux-x86_64.tar.gz` | `release.yml` | Layout identical to the `.deb` payload |
| Man page, bash/zsh/fish/PowerShell/elvish completions | `systui man`, `systui completions <shell>` | Tested; shipped in the `.deb` and the Linux tarball |

### Linux compatibility (important)
A binary built on a new distribution requires its newer glibc. Measured: built on Ubuntu 24.04 the binary needs
glibc symbol **2.39** and, on Ubuntu 22.04 / Debian 12, still works but prints
``weak version `GLIBC_2.39' not found`` on every start. Built inside Ubuntu 22.04 it needs **2.34**, runs with a
clean stderr on Ubuntu 22.04, Debian 12 and Ubuntu 24.04 (all three tested in containers). The release workflow
therefore builds on `ubuntu-22.04` and has a guard step that fails the release if the binary ever needs glibc > 2.35.
Not supported/tested: musl/Alpine, ARM, macOS, Windows ARM.

## Building locally

```powershell
# Windows
cargo build --release --locked
dotnet tool install --global wix --version 5.0.2      # once (user-level)
.\scripts\build-msi.ps1                               # -> dist\systui-<ver>-x64.msi
.\scripts\make-manifests.ps1 -Owner <you> -Repo <repo> -Msi dist\systui-<ver>-x64.msi `
    -Zip dist\systui-v<ver>-windows-x86_64.zip -Validate
```
```sh
# Debian / Ubuntu (needs dpkg-dev so that Depends is filled in)
cargo install cargo-deb --locked
sh scripts/build-deb.sh                               # -> target/debian/*.deb
```

MSI options: `msiexec /i systui.msi ADDLOCAL=Main` installs **without** touching `PATH`;
the default also adds the install folder to the *user* `PATH`. Uninstall via Apps & features or `msiexec /x`.

## Code signing

Signing proves who built a file and that it was not modified. It does **not** make SmartScreen silent by itself:
Windows also weighs publisher reputation, which a new certificate has to earn (EV certificates and Azure/SignPath
signing start with better standing).

### Windows (Authenticode)
`scripts/sign-windows.ps1` signs with SHA-256 and an RFC 3161 timestamp (so signatures outlive the certificate).
It takes a `.pfx` (`-PfxPath`), a base64 secret (`WINDOWS_CERT_PFX_BASE64` + `WINDOWS_CERT_PASSWORD`, how CI
passes it) or a store thumbprint. **Verified** with a *temporary self-signed* certificate that was created and
deleted for the test: signing and timestamping a real `systui.exe` and `.msi`, the strict mode refusing an
untrusted chain, tamper detection (`HashMismatch`), the base64 path leaving no file behind, and a signed MSI still
installing and uninstalling. **Not verified with a real, trusted certificate**, because none exists yet.

Ways to get a trusted signature, cheapest first:
1. **SignPath Foundation**: free code signing for open-source projects (application required; it supplies the
   certificate and signing service).
2. **Azure Trusted Signing / Artifact Signing**: low monthly cost, no hardware token, identity validation needed.
3. A conventional **OV/EV code-signing certificate** from a CA (annual cost; EV requires a hardware token or cloud HSM).

To enable signing in the release workflow, add the repository secrets `WINDOWS_CERT_PFX_BASE64` and
`WINDOWS_CERT_PASSWORD`; the workflow signs `systui.exe` *before* packaging (so the zip and the MSI contain the
signed binary) and then signs the MSI. Without the secrets, Windows files are published unsigned. For Azure or
SignPath, replace the two signing steps with that service's action.

### Linux and provenance
The release workflow signs every artifact **keylessly with sigstore/cosign** (the signature is bound to the
workflow's GitHub identity, so there is no key to leak or rotate), publishes `SHA256SUMS`, and creates GitHub
**build-provenance attestations**. Users verify with:
```sh
sha256sum -c SHA256SUMS --ignore-missing
gh attestation verify systui-v0.1.0-linux-x86_64.tar.gz --repo OWNER/REPO
cosign verify-blob --bundle <file>.bundle \
  --certificate-identity-regexp 'https://github.com/OWNER/REPO/.github/workflows/release.yml@refs/tags/v.*' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com <file>
```
**Not executed**: this requires a GitHub repository and a tag push. If you prefer a classic detached GPG signature,
add a step that signs `SHA256SUMS` with a key held in a repository secret.

## Publishing checklist (manual, in order)
1. Replace the placeholders: the copyright holder in `LICENSE`, the maintainer in `[package.metadata.deb]`, the
   publisher in `packaging/winget/*.in`, and add `repository` to `Cargo.toml` (then drop `publish = false` if you
   also want crates.io).
2. Push to GitHub, confirm the CI workflow is green, then `git tag v0.1.0 && git push --tags`. The release workflow
   builds, signs (if secrets exist), attests and publishes everything, including a zip of generated manifests.
3. **winget**: fork `microsoft/winget-pkgs`, add the three generated files under
   `manifests/<first-letter>/<Owner>/systui/<version>/`, and open a pull request (their bot validates it).
   Alternatively use `wingetcreate update`.
4. **Scoop**: put `systui.json` in your own bucket repository (`scoop bucket add <name> <url>`), or submit to a
   community bucket.
5. **Chocolatey**: `choco push systui.<ver>.nupkg --source https://push.chocolatey.org/` (needs an account and API
   key; community packages go through moderation, which asks for a checksum-verifiable URL and a silent install,
   both of which the package has).
6. **Debian/Ubuntu**: attach the `.deb` to the release (done by the workflow), or host an APT repository /
   Launchpad PPA. Check the package with `lintian` before announcing it; that has not been done yet.
7. Not covered: Homebrew (macOS is unsupported), an AUR `PKGBUILD`, RPM, Flatpak/Snap, MSIX.
