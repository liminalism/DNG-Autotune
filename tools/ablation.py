#!/usr/bin/env python3
"""
Ablation harness for white-blowout advice (docs/white-blowout-advice.md:84).

Runs the preliminary CLI ablation sweep on one RAW and writes lossless PNGs.
The definitive comparison (frozen ToneParams) is via the synthetic suite
(src/synthetic.rs) and the highlight diagnostic dumps (--dump-stages).

Usage:
  python tools/ablation.py raw/raw_3rd_batch/_DSC1289.ARW /tmp/ablation
  python tools/ablation.py --all /tmp/ablation  # runs over a few samples
"""
import subprocess, sys, pathlib, json, os

TABLE = [
    ("recon-0", ["--highlight-reconstruction", "0"]),
    ("recon-0.25", ["--highlight-reconstruction", "0.25"]),
    ("recon-0.75", ["--highlight-reconstruction", "0.75"]),
    ("recon-1.0", ["--highlight-reconstruction", "1.0"]),
    ("rec2020", ["--working-space", "rec2020"]),
    ("highlight-contrast-1.2", ["--highlight-contrast", "1.2"]),
    ("recon0-local0.35", ["--highlight-reconstruction", "0", "--local-tone", "0.35"]),
]

def run_one(raw, outdir, extra):
    outdir = pathlib.Path(outdir)
    outdir.mkdir(parents=True, exist_ok=True)
    stem = pathlib.Path(raw).stem
    # Use dump-stages to emit the 6 diagnostic buffers plus gamut
    dump = outdir / extra[0]
    cmd = [
        "cargo", "run", "--release", "--", str(raw),
        "--output", str(outdir / extra[0]),
        "--format", "png",
        "--overwrite",
        "--dump-stages", str(dump),
    ] + extra[1]
    print(f"== {extra[0]}: {' '.join(cmd)}")
    subprocess.run(cmd, check=False)
    # Also collect --dry-run --summary for metrics
    summary = outdir / f"{extra[0]}-summary.json"
    cmd2 = [
        "cargo", "run", "--release", "--", str(raw),
        "--dry-run", "--summary", str(summary),
    ] + extra[1]
    subprocess.run(cmd2, check=False)
    if summary.exists():
        try:
            data = json.loads(summary.read_text())
            # Print clipped fractions if present
            for f in data.get("files", []):
                ana = f.get("analysis", {})
                print(f"  {extra[0]}: c1={ana.get('clipped_1_fraction',0):.4f} c2={ana.get('clipped_2_fraction',0):.4f} c3={ana.get('clipped_3_fraction',0):.4f}")
        except Exception as e:
            print(f"  summary parse failed: {e}")

def main():
    if len(sys.argv) < 3:
        print(__doc__)
        sys.exit(1)
    raw = sys.argv[1]
    out = sys.argv[2]
    if raw == "--all":
        # Run on a few representative files if available
        samples = list(pathlib.Path("raw/raw_3rd_batch").glob("*.ARW"))[:3]
        if not samples:
            samples = list(pathlib.Path("raw/arw").glob("*.ARW"))[:3]
        for s in samples:
            for name, args in TABLE:
                run_one(str(s), os.path.join(out, s.stem), (name, args))
    else:
        for name, args in TABLE:
            run_one(raw, out, (name, args))

if __name__ == "__main__":
    main()
