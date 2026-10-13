#!/usr/bin/env python3
"""Check or explicitly refresh the redistribution notices of pinned Craft engines."""
import argparse

from document_engines import ROOT, check_notices, engine_sources, expected_notices


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--update", action="store_true", help="copy notices after reviewing upstream license changes")
    parser.add_argument("--offline", action="store_true")
    args = parser.parse_args()
    expected = expected_notices(engine_sources(ROOT, args.offline))
    if args.update:
        directory = ROOT / "data/licenses"
        for name, data in expected.items():
            path = directory / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        for path in directory.rglob("*"):
            if path.is_file() and str(path.relative_to(directory)) not in expected:
                path.unlink()
    check_notices(ROOT, expected)
    print("Craft engine pins, Cargo.lock and redistribution notices match.")


if __name__ == "__main__":
    main()
