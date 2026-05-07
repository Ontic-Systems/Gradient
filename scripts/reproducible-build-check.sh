#!/usr/bin/env bash
# scripts/reproducible-build-check.sh — verify two clean builds of
# the Gradient compiler produce bit-identical artifacts.
#
# Used by .github/workflows/reproducible-build.yml. Also runnable
# locally:
#
#   scripts/reproducible-build-check.sh
#
# Exit codes:
#   0 — artifacts are bit-identical
#   1 — drift detected (artifacts differ)
#   2 — environmental error (e.g. cargo missing)
#
# Determinism levers used:
#   - SOURCE_DATE_EPOCH locked to commit timestamp.
#   - --frozen --locked on cargo to use Cargo.lock verbatim.
#   - Single-threaded codegen (-C codegen-units=1) and split target
#     dirs so no warm artifact bleeds across the pair.
#
# Known limitations (see docs/security/reproducible-builds.md):
#   - Only the Cranelift host build is checked today; the LLVM backend
#     determinism story is gated on E6 (backend split, ADR 0004).
#   - Build-id and timestamps inside the ELF are not yet stripped via
#     post-processing; if cargo's defaults regress on this, the script
#     will surface it as drift.
#
# Pure bash + sha256sum + cargo. No extra dependencies.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if ! command -v cargo > /dev/null; then
    echo "ERROR: cargo not on PATH" >&2
    exit 2
fi

# Lock SOURCE_DATE_EPOCH to the commit timestamp. This is the same
# convention reproducible-builds.org uses; it survives across re-checkouts
# of the same commit on different machines.
COMMIT_EPOCH=$(git log -1 --pretty=%ct 2>/dev/null || date -u +%s)
export SOURCE_DATE_EPOCH="$COMMIT_EPOCH"

ARTIFACT_PATH="codebase"

build_into() {
    local target_dir="$1"
    rm -rf "$target_dir"
    mkdir -p "$target_dir"
    # Determinism levers:
    #   -C codegen-units=1            single-threaded codegen
    #   -C link-arg=-Wl,--build-id=none   strip the per-link random build-id
    #   --remap-path-prefix           normalize embedded source paths
    if ! CARGO_TARGET_DIR="$target_dir" \
        RUSTFLAGS="-C codegen-units=1 -C link-arg=-Wl,--build-id=none --remap-path-prefix=$ROOT=. --remap-path-prefix=$HOME/.cargo=/cargo" \
            cargo build --manifest-path "$ARTIFACT_PATH/Cargo.toml" \
            -p gradient-compiler \
            --release --bin gradient-compiler --locked \
            > "$target_dir/build.log" 2>&1; then
        echo "ERROR: cargo build failed (target=$target_dir). Tail of build.log:" >&2
        tail -80 "$target_dir/build.log" >&2 || true
        exit 2
    fi
}

hash_binary() {
    local target_dir="$1"
    local bin="$target_dir/release/gradient-compiler"
    if [ ! -f "$bin" ]; then
        echo "ERROR: expected binary missing: $bin" >&2
        tail -50 "$target_dir/build.log" >&2 || true
        exit 2
    fi
    sha256sum "$bin" | awk '{print $1}'
}

TMP_A=$(mktemp -d -t gradient-build-a-XXXXXXXX)
TMP_B=$(mktemp -d -t gradient-build-b-XXXXXXXX)
trap 'rm -rf "$TMP_A" "$TMP_B"' EXIT

echo "[1/2] First build (target=$TMP_A) with SOURCE_DATE_EPOCH=$SOURCE_DATE_EPOCH..."
build_into "$TMP_A"
HASH_A=$(hash_binary "$TMP_A")
echo "      sha256 = $HASH_A"

echo "[2/2] Second build (target=$TMP_B) with SOURCE_DATE_EPOCH=$SOURCE_DATE_EPOCH..."
build_into "$TMP_B"
HASH_B=$(hash_binary "$TMP_B")
echo "      sha256 = $HASH_B"

if [ "$HASH_A" = "$HASH_B" ]; then
    echo "REPRODUCIBLE: both builds produced sha256 = $HASH_A"
    exit 0
fi

echo "ERROR: builds differ — sha256 a=$HASH_A b=$HASH_B" >&2
echo "Diff hint: try diffoscope $TMP_A/release/gradient-compiler $TMP_B/release/gradient-compiler" >&2
exit 1
