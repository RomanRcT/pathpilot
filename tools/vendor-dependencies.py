#!/usr/bin/env python3
"""Vendor registry/git crates and WordCraft's compile-time assets for offline builds."""
import json
from pathlib import Path
import shutil
import subprocess

root = Path(__file__).resolve().parent.parent
configuration = subprocess.check_output(
    ["cargo", "vendor", "--locked", "vendor"], cwd=root, text=True
)
metadata = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--locked", "--offline", "--format-version", "1"], cwd=root, text=True
))
fonts = next(package for package in metadata["packages"] if package["name"] == "wordcraft-fonts")
wordcraft = Path(fonts["manifest_path"]).parent.parent.parent
# Upstream include_bytes! paths reach outside the individual crates. Cargo vendor
# copies crate directories, so the workspace-level assets need a separate copy.
for directory in ("fonts", "hyphenation", "spelling"):
    source = wordcraft / "assets" / directory
    destination = root / "assets" / directory
    if source.resolve() != destination.resolve():
        shutil.copytree(source, destination, dirs_exist_ok=True)
(root / ".cargo").mkdir(exist_ok=True)
(root / ".cargo" / "config.toml").write_text(configuration)
