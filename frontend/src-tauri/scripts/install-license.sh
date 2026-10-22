#!/usr/bin/env bash
set -euo pipefail

[ "$#" -eq 1 ] || { echo "Usage: sudo $0 <license.json>" >&2; exit 2; }
install -d -m 755 /etc/meetingbox
runtime_user="${MEETINGBOX_LICENSE_USER:-${SUDO_USER:-root}}"
if [ "$runtime_user" = "root" ]; then
  install -o root -g root -m 600 "$1" /etc/meetingbox/license.json
  echo "Installed /etc/meetingbox/license.json with root-only permissions"
else
  runtime_group=$(id -gn "$runtime_user")
  install -o root -g "$runtime_group" -m 640 "$1" /etc/meetingbox/license.json
  echo "Installed /etc/meetingbox/license.json for runtime user $runtime_user (group $runtime_group)"
fi
