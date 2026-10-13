"""Shared metadata for pinned Craft engines and their redistribution notices."""
import json
from pathlib import Path
import re
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parent.parent
ENGINES = {
    "wordcraft": {"repository": "https://github.com/storytold/wordcraft", "package": "wordcraft-fonts"},
    "gridcraft": {"repository": "https://github.com/storytold/gridcraft", "package": "gridcraft-model"},
}


def pinned_engines(root=ROOT):
    manifest = tomllib.loads((root / "Cargo.toml").read_text())
    dependencies = manifest["workspace"]["dependencies"]
    result = {}
    for name, engine in ENGINES.items():
        modules = [value for key, value in dependencies.items() if key.startswith(name + "-")]
        if not modules or any(value.get("git") != engine["repository"] for value in modules):
            raise ValueError(f"{name}: unexpected or missing Git dependencies")
        revisions = {value.get("rev", "") for value in modules}
        if len(revisions) != 1 or not re.fullmatch(r"[0-9a-f]{40}", next(iter(revisions))):
            raise ValueError(f"{name}: all modules must pin the same full commit SHA")
        result[name] = {"repository": engine["repository"], "revision": revisions.pop(), "license": "MIT"}
    return result


def engine_sources(root=ROOT, offline=False):
    host = next(line.split(": ", 1)[1] for line in subprocess.check_output(
        ["rustc", "-vV"], text=True).splitlines() if line.startswith("host: "))
    command = ["cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", host]
    if offline:
        command.append("--offline")
    metadata = json.loads(subprocess.check_output(command, cwd=root, text=True))
    pins = pinned_engines(root)
    sources = {}
    for name, engine in ENGINES.items():
        package = next(package for package in metadata["packages"] if package["name"] == engine["package"])
        expected = f'git+{engine["repository"]}?rev={pins[name]["revision"]}#{pins[name]["revision"]}'
        if package.get("source") != expected:
            raise ValueError(f"{name}: Cargo.lock does not match the manifest's pinned revision")
        sources[name] = Path(package["manifest_path"]).parent.parent.parent
    return sources


def expected_notices(sources):
    files = {}
    for name, source in sources.items():
        for filename in ["LICENSE-MIT", "LICENSE-APACHE", "NOTICE", "ATTRIBUTION.md"]:
            files[f"{name}/{filename}"] = (source / filename).read_bytes()
        if name == "wordcraft":
            for license_file in sorted((source / "assets/fonts").glob("OFL-*.txt")):
                files[f"{name}/{license_file.name}"] = license_file.read_bytes()
            for filename in ["pl.pat", "pt.pat"]:
                files[f"{name}/{filename}"] = (source / "assets/hyphenation" / filename).read_bytes()
    return files


def check_notices(root, expected):
    directory = root / "data/licenses"
    actual = {str(path.relative_to(directory)) for path in directory.rglob("*") if path.is_file()}
    differences = set(expected) ^ actual
    differences.update(name for name, data in expected.items()
                       if not (directory / name).is_file() or (directory / name).read_bytes() != data)
    if differences:
        raise ValueError("License notices differ from the pinned sources: " + ", ".join(sorted(differences))
                         + ". Review upstream changes, then run tools/document-licenses.py --update.")
