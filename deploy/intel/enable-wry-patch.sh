#!/usr/bin/env bash
# Enable the vendored tauri-runtime-wry patch used by the Intel desktop build.
#
# Why opt-in: `[patch.crates-io]` rewrites the dependency source for *every*
# platform in the workspace, and it changes Cargo.lock. Keeping it out of the
# default manifest means RK/aarch64 builds stay on the registry crate until
# someone verifies the vendored copy on device.
#
# Run this once on a networked machine before an Intel release build.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
MANIFEST="$ROOT/Cargo.toml"
VENDOR="vendor/tauri-runtime-wry-2.11.1"

if [ ! -d "$ROOT/$VENDOR" ]; then
  echo "error: $VENDOR missing" >&2
  exit 1
fi

if grep -q 'tauri-runtime-wry' "$MANIFEST"; then
  echo "patch already present in Cargo.toml"
  exit 0
fi

cat >> "$MANIFEST" <<EOF

# Added by deploy/intel/enable-wry-patch.sh: undecorated window resizing fix.
[patch.crates-io]
tauri-runtime-wry = { path = "$VENDOR" }
EOF

echo "==> appended [patch.crates-io] to Cargo.toml"
echo "==> refreshing lockfile entry"
cd "$ROOT"
cargo update -p tauri-runtime-wry
echo "==> done; remember this changes Cargo.lock for all platforms"
