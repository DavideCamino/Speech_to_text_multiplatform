#!/usr/bin/env python3
"""Generate Flatpak sources from Cargo.lock using only Python 3.11+ stdlib.

Run from any directory. Commit cargo-sources.json; CI checks it is current.
Crate archives are verified by Flatpak against Cargo.lock's SHA-256 checksums.
No network requests or Cargo.lock changes are made by this script.
"""
import argparse
import json
from pathlib import Path
import re
import sys
import tomllib
from urllib.parse import quote

ROOT = Path(__file__).resolve().parents[2]
OUTPUT = Path(__file__).with_name("cargo-sources.json")
REGISTRY = "registry+https://github.com/rust-lang/crates.io-index"


def generate(lock: dict) -> str:
    sources = []
    packages = sorted(lock["package"], key=lambda package: (package["name"], package["version"]))
    for package in packages:
        source = package.get("source")
        if source is None:  # The workspace itself.
            continue
        if source != REGISTRY:
            raise ValueError(f"Unsupported source for {package['name']}: {source}. "
                             "Add explicit Flatpak sources for git/private-registry dependencies.")
        name, version = package["name"], package["version"]
        checksum = package.get("checksum", "")
        if not re.fullmatch(r"[a-f0-9]{64}", checksum):
            raise ValueError(f"Missing/invalid archive checksum for {name} {version}")
        if not re.fullmatch(r"[A-Za-z0-9_-]+", name) or not re.fullmatch(r"[A-Za-z0-9.+-]+", version):
            raise ValueError(f"Invalid crate name/version: {name} {version}")
        destination = f"cargo/vendor/{name}-{version}"
        archive = quote(f"{name}-{version}.crate", safe="")
        sources.extend([
            {
                "type": "archive",
                "archive-type": "tar-gzip",
                "url": f"https://static.crates.io/crates/{name}/{archive}",
                "sha256": checksum,
                "strip-components": 1,
                "dest": destination,
            },
            {
                "type": "inline",
                "dest": destination,
                "dest-filename": ".cargo-checksum.json",
                # The complete archive is SHA-256 verified before extraction.
                "contents": json.dumps({"package": checksum, "files": {}}, sort_keys=True) + "\n",
            },
        ])
    return json.dumps(sources, indent=2, ensure_ascii=False) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="Fail if committed sources differ from Cargo.lock")
    args = parser.parse_args()
    try:
        with (ROOT / "Cargo.lock").open("rb") as handle:
            content = generate(tomllib.load(handle))
        if args.check:
            if not OUTPUT.exists() or OUTPUT.read_text() != content:
                print("Flatpak Cargo sources are stale. Run: python3 build-aux/flatpak/generate-cargo-sources.py", file=sys.stderr)
                return 1
            print("Flatpak Cargo sources match Cargo.lock.")
        else:
            OUTPUT.write_text(content)
            print(f"Generated {OUTPUT.relative_to(ROOT)}")
    except (OSError, ValueError, KeyError) as error:
        print(f"Cannot generate Flatpak Cargo sources: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
