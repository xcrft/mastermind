#!/usr/bin/env bash
# Populate an npm/platforms/<variant>/bin/ directory with a freshly-built mmcg
# binary, set the executable bit, and bump the package.json version to match
# the Rust crate. Used by both:
#   - local smoke (one platform — your host's triple)
#   - the GitHub Actions publish workflow (all 6 platforms, one job per target)
#
# Usage:
#   scripts/build-npm-packages.sh <rust-target-triple> <path-to-built-binary>
#
# Examples:
#   scripts/build-npm-packages.sh aarch64-apple-darwin \
#     mcp/servers/mmcg/target/aarch64-apple-darwin/release/mmcg
#   scripts/build-npm-packages.sh x86_64-pc-windows-msvc \
#     mcp/servers/mmcg/target/x86_64-pc-windows-msvc/release/mmcg.exe
#
# After this runs, `npm/platforms/<variant>/` is ready for `npm pack` / `npm publish`.

set -euo pipefail

if [ $# -ne 2 ]; then
    echo "usage: $0 <rust-target-triple> <path-to-built-binary>" >&2
    echo "       e.g. $0 aarch64-apple-darwin .../target/aarch64-apple-darwin/release/mmcg" >&2
    exit 2
fi

TARGET="$1"
BIN_SRC="$2"

# Resolve repo root from this script's location so the helper works regardless
# of the invoker's cwd (CI runs from repo root; local devs vary).
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# Map Rust target triple → npm platform package directory.
case "$TARGET" in
    aarch64-apple-darwin)             VARIANT="darwin-arm64"     ; EXE="mmcg" ;;
    x86_64-apple-darwin)              VARIANT="darwin-x64"       ; EXE="mmcg" ;;
    x86_64-unknown-linux-gnu)         VARIANT="linux-x64-gnu"    ; EXE="mmcg" ;;
    aarch64-unknown-linux-gnu)        VARIANT="linux-arm64-gnu"  ; EXE="mmcg" ;;
    x86_64-unknown-linux-musl)        VARIANT="linux-x64-musl"   ; EXE="mmcg" ;;
    aarch64-unknown-linux-musl)       VARIANT="linux-arm64-musl" ; EXE="mmcg" ;;
    x86_64-pc-windows-msvc)           VARIANT="win32-x64-msvc"   ; EXE="mmcg.exe" ;;
    *)
        echo "error: unknown Rust target triple '$TARGET'" >&2
        echo "       supported: aarch64-apple-darwin, x86_64-apple-darwin," >&2
        echo "                  x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu," >&2
        echo "                  x86_64-unknown-linux-musl, aarch64-unknown-linux-musl," >&2
        echo "                  x86_64-pc-windows-msvc" >&2
        exit 2
        ;;
esac

PLATFORM_DIR="$REPO_ROOT/npm/platforms/$VARIANT"
PLATFORM_BIN="$PLATFORM_DIR/bin"

if [ ! -d "$PLATFORM_DIR" ]; then
    echo "error: platform package dir not found at $PLATFORM_DIR" >&2
    exit 1
fi

if [ ! -f "$BIN_SRC" ]; then
    echo "error: built binary not found at $BIN_SRC" >&2
    exit 1
fi

mkdir -p "$PLATFORM_BIN"
cp "$BIN_SRC" "$PLATFORM_BIN/$EXE"
chmod +x "$PLATFORM_BIN/$EXE" 2>/dev/null || true   # no-op on Windows .exe

# Verify the version in the platform package.json matches the Rust crate.
# Mismatch = release pipeline drift; better to fail loud than ship inconsistent.
CRATE_VERSION=$(awk -F'"' '/^version *= *"/ {print $2; exit}' \
    "$REPO_ROOT/mcp/servers/mmcg/Cargo.toml")
PKG_VERSION=$(awk -F'"' '/"version":/ {print $4; exit}' "$PLATFORM_DIR/package.json")

if [ "$CRATE_VERSION" != "$PKG_VERSION" ]; then
    echo "error: version mismatch — Cargo.toml=$CRATE_VERSION, $VARIANT/package.json=$PKG_VERSION" >&2
    echo "       fix: bump both to the same value before publishing" >&2
    exit 1
fi

# Sanity-check the binary runs (skip on cross-compiled targets — best-effort).
HOST_OS="$(uname -s)"
HOST_ARCH="$(uname -m)"
case "$TARGET" in
    aarch64-apple-darwin)
        [ "$HOST_OS" = "Darwin" ] && [ "$HOST_ARCH" = "arm64" ] && CAN_RUN=1 ;;
    x86_64-apple-darwin)
        [ "$HOST_OS" = "Darwin" ] && [ "$HOST_ARCH" = "x86_64" ] && CAN_RUN=1 ;;
    x86_64-unknown-linux-gnu|x86_64-unknown-linux-musl)
        [ "$HOST_OS" = "Linux" ] && [ "$HOST_ARCH" = "x86_64" ] && CAN_RUN=1 ;;
    aarch64-unknown-linux-gnu|aarch64-unknown-linux-musl)
        [ "$HOST_OS" = "Linux" ] && [ "$HOST_ARCH" = "aarch64" ] && CAN_RUN=1 ;;
esac
if [ "${CAN_RUN:-0}" = "1" ]; then
    VERSION_OUT=$("$PLATFORM_BIN/$EXE" --version 2>&1 || true)
    if ! echo "$VERSION_OUT" | grep -q "$CRATE_VERSION"; then
        echo "warning: $PLATFORM_BIN/$EXE --version did not report v$CRATE_VERSION:" >&2
        echo "  $VERSION_OUT" >&2
    fi
fi

echo "✓ assembled $VARIANT: $PLATFORM_BIN/$EXE (v$CRATE_VERSION)"
