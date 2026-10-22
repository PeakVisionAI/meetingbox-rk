#!/usr/bin/env bash
# On-device installer for the RK bridge stack.
#
#   sudo bash install.sh            # install, enable services, run health check
#   sudo bash install.sh --no-start # lay down files only
#
# Expects the tarball layout produced by deploy/rk/make-bundle.sh:
#   install.sh
#   services/asrserve-bridge/...
#   services/rk-asr/...
#   config/profile-rk-bridge.json
#
# Idempotent: re-running replaces the code and restarts both units.
set -euo pipefail

PREFIX="${PREFIX:-/opt/meetingbox-rk}"
DATA_DIR="${DATA_DIR:-/var/lib/meetingbox}"
SERVICE_USER="${SERVICE_USER:-meetingbox}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
START=1
[ "${1:-}" = "--no-start" ] && START=0

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || die "run with sudo"
[ -d "$HERE/services/asrserve-bridge" ] || die "tarball incomplete: services/asrserve-bridge missing"

ARCH="$(uname -m)"
log "arch=$ARCH prefix=$PREFIX"
[ "$ARCH" = "aarch64" ] || log "warning: expected aarch64, found $ARCH (continuing)"

log "installing code to $PREFIX"
mkdir -p "$PREFIX/services" "$DATA_DIR"
cp -a "$HERE/services/asrserve-bridge" "$PREFIX/services/"
cp -a "$HERE/services/rk-asr" "$PREFIX/services/"
mkdir -p "$PREFIX/config"
cp -a "$HERE/config/profile-rk-bridge.json" "$PREFIX/config/"

id "$SERVICE_USER" >/dev/null 2>&1 || {
  log "creating service user $SERVICE_USER"
  useradd --system --home "$PREFIX" --shell /usr/sbin/nologin "$SERVICE_USER" 2>/dev/null \
    || adduser -S -h "$PREFIX" -s /sbin/nologin "$SERVICE_USER"
}

log "preparing python venv"
if [ ! -x "$PREFIX/venv/bin/python" ]; then
  python3 -m venv "$PREFIX/venv" || die "python3-venv missing (apt install python3-venv)"
fi
# python-multipart is a hard requirement for the FastAPI form endpoints.
if ! "$PREFIX/venv/bin/python" -c 'import fastapi, uvicorn, httpx, pydantic, multipart' 2>/dev/null; then
  log "installing python dependencies (needs network)"
  "$PREFIX/venv/bin/pip" install --quiet --disable-pip-version-check \
    fastapi uvicorn httpx pydantic python-multipart \
    || die "pip install failed; if the device is offline, copy wheels with 'pip download' first"
else
  log "python dependencies already present"
fi

# Optional enrichment deps: absence is fine, features degrade.
"$PREFIX/venv/bin/python" -c 'import sherpa_onnx' 2>/dev/null \
  || log "note: sherpa-onnx absent -> speaker labels and punctuation refinement stay off"

log "installing systemd units"
install -m644 "$HERE/services/rk-asr/deploy/meetingbox-rk-asr.service" /etc/systemd/system/ 2>/dev/null \
  || log "warning: rk-asr unit not found in tarball"
install -m644 "$HERE/services/asrserve-bridge/deploy/meetingbox-bridge.service" /etc/systemd/system/

chown -R "$SERVICE_USER:$SERVICE_USER" "$DATA_DIR"
systemctl daemon-reload

if [ -f /etc/systemd/system/meetingbox-rk-asr.service ]; then
  systemctl enable meetingbox-rk-asr >/dev/null 2>&1 || true
  [ "$START" -eq 1 ] && systemctl restart meetingbox-rk-asr
fi
systemctl enable meetingbox-bridge >/dev/null 2>&1 || true

if [ "$START" -eq 0 ]; then
  log "files installed; services not started (--no-start)"
  exit 0
fi

log "starting bridge"
systemctl restart meetingbox-bridge
sleep 3

log "health"
if command -v curl >/dev/null 2>&1; then
  curl -fsS --max-time 5 http://127.0.0.1:8020/healthz 2>/dev/null \
    && echo || echo "rk-asr not answering on :8020 (models prepared? see below)"
  curl -fsS --max-time 5 http://127.0.0.1:8030/healthz 2>/dev/null \
    && echo || echo "bridge not answering on :8030 (journalctl -u meetingbox-bridge -n 50)"
fi

cat <<EOF

Next:
  1) prepare the NPU assets (once, needs network + ~2 GiB free):
       cd $PREFIX && python3 -B services/rk-asr/prepare.py --output services/rk-asr/models
       python3 -B services/rk-asr/build.py --assets services/rk-asr/models
       systemctl restart meetingbox-rk-asr
  2) smoke test:
       cd $PREFIX/services/asrserve-bridge
       python3 smoke_rk.py --health-only
       python3 smoke_rk.py --tone 3
       python3 smoke_rk.py --wav /path/to/sample.wav
  3) point the desktop app at this profile:
       MEETINGBOX_CONFIG=$PREFIX/config/profile-rk-bridge.json
EOF
