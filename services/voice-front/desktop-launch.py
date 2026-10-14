#!/usr/bin/python3
"""RK3588 X11 launcher: recover WebKit's stale initial fullscreen surface."""
import os
from pathlib import Path
import signal
import subprocess
import sys
import time


def main():
    env = os.environ.copy()
    env.setdefault("MEETINGBOX_ASR_BACKEND", "rk")
    env.setdefault("MEETINGBOX_RK_ASR_URL", "http://127.0.0.1:8020")
    env.setdefault("MEETINGBOX_WAKE_ENABLED", "1")
    env.setdefault("MEETINGBOX_VOICE_URL", "http://127.0.0.1:8021")
    process = subprocess.Popen(["/usr/bin/meetily", *sys.argv[1:]], env=env)

    def terminate(signum, frame):
        if process.poll() is None:
            process.send_signal(signum)

    signal.signal(signal.SIGTERM, terminate)
    signal.signal(signal.SIGINT, terminate)
    # Scope the workaround to this child's window; never remap unrelated windows.
    window = None
    if env.get("DISPLAY") and Path("/usr/bin/xdotool").exists():
        for _ in range(40):
            if process.poll() is not None:
                return process.returncode
            result = subprocess.run(["xdotool", "search", "--all", "--pid", str(process.pid), "--name", "^HYOS$"],
                                    capture_output=True, text=True, timeout=2, env=env)
            if result.returncode == 0 and result.stdout.strip():
                window = result.stdout.splitlines()[0]
                break
            time.sleep(0.25)
        if window:
            time.sleep(12)
            if process.poll() is None:
                subprocess.run(["xdotool", "windowunmap", window], capture_output=True, timeout=3, env=env)
                time.sleep(1)
                for args in [["windowmap", window], ["windowactivate", "--sync", window]]:
                    subprocess.run(["xdotool", *args], capture_output=True, timeout=3, env=env)
    return process.wait()


if __name__ == "__main__":
    raise SystemExit(main())
