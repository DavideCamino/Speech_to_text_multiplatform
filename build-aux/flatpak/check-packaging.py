#!/usr/bin/env python3
"""Check packaging identity/version consistency; --release rejects placeholders."""
import argparse
import configparser
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tomllib
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]


def check(manifest_path: Path, release: bool) -> None:
    manifest = json.loads(manifest_path.read_text())
    app_id = manifest["app-id"]
    desktop = ROOT / "data" / f"{app_id}.desktop"
    metainfo = ROOT / "data" / f"{app_id}.metainfo.xml"
    icon = ROOT / "data" / "icons" / f"{app_id}.svg"
    for path in [desktop, metainfo, icon, ROOT / "LICENSE"]:
        if not path.is_file():
            raise ValueError(f"Missing packaging file: {path.relative_to(ROOT)}")
    if manifest_path.stem != app_id:
        raise ValueError("Manifest filename must match its app-id")
    main = (ROOT / "src/main.rs").read_text()
    if not re.search(r'\.application_id\(\s*"' + re.escape(app_id) + r'"\s*\)', main):
        raise ValueError("src/main.rs application_id does not match the Flatpak manifest")
    entry = configparser.ConfigParser(interpolation=None)
    entry.read(desktop)
    if entry["Desktop Entry"]["Icon"] != app_id or entry["Desktop Entry"]["Exec"] != manifest["command"]:
        raise ValueError("Desktop icon/command does not match the manifest")
    metadata = ET.parse(metainfo).getroot()
    if metadata.findtext("id") != app_id or metadata.findtext("launchable") != desktop.name:
        raise ValueError("AppStream identity/desktop launchable does not match the manifest")
    ET.parse(icon)
    with (ROOT / "Cargo.toml").open("rb") as handle:
        version = tomllib.load(handle)["package"]["version"]
    latest = metadata.find("releases/release")
    if latest is None or latest.get("version") != version:
        raise ValueError("First AppStream release version must match Cargo.toml")
    subprocess.run([sys.executable, str(ROOT / "build-aux/flatpak/generate-cargo-sources.py"), "--check"], check=True)
    if release:
        files = [manifest_path, desktop, metainfo, icon, ROOT / "src/main.rs", ROOT / "src/settings.rs",
                 ROOT / ".github/workflows/flatpak.yml"]
        for path in files:
            if any(token in str(path.relative_to(ROOT)) + path.read_text()
                   for token in ["YOUR_NAME", "YOUR_REPOSITORY", "io.github.yourname"]):
                raise ValueError(f"Replace identity placeholders before releasing: {path.relative_to(ROOT)}")
        if os.environ.get("GITHUB_REF_TYPE") == "tag" and os.environ.get("GITHUB_REF_NAME") != f"v{version}":
            raise ValueError(f"Release tag must be v{version}, matching Cargo.toml and AppStream metadata")
    print(f"Packaging identity and version are consistent: {app_id} {version}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("--release", action="store_true")
    args = parser.parse_args()
    try:
        check(args.manifest.resolve(), args.release)
    except (OSError, ValueError, KeyError, ET.ParseError, subprocess.CalledProcessError) as error:
        print(f"Packaging check failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
