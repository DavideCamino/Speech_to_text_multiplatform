"""Platform-neutral checks for the license collector used by both packages."""
import importlib.util
import json
import ntpath
from pathlib import Path, PureWindowsPath
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
SCRIPT = HERE / "install-licenses.py"
SPEC = importlib.util.spec_from_file_location("dependency_licenses", SCRIPT)
LICENSES = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(LICENSES)


class LicenseInstallerTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.vendor = self.root / "vendor"
        self.vendor.mkdir()
        self.destination = self.root / "output/dependencies"

    def put(self, relative, content):
        path = self.vendor / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
        return path

    def crate(self, directory="example-1.0", extra='license = "MIT"\n'):
        self.put(f"{directory}/Cargo.toml",
                 f'[package]\nname = "{directory}"\nversion = "1.0.0"\n{extra}')

    def test_standard_notices_preserve_contents_and_relative_paths(self):
        self.crate()
        paths = ["LICENSE", "LICENCE.txt", "COPYING", "NOTICE.md", "COPYRIGHT",
                 "nested/License-MIT", "whisper.cpp/LICENSE"]
        for relative in paths:
            self.put(f"example-1.0/{relative}", f"Notice for {relative}\n")
        self.put("example-1.0/src/lib.rs", "not a notice")
        self.put("example-1.0/README.md", "not a notice")
        LICENSES.install(self.vendor, self.destination)
        copied = {path.relative_to(self.destination).as_posix()
                  for path in self.destination.rglob("*") if path.is_file()}
        self.assertEqual(copied, {f"example-1.0/{path}" for path in paths} | {"packages.json"})
        for relative in paths:
            self.assertEqual((self.destination / "example-1.0" / relative).read_bytes(),
                             (self.vendor / "example-1.0" / relative).read_bytes())

    def test_nested_license_directories_include_arbitrary_filenames(self):
        self.crate()
        paths = ["licenses/MIT.txt", "nested/Licences/third-party/terms.txt"]
        for relative in paths:
            self.put(f"example-1.0/{relative}", "third-party terms")
        LICENSES.install(self.vendor, self.destination)
        for relative in paths:
            self.assertEqual((self.destination / "example-1.0" / relative).read_text(),
                             "third-party terms")

    def test_declared_license_file_and_missing_license_metadata(self):
        self.crate(extra='license-file = "legal/terms.txt"\n')
        self.put("example-1.0/legal/terms.txt", "custom license")
        self.put("example-1.0/legal/unrelated.txt", "do not copy")
        LICENSES.install(self.vendor, self.destination)
        self.assertEqual((self.destination / "example-1.0/legal/terms.txt").read_text(), "custom license")
        self.assertFalse((self.destination / "example-1.0/legal/unrelated.txt").exists())
        self.assertEqual(json.loads((self.destination / "packages.json").read_text()), [
            {"name": "example-1.0", "version": "1.0.0", "license": "See included license files"}])

    def test_package_metadata_is_ordered_by_crate_directory(self):
        self.crate("z-last", 'license = "MIT OR Apache-2.0"\n')
        self.crate("a-first", 'license = "MPL-2.0"\n')
        self.put("not-a-crate.txt", "ignored")
        LICENSES.install(self.vendor, self.destination)
        metadata = (self.destination / "packages.json").read_text()
        self.assertTrue(metadata.endswith("\n"))
        self.assertEqual(json.loads(metadata), [
            {"name": "a-first", "version": "1.0.0", "license": "MPL-2.0"},
            {"name": "z-last", "version": "1.0.0", "license": "MIT OR Apache-2.0"}])

    def test_empty_vendor_still_writes_package_inventory(self):
        LICENSES.install(self.vendor, self.destination)
        self.assertEqual((self.destination / "packages.json").read_text(), "[]\n")
        self.assertEqual([path.name for path in self.destination.iterdir()], ["packages.json"])

    def test_path_normalization_does_not_resolve_symlinks(self):
        path = self.vendor / "example-1.0/nested/../LICENSE"
        with patch.object(Path, "resolve", side_effect=AssertionError("must remain lexical")):
            self.assertEqual(LICENSES.normalized_path(path), self.vendor / "example-1.0/LICENSE")

    def test_license_paths_normalize_mixed_windows_separators(self):
        # Reproduce scandir's mixed strings from the Windows failure. Emulate
        # Windows lexical paths on Linux, without accessing a Windows drive.
        parent = r"D:/a/_temp/msys64/tmp/vendor\example-1.0"
        child = parent + "/nested/COPYING"
        with patch.object(LICENSES, "normpath", ntpath.normpath), \
                patch.object(LICENSES, "Path", wraps=PureWindowsPath) as factory:
            crate = LICENSES.normalized_path(parent)
            source = LICENSES.normalized_path(child)
            self.assertEqual(source.relative_to(crate).as_posix(), "nested/COPYING")
            # Check that strings are normalized *before* Path construction,
            # not merely that PureWindowsPath can parse a mixed input itself.
            self.assertEqual(factory.call_args_list[0].args, (ntpath.normpath(parent),))
            self.assertEqual(factory.call_args_list[1].args, (ntpath.normpath(child),))

    def test_cli_installs_into_paths_with_spaces(self):
        self.crate()
        self.put("example-1.0/LICENSE", "dependency notice")
        destination = self.root / "package with spaces/licenses"
        subprocess.run([sys.executable, str(SCRIPT), str(self.vendor), str(destination)],
                       check=True, capture_output=True, text=True)
        self.assertEqual((destination / "example-1.0/LICENSE").read_text(), "dependency notice")
        self.assertEqual(json.loads((destination / "packages.json").read_text()), [
            {"name": "example-1.0", "version": "1.0.0", "license": "MIT"}])

    def test_cli_rejects_wrong_argument_counts(self):
        for args in [[], [str(self.vendor)], [str(self.vendor), str(self.destination), "extra"]]:
            with self.subTest(args=args):
                result = subprocess.run([sys.executable, str(SCRIPT), *args],
                                        capture_output=True, text=True)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("Usage: install-licenses.py VENDOR_DIRECTORY DESTINATION", result.stderr)
                self.assertFalse(self.destination.exists())


if __name__ == "__main__":
    unittest.main()
