"""Offline unit checks for the packaging generator and sandbox contract."""
import importlib.util
import json
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import tomllib
import unittest

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
spec = importlib.util.spec_from_file_location("cargo_sources", HERE / "generate-cargo-sources.py")
generator = importlib.util.module_from_spec(spec)
spec.loader.exec_module(generator)


class CargoSourcesTests(unittest.TestCase):
    def test_checksums_and_destinations(self):
        lock = {"package": [
            {"name": "workspace", "version": "0.1.0"},
            {"name": "example", "version": "1.2.3+build", "source": generator.REGISTRY, "checksum": "a" * 64},
        ]}
        archive, checksum_file = json.loads(generator.generate(lock))
        self.assertEqual(archive["sha256"], "a" * 64)
        self.assertEqual(archive["dest"], "cargo/vendor/example-1.2.3+build")
        self.assertIn("%2B", archive["url"])
        self.assertEqual(archive["dest"], checksum_file["dest"])
        self.assertEqual(json.loads(checksum_file["contents"]), {"package": "a" * 64, "files": {}})

    def test_unpinned_or_non_registry_dependencies_are_rejected(self):
        for source, checksum in [("git+https://example.com/project", "a" * 64), (generator.REGISTRY, "bad")]:
            with self.subTest(source=source), self.assertRaises(ValueError):
                generator.generate({"package": [{"name": "example", "version": "1.0.0", "source": source, "checksum": checksum}]})

    def test_generated_sources_match_the_lockfile(self):
        with (ROOT / "Cargo.lock").open("rb") as handle:
            expected = generator.generate(tomllib.load(handle))
        self.assertEqual((HERE / "cargo-sources.json").read_text(), expected)

    def test_order_does_not_depend_on_lockfile_order(self):
        packages = [{"name": name, "version": "1.0.0", "source": generator.REGISTRY, "checksum": "a" * 64}
                    for name in ["second", "first"]]
        self.assertEqual(generator.generate({"package": packages}), generator.generate({"package": list(reversed(packages))}))


class ManifestTests(unittest.TestCase):
    def test_cpu_vulkan_build_is_offline_and_not_host_cpu_specific(self):
        paths = list(HERE.glob("io.*.json"))
        self.assertEqual(len(paths), 1)
        manifest = json.loads(paths[0].read_text())
        command = manifest["modules"][0]["build-commands"][0]
        self.assertIn("--frozen --offline", command)
        self.assertIn("--features vulkan", command)
        self.assertEqual(manifest["build-options"]["env"]["GGML_NATIVE"], "OFF")
        with (HERE / "cargo-config.toml").open("rb") as handle:
            config = tomllib.load(handle)
        self.assertTrue(config["net"]["offline"])
        self.assertEqual(config["source"]["flatpak-vendor"]["directory"], "cargo/vendor")

    def test_project_and_dependency_license_notice_is_installed(self):
        manifest = json.loads(next(HERE.glob("io.*.json")).read_text())
        commands = manifest["modules"][0]["build-commands"]
        self.assertIn("install -Dm644 LICENSES.txt /app/share/licenses/speech2text/LICENSES.txt", commands)
        notice = (ROOT / "LICENSES.txt").read_text()
        self.assertIn("project license is MIT", notice)
        self.assertIn("MPL-2.0", notice)
        self.assertIn("LGPL", notice)

    def test_local_dependency_notices_are_installed(self):
        manifest = json.loads(next(HERE.glob("io.*.json")).read_text())
        commands = manifest["modules"][0]["build-commands"]
        registry_command = "python3 build-aux/packaging/install-licenses.py cargo/vendor /app/share/licenses/speech2text/dependencies"
        local_command = "python3 build-aux/packaging/install-licenses.py vendor /app/share/licenses/speech2text/local-dependencies"
        self.assertIn(registry_command, commands)
        self.assertIn(local_command, commands)
        # Run the manifest's local-vendor installer against the real patched
        # dependency, redirecting only the /app destination to a temporary dir.
        _, script, source, destination = shlex.split(local_command)
        self.assertNotEqual(destination, shlex.split(registry_command)[-1])
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "local-dependencies"
            subprocess.run([sys.executable, str(ROOT / script), str(ROOT / source), str(output)],
                           check=True, capture_output=True, text=True)
            relative = Path("whisper-rs-sys/whisper.cpp/LICENSE")
            self.assertEqual((output / relative).read_bytes(), (ROOT / source / relative).read_bytes())
            packages = json.loads((output / "packages.json").read_text())
            with (ROOT / source / "whisper-rs-sys/Cargo.toml").open("rb") as handle:
                package = tomllib.load(handle)["package"]
            self.assertIn({"name": package["name"], "version": package["version"],
                           "license": package["license"]}, packages)

    def test_permissions_do_not_expose_the_home_directory_or_all_devices(self):
        manifest = json.loads(next(HERE.glob("io.*.json")).read_text())
        permissions = manifest["finish-args"]
        self.assertFalse(any(flag.startswith("--filesystem=") for flag in permissions))
        self.assertNotIn("--device=all", permissions)
        self.assertIn("--device=dri", permissions)
        self.assertIn("--socket=pulseaudio", permissions)
        source = manifest["modules"][0]["sources"][0]
        self.assertIn(".private", source["skip"])
        self.assertIn("target", source["skip"])


if __name__ == "__main__":
    unittest.main()
