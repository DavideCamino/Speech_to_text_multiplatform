"""Offline regression checks for the Windows-only vendored dependency fix."""
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location("whisper_patch", Path(__file__).with_name("patch-whisper.py"))
PATCH = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PATCH)


class WhisperPatchTests(unittest.TestCase):
    def fixture(self, root, source=None, version=PATCH.VERSION):
        vendor = root / "vendor"
        crate = vendor / f"whisper-rs-sys-{PATCH.VERSION}"
        crate.mkdir(parents=True)
        (crate / "Cargo.toml").write_text(
            f'[package]\nname = "whisper-rs-sys"\nversion = "{version}"\n', encoding="utf-8")
        source = PATCH.ORIGINAL if source is None else source
        (crate / "build.rs").write_bytes(source)
        checksums = {"package": "a" * 64, "files": {
            "build.rs": hashlib.sha256(source).hexdigest(), "unrelated.txt": "b" * 64}}
        (crate / ".cargo-checksum.json").write_text(json.dumps(checksums), encoding="utf-8")
        return vendor, crate, checksums

    def test_patch_limits_flag_to_msvc_and_updates_only_its_file_checksum(self):
        with tempfile.TemporaryDirectory() as directory:
            vendor, crate, original = self.fixture(Path(directory))
            build = PATCH.apply(vendor)
            patched = build.read_bytes()
            self.assertEqual(patched, PATCH.REPLACEMENT)
            self.assertIn(b'if target.ends_with("-msvc")', patched)
            self.assertIn(b'println!("cargo:rustc-link-lib=advapi32");', patched)
            checksums = json.loads((crate / ".cargo-checksum.json").read_text())
            self.assertEqual(checksums["files"]["build.rs"], hashlib.sha256(patched).hexdigest())
            self.assertEqual(checksums["package"], original["package"])
            self.assertEqual(checksums["files"]["unrelated.txt"], original["files"]["unrelated.txt"])

    def test_unexpected_upstream_source_fails_without_modifying_files(self):
        for source in [b"changed upstream", PATCH.ORIGINAL * 2, PATCH.REPLACEMENT]:
            with self.subTest(source=source), tempfile.TemporaryDirectory() as directory:
                vendor, crate, _ = self.fixture(Path(directory), source)
                checksum = (crate / ".cargo-checksum.json").read_bytes()
                with self.assertRaisesRegex(ValueError, "exactly once"):
                    PATCH.apply(vendor)
                self.assertEqual((crate / "build.rs").read_bytes(), source)
                self.assertEqual((crate / ".cargo-checksum.json").read_bytes(), checksum)

    def test_bad_original_checksum_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            vendor, crate, _ = self.fixture(Path(directory))
            (crate / "build.rs").write_bytes(PATCH.ORIGINAL + b"// unexpected edit\n")
            before = (crate / "build.rs").read_bytes()
            with self.assertRaisesRegex(ValueError, "original file checksum"):
                PATCH.apply(vendor)
            self.assertEqual((crate / "build.rs").read_bytes(), before)

    def test_version_changes_require_patch_review(self):
        with tempfile.TemporaryDirectory() as directory:
            vendor, _, _ = self.fixture(Path(directory), version="0.16.0")
            with self.assertRaisesRegex(ValueError, "version"):
                PATCH.apply(vendor)


if __name__ == "__main__":
    unittest.main()
