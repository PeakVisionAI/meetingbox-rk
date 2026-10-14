import hashlib
import io
import json
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "services" / "rk-asr"))
from assets import verify_assets
from prepare import download, extract


class AssetTests(unittest.TestCase):
    def test_inventory_detects_tampering_and_path_escape(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            asset = directory / "model.rknn"
            asset.write_bytes(b"test only")
            manifest = {"schema": 1, "target": "rk3588", "backend": "sensevoice-rknn", "files": {
                "model.rknn": {"bytes": 9, "sha256": hashlib.sha256(b"test only").hexdigest()}}}
            (directory / "manifest.json").write_text(json.dumps(manifest))
            verify_assets(directory)
            asset.write_bytes(b"changed!!")
            with self.assertRaises(ValueError):
                verify_assets(directory)
            manifest["files"] = {"../model.rknn": {"bytes": 9, "sha256": "bad"}}
            (directory / "manifest.json").write_text(json.dumps(manifest))
            with self.assertRaises(ValueError):
                verify_assets(directory)

    def test_archive_rejects_escape_and_unsafe_links(self):
        for name, link in (("../escape", None), ("/absolute", None), ("link", "../../escape")):
            with tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                archive = directory / "input.tar.bz2"
                with tarfile.open(archive, "w:bz2") as output:
                    member = tarfile.TarInfo(name)
                    if link:
                        member.type = tarfile.SYMTYPE
                        member.linkname = link
                        output.addfile(member)
                    else:
                        member.size = 4
                        output.addfile(member, io.BytesIO(b"test"))
                with self.assertRaises(ValueError):
                    extract(archive, directory / "out")

    def test_extract_regular_files_and_reject_duplicates(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            archive = directory / "input.tar.bz2"
            with tarfile.open(archive, "w:bz2") as output:
                member = tarfile.TarInfo("bundle/tokens.txt")
                member.size = 4
                output.addfile(member, io.BytesIO(b"test"))
            extract(archive, directory / "out")
            self.assertEqual((directory / "out/bundle/tokens.txt").read_bytes(), b"test")
            with self.assertRaises(ValueError):
                extract(archive, directory / "out")


if __name__ == "__main__":
    unittest.main()
