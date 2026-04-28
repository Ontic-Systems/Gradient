#!/usr/bin/env bash
#
# install.sh — Build the Gradient compiler + build-system and symlink
#              both binaries into a user-writable bin directory.
#
# Usage:
#   scripts/install.sh                       # install to ~/.local/bin
#   scripts/install.sh --prefix /custom/dir  # install to /custom/dir
#   scripts/install.sh --uninstall           # remove the symlinks
#   scripts/install.sh --help                # show this message
#
# The build-system binary (`gradient`) discovers `gradient-compiler` via:
#   1. $GRADIENT_COMPILER env var
#   2. `gradient-compiler` on PATH
#   3. development fallback paths
# Symlinking both binaries into the same directory on PATH satisfies (2),
# so no environment variables are required after install.
#
# This script never uses sudo and never edits your shell config.

set -euo pipefail

# ---------------------------------------------------------------------------
# Resolve repo root from the script's own location, so the script works no
# matter where it is invoked from.
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
MANIFEST_PATH="$REPO_ROOT/codebase/Cargo.toml"
RELEASE_DIR="$REPO_ROOT/codebase/target/release"

# L-2: SHA256SUMS file path
SHA256SUMS_PATH="$REPO_ROOT/SHA256SUMS"

BINARIES=("gradient-compiler" "gradient")

PREFIX="$HOME/.local/bin"
UNINSTALL=0

usage() {
    sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'
}

# ---------------------------------------------------------------------------
# Argument parsing — POSIX-style long options, no getopt required.
# ---------------------------------------------------------------------------
while [ $# -gt 0 ]; do
    case "$1" in
        --prefix)
            if [ $# -lt 2 ]; then
                echo "error: --prefix requires a directory argument" >&2
                exit 2
            fi
            PREFIX="$2"
            shift 2
            ;;
        --prefix=*)
            PREFIX="${1#--prefix=}"
            shift
            ;;
        --uninstall)
            UNINSTALL=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "error: unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

# Expand ~ in --prefix if the user passed it literally.
case "$PREFIX" in
    "~"|"~/"*) PREFIX="$HOME${PREFIX#~}" ;;
esac

mkdir -p "$PREFIX"
PREFIX="$(cd "$PREFIX" && pwd)"

# ---------------------------------------------------------------------------
# Uninstall mode — remove only symlinks that point at our release binaries.
# ---------------------------------------------------------------------------
if [ "$UNINSTALL" -eq 1 ]; then
    removed=0
    for bin in "${BINARIES[@]}"; do
        link="$PREFIX/$bin"
        if [ -L "$link" ]; then
            rm -f "$link"
            echo "removed $link"
            removed=$((removed + 1))
        elif [ -e "$link" ]; then
            echo "skip    $link (not a symlink, leaving alone)"
        else
            echo "skip    $link (not present)"
        fi
    done
    echo
    echo "Uninstall complete ($removed symlink(s) removed)."
    exit 0
fi

# ---------------------------------------------------------------------------
# Install mode — verify cargo, build release, symlink both binaries.
# ---------------------------------------------------------------------------
if ! command -v cargo >/dev/null 2>&1; then
    echo "error: 'cargo' is not on PATH." >&2
    echo "Install Rust from https://rustup.rs and re-run this script." >&2
    exit 1
fi

cd "$REPO_ROOT"

echo "==> Building Gradient (release) from $MANIFEST_PATH"
cargo build --release --locked --manifest-path "$MANIFEST_PATH"

# L-2: Generate SHA256SUMS for release binaries
echo
echo "==> Generating SHA256SUMS"
rm -f "$SHA256SUMS_PATH"
for bin in "${BINARIES[@]}"; do
    src="$RELEASE_DIR/$bin"
    if [ -f "$src" ]; then
        sha256sum "$src" >> "$SHA256SUMS_PATH"
    fi
done
echo "  Created $SHA256SUMS_PATH"

echo
echo "==> Installing symlinks into $PREFIX"
for bin in "${BINARIES[@]}"; do
    src="$RELEASE_DIR/$bin"
    if [ ! -x "$src" ]; then
        echo "error: expected binary not found: $src" >&2
        echo "       Did the cargo build fail silently?" >&2
        exit 1
    fi
    link="$PREFIX/$bin"
    # `ln -sfn` overwrites stale symlinks atomically and never follows
    # an existing symlink-to-directory, which keeps the script idempotent.
    ln -sfn "$src" "$link"
    echo "  $link -> $src"
done

# L-2: Print installed binary SHA256s
echo
echo "==> Installed binary SHA256 hashes:"
for bin in "${BINARIES[@]}"; do
    src="$RELEASE_DIR/$bin"
    hash=$(sha256sum "$src" | cut -d' ' -f1)
    echo "  $bin: $hash"
done

echo
echo "==> Installed:"
for bin in "${BINARIES[@]}"; do
    echo "  - $bin"
done

# PATH-hint: only print the suggestion if PREFIX isn't already on PATH.
case ":$PATH:" in
    *":$PREFIX:"*)
        echo
        echo "$PREFIX is already on your PATH. You're ready to go:"
        echo "  gradient --help"
        ;;
    *)
        echo
        echo "NOTE: $PREFIX is not on your PATH."
        echo "      Add this to your shell config to fix that:"
        echo
        echo "        export PATH=\"$PREFIX:\$PATH\""
        echo
        echo "      Then re-open your shell and run: gradient --help"
        ;;
esac
