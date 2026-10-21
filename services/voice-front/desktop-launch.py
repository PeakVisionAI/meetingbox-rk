#!/usr/bin/python3
"""Start the Linux desktop with local ASR and idle voice wake defaults."""
import os
from pathlib import Path
import signal
import subprocess
import sys


def main():
    env = os.environ.copy()
    env.setdefault("MEETINGBOX_ASR_BACKEND", "rk")
    env.setdefault("MEETINGBOX_RK_ASR_URL", "http://127.0.0.1:8020")
    env.setdefault("MEETINGBOX_WAKE_ENABLED", "1")
    env.setdefault("MEETINGBOX_VOICE_URL", "http://127.0.0.1:8021")
    if env.get("DISPLAY") and not env.get("WAYLAND_DISPLAY"):
        try:
            compatible = Path("/proc/device-tree/compatible").read_bytes().split(b"\0")
        except OSError:
            compatible = []
        if b"rockchip,rk3588" in compatible:
            # DMA-BUF causes large stale black regions with the RK3588 Mali X11 stack.
            env.setdefault("WEBKIT_DISABLE_DMABUF_RENDERER", "1")
    process = subprocess.Popen(["/usr/bin/meetily", *sys.argv[1:]], env=env)

    def terminate(signum, frame):
        if process.poll() is None:
            process.send_signal(signum)

    signal.signal(signal.SIGTERM, terminate)
    signal.signal(signal.SIGINT, terminate)
    return process.wait()


if __name__ == "__main__":
    raise SystemExit(main())
