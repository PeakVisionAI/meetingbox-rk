#!/usr/bin/env bash
set -euo pipefail

# Build only the distributable application. Source trees, Cargo targets and
# signing keys remain outside the artifact directory.
repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
frontend_dir="$repo_root/frontend"
bundle_dir="$frontend_dir/src-tauri/target/release/bundle"
if [ ! -d "$bundle_dir" ] && [ -d "$repo_root/target/release/bundle" ]; then
  bundle_dir="$repo_root/target/release/bundle"
fi

command -v pnpm >/dev/null 2>&1 || {
  echo "pnpm is required to build the release" >&2
  exit 1
}

export NODE_ENV=production
export NEXT_TELEMETRY_DISABLED=1
export RUSTFLAGS="${RUSTFLAGS:-} -C debuginfo=0"

echo "Building Tauri release..."
pnpm --dir "$frontend_dir" tauri build

[ -d "$bundle_dir" ] || {
  echo "Tauri bundle directory was not created: $bundle_dir" >&2
  exit 1
}

# Keep package files auditable and reproducible.
find "$bundle_dir" -type f ! -name SHA256SUMS -print0 | sort -z | xargs -0 sha256sum > "$bundle_dir/SHA256SUMS"

"$repo_root/scripts/audit-release.sh" "$bundle_dir"
echo "Release artifacts and SHA256SUMS are in $bundle_dir"
