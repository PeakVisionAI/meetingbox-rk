#!/usr/bin/env bash
set -euo pipefail

read_identity() {
  local path=$1 prefix=$2 value
  if [ -r "$path" ]; then
    value=$(tr -d '\000\r\n' < "$path")
    if [ -n "$value" ]; then
      printf '%s:%s\n' "$prefix" "$value"
    fi
  fi
}

source=$( {
  read_identity /sys/firmware/devicetree/base/serial-number hardware
  read_identity /sys/class/dmi/id/product_uuid hardware
  read_identity /sys/class/dmi/id/product_serial hardware
  if [ -r /proc/cpuinfo ]; then
    serial=$(awk -F: 'tolower($1) ~ /^serial[[:space:]]*$/ { gsub(/^[[:space:]]+|[[:space:]]+$/, "", $2); print $2; exit }' /proc/cpuinfo)
    [ -z "$serial" ] || printf 'cpu:%s\n' "$serial"
  fi
  read_identity /etc/machine-id system
  read_identity /var/lib/dbus/machine-id system
} | awk '!seen[$0]++' | paste -sd '|' - )
[ -n "$source" ] || { echo "No stable device identity found" >&2; exit 1; }
printf '%s' "$source" | sha256sum | awk '{print "sha256:" $1}'
