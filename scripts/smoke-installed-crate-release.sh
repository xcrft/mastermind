#!/usr/bin/env bash
# Install the just-published crate from crates.io into an isolated prefix and
# execute the shipped binary. This is intentionally after exact-byte publish.

set -euo pipefail

if [ "$#" -ne 1 ] || [ ! -f "$1" ]; then
    echo "usage: $0 <Cargo.toml>" >&2
    exit 2
fi

VERSION=$(awk -F'"' '/^version *= *"/ {print $2; exit}' "$1")
if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
    echo "error: invalid Cargo package version" >&2
    exit 2
fi
ATTEMPTS="${CRATE_INSTALL_SMOKE_ATTEMPTS:-6}"
DELAY="${CRATE_INSTALL_SMOKE_DELAY_SECONDS:-20}"
case "$ATTEMPTS" in ''|*[!0-9]*) echo "error: CRATE_INSTALL_SMOKE_ATTEMPTS must be a positive integer" >&2; exit 2 ;; esac
case "$DELAY" in ''|*[!0-9]*) echo "error: CRATE_INSTALL_SMOKE_DELAY_SECONDS must be a non-negative integer" >&2; exit 2 ;; esac
if [ "$ATTEMPTS" -lt 1 ]; then
    echo "error: CRATE_INSTALL_SMOKE_ATTEMPTS must be a positive integer" >&2
    exit 2
fi

SMOKE_ROOT=$(mktemp -d)
trap 'rm -rf "$SMOKE_ROOT"' EXIT
CARGO_HOME="$SMOKE_ROOT/cargo-home"
export CARGO_HOME
attempt=1
while [ "$attempt" -le "$ATTEMPTS" ]; do
    PREFIX="$SMOKE_ROOT/install-$attempt"
    if cargo install \
        --locked \
        --root "$PREFIX" \
        --version "=$VERSION" \
        mmcg >"$SMOKE_ROOT/cargo-install-$attempt.log" 2>&1; then
        BIN="$PREFIX/bin/mmcg"
        test "$($BIN --version)" = "mastermind $VERSION"
        "$BIN" facts --help >/dev/null
        "$BIN" facts keygen --help >/dev/null
        "$BIN" team --help >/dev/null
        "$BIN" review export --help >/dev/null
        echo "✓ installed mmcg $VERSION from crates.io and exercised the shipped binary"
        exit 0
    fi
    if [ "$attempt" -lt "$ATTEMPTS" ]; then
        echo "crates.io install for mmcg $VERSION not ready (attempt $attempt/$ATTEMPTS); retrying"
        sleep "$DELAY"
    fi
    attempt=$((attempt + 1))
done

echo "error: mmcg $VERSION never passed the installed-registry smoke" >&2
tail -n 40 "$SMOKE_ROOT"/cargo-install-*.log >&2
exit 1
