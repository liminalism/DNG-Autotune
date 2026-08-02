#!/usr/bin/env python3
"""Reproducible flamegraph profiling harness for the raw-autotune binary.

Modeled on jpegXL-rs/jxl-encoder/scripts/paired_profile.py's conventions —
argument vectors instead of shell strings, a metadata JSON evidence trail
(host, git revision, build command, artifact hashes), and a perf preflight
check — but for a single batch binary rather than a paired A/B comparison.

`raw-autotune` averages ~1.5s/file; most of that is decode, demosaic,
analyze, tone-map and encode all running inside one process per file, so a
sampling profiler over a real batch is the direct way to see where it goes.
The default run is `--jobs 1` over a sample of files: single-threaded, so
every stack sample attributes to the pipeline stage that was actually
running, not to rayon's scheduler interleaving several files' work at once.
Pass `--raw-autotune-jobs auto` to profile the real concurrent path instead
once the single-threaded picture is understood.

Usage:
    tools/flamegraph_profile.py --input raw/arw --sample-size 30
    tools/flamegraph_profile.py --input raw/arw --sample-size 30 --dry-run
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import platform
import re
import shlex
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any

RAW_EXTENSIONS = {".arw", ".dng", ".nef", ".cr2", ".cr3", ".raf"}


class HarnessError(RuntimeError):
    pass


def run_command(command: list[str], *, cwd: Path | None = None) -> subprocess.CompletedProcess[str]:
    return subprocess.run(command, cwd=cwd, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)


def run_redirected(command: list[str], stdout_path: Path, stderr_path: Path, *, cwd: Path | None = None) -> None:
    with stdout_path.open("w", encoding="utf-8") as stdout, stderr_path.open("w", encoding="utf-8") as stderr:
        result = subprocess.run(command, stdout=stdout, stderr=stderr, text=True, cwd=cwd, check=False)
    if result.returncode:
        raise HarnessError(f"command failed ({result.returncode}): {' '.join(command)}; inspect {stderr_path}")


def run_redirected_stdin(command: list[str], input_path: Path, stdout_path: Path, stderr_path: Path) -> None:
    with input_path.open("r", encoding="utf-8") as stdin, stdout_path.open("w", encoding="utf-8") as stdout, stderr_path.open("w", encoding="utf-8") as stderr:
        result = subprocess.run(command, stdin=stdin, stdout=stdout, stderr=stderr, text=True, check=False)
    if result.returncode:
        raise HarnessError(f"command failed ({result.returncode}): {' '.join(command)}; inspect {stderr_path}")


def command_version(command: list[str]) -> dict[str, Any]:
    resolved = shutil.which(command[0]) or command[0]
    try:
        result = run_command(command)
    except OSError as error:
        return {"command": command, "path": resolved, "available": False, "error": str(error)}
    return {
        "command": command,
        "path": resolved,
        "available": result.returncode == 0,
        "returncode": result.returncode,
        "stdout": result.stdout.strip(),
        "stderr": result.stderr.strip(),
    }


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def file_evidence(path: Path, *, relative_to: Path | None = None) -> dict[str, Any]:
    if not path.is_file():
        raise HarnessError(f"expected artifact is not a file: {path}")
    evidence_path = str(path.relative_to(relative_to)) if relative_to else str(path)
    return {"path": evidence_path, "bytes": path.stat().st_size, "sha256": sha256(path)}


def host_metadata() -> dict[str, Any]:
    data: dict[str, Any] = {
        "platform": platform.platform(),
        "uname": platform.uname()._asdict(),
        "python": sys.version,
        "cpu_count": os.cpu_count(),
    }
    for name, command in {
        "lscpu": ["lscpu"],
        "rustc": ["rustc", "-Vv"],
        "cargo": ["cargo", "-V"],
        "perf": ["perf", "--version"],
        "git": ["git", "--version"],
    }.items():
        data[name] = command_version(command)
    return data


def git_metadata(directory: Path) -> dict[str, Any]:
    if not (directory / ".git").exists():
        return {"path": str(directory), "git": False}
    revision = run_command(["git", "rev-parse", "HEAD"], cwd=directory)
    status = run_command(["git", "status", "--porcelain=v1"], cwd=directory)
    return {
        "path": str(directory),
        "git": revision.returncode == 0,
        "revision": revision.stdout.strip() if revision.returncode == 0 else None,
        "dirty": status.stdout.splitlines() if status.returncode == 0 else None,
    }


def write_json(path: Path, value: Any) -> None:
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def safe_run_id(value: str) -> str:
    cleaned = re.sub(r"[^A-Za-z0-9._-]+", "-", value).strip(".-")
    if not cleaned:
        raise HarnessError("run id is empty after sanitization")
    return cleaned


def perf_event_access_metadata(path: Path) -> dict[str, Any]:
    metadata: dict[str, Any] = {
        "path": str(path),
        "effective_uid": os.geteuid(),
        "is_root": os.geteuid() == 0,
        "checked_at_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
    }
    try:
        metadata["value"] = int(path.read_text(encoding="utf-8").strip())
    except (OSError, ValueError) as error:
        metadata["value"] = None
        metadata["error"] = str(error)
    return metadata


def require_kernel_perf_access(metadata: dict[str, Any]) -> None:
    value = metadata.get("value")
    if value is None:
        raise HarnessError(
            f"unable to read kernel perf access setting from {metadata['path']}: "
            f"{metadata.get('error', 'unknown error')}"
        )
    if not metadata["is_root"] and value > 0:
        raise HarnessError(
            f"kernel.perf_event_paranoid={value}; perf-based flamegraph profiling requires 0 "
            "for this non-root session. Run exactly: sudo sysctl -w kernel.perf_event_paranoid=0"
        )


def require_free_disk_bytes(path: Path, minimum_bytes: int) -> None:
    usage = shutil.disk_usage(path)
    if usage.free < minimum_bytes:
        raise HarnessError(
            f"only {usage.free / 1024**3:.1f} GiB free on {path}'s filesystem, "
            f"below the {minimum_bytes / 1024**3:.1f} GiB floor; a prior run with "
            "--call-graph dwarf filled this disk (1.5+ GB perf.data for 30 files) — "
            "free space or lower --min-free-disk-gib deliberately."
        )


def ensure_flamegraph_tools() -> dict[str, str]:
    required = {"perf": "perf", "nm": "nm (binutils)", "readelf": "readelf (binutils)"}
    found: dict[str, str] = {}
    missing: list[str] = []
    for executable, package in required.items():
        path = shutil.which(executable)
        if path:
            found[executable] = path
        else:
            missing.append(package)
    if missing:
        raise HarnessError("flamegraph prerequisites missing: " + ", ".join(missing) + ". Install Linux perf and binutils.")
    perl_collapse = shutil.which("stackcollapse-perf.pl")
    perl_render = shutil.which("flamegraph.pl")
    inferno_collapse = shutil.which("inferno-collapse-perf")
    inferno_render = shutil.which("inferno-flamegraph")
    if perl_collapse and perl_render:
        found.update({"collapse": perl_collapse, "render": perl_render, "flavor": "perl"})
    elif inferno_collapse and inferno_render:
        found.update({"collapse": inferno_collapse, "render": inferno_render, "flavor": "inferno"})
    else:
        raise HarnessError(
            "flamegraph prerequisites missing: install either stackcollapse-perf.pl plus "
            "flamegraph.pl (FlameGraph) or inferno-collapse-perf plus inferno-flamegraph "
            "(cargo install inferno)."
        )
    return found


def discover_raw_inputs(input_path: Path, sample_size: int | None) -> list[Path]:
    if input_path.is_file():
        return [input_path]
    if not input_path.is_dir():
        raise HarnessError(f"--input is neither a file nor a directory: {input_path}")
    files = sorted(
        p for p in input_path.rglob("*") if p.is_file() and p.suffix.lower() in RAW_EXTENSIONS
    )
    if not files:
        raise HarnessError(f"no RAW files ({', '.join(sorted(RAW_EXTENSIONS))}) found under {input_path}")
    if sample_size is not None:
        files = files[:sample_size]
    return files


def build_profiling_binary(args: argparse.Namespace, run_dir: Path) -> tuple[Path, dict[str, Any]]:
    binary = (args.repo_dir / "target" / args.cargo_profile / "raw-autotune").resolve()
    record: dict[str, Any] = {
        "command": None,
        "executed": not args.skip_build,
        "binary": str(binary),
    }
    if args.skip_build:
        if not binary.is_file():
            raise HarnessError(f"--skip-build given but binary does not exist: {binary}")
        return binary, record
    env = os.environ.copy()
    # Frame-pointer unwinding is a cheap safety net alongside --call-graph dwarf;
    # matches paired_profile.py's required_build_settings for the same reason.
    env["RUSTFLAGS"] = (env.get("RUSTFLAGS", "") + " -C force-frame-pointers=yes").strip()
    command = ["cargo", "build", "--locked", f"--profile={args.cargo_profile}", "--bin", "raw-autotune"]
    record["command"] = command
    record["rustflags"] = env["RUSTFLAGS"]
    if args.dry_run:
        return binary, record
    stdout_path = run_dir / "logs" / "cargo-build.stdout"
    stderr_path = run_dir / "logs" / "cargo-build.stderr"
    with stdout_path.open("w", encoding="utf-8") as stdout, stderr_path.open("w", encoding="utf-8") as stderr:
        result = subprocess.run(command, cwd=args.repo_dir, env=env, stdout=stdout, stderr=stderr, text=True, check=False)
    record["exit_code"] = result.returncode
    if result.returncode:
        raise HarnessError(f"profiling build failed; inspect {stderr_path}")
    if not binary.is_file():
        raise HarnessError(f"build reported success but binary is missing: {binary}")
    return binary, record


def execute(args: argparse.Namespace) -> int:
    repo_dir = args.repo_dir.resolve()
    input_path = args.input.resolve()
    if not args.dry_run and not input_path.exists():
        raise HarnessError(f"--input does not exist: {input_path}")

    default_id = f"{dt.datetime.now(dt.timezone.utc):%Y%m%dT%H%M%SZ}-raw-autotune"
    run_id = safe_run_id(args.run_id or default_id)
    run_dir = (args.artifact_root / run_id).resolve()
    if run_dir.exists() and any(run_dir.iterdir()):
        raise HarnessError(f"artifact directory already exists and is non-empty: {run_dir}")
    for directory in (run_dir, run_dir / "logs", run_dir / "outputs"):
        directory.mkdir(parents=True, exist_ok=True)

    metadata: dict[str, Any] = {
        "schema_version": 1,
        "run_id": run_id,
        "created_at_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "dry_run": args.dry_run,
        "configuration": {
            "input": str(input_path),
            "sample_size": args.sample_size,
            "raw_autotune_jobs": args.raw_autotune_jobs,
            "raw_autotune_format": args.raw_autotune_format,
            "cargo_profile": args.cargo_profile,
            "perf_frequency": args.perf_frequency,
            "skip_build": args.skip_build,
        },
        "source": git_metadata(repo_dir),
        "host_and_tools": host_metadata(),
    }
    write_json(run_dir / "run.json", metadata)

    binary, build_record = build_profiling_binary(args, run_dir)
    metadata["build"] = build_record
    metadata["binary"] = {
        "path": str(binary),
        "sha256": sha256(binary) if binary.is_file() and not args.dry_run else None,
    }
    write_json(run_dir / "run.json", metadata)

    inputs = discover_raw_inputs(input_path, args.sample_size) if (args.dry_run or input_path.exists()) else []
    metadata["inputs"] = {
        "count": len(inputs),
        "files": [str(p) for p in inputs],
    }

    output_dir = run_dir / "outputs"
    command = [
        str(binary),
        *[str(p) for p in inputs],
        "--output", str(output_dir),
        "--format", args.raw_autotune_format,
        "--jobs", args.raw_autotune_jobs,
    ]
    metadata["planned_command"] = command
    write_json(run_dir / "run.json", metadata)

    if args.dry_run:
        print(run_dir)
        return 0

    perf_access = perf_event_access_metadata(args.perf_event_paranoid_path)
    metadata["perf_preflight"] = perf_access
    write_json(run_dir / "run.json", metadata)
    require_kernel_perf_access(perf_access)
    tools = ensure_flamegraph_tools()
    metadata["flamegraph_toolchain"] = {
        name: {"path": path, "sha256": sha256(Path(path)) if Path(path).is_file() else None}
        for name, path in tools.items()
        if name != "flavor"
    } | {"flavor": tools["flavor"]}
    write_json(run_dir / "run.json", metadata)

    require_free_disk_bytes(run_dir, args.min_free_disk_gib * 1024**3)

    perf_data = run_dir / "raw-autotune.perf.data"
    perf_stdout = run_dir / "logs" / "raw-autotune.stdout"
    perf_stderr = run_dir / "logs" / "raw-autotune.stderr"
    # Frame-pointer unwinding, not --call-graph dwarf: the profiling build sets
    # -C force-frame-pointers=yes for exactly this, and dwarf's per-sample stack
    # dump (default 8 KiB) makes perf.data enormous — 30 files at 999 Hz produced
    # a 1.5 GB perf.data plus a 1.4 GB perf.script that filled this machine's
    # already-99%-full /mnt/Samsung980_1TB to 100% mid-run. fp samples are a
    # handful of return addresses each; same symbols resolve since the binary
    # (and, via RUSTFLAGS, its dependencies) keep frame pointers.
    profiled = [tools["perf"], "record", "--call-graph", "fp", "-F", str(args.perf_frequency), "-o", str(perf_data), "--", *command]
    metadata["perf_record_command"] = profiled
    write_json(run_dir / "run.json", metadata)
    run_redirected(profiled, perf_stdout, perf_stderr)

    produced = sorted(p for p in output_dir.rglob("*") if p.is_file())
    if not produced:
        raise HarnessError(f"perf record succeeded but raw-autotune produced no output under {output_dir}")

    script_path = run_dir / "raw-autotune.perf.script"
    run_redirected([tools["perf"], "script", "-i", str(perf_data)], script_path, run_dir / "logs" / "perf-script.stderr")

    folded = run_dir / "raw-autotune.folded"
    if tools["flavor"] == "perl":
        run_redirected([tools["collapse"], str(script_path)], folded, run_dir / "logs" / "stackcollapse.stderr")
    else:
        run_redirected_stdin([tools["collapse"]], script_path, folded, run_dir / "logs" / "stackcollapse.stderr")

    svg = run_dir / "flamegraph.svg"
    title = f"raw-autotune ({len(inputs)} file(s), jobs={args.raw_autotune_jobs})"
    if tools["flavor"] == "perl":
        run_redirected([tools["render"], "--title", title, str(folded)], svg, run_dir / "logs" / "flamegraph.stderr")
    else:
        run_redirected_stdin([tools["render"], "--title", title], folded, svg, run_dir / "logs" / "flamegraph.stderr")

    inclusive = run_dir / "perf-report-inclusive.txt"
    exclusive = run_dir / "perf-report-exclusive.txt"
    run_redirected([tools["perf"], "report", "--stdio", "--children", "-i", str(perf_data)], inclusive, run_dir / "logs" / "perf-report-inclusive.stderr")
    run_redirected([tools["perf"], "report", "--stdio", "--no-children", "-i", str(perf_data)], exclusive, run_dir / "logs" / "perf-report-exclusive.stderr")

    nm_path = run_dir / "raw-autotune.nm.txt"
    readelf_path = run_dir / "raw-autotune.readelf.txt"
    run_redirected([tools["nm"], "--demangle", "--numeric-sort", str(binary)], nm_path, run_dir / "logs" / "nm.stderr")
    run_redirected([tools["readelf"], "--wide", "--symbols", str(binary)], readelf_path, run_dir / "logs" / "readelf.stderr")

    required_evidence = (perf_data, script_path, folded, svg, inclusive, exclusive, nm_path, readelf_path)
    missing_evidence = [str(p) for p in required_evidence if not p.is_file() or p.stat().st_size == 0]
    if missing_evidence:
        raise HarnessError(f"flamegraph evidence is missing or empty: {', '.join(missing_evidence)}")

    metadata["evidence"] = {
        "perf_data": file_evidence(perf_data, relative_to=run_dir),
        "perf_script": file_evidence(script_path, relative_to=run_dir),
        "folded": file_evidence(folded, relative_to=run_dir),
        "svg": file_evidence(svg, relative_to=run_dir),
        "perf_report_inclusive": file_evidence(inclusive, relative_to=run_dir),
        "perf_report_exclusive": file_evidence(exclusive, relative_to=run_dir),
        "nm": file_evidence(nm_path, relative_to=run_dir),
        "readelf": file_evidence(readelf_path, relative_to=run_dir),
        "outputs": [file_evidence(p, relative_to=run_dir) for p in produced],
    }
    metadata["completed_at_utc"] = dt.datetime.now(dt.timezone.utc).isoformat()
    write_json(run_dir / "run.json", metadata)
    print(run_dir)
    print(svg)
    return 0


def make_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--input", type=Path, default=Path("raw/arw"), help="RAW file or directory to sample from")
    parser.add_argument("--sample-size", type=int, default=30, help="number of files to profile, first N by sorted path (0 = all)")
    parser.add_argument("--raw-autotune-jobs", default="1", help="raw-autotune --jobs value; '1' isolates per-stage attribution, 'auto' profiles the real concurrent path")
    parser.add_argument("--raw-autotune-format", default="jpeg", choices=["jpeg", "png", "tiff"], help="output format, so the profile includes real encode cost")
    parser.add_argument("--cargo-profile", default="profiling", help="Cargo profile to build (see [profile.profiling] in Cargo.toml)")
    parser.add_argument("--skip-build", action="store_true", help="reuse the existing target/<cargo-profile>/raw-autotune binary")
    parser.add_argument("--perf-frequency", type=int, default=999)
    parser.add_argument("--min-free-disk-gib", type=float, default=2.0, help="abort before perf record if the artifact filesystem has less free space than this")
    parser.add_argument("--perf-event-paranoid-path", type=Path, default=Path("/proc/sys/kernel/perf_event_paranoid"), help=argparse.SUPPRESS)
    parser.add_argument("--artifact-root", type=Path, default=Path("target/profile-artifacts"))
    parser.add_argument("--run-id", help="unique artifact directory name")
    parser.add_argument("--repo-dir", type=Path, default=Path.cwd())
    parser.add_argument("--dry-run", action="store_true", help="write the planned run without executing anything")
    return parser


def main() -> int:
    args = make_parser().parse_args()
    if args.sample_size == 0:
        args.sample_size = None
    try:
        return execute(args)
    except HarnessError as error:
        print(f"flamegraph_profile: error: {error}", file=sys.stderr)
        return 2
    except OSError as error:
        print(f"flamegraph_profile: OS error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
