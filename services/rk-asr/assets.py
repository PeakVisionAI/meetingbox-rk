"""Local asset verification shared by provisioning and runtime startup."""
import hashlib
import json
from pathlib import Path


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def verify_assets(directory):
    directory = Path(directory).resolve()
    manifest = json.loads((directory / "manifest.json").read_text(encoding="utf-8"))
    if manifest.get("schema") != 1 or manifest.get("target") != "rk3588" or manifest.get("backend") != "sensevoice-rknn":
        raise ValueError("unsupported asset manifest")
    files = manifest.get("files")
    if not isinstance(files, dict) or not files:
        raise ValueError("asset inventory is empty")
    for key in ("model_file", "tokens_file"):
        if key in manifest and manifest[key] not in files:
            raise ValueError("selected asset is outside inventory")
    for name, info in files.items():
        relative = Path(name)
        if relative.is_absolute() or ".." in relative.parts:
            raise ValueError("invalid asset path")
        path = directory / relative
        if not path.is_file() or path.is_symlink() or directory not in path.resolve().parents:
            raise ValueError("missing or unsafe asset")
        if path.stat().st_size != info["bytes"] or sha256(path) != info["sha256"]:
            raise ValueError("asset integrity mismatch: " + name)
    return manifest
