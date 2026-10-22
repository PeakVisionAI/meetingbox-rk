#!/usr/bin/env bash
# Build the unified MeetingBox codebase for one deployment profile.
#
#   ./scripts/build-profile.sh rk              # RK3588 device build (aarch64 Linux)
#   ./scripts/build-profile.sh intel           # Intel desktop build (Windows)
#   ./scripts/build-profile.sh intel --check   # type/compile check only
#
# The profile only changes runtime defaults (service endpoints, UI capabilities).
# Both builds come from this single tree; nothing is forked.
set -euo pipefail

usage() {
  cat <<'EOF'
usage: scripts/build-profile.sh <intel|rk> [--check] [cargo args...]

  intel    Windows desktop + Ubuntu VM service stack (streaming ASR)
  rk       RK3588/RK1828 device (chunked NPU ASR, on-device services)
  --check  run tsc + cargo check instead of producing a package
EOF
}

PROFILE="${1:-}"
case "$PROFILE" in
  intel|rk) ;;
  *) usage; exit 2 ;;
esac
shift

CHECK_ONLY=0
CARGO_ARGS=()
for arg in "$@"; do
  case "$arg" in
    --check) CHECK_ONLY=1 ;;
    *) CARGO_ARGS+=("$arg") ;;
  esac
done

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

export MEETINGBOX_PROFILE="$PROFILE"
echo "==> profile: $PROFILE"

case "$PROFILE" in
  intel)
    echo "    ASR: streaming bridge (services/asrserve-bridge)"
    echo "    summary: 35B adapter (services/qwen-openvino)"
    echo "    knowledge centre: enabled"
    if ! grep -q 'tauri-runtime-wry' "$ROOT/Cargo.toml"; then
      echo "    note: run deploy/intel/enable-wry-patch.sh once for the undecorated-window fix"
    fi
    ;;
  rk)
    echo "    ASR: on-device RKNN service with NPU routing + BossAI CPU fallback"
    echo "    summary: RK1828 sidecar"
    echo "    knowledge centre: enabled (service optional)"
    ;;
esac

if [ -d frontend/node_modules ] && command -v npx >/dev/null 2>&1; then
  echo "==> TypeScript check"
  (cd frontend && npx --no-install tsc --noEmit --pretty false)
else
  echo "==> skipping TypeScript check (frontend/node_modules missing)"
fi

if [ "$CHECK_ONLY" -eq 1 ]; then
  echo "==> cargo check"
  cargo check --workspace "${CARGO_ARGS[@]+"${CARGO_ARGS[@]}"}"
  exit 0
fi

if command -v pnpm >/dev/null 2>&1; then
  echo "==> packaging Tauri app"
  (cd frontend && pnpm run tauri:build:cpu)
else
  echo "==> pnpm not found; falling back to cargo build --release"
  cargo build --release --workspace "${CARGO_ARGS[@]+"${CARGO_ARGS[@]}"}"
fi

echo "==> done"
