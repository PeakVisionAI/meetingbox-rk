#!/usr/bin/env python3
"""Download pinned upstream assets, validate archives, and write a local inventory."""
import argparse
import json
import shutil
import tarfile
import tempfile
import urllib.request
from pathlib import Path
from assets import sha256, verify_assets

MAX_EXPANDED = 2 * 1024**3


def download(source, output):
    request = urllib.request.Request(source["url"], headers={"User-Agent": "meetingbox-rk-assets/1"})
    size = 0
    with urllib.request.urlopen(request, timeout=60) as response, output.open("xb") as target:
        if not response.geturl().startswith("https://"):
            raise ValueError("asset redirect must use HTTPS")
        for block in iter(lambda: response.read(1024 * 1024), b""):
            size += len(block)
            if size > source["bytes"]:
                raise ValueError("download exceeds pinned asset size")
            target.write(block)
    if size != source["bytes"] or sha256(output) != source["sha256"]:
        raise ValueError("upstream checksum mismatch")


def extract(archive, output):
    total = 0
    with tarfile.open(archive, "r:bz2") as source:
        for member in source:
            relative = Path(member.name)
            if relative.is_absolute() or ".." in relative.parts:
                raise ValueError("unsafe archive path")
            if member.isdir():
                continue
            # Only required regular files; upstream .so link aliases are created separately.
            if member.issym():
                link = Path(member.linkname)
                if link.is_absolute() or ".." in link.parts:
                    raise ValueError("unsafe archive symlink")
                continue
            if not member.isfile():
                raise ValueError("unsupported archive member")
            total += member.size
            if total > MAX_EXPANDED:
                raise ValueError("archive exceeds expanded size budget")
            target = output / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            if target.exists():
                raise ValueError("duplicate archive member")
            with source.extractfile(member) as item, target.open("xb") as file:
                shutil.copyfileobj(item, file)
            target.chmod(0o644)


def prepare(output, cache=None):
    output = output.resolve()
    if output.exists():
        raise ValueError("output exists; assets are never overwritten")
    output.parent.mkdir(parents=True, exist_ok=True)
    if shutil.disk_usage(output.parent).free < 2 * 1024**3:
        raise ValueError("at least 2 GiB free disk space required")
    upstream = json.loads(Path(__file__).with_name("upstream.json").read_text())
    required = {"model", "sherpa", "header", "rknnrt", "sherpa_license", "rknn_license", "model_license"}
    if set(upstream["sources"]) != required:
        raise ValueError("upstream pins are incomplete")
    with tempfile.TemporaryDirectory(prefix=".meetingbox-assets-", dir=output.parent) as temporary:
        work = Path(temporary)
        destination = work / "assets"
        destination.mkdir()
        for key, entry in upstream["sources"].items():
            print("Preparing " + key, flush=True)
            archive = work / (key + ".download")
            if cache:
                cached = cache / key
                if not cached.is_file() or cached.stat().st_size != entry["bytes"] or sha256(cached) != entry["sha256"]:
                    raise ValueError("offline cache checksum mismatch: " + key)
                shutil.copyfile(cached, archive)
            else:
                download(entry, archive)
            if entry.get("archive"):
                unpacked = work / key
                unpacked.mkdir()
                extract(archive, unpacked)
                if key == "model":
                    for name in ("model.rknn", "tokens.txt"):
                        matches = list(unpacked.rglob(name))
                        if len(matches) != 1:
                            raise ValueError("expected exactly one " + name)
                        shutil.copyfile(matches[0], destination / name)
                    for index, license_path in enumerate(unpacked.rglob("LICENSE*")):
                        if license_path.is_file():
                            shutil.copyfile(license_path, destination / ("model-bundle-LICENSE-" + str(index)))
                else:
                    library_dir = destination / "lib"
                    library_dir.mkdir()
                    for path in unpacked.rglob("*.so*"):
                        if path.is_file():
                            target = library_dir / path.name
                            if target.exists():
                                raise ValueError("duplicate runtime library")
                            shutil.copyfile(path, target)
                    # Linker names for versioned SONAME files, without trusting tar links.
                    for library in list(library_dir.glob("*.so.*")):
                        alias = library_dir / (library.name.split(".so.")[0] + ".so")
                        if not alias.exists():
                            shutil.copyfile(library, alias)
                    if not (library_dir / "libsherpa-onnx-c-api.so").exists():
                        raise ValueError("sherpa C API shared library missing")
            else:
                target = destination / entry["path"]
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(archive, target)
            archive.unlink()
        inventory = {}
        for path in sorted(destination.rglob("*")):
            if path.is_file():
                inventory[str(path.relative_to(destination))] = {"bytes": path.stat().st_size, "sha256": sha256(path)}
        manifest = {
            "schema": 1, "backend": "sensevoice-rknn", "target": "rk3588",
            "model_id": upstream["model_id"], "model_file": "model.rknn", "tokens_file": "tokens.txt",
            "sherpa_onnx_version": upstream["sherpa_onnx_version"], "sources": upstream["sources"],
            "licenses": upstream["licenses"], "files": inventory,
        }
        (destination / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        verify_assets(destination)
        # Rename atomically, refusing a concurrent output directory.
        if output.exists():
            raise ValueError("output appeared during preparation")
        destination.rename(output)
    print("Verified assets: " + str(output))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--cache", type=Path, help="Offline cache containing files named by source keys")
    args = parser.parse_args()
    try:
        prepare(args.output, args.cache)
    except (OSError, ValueError, tarfile.TarError) as error:
        parser.exit(1, "Asset preparation failed: " + str(error) + "\n")


if __name__ == "__main__":
    main()
