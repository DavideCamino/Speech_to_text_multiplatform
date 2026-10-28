import hashlib
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import zipfile

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("windows_package", HERE / "package.py")
PACKAGE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PACKAGE)


class WindowsPackagingTests(unittest.TestCase):
    def test_parses_only_x86_64_pe_imports(self):
        output = "file format pei-x86-64\n\tDLL Name: KERNEL32.dll\n\tDLL Name: libgtk-4-1.dll\n"
        self.assertEqual(PACKAGE.parse_imports(output), ["KERNEL32.dll", "libgtk-4-1.dll"])
        for output in ["file format elf64-x86-64", "file format pei-i386"]:
            with self.assertRaisesRegex(ValueError, "64-bit Windows PE"):
                PACKAGE.parse_imports(output)

    def test_system_libraries_do_not_include_third_party_runtimes(self):
        for name in ["KERNEL32.DLL", "ucrtbase.dll", "api-ms-win-crt-stdio-l1-1-0.dll", "ext-ms-win-test.dll"]:
            self.assertTrue(PACKAGE.is_system_dll(name))
        for name in ["libgtk-4-1.dll", "vulkan-1.dll", "vcruntime140.dll", "msys-2.0.dll"]:
            self.assertFalse(PACKAGE.is_system_dll(name))

    def test_transitive_case_insensitive_dll_closure_and_plugin_seeds(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            prefix, bundle = root / "ucrt64", root / "bundle"
            (prefix / "bin").mkdir(parents=True)
            (bundle / "bin").mkdir(parents=True)
            for name in ["liba.dll", "libb.dll", "unused.dll"]:
                (prefix / "bin" / name).write_bytes(b"DLL")
            exe = bundle / "bin/speech2text.exe"
            plugin = bundle / "loader.dll"
            exe.touch()
            plugin.touch()
            dependencies = {
                "speech2text.exe": ["LIBA.DLL", "kernel32.dll"],
                "liba.dll": ["libb.dll"],
                "libb.dll": ["liba.dll", "api-ms-win-crt-runtime-l1-1-0.dll"],
                "loader.dll": ["libb.dll"],
            }
            report = PACKAGE.collect_dlls(prefix, bundle, [exe, plugin], lambda path: dependencies[path.name])
            self.assertEqual(report["bundled_dlls"], ["liba.dll", "libb.dll"])
            self.assertFalse((bundle / "bin/unused.dll").exists())
            self.assertFalse((bundle / "bin/kernel32.dll").exists())

    def test_unknown_or_msys_dependencies_fail_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "prefix/bin").mkdir(parents=True)
            (root / "bundle/bin").mkdir(parents=True)
            for dependency, message in [("not-in-prefix.dll", "Missing DLL"), ("msys-2.0.dll", "MSYS runtime")]:
                with self.assertRaisesRegex(ValueError, message):
                    PACKAGE.collect_dlls(root / "prefix", root / "bundle", [root / "app.exe"],
                                         lambda path: [dependency])

    def test_refuses_unsafe_destination_before_deleting(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "target/windows-vendor/crate").mkdir(parents=True)
            (root / "target/windows-vendor/crate/Cargo.toml").touch()
            (root / "target/windows-msys2-packages.txt").touch()
            binary = root / "app.exe"
            binary.touch()
            safe = root / "must-not-delete"
            safe.mkdir()
            with patch.object(PACKAGE, "ROOT", root):
                with self.assertRaisesRegex(ValueError, "destination"):
                    PACKAGE.assemble(root, binary, root / "target/windows-vendor", safe)
            self.assertTrue(safe.is_dir())

    def test_full_bundle_layout_licenses_archive_and_checksum_with_mock_pe_tools(self):
        # This checks assembly, not Windows execution. Real PE/tool/GUI validation
        # belongs to the Windows workflow; no fake binaries become release files.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            prefix = root / "ucrt64"
            def put(relative, content="fixture"):
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(content, encoding="utf-8")
                return path
            binary = put("target/release/speech2text.exe")
            put("target/windows-msys2-packages.txt", "example-package 1.0-1\n")
            put("Cargo.toml", '[package]\nname="speech2text"\nversion="0.1.0"\n')
            put("Cargo.lock")
            put("LICENSE", "MIT fixture")
            put("data/icons/io.github.davidecamino.Speech2Text.svg", "<svg/>")
            put("build-aux/windows/Start-Speech2Text.cmd", (HERE / "Start-Speech2Text.cmd").read_text())
            helper = put("build-aux/flatpak/install-licenses.py")
            shutil.copyfile(PACKAGE.ROOT / "build-aux/flatpak/install-licenses.py", helper)
            vendor = root / "target/windows-vendor"
            put("target/windows-vendor/example-1.0/Cargo.toml", '[package]\nname="example"\nversion="1.0.0"\nlicense="MIT"\n')
            put("target/windows-vendor/example-1.0/LICENSE", "dependency notice")
            for name in ["objdump", "glib-compile-schemas", "gdk-pixbuf-query-loaders"]:
                put(f"ucrt64/bin/{name}.exe")
            put("ucrt64/bin/libexample.dll")
            put("ucrt64/lib/gdk-pixbuf-2.0/2.10.0/loaders/libpixbufloader-svg.dll")
            put("ucrt64/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache", "STALE SDK PATH")
            for theme in ["Adwaita", "hicolor"]:
                put(f"ucrt64/share/icons/{theme}/index.theme")
                put(f"ucrt64/share/icons/{theme}/icon-theme.cache", "STALE CACHE")
            put("ucrt64/share/glib-2.0/schemas/gtk.gschema.xml")
            put("ucrt64/share/licenses/example/LICENSE", "native notice")

            def fake_run(command, **kwargs):
                if Path(command[0]).stem == "glib-compile-schemas":
                    (Path(command[-1]) / "gschemas.compiled").write_bytes(b"compiled schema")
                return subprocess.CompletedProcess(command, 0, stdout="file format pei-x86-64\nDLL Name: libexample.dll\nDLL Name: KERNEL32.dll\n")
            with patch.object(PACKAGE, "ROOT", root), patch.object(PACKAGE.subprocess, "run", side_effect=fake_run):
                archive = PACKAGE.assemble(prefix, binary, vendor, root / "dist/Speech2Text")
            with zipfile.ZipFile(archive) as output:
                names = set(output.namelist())
                for expected in ["bin/speech2text.exe", "bin/libexample.dll", "Start-Speech2Text.cmd",
                                 "share/glib-2.0/schemas/gschemas.compiled", "licenses/rust/packages.json",
                                 "licenses/msys2/example/LICENSE", "licenses/msys2-packages.txt",
                                 "share/icons/hicolor/scalable/apps/io.github.davidecamino.Speech2Text.svg"]:
                    self.assertIn("Speech2Text/" + expected, names)
                self.assertFalse(any(name.endswith(("icon-theme.cache", "loaders.cache")) for name in names))
                self.assertFalse(any("\\" in name for name in names))
                launcher = output.read("Speech2Text/Start-Speech2Text.cmd")
                self.assertNotIn(b"\n", launcher.replace(b"\r\n", b""))
                report = json.loads(output.read("Speech2Text/runtime-manifest.json"))
                self.assertEqual(report["inference_backend"], "CPU")
                self.assertEqual(report["version"], "0.1.0")
            digest, name = archive.with_suffix(".sha256").read_text().split()
            self.assertEqual(name, archive.name)
            self.assertEqual(digest, hashlib.sha256(archive.read_bytes()).hexdigest())


if __name__ == "__main__":
    unittest.main()
