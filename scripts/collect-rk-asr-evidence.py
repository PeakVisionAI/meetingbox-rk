#!/usr/bin/env python3
"""Collect target runtime inventory and optional NPU activity during a probe."""
import argparse
import json
import platform
import subprocess
import sys
import threading
import time
from pathlib import Path


def read_if_available(path):
    try:
        return Path(path).read_text(encoding="utf-8", errors="replace")[:16384].strip()
    except OSError:
        return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--audio", type=Path, help="Optional WAV sent to the loopback service during activity sampling")
    parser.add_argument("--assets", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        parser.error("output already exists; select a new path")
    report = {
        "host": {"system": platform.system(), "machine": platform.machine(), "kernel": platform.release(),
                 "os_release": read_if_available("/etc/os-release"), "memory": read_if_available("/proc/meminfo")},
        "driver": {"version": read_if_available("/sys/kernel/debug/rknpu/version"),
                   "nodes": [str(path) for path in Path("/dev/dri").glob("renderD*")]},
        "hardware_verified": False,
        "note": "Inventory and activity samples require manual correlation with actual inference; no automatic hardware pass.",
    }
    if args.assets:
        manifest = args.assets / "manifest.json"
        report["assets"] = json.loads(manifest.read_text(encoding="utf-8"))
    samples = []
    stop = threading.Event()

    def collect():
        started = time.monotonic()
        while not stop.is_set():
            samples.append({"elapsed_seconds": round(time.monotonic() - started, 3),
                            "load": read_if_available("/sys/kernel/debug/rknpu/load")})
            stop.wait(0.1)

    if args.audio:
        thread = threading.Thread(target=collect)
        thread.start()
        try:
            result = subprocess.run([sys.executable, str(Path(__file__).with_name("check-rk-asr.py")),
                                     "--audio", str(args.audio)], capture_output=True, text=True, timeout=100)
            report["probe"] = json.loads(result.stdout)
            report["probe_exit_code"] = result.returncode
        except (subprocess.TimeoutExpired, ValueError):
            report["probe_error"] = "probe failed or exceeded evidence timeout"
        finally:
            stop.set()
            thread.join()
        report["npu_activity"] = samples
    with args.output.open("x", encoding="utf-8") as target:
        json.dump(report, target, ensure_ascii=False, indent=2)
        target.write("\n")
    print("Evidence written to " + str(args.output))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
