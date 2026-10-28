#!/usr/bin/env python3
"""Assemble a relocatable CPU-only Windows GTK runtime from one UCRT64 prefix.

Run with MSYS2's native UCRT64 Python after building speech2text.exe. Never
resolve dependencies from PATH: all non-system DLLs must come from this prefix.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[2]
PIXBUF = Path("lib/gdk-pixbuf-2.0/2.10.0/loaders")
# Windows 10/11 inbox libraries, not files to redistribute. Unknown dependencies
# fail closed rather than accidentally using software installed on the CI runner.
SYSTEM_DLLS = set("""
advapi32.dll authz.dll avrt.dll bcrypt.dll bcryptprimitives.dll cabinet.dll cfgmgr32.dll
clbcatq.dll combase.dll comctl32.dll comdlg32.dll crypt32.dll cryptbase.dll
cryptnet.dll cryptsp.dll cryptui.dll d2d1.dll d3d11.dll d3d12.dll d3d9.dll
d3dcompiler_47.dll dcomp.dll dbgcore.dll dbghelp.dll dhcpcsvc.dll dinput8.dll dnsapi.dll dsound.dll dwmapi.dll
dwrite.dll dxgi.dll dxva2.dll fwpuclnt.dll gdi32.dll gdiplus.dll glu32.dll
hid.dll imm32.dll iphlpapi.dll kernel32.dll kernelbase.dll mpr.dll msimg32.dll
msvcp_win.dll msvcrt.dll mswsock.dll ncrypt.dll netapi32.dll normaliz.dll nsi.dll
ntdll.dll ole32.dll oleacc.dll oleaut32.dll opengl32.dll powrprof.dll propsys.dll
psapi.dll rasapi32.dll rpcrt4.dll secur32.dll setupapi.dll shcore.dll shell32.dll shlwapi.dll
ucrtbase.dll uiautomationcore.dll user32.dll userenv.dll usp10.dll uxtheme.dll
version.dll wevtapi.dll win32u.dll windowscodecs.dll winhttp.dll wininet.dll winmm.dll
winspool.drv wintrust.dll wlanapi.dll wldap32.dll ws2_32.dll wsock32.dll
wtsapi32.dll
""".split())


def is_system_dll(name: str) -> bool:
    name = name.casefold()
    return name in SYSTEM_DLLS or name.startswith(("api-ms-win-", "ext-ms-win-"))


def parse_imports(output: str) -> list[str]:
    if "file format pei-x86-64" not in output:
        raise ValueError("Expected a 64-bit Windows PE executable/DLL")
    return re.findall(r"DLL Name:\s*(\S+)", output)


def imports(path: Path, objdump: Path) -> list[str]:
    result = subprocess.run([str(objdump), "-p", str(path)], check=True,
                            capture_output=True, text=True, encoding="utf-8", errors="replace",
                            env={**os.environ, "LC_ALL": "C"})
    return parse_imports(result.stdout)


def collect_dlls(prefix: Path, bundle: Path, seeds: list[Path], read_imports) -> dict:
    available = {path.name.casefold(): path for path in (prefix / "bin").iterdir()
                 if path.is_file() and path.suffix.casefold() == ".dll"}
    pending = list(seeds)
    checked = set()
    copied = set()
    system = set()
    while pending:
        binary = pending.pop()
        key = str(binary.resolve()).casefold()
        if key in checked:
            continue
        checked.add(key)
        for name in read_imports(binary):
            if name.casefold().startswith("msys-"):
                raise ValueError(f"MSYS runtime dependency {name} in {binary.name}; use UCRT64 packages only")
            if is_system_dll(name):
                system.add(name.casefold())
                continue
            source = available.get(name.casefold())
            if source is None:
                raise ValueError(f"Missing DLL {name}, imported by {binary.name}; not found in {prefix / 'bin'}")
            destination = bundle / "bin" / source.name
            if not destination.exists():
                shutil.copy2(source, destination)
            copied.add(source.name)
            pending.append(destination)
    return {"bundled_dlls": sorted(copied), "windows_system_dlls": sorted(system)}


def copy_tree(source: Path, destination: Path, *, required: bool = False) -> bool:
    if not source.is_dir():
        if required:
            raise ValueError(f"Required runtime resources are missing: {source}")
        return False
    shutil.copytree(source, destination, dirs_exist_ok=True)
    return True


def tool(prefix: Path, name: str) -> Path:
    path = prefix / "bin" / f"{name}.exe"
    if not path.is_file():
        raise ValueError(f"Missing UCRT64 tool: {path}")
    return path


def assemble(prefix: Path, binary: Path, vendor: Path, bundle: Path) -> Path:
    if not binary.is_file():
        raise ValueError(f"Missing release executable: {binary}")
    inventory = ROOT / "target/windows-msys2-packages.txt"
    if not inventory.is_file():
        raise ValueError("Record 'pacman -Q > target/windows-msys2-packages.txt' before packaging")
    if not vendor.is_dir() or not any(vendor.glob("*/Cargo.toml")):
        raise ValueError("Missing vendored Rust sources; run cargo vendor --locked --versioned-dirs target/windows-vendor")
    # Do not let a typo recursively delete arbitrary directories.
    if bundle.parent.resolve() != (ROOT / "dist").resolve() or bundle.name != "Speech2Text":
        raise ValueError("Bundle destination must be the repository's dist/Speech2Text directory")
    if bundle.exists():
        shutil.rmtree(bundle)
    (bundle / "bin").mkdir(parents=True)
    executable = bundle / "bin/speech2text.exe"
    shutil.copy2(binary, executable)
    seeds = [executable]

    # Runtime-loaded DLLs do not appear in the executable's import table.
    for relative in [PIXBUF, Path("lib/gio/modules"), Path("lib/gtk-4.0")]:
        source = prefix / relative
        if relative == PIXBUF and not source.is_dir():
            raise ValueError(f"Missing GdkPixbuf loaders: {source}")
        (bundle / relative).mkdir(parents=True, exist_ok=True)
        if source.is_dir():
            for path in source.rglob("*"):
                if path.is_file() and path.suffix.casefold() == ".dll":
                    target = bundle / relative / path.relative_to(source)
                    target.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(path, target)
                    seeds.append(target)
    if not any("svg" in path.name.casefold() for path in (bundle / PIXBUF).glob("*.dll")):
        raise ValueError("Missing SVG loader; install mingw-w64-ucrt-x86_64-librsvg")

    query = tool(prefix, "gdk-pixbuf-query-loaders")
    shutil.copy2(query, bundle / "bin" / query.name)
    seeds.append(bundle / "bin" / query.name)
    for name in ["gspawn-win64-helper.exe", "gspawn-win64-helper-console.exe", "dbus-daemon.exe"]:
        source = prefix / "bin" / name
        if source.is_file():
            shutil.copy2(source, bundle / "bin" / name)
            seeds.append(bundle / "bin" / name)

    for relative in ["share/gtk-4.0", "share/libadwaita-1", "share/locale", "share/fonts",
                     "etc/fonts", "etc/dbus-1", "share/dbus-1", "share/mime"]:
        copy_tree(prefix / relative, bundle / relative)
    for theme in ["Adwaita", "hicolor", "AdwaitaLegacy"]:
        copy_tree(prefix / "share/icons" / theme, bundle / "share/icons" / theme,
                  required=theme != "AdwaitaLegacy")
    schemas = Path("share/glib-2.0/schemas")
    copy_tree(prefix / schemas, bundle / schemas, required=True)
    subprocess.run([str(tool(prefix, "glib-compile-schemas")), "--strict", str(bundle / schemas)], check=True)
    if not (bundle / schemas / "gschemas.compiled").is_file():
        raise ValueError("GLib schema compilation did not produce gschemas.compiled")
    icon = ROOT / "data/icons/io.github.davidecamino.Speech2Text.svg"
    target = bundle / "share/icons/hicolor/scalable/apps" / icon.name
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(icon, target)
    # SDK icon caches predate our app icon; GTK can scan these portable themes.
    for cache in (bundle / "share/icons").rglob("icon-theme.cache"):
        cache.unlink()

    report = collect_dlls(prefix, bundle, seeds, lambda path: imports(path, tool(prefix, "objdump")))
    gio_modules = bundle / "lib/gio/modules"
    if list(gio_modules.glob("*.dll")):
        subprocess.run([str(tool(prefix, "gio-querymodules")), str(gio_modules)], check=True)

    # Include Rust and native dependency notices. Extra SDK license notices are
    # intentional; tools themselves (Rust, Python, LLVM, MSYS2) are not shipped.
    licenses = bundle / "licenses"
    licenses.mkdir()
    shutil.copy2(ROOT / "LICENSE", licenses / "Speech2Text-LICENSE")
    shutil.copy2(ROOT / "Cargo.lock", licenses / "Cargo.lock")
    shutil.copy2(inventory, licenses / "msys2-packages.txt")
    (licenses / "SOURCES.txt").write_text(
        "Rust dependency versions: Cargo.lock; sources: https://crates.io/\n"
        "MSYS2 package versions: msys2-packages.txt (includes build tools).\n"
        "Native package information: https://packages.msys2.org/\n"
        "Native source archives: https://repo.msys2.org/mingw/sources/\n"
        "Build recipes/patches: https://github.com/msys2/MINGW-packages\n"
        "See individual license notices for terms; runtime DLLs remain replaceable.\n",
        encoding="utf-8")
    copy_tree(prefix / "share/licenses", licenses / "msys2", required=True)
    helper = ROOT / "build-aux/flatpak/install-licenses.py"
    spec = importlib.util.spec_from_file_location("dependency_licenses", helper)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    module.install(vendor, licenses / "rust")

    with (ROOT / "Cargo.toml").open("rb") as handle:
        version = tomllib.load(handle)["package"]["version"]
    report.update({"version": version, "architecture": "x86_64", "inference_backend": "CPU"})
    (bundle / "runtime-manifest.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    launcher = (ROOT / "build-aux/windows/Start-Speech2Text.cmd").read_text(encoding="ascii")
    (bundle / "Start-Speech2Text.cmd").write_text(launcher, encoding="ascii", newline="\r\n")
    (bundle / "README.txt").write_text(
        f"Speech2Text {version} - experimental Windows x86_64 CPU build\n\n"
        "Extract this whole folder, then double-click Start-Speech2Text.cmd.\n"
        "Do not run the application from inside the ZIP or copy just the EXE.\n"
        "No MSYS2, GTK, Python, Rust, or installer is needed on the test machine.\n"
        "A console window remains open for diagnostics in this initial build.\n"
        "Select/download a whisper.cpp model in Settings, then open an audio file.\n"
        "Models are not bundled. Preferences and downloads use your Windows user profile.\n"
        "This build is unsigned and experimental. Report errors and your Windows version:\n"
        "https://github.com/DavideCamino/Speech_to_text_multiplatform/issues\n",
        encoding="utf-8")

    archive = bundle.parent / "Speech2Text-windows-x86_64.zip"
    with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=6) as output:
        for path in sorted(bundle.rglob("*")):
            if path.is_file():
                output.write(path, Path("Speech2Text") / path.relative_to(bundle))
    checksum = archive.with_suffix(".sha256")
    with archive.open("rb") as handle:
        digest = hashlib.file_digest(handle, "sha256").hexdigest()
    checksum.write_text(f"{digest}  {archive.name}\n", encoding="utf-8")
    return archive


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prefix", type=Path, required=True, help="Native Windows path to the MSYS2 UCRT64 prefix")
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/speech2text.exe")
    parser.add_argument("--vendor", type=Path, default=ROOT / "target/windows-vendor")
    args = parser.parse_args()
    try:
        if os.name != "nt":
            raise ValueError("Assemble the real bundle on the Windows runner; Linux can run unit tests only")
        archive = assemble(args.prefix.resolve(), args.binary.resolve(), args.vendor.resolve(), ROOT / "dist/Speech2Text")
        print(f"Created {archive}")
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        print(f"Windows packaging failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
