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
    def test_copy_tree_skips_missing_optional_source(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            destination = root / "destination"
            self.assertIsNone(PACKAGE.copy_tree(root / "missing", destination))
            self.assertFalse(destination.exists())

    def test_copy_tree_rejects_missing_required_source(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            destination = root / "destination"
            with self.assertRaisesRegex(ValueError, "Required runtime resources"):
                PACKAGE.copy_tree(root / "missing", destination, required=True)
            self.assertFalse(destination.exists())

    def test_copy_tree_copies_existing_source(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, destination = root / "source", root / "destination"
            (source / "nested").mkdir(parents=True)
            (source / "nested/resource.txt").write_text("runtime resource")
            self.assertIsNone(PACKAGE.copy_tree(source, destination, required=True))
            self.assertEqual((destination / "nested/resource.txt").read_text(), "runtime resource")

    def test_installer_displays_combined_licensing_notice(self):
        installer = (HERE / "installer.iss").read_text()
        self.assertIn(r"LicenseFile=..\..\LICENSES.txt", installer)
        notice = (PACKAGE.ROOT / "LICENSES.txt").read_text(encoding="utf-8")
        self.assertIn("project license is MIT", notice)
        self.assertIn("MPL-2.0", notice)
        self.assertIn("LGPL", notice)

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
            for destination in [root / "Speech2Text", root / "dist/must-not-delete"]:
                with self.subTest(destination=destination):
                    destination.mkdir(parents=True)
                    marker = destination / "existing.txt"
                    marker.write_text("do not delete")
                    with patch.object(PACKAGE, "ROOT", root):
                        with self.assertRaisesRegex(ValueError, "destination"):
                            PACKAGE.assemble(root, binary, root / "target/windows-vendor", destination)
                    self.assertEqual(marker.read_text(), "do not delete")

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
            put("LICENSES.txt", (PACKAGE.ROOT / "LICENSES.txt").read_text(encoding="utf-8"))
            put("data/icons/io.github.davidecamino.Speech2Text.svg", "<svg/>")
            put("build-aux/windows/Start-Speech2Text.cmd", (HERE / "Start-Speech2Text.cmd").read_text())
            put("vendor/whisper-rs-sys/PATCH.md", "local build fixes")
            put("vendor/whisper-rs-sys/Cargo.toml", '[package]\nname="whisper-rs-sys"\nversion="0.15.0"\nlicense="Unlicense"\n')
            put("vendor/whisper-rs-sys/whisper.cpp/LICENSE", "native dependency notice")
            helper = put("build-aux/packaging/install-licenses.py")
            shutil.copyfile(PACKAGE.ROOT / "build-aux/packaging/install-licenses.py", helper)
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
                                 "licenses/whisper-rs-sys-PATCH.md", "licenses/whisper-rs-sys-source.zip",
                                 "licenses/local-rust/packages.json", "licenses/Speech2Text-LICENSE",
                                 "licenses/LICENSES.txt",
                                 "share/icons/hicolor/scalable/apps/io.github.davidecamino.Speech2Text.svg"]:
                    self.assertIn("Speech2Text/" + expected, names)
                self.assertFalse(any(name.endswith(("icon-theme.cache", "loaders.cache")) for name in names))
                self.assertFalse(any("\\" in name for name in names))
                self.assertIn(b"project license is MIT", output.read("Speech2Text/licenses/LICENSES.txt"))
                self.assertIn(b"MPL-2.0", output.read("Speech2Text/licenses/LICENSES.txt"))
                launcher = output.read("Speech2Text/Start-Speech2Text.cmd")
                self.assertNotIn(b"\n", launcher.replace(b"\r\n", b""))
                report = json.loads(output.read("Speech2Text/runtime-manifest.json"))
                self.assertEqual(report, {"bundled_dlls": ["libexample.dll"],
                                          "windows_system_dlls": ["kernel32.dll"],
                                          "version": "0.1.0", "architecture": "x86_64",
                                          "inference_backend": "CPU + Vulkan"})
                self.assertEqual(json.loads(output.read("Speech2Text/licenses/rust/packages.json")), [
                    {"name": "example", "version": "1.0.0", "license": "MIT"}])
                self.assertEqual(output.read("Speech2Text/licenses/rust/example-1.0/LICENSE"), b"dependency notice")
                self.assertEqual(output.read("Speech2Text/licenses/local-rust/whisper-rs-sys/whisper.cpp/LICENSE"),
                                 b"native dependency notice")
            with zipfile.ZipFile(root / "dist/Speech2Text/licenses/whisper-rs-sys-source.zip") as sources:
                self.assertEqual(set(sources.namelist()), {"whisper-rs-sys/PATCH.md", "whisper-rs-sys/Cargo.toml",
                                                          "whisper-rs-sys/whisper.cpp/LICENSE"})
                self.assertEqual(sources.read("whisper-rs-sys/PATCH.md"), b"local build fixes")
            digest, name = archive.with_suffix(".sha256").read_text().split()
            self.assertEqual(name, archive.name)
            self.assertEqual(digest, hashlib.sha256(archive.read_bytes()).hexdigest())


class RuntimeStageTests(unittest.TestCase):
    """Exercise native-tool boundaries with fixtures, not real PE executables."""

    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.prefix = self.root / "ucrt64"
        self.bundle = self.root / "dist/Speech2Text"
        self.binary = self.put("target/release/speech2text.exe")
        self.put("data/icons/io.github.davidecamino.Speech2Text.svg", "<svg/>")
        for name in ["glib-compile-schemas", "gdk-pixbuf-query-loaders", "gio-querymodules", "objdump"]:
            self.put(f"ucrt64/bin/{name}.exe")
        self.loader = self.put(f"ucrt64/{PACKAGE.PIXBUF.as_posix()}/libpixbufloader-svg.dll")
        for theme in ["Adwaita", "hicolor"]:
            self.put(f"ucrt64/share/icons/{theme}/index.theme")
        self.put("ucrt64/share/glib-2.0/schemas/gtk.gschema.xml")
        PACKAGE.prepare_bundle(self.bundle)

    def put(self, relative, content="fixture"):
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
        return path

    @staticmethod
    def compile_schemas(command, **kwargs):
        (Path(command[-1]) / "gschemas.compiled").write_bytes(b"compiled schema")
        return subprocess.CompletedProcess(command, 0)

    def copy_runtime(self, run=None):
        with patch.object(PACKAGE, "ROOT", self.root), \
                patch.object(PACKAGE.subprocess, "run", side_effect=run or self.compile_schemas):
            return PACKAGE.copy_runtime(self.prefix, self.binary, self.bundle)

    def test_missing_loader_directory_fails(self):
        shutil.rmtree(self.loader.parent)
        with self.assertRaisesRegex(ValueError, "Missing GdkPixbuf loaders"):
            self.copy_runtime()

    def test_missing_svg_loader_fails(self):
        self.loader.unlink()
        self.put(f"ucrt64/{PACKAGE.PIXBUF.as_posix()}/other.dll")
        with self.assertRaisesRegex(ValueError, "Missing SVG loader"):
            self.copy_runtime()

    def test_missing_required_themes_and_schemas_fail(self):
        for relative in ["share/icons/Adwaita", "share/icons/hicolor", "share/glib-2.0/schemas"]:
            with self.subTest(resource=relative):
                resource = self.prefix / relative
                backup = self.root / "resource-backup"
                shutil.copytree(resource, backup)
                shutil.rmtree(resource)
                with self.assertRaisesRegex(ValueError, "Required runtime resources"):
                    self.copy_runtime()
                shutil.copytree(backup, resource)
                shutil.rmtree(backup)

    def test_schema_compiler_must_produce_output(self):
        with self.assertRaisesRegex(ValueError, "did not produce gschemas.compiled"):
            self.copy_runtime(lambda command, **kwargs: subprocess.CompletedProcess(command, 0))

    def test_schema_compiler_failure_propagates(self):
        def fail(command, **kwargs):
            raise subprocess.CalledProcessError(1, command)
        with self.assertRaises(subprocess.CalledProcessError):
            self.copy_runtime(fail)

    def test_plugins_and_helpers_seed_the_dependency_closure(self):
        plugins = [f"{PACKAGE.PIXBUF.as_posix()}/libpixbufloader-svg.dll",
                   "lib/gio/modules/gio-plugin.dll", "lib/gtk-4.0/nested/gtk-plugin.DLL"]
        for relative in plugins[1:]:
            self.put(f"ucrt64/{relative}")
        helpers = ["gspawn-win64-helper.exe", "gspawn-win64-helper-console.exe", "dbus-daemon.exe"]
        for name in helpers:
            self.put(f"ucrt64/bin/{name}")
        self.put("ucrt64/lib/gio/modules/giomodule.cache", "STALE SDK PATH")
        self.put("ucrt64/lib/gtk-4.0/unrelated.txt", "not a DLL")
        self.put("ucrt64/bin/plugin-only.dll")
        seeds = self.copy_runtime()
        expected = ["bin/speech2text.exe", "bin/gdk-pixbuf-query-loaders.exe",
                    *plugins, *(f"bin/{name}" for name in helpers)]
        self.assertEqual({path.relative_to(self.bundle).as_posix() for path in seeds}, set(expected))
        self.assertFalse((self.bundle / "lib/gtk-4.0/unrelated.txt").exists())
        self.assertFalse((self.bundle / "lib/gio/modules/giomodule.cache").exists())

        def query_modules(command, **kwargs):
            self.assertEqual(Path(command[0]), self.prefix / "bin/gio-querymodules.exe")
            (Path(command[-1]) / "giomodule.cache").write_text("relocatable module metadata")
            return subprocess.CompletedProcess(command, 0)

        def read_imports(path, objdump):
            self.assertEqual(objdump, self.prefix / "bin/objdump.exe")
            return ["plugin-only.dll"] if path.name == "gtk-plugin.DLL" else ["KERNEL32.dll"]

        with patch.object(PACKAGE, "imports", side_effect=read_imports), \
                patch.object(PACKAGE.subprocess, "run", side_effect=query_modules):
            report = PACKAGE.collect_dependencies(self.prefix, self.bundle, seeds)
        self.assertEqual(report, {"bundled_dlls": ["plugin-only.dll"],
                                  "windows_system_dlls": ["kernel32.dll"]})
        self.assertTrue((self.bundle / "bin/plugin-only.dll").is_file())
        self.assertEqual((self.bundle / "lib/gio/modules/giomodule.cache").read_text(),
                         "relocatable module metadata")

    def test_no_gio_plugins_does_not_run_querymodules(self):
        seeds = self.copy_runtime()
        with patch.object(PACKAGE, "imports", return_value=[]), \
                patch.object(PACKAGE.subprocess, "run") as run:
            self.assertEqual(PACKAGE.collect_dependencies(self.prefix, self.bundle, seeds),
                             {"bundled_dlls": [], "windows_system_dlls": []})
            run.assert_not_called()

    def test_missing_inputs_leave_existing_bundle_untouched(self):
        inventory = self.put("target/windows-msys2-packages.txt")
        vendor = self.root / "target/windows-vendor"
        manifest = self.put("target/windows-vendor/example/Cargo.toml")
        marker = self.put("dist/Speech2Text/existing.txt", "keep this bundle")
        for path, message in [(self.binary, "Missing release executable"),
                              (inventory, "Record 'pacman"), (manifest, "Missing vendored Rust sources")]:
            with self.subTest(input=path):
                content = path.read_bytes()
                path.unlink()
                with patch.object(PACKAGE, "ROOT", self.root):
                    with self.assertRaisesRegex(ValueError, message):
                        PACKAGE.assemble(self.prefix, self.binary, vendor, self.bundle)
                self.assertEqual(marker.read_text(), "keep this bundle")
                path.write_bytes(content)


if __name__ == "__main__":
    unittest.main()
