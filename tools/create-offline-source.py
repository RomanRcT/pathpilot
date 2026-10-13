#!/usr/bin/env python3
"""Create a release source archive with registry/Git crates, assets and notices."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib

from document_engines import ROOT, check_notices, engine_sources, expected_notices, pinned_engines

# Copy project inputs explicitly; local credentials, build outputs and Cargo
# configuration must never enter a distributed source archive.
PROJECT_DIRECTORIES = [".github", "crates", "data", "docs", "packaging", "tools"]
PROJECT_FILES = ["Cargo.toml", "Cargo.lock", "Makefile", "README.md", "CHANGELOG.md", "LICENSE",
                 "CONTRIBUTING.md", "pathpilot_project_plan.md", ".gitignore", "renovate.json"]


def copy_project(root, destination):
    destination.mkdir()
    for name in PROJECT_DIRECTORIES:
        if (root / name).is_dir():
            shutil.copytree(root / name, destination / name,
                            ignore=shutil.ignore_patterns("target", "__pycache__", ".git"))
    for name in PROJECT_FILES:
        if (root / name).is_file():
            shutil.copy2(root / name, destination / name)


def write_archive(stage, output):
    output.parent.mkdir(parents=True, exist_ok=True)
    with tarfile.open(output, "w:gz", compresslevel=6) as archive:
        archive.add(stage, arcname=stage.name)
    with output.open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    output.with_name(output.name + ".sha256").write_text(f"{digest}  {output.name}\n")
    return digest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "release-assets")
    parser.add_argument("--offline", action="store_true", help="use already cached dependency sources only")
    args = parser.parse_args()
    notices = expected_notices(engine_sources(ROOT, args.offline))
    check_notices(ROOT, notices)
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    if not all(character.isalnum() or character in ".-+" for character in version):
        raise ValueError("Invalid package version")
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    dirty = bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT))
    with tempfile.TemporaryDirectory(prefix="pathpilot-source-") as directory:
        stage = Path(directory) / f"pathpilot-{version}"
        copy_project(ROOT, stage)
        command = [sys.executable, str(stage / "tools/vendor-dependencies.py")]
        if args.offline:
            command.append("--offline")
        subprocess.check_call(command, cwd=stage)
        manifest = {
            "version": version, "pathpilot_commit": commit, "working_tree_modified": dirty,
            "document_engines": pinned_engines(ROOT),
            "cargo_lock_sha256": hashlib.sha256((stage / "Cargo.lock").read_bytes()).hexdigest(),
            "notice_sha256": {name: hashlib.sha256(data).hexdigest() for name, data in sorted(notices.items())},
        }
        (stage / "SOURCE-MANIFEST.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
        (stage / "OFFLINE-BUILD.md").write_text(
            "# Offline source build\n\n"
            "This archive includes the locked registry/Git crates, WordCraft assets and license notices.\n"
            "Install Rust 1.90+ and the GTK 4, libadwaita, VTE GTK4, WebKitGTK 6.0, Cairo/Pango development packages first.\n"
            "From this directory run:\n\n"
            "```sh\ncargo build --release --locked --offline\n```\n\n"
            "No access to crates.io or the Craft GitHub repositories is needed for this build.\n"
            "SOURCE-MANIFEST.json records the exact engine commits, lockfile and notice hashes.\n"
            "The adjacent .sha256 file verifies the source archive; release SHA256SUMS covers all assets.\n"
        )
        output = args.output.resolve() / f"pathpilot-{version}-offline-source.tar.gz"
        digest = write_archive(stage, output)
    print(f"Created {output}\nSHA256 {digest}")


if __name__ == "__main__":
    main()
