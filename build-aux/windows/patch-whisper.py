#!/usr/bin/env python3
"""Patch only the vendored whisper-rs-sys 0.15.0 Windows compiler flag.

Run after cargo vendor, before building with its source-replacement config.
Never modify Cargo's registry cache or the Flatpak vendor sources.
"""
import argparse
import hashlib
import json
from pathlib import Path
import sys
import tomllib

VERSION = "0.15.0"
ORIGINAL = b'''    if cfg!(target_os = "windows") {
        config.cxxflag("/utf-8");
        println!("cargo:rustc-link-lib=advapi32");
    }
'''
REPLACEMENT = b'''    if cfg!(target_os = "windows") {
        if target.ends_with("-msvc") {
            config.cxxflag("/utf-8");
        }
        println!("cargo:rustc-link-lib=advapi32");
    }
'''


def apply(vendor: Path) -> Path:
    crate = vendor / f"whisper-rs-sys-{VERSION}"
    with (crate / "Cargo.toml").open("rb") as handle:
        package = tomllib.load(handle)["package"]
    if (package["name"], package["version"]) != ("whisper-rs-sys", VERSION):
        raise ValueError("Unexpected whisper-rs-sys version; review the patch before upgrading")
    build = crate / "build.rs"
    original = build.read_bytes()
    checksum_file = crate / ".cargo-checksum.json"
    checksums = json.loads(checksum_file.read_text(encoding="utf-8"))
    if checksums["files"].get("build.rs") != hashlib.sha256(original).hexdigest():
        raise ValueError("Vendored build.rs does not match its original file checksum")
    if original.count(ORIGINAL) != 1:
        raise ValueError("Expected upstream Windows flag block exactly once; review the patch")
    patched = original.replace(ORIGINAL, REPLACEMENT, 1)
    checksums["files"]["build.rs"] = hashlib.sha256(patched).hexdigest()
    build.write_bytes(patched)
    # Cargo verifies directory-source file hashes. Change only this file's hash;
    # retain the original registry package checksum checked against Cargo.lock.
    checksum_file.write_text(json.dumps(checksums, indent=2) + "\n", encoding="utf-8", newline="\n")
    return build


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("vendor", type=Path, help="Versioned directory created by cargo vendor")
    args = parser.parse_args()
    try:
        print(f"Patched MSVC-only /utf-8 flag in {apply(args.vendor)}")
    except (OSError, ValueError, KeyError) as error:
        print(f"Whisper Windows patch failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
