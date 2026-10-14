#!/usr/bin/env python3
"""Package a verified offline source tree using the current Debian recipe."""
import argparse
from email.utils import formatdate
import os
from pathlib import Path
import re
import shutil
import subprocess
import time
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def changelog(version, timestamp):
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version):
        raise ValueError("Expected a numeric release version")
    date = formatdate(timestamp, localtime=False)
    return (f"pathpilot ({version}-1) unstable; urgency=medium\n\n"
            "  * Package the pinned, vendored PathPilot release sources.\n\n"
            f" -- RomanRcT <RomanRcT@users.noreply.github.com>  {date}\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    source = args.source.resolve()
    version = tomllib.loads((source / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    timestamp = int(os.environ.get("SOURCE_DATE_EPOCH", time.time()))
    text = changelog(version, timestamp)
    if (source / "debian").exists():
        raise ValueError("Use a fresh source extraction without a debian directory")
    shutil.copytree(ROOT / "packaging/debian", source / "debian")
    (source / "debian/changelog").write_text(text)
    # Rust is supplied by rustup in CI, not registered in dpkg's package database.
    # All native build dependencies are explicitly installed by the workflow.
    subprocess.run(["dpkg-buildpackage", "-us", "-uc", "-b", "-d"], cwd=source, check=True)
    architecture = subprocess.check_output(["dpkg", "--print-architecture"], text=True).strip()
    package = source.parent / f"pathpilot_{version}-1_{architecture}.deb"
    args.output.mkdir(parents=True, exist_ok=True)
    shutil.copy2(package, args.output / package.name)
    print(f"Created {args.output / package.name}")


if __name__ == "__main__":
    main()
