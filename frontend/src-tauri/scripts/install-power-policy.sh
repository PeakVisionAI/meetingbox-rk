#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "Power policy installation is only needed on Linux."
  exit 0
fi

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
rules_path=/etc/polkit-1/rules.d/50-meetingbox-power.rules
pkla_path=/etc/polkit-1/localauthority/50-local.d/50-meetingbox-power.pkla
app_user=${1:-${SUDO_USER:-}}

if [[ ! -d /etc/polkit-1/rules.d && ! -d /etc/polkit-1/localauthority ]]; then
  echo "polkit policy directories are not available; install polkit before enabling power controls." >&2
  exit 1
fi

if [[ -z "$app_user" || ! "$app_user" =~ ^[a-z_][a-z0-9_-]*[$]?$ ]]; then
  echo "Usage: sudo $0 [application-user]" >&2
  exit 2
fi

temporary_rules=$(mktemp)
temporary_pkla=$(mktemp)
trap 'rm -f "$temporary_rules" "$temporary_pkla"' EXIT

if [[ -d /etc/polkit-1/rules.d ]]; then
  sed "s/@MEETINGBOX_POWER_USER@/$app_user/g" \
    "$script_dir/50-meetingbox-power.rules" > "$temporary_rules"
  install -m 0644 "$temporary_rules" "$rules_path"
  echo "Installed $rules_path"
fi

# Ubuntu 22.04's polkit 0.105 uses .pkla policies rather than JavaScript rules.
# Provision its directory even when the image only contains rules.d.
install -d -m 0755 "$(dirname "$pkla_path")"
sed "s/@MEETINGBOX_POWER_USER@/$app_user/g" \
  "$script_dir/50-meetingbox-power.pkla" > "$temporary_pkla"
install -m 0644 "$temporary_pkla" "$pkla_path"
echo "Installed $pkla_path"

if command -v systemctl >/dev/null 2>&1; then
  systemctl try-restart polkit.service || true
fi

echo "Power controls enabled for application user $app_user"
