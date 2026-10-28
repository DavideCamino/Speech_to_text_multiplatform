#!/usr/bin/env python3
"""Keep dependency license/notice files, including bundled whisper.cpp/ggml."""
import json
from pathlib import Path
import shutil
import sys
import tomllib


def install(vendor: Path, destination: Path) -> None:
    packages = []
    for crate in sorted(vendor.iterdir()):
        if not crate.is_dir():
            continue
        with (crate / "Cargo.toml").open("rb") as handle:
            package = tomllib.load(handle)["package"]
        packages.append({"name": package["name"], "version": package["version"],
                         "license": package.get("license", "See included license files")})
        declared = package.get("license-file")
        for source in crate.rglob("*"):
            if not source.is_file():
                continue
            relative = source.relative_to(crate)
            name = source.name.lower()
            if (name.startswith(("license", "licence", "copying", "notice", "copyright"))
                    or any(part.lower() in ("licenses", "licences") for part in relative.parts[:-1])
                    or (declared and relative.as_posix() == declared)):
                target = destination / crate.name / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(source, target)
    destination.mkdir(parents=True, exist_ok=True)
    (destination / "packages.json").write_text(json.dumps(packages, indent=2) + "\n")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit("Usage: install-licenses.py VENDOR_DIRECTORY DESTINATION")
    install(Path(sys.argv[1]), Path(sys.argv[2]))
