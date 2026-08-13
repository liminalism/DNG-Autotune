#!/usr/bin/env python3
"""Download pinned upstream weights into models/artifacts/upstream/."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_MANIFEST = ROOT / "models" / "manifest.json"


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def download(url: str, dest: Path) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)
    print(f"GET {url}", file=sys.stderr)
    with urllib.request.urlopen(url) as response, dest.open("wb") as out:
        while True:
            chunk = response.read(1 << 20)
            if not chunk:
                break
            out.write(chunk)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--only", action="append", default=[])
    args = parser.parse_args()
    manifest = json.loads(args.manifest.read_text())
    artifacts = ROOT / manifest["artifacts_dir"]
    wanted = set(args.only)

    for name, spec in manifest["upstream"].items():
        if wanted and name not in wanted:
            continue
        dest = artifacts / "upstream" / spec["filename"]
        if dest.exists() and spec.get("sha256"):
            actual = sha256_file(dest)
            if actual == spec["sha256"]:
                print(f"ok  {name} {dest}")
                continue
            print(f"hash mismatch for {dest}, re-downloading", file=sys.stderr)
        download(spec["url"], dest)
        actual = sha256_file(dest)
        expected = spec.get("sha256")
        if expected and actual != expected:
            raise SystemExit(f"{name}: sha256 {actual} != pinned {expected}")
        if not expected:
            print(f"PIN {name} sha256 {actual}", file=sys.stderr)
        print(f"got {name} {dest} {actual}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
