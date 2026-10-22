#!/usr/bin/env bash
set -euo pipefail

[ "$#" -eq 2 ] || { echo "Usage: sudo $0 <app-package.deb|appimage> <license.json>" >&2; exit 2; }
package_path=$1
license_path=$2

[ -f "$package_path" ] || { echo "Package not found: $package_path" >&2; exit 1; }
[ -f "$license_path" ] || { echo "License not found: $license_path" >&2; exit 1; }

"$(dirname "$0")/install-license.sh" "$license_path"
"$(dirname "$0")/install-power-policy.sh"
if [[ "$package_path" == *.deb ]]; then
  apt-get install -y "$package_path"
else
  install -d -m 755 /opt/meetingbox
  install -m 755 "$package_path" /opt/meetingbox/meetingbox.AppImage
fi

echo "Release installed. Do not copy source trees, Cargo target directories, or private keys to the customer device."
