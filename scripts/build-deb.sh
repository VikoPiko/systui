#!/usr/bin/env sh
# Build a Debian/Ubuntu package (.deb) for systui.
#
#   scripts/build-deb.sh
#
# Needs: cargo, gzip, and cargo-deb (cargo install cargo-deb --locked). Output: target/debian/*.deb
# The man page and the shell completions are generated from the freshly built binary, so they always
# match its real command line.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

command -v cargo-deb >/dev/null 2>&1 || {
    echo "cargo-deb is not installed. Run: cargo install cargo-deb --locked" >&2
    exit 1
}

command -v dpkg-shlibdeps >/dev/null 2>&1 || {
    if [ -n "${CI:-}" ]; then
        echo "dpkg-shlibdeps (package dpkg-dev) is required in CI so that Depends is filled in." >&2
        exit 1
    fi
    echo "WARNING: dpkg-shlibdeps not found (install dpkg-dev). The .deb will have an EMPTY Depends" >&2
    echo "         field: fine for a local test, NOT acceptable for publishing." >&2
}

cargo build --release --locked
bin=target/release/systui

pkg=target/pkg
rm -rf "$pkg"
mkdir -p "$pkg"
"$bin" man | gzip -9n >"$pkg/systui.1.gz"
"$bin" completions bash >"$pkg/systui.bash"
"$bin" completions zsh >"$pkg/_systui"
"$bin" completions fish >"$pkg/systui.fish"

# --no-build: use exactly the binary the completions were generated from.
cargo deb --no-build --locked
ls -l target/debian/*.deb
