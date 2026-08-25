#!/usr/bin/env bash
set -euo pipefail

[ "$#" -eq 1 ] || {
  echo "Usage: $0 <bundle-directory|package-file>" >&2
  exit 2
}

target=$1
[ -e "$target" ] || { echo "Release target not found: $target" >&2; exit 1; }

fail=0
report() {
  echo "[release-audit] $*"
}
reject() {
  echo "[release-audit] ERROR: $*" >&2
  fail=1
}

if [ -d "$target" ]; then
  while IFS= read -r -d '' path; do
    case "$path" in
      *.rs|*.ts|*.tsx|*.map|*/.git/*|*/target/*|*/node_modules/*|*/Cargo.toml|*/Cargo.lock|*/package.json|*/pnpm-lock.yaml)
        reject "source or development file included: ${path#$target/}"
        ;;
    esac
  done < <(find "$target" -type f -print0)

  if find "$target" -type f -name '*.map' -print -quit | grep -q .; then
    reject "browser source maps are present"
  fi
else
  case "$target" in
    *.deb)
      if command -v dpkg-deb >/dev/null 2>&1; then
        while IFS= read -r path; do
          case "$path" in
            *.rs|*.ts|*.tsx|*.map|*/.git/*|*/target/*|*/node_modules/*|*/Cargo.toml|*/Cargo.lock|*/package.json|*/pnpm-lock.yaml)
              reject "source or development file included: $path"
              ;;
          esac
        done < <(dpkg-deb -c "$target" | awk '{print $6}')
      else
        report "dpkg-deb unavailable; package contents were not enumerated"
      fi
      ;;
    *.AppImage|*.appimage)
      report "AppImage is compressed; inspect its mounted contents during acceptance testing"
      ;;
  esac
fi

if [ -f "$target/SHA256SUMS" ]; then
  (cd "$target" && sha256sum -c SHA256SUMS >/dev/null) || reject "SHA256SUMS verification failed"
fi

if [ "$fail" -ne 0 ]; then
  exit 1
fi

report "passed: no source maps or common source/development files found"
