"""Tests for the source archive boundary and license/pin checks."""
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

from document_engines import check_notices, pinned_engines


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


creator = load("source_creator", "create-offline-source.py")
verifier = load("source_verifier", "verify-offline-source.py")


class ReleaseToolsTests(unittest.TestCase):
    def test_copy_excludes_local_credentials_and_build_outputs(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "repo"
            root.mkdir()
            for name in ["Cargo.toml", ".cargo/credentials.toml", ".aws/credentials",
                         "target/app", "crates/app/src/main.rs", "tools/__pycache__/cache"]:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("test")
            destination = Path(temporary) / "source"
            creator.copy_project(root, destination)
            self.assertTrue((destination / "crates/app/src/main.rs").is_file())
            self.assertEqual(sorted(str(p.relative_to(destination)) for p in destination.rglob("*")
                                    if p.is_file()), ["Cargo.toml", "crates/app/src/main.rs"])

    def test_archive_roundtrip_and_tamper_detection(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            stage = directory / "pathpilot-test"
            stage.mkdir()
            for name in ["vendor", "assets", "data/licenses", ".cargo"]:
                (stage / name).mkdir(parents=True)
            (stage / ".cargo/config.toml").write_text("# test")
            (stage / "Cargo.lock").write_text("locked")
            notice = b"copyright example"
            (stage / "data/licenses/MIT").write_bytes(notice)
            manifest = {"cargo_lock_sha256": hashlib.sha256(b"locked").hexdigest(),
                        "notice_sha256": {"MIT": hashlib.sha256(notice).hexdigest()}}
            (stage / "SOURCE-MANIFEST.json").write_text(json.dumps(manifest))
            archive = directory / "source.tar.gz"
            creator.write_archive(stage, archive)
            extracted = verifier.unpack(archive, directory / "unpacked")
            self.assertEqual((extracted / "Cargo.lock").read_text(), "locked")
            (stage / "data/licenses/MIT").write_text("changed")
            creator.write_archive(stage, archive)
            with self.assertRaisesRegex(ValueError, "notice hash mismatch"):
                verifier.unpack(archive, directory / "changed-notice")
            archive.write_bytes(archive.read_bytes() + b"corruption")
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                verifier.unpack(archive, directory / "corrupt")

    def test_notices_reject_changed_missing_or_extra_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            licenses = root / "data/licenses"
            licenses.mkdir(parents=True)
            expected = {"MIT": b"copyright"}
            with self.assertRaises(ValueError):
                check_notices(root, expected)
            (licenses / "MIT").write_bytes(expected["MIT"])
            check_notices(root, expected)
            (licenses / "MIT").write_text("changed")
            with self.assertRaises(ValueError):
                check_notices(root, expected)
            (licenses / "MIT").write_bytes(expected["MIT"])
            (licenses / "extra").write_text("new")
            with self.assertRaises(ValueError):
                check_notices(root, expected)

    def test_pins_reject_mismatched_engine_modules(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            content = '[workspace.dependencies]\n'
            for name in ["wordcraft-model", "wordcraft-fonts", "gridcraft-model"]:
                engine = name.split('-')[0]
                content += f'{name} = {{ git = "https://github.com/storytold/{engine}", rev = "{"a" * 40}" }}\n'
            (root / "Cargo.toml").write_text(content)
            self.assertEqual(pinned_engines(root)["wordcraft"]["revision"], "a" * 40)
            (root / "Cargo.toml").write_text(content.replace('"' + 'a' * 40 + '"', '"' + 'b' * 40 + '"', 1))
            with self.assertRaisesRegex(ValueError, "same full commit SHA"):
                pinned_engines(root)


if __name__ == "__main__":
    unittest.main()
