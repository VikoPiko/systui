#!/usr/bin/env sh
# Per-user install of systui on Linux/macOS. Never uses sudo.
#
#   scripts/install.sh [--prefix DIR] [--binary PATH]
#
# Builds the release binary with cargo (unless --binary is given), installs it
# to DIR (default: ~/.local/bin) and tells you how to put DIR on PATH.
# It does NOT edit your shell profile.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(dirname "$here")
prefix="${HOME}/.local/bin"
binary=""

while [ $# -gt 0 ]; do
    case "$1" in
        --prefix) prefix="${2:?--prefix needs a directory}"; shift 2 ;;
        --binary) binary="${2:?--binary needs a path}"; shift 2 ;;
        -h|--help) sed -n '2,8p' "$0"; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

if [ -z "$binary" ]; then
    command -v cargo >/dev/null 2>&1 || {
        echo "cargo not found. Install Rust from https://rustup.rs or pass --binary PATH." >&2
        exit 1
    }
    echo "Building release binary..."
    (cd "$root" && cargo build --release --locked)
    binary="$root/target/release/systui"
    [ -f "$binary" ] || binary="$root/target/release/systui.exe"
fi
[ -f "$binary" ] || { echo "binary not found: $binary" >&2; exit 1; }

mkdir -p "$prefix"
dest="$prefix/$(basename "$binary" | sed 's/\.exe$//')"
case "$binary" in *.exe) dest="$prefix/systui.exe" ;; esac
cp "$binary" "$dest"
chmod 755 "$dest"
echo "Installed: $dest"

case ":${PATH}:" in
    *":${prefix}:"*) echo "$prefix is already on your PATH." ;;
    *)
        echo "$prefix is not on your PATH. Add this line to ~/.profile (or your shell rc), then open a new shell:"
        echo "    export PATH=\"$prefix:\$PATH\""
        ;;
esac
"$dest" --version
