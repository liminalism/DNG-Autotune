#!/usr/bin/env python3
"""Fetch, prepare YuNet, and export the remaining pack members."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TOOLS = Path(__file__).resolve().parent
DEFAULT_MANIFEST = ROOT / "models" / "manifest.json"


def run(cmd: list[str]) -> None:
    print("+", " ".join(cmd), file=sys.stderr)
    subprocess.check_call(cmd)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--skip-export", action="store_true")
    args = parser.parse_args()
    manifest = json.loads(args.manifest.read_text())
    artifacts = ROOT / manifest["artifacts_dir"]

    run([sys.executable, str(TOOLS / "fetch.py"), "--manifest", str(args.manifest)])

    yunet_src = artifacts / "upstream" / manifest["upstream"]["yunet_2023mar"]["filename"]
    yunet_dst = artifacts / manifest["prepared"]["yunet"]["filename"]
    run(
        [
            sys.executable,
            str(TOOLS / "prepare.py"),
            str(yunet_src),
            str(yunet_dst),
            "--source-url",
            manifest["upstream"]["yunet_2023mar"]["url"],
            "--license",
            manifest["prepared"]["yunet"]["license"],
            "--role",
            manifest["prepared"]["yunet"]["role"],
            "--preprocess",
            json.dumps(manifest["prepared"]["yunet"]["preprocess"]),
        ]
    )

    if not args.skip_export:
        run([sys.executable, str(TOOLS / "export.py"), "--manifest", str(args.manifest)])
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
