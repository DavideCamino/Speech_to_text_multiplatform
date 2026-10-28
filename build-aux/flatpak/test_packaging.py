"""Offline unit checks for the packaging generator and sandbox contract."""
import importlib.util
import json
from pathlib import Path
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
