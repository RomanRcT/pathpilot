#!/usr/bin/env python3
"""Verify a source snapshot and compile it with an empty Cargo cache, offline."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile


def unpack(archive_path, directory):
    checksum = archive_path.with_name(archive_path.name + ".sha256")
    expected = checksum.read_text().split()[0]
    with archive_path.open("rb") as source:
        actual = hashlib.file_digest(source, "sha256").hexdigest()
    if actual != expected:
        raise ValueError("Offline source archive checksum mismatch")
    with tarfile.open(archive_path, "r:gz") as archive:
        roots = {member.name.split("/", 1)[0] for member in archive.getmembers()}
        if len(roots) != 1:
            raise ValueError("Expected one source directory in the archive")
        archive.extractall(directory, filter="data")
    root = directory / roots.pop()
    for required in ["Cargo.lock", ".cargo/config.toml", "vendor", "assets", "data/licenses", "SOURCE-MANIFEST.json"]:
        if not (root / required).exists():
            raise ValueError(f"Offline source snapshot is missing {required}")
    manifest = json.loads((root / "SOURCE-MANIFEST.json").read_text())
    if hashlib.sha256((root / "Cargo.lock").read_bytes()).hexdigest() != manifest["cargo_lock_sha256"]:
        raise ValueError("Source manifest lockfile hash mismatch")
    for name, digest in manifest["notice_sha256"].items():
        if hashlib.sha256((root / "data/licenses" / name).read_bytes()).hexdigest() != digest:
            raise ValueError(f"Source manifest notice hash mismatch: {name}")
    return root


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    parser.add_argument("--release", action="store_true", help="link release binaries instead of checking all targets")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="pathpilot-offline-verify-") as temporary:
        directory = Path(temporary)
        root = unpack(args.archive.resolve(), directory)
        environment = dict(os.environ, CARGO_HOME=str(directory / "cargo-home"),
                           CARGO_TARGET_DIR=str(directory / "target"), CARGO_NET_OFFLINE="true")
        command = ["cargo", "build", "--release"] if args.release else ["cargo", "check", "--workspace", "--all-targets"]
        subprocess.check_call(command + ["--locked", "--offline"], cwd=root, env=environment)
    print("Offline source verified with a fresh Cargo cache.")


if __name__ == "__main__":
    main()
