#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "Usage: MEETINGBOX_LICENSE_PRIVATE_KEY=/secure/key.pem $0 --device-id <fingerprint> --issued-at <RFC3339> [--expires-at <RFC3339>] --output <license.json>" >&2
  exit 2
}

device_id=""
issued_at=""
expires_at=""
output=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --device-id) device_id="${2:-}"; shift 2 ;;
    --issued-at) issued_at="${2:-}"; shift 2 ;;
    --expires-at) expires_at="${2:-}"; shift 2 ;;
    --output) output="${2:-}"; shift 2 ;;
    *) usage ;;
  esac
done

private_key="${MEETINGBOX_LICENSE_PRIVATE_KEY:-$HOME/.config/meetingbox-license/ed25519-private.pem}"
[ -f "$private_key" ] || { echo "Private key not found: $private_key" >&2; exit 1; }
[ -n "$device_id" ] && [ -n "$issued_at" ] && [ -n "$output" ] || usage
case "$device_id" in *[!a-zA-Z0-9:_-]*|'') echo "Invalid device id" >&2; exit 2 ;; esac
case "$issued_at${expires_at}" in *[!0-9T:+-Z.]* ) echo "Invalid RFC3339 timestamp" >&2; exit 2 ;; esac

payload_file=$(mktemp)
signature_file=$(mktemp)
trap 'rm -f "$payload_file" "$signature_file"' EXIT
if [ -n "$expires_at" ]; then
  printf '{"schema":1,"product":"meetingbox-rk","device_id":"%s","issued_at":"%s","expires_at":"%s"}' "$device_id" "$issued_at" "$expires_at" > "$payload_file"
else
  printf '{"schema":1,"product":"meetingbox-rk","device_id":"%s","issued_at":"%s","expires_at":null}' "$device_id" "$issued_at" > "$payload_file"
fi
openssl pkeyutl -sign -rawin -inkey "$private_key" -in "$payload_file" -out "$signature_file"
payload=$(base64 < "$payload_file" | tr -d '\n')
signature=$(base64 < "$signature_file" | tr -d '\n')
umask 077
printf '{"payload":"%s","signature":"%s"}\n' "$payload" "$signature" > "$output"
echo "Wrote signed license: $output"
