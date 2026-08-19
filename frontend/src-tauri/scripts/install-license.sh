#!/usr/bin/env bash
set -euo pipefail

[ "$#" -eq 1 ] || { echo "Usage: sudo $0 <license.json>" >&2; exit 2; }
install -d -m 755 /etc/meetingbox
install -m 644 "$1" /etc/meetingbox/license.json
echo "Installed /etc/meetingbox/license.json"
