#!/usr/bin/env python3
"""Build the thin C bridge on target Linux/aarch64 after asset verification."""
import argparse
import json
import os
import platform
import subprocess
from pathlib import Path
from assets import sha256, verify_assets


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--assets", type=Path, required=True)
    args = parser.parse_args()
    if platform.system() != "Linux" or platform.machine() not in ("aarch64", "arm64"):
        parser.error("build on the target Linux/aarch64 device")
    directory = args.assets.resolve()
    manifest = verify_assets(directory)
    service = Path(__file__).resolve().parent
    target = directory / "libmeetingbox_rknn.so"
    if target.exists():
        parser.error("bridge exists; review/remove it before rebuilding")
    subprocess.run(["cc", "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-shared", "-fPIC",
                    "-I" + str(directory / "include"), str(service / "native.c"),
                    "-L" + str(directory / "lib"), "-lsherpa-onnx-c-api",
                    "-Wl,-rpath,$ORIGIN/lib", "-o", str(target)], check=True)
    result = subprocess.run(["ldd", str(target)], check=True, capture_output=True, text=True,
                            env={**os.environ, "LD_LIBRARY_PATH": str(directory / "lib")})
    print(result.stdout, end="")
    if "not found" in result.stdout:
        target.unlink()
        raise RuntimeError("native dependencies missing; bridge removed")
    manifest["files"][target.name] = {"bytes": target.stat().st_size, "sha256": sha256(target)}
    pending = directory / "manifest.build.json"
    pending.write_text(json.dumps(manifest, indent=2) + "\n")
    pending.replace(directory / "manifest.json")
    print("Bridge built and inventoried; start server to validate actual RKNN initialization.")


if __name__ == "__main__":
    main()
