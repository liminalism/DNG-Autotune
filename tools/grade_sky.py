#!/usr/bin/env python3
"""Grade rendered frames against the camera's own JPEG of the same capture.

Why this exists
---------------
The sky questions this measures are not answerable from a sidecar. `--summary`
reports what the controller *decided*; this reports what the picture *looks
like*, and specifically the three things that went wrong on the tropical batch:

1. Is the sky the right brightness? Measured against the paired camera JPEG,
   not against an absolute target, because the camera is the only reference
   this corpus has.
2. Did the ground follow the sky? A sky control that drags the ground with it
   is a contrast control, and the ground already matches the camera to within
   0.07 EV on this batch. This is the number that catches that.
3. Is the sky *coloured wrong*? The lavender-magenta failure showed up as a
   green deficit against the brightest channel in bright pixels. That is the
   quantity, not "looks purple".

It reads finished JPEG or PNG renders, so it does not care which renderer
produced them and cannot drift out of sync with the pipeline the way a
reimplementation would.

Usage
-----
    python tools/grade_sky.py RENDERED_DIR [--reference raw/jpeg] [--json OUT]

    # compare a parameter sweep, one directory per setting
    python tools/grade_sky.py raw-autotune-output/sweep/hc1.00 \\
                              raw-autotune-output/sweep/hc1.20 \\
                              raw-autotune-output/sweep/hc1.40

Each RENDERED_DIR is searched recursively and matched to the reference by
filename stem, so `_DSC1283_auto.jpg` pairs with `_DSC1283.JPG` even when a
normal recursive render placed it below an input-folder child directory.
Frames without a pair are skipped and counted.

Requires only Pillow and numpy.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

try:
    import numpy as np
    from PIL import Image
except ImportError:  # pragma: no cover - a setup problem, not a logic one
    sys.exit("grade_sky.py needs Pillow and numpy: pip install pillow numpy")

# Rec.709 luminance, matching analyze::luminance so the numbers here and the
# ones in the sidecar mean the same thing.
LUMA = np.array([0.2126, 0.7152, 0.0722], dtype=np.float64)

# The sky band. A fixed top fraction rather than a segmentation: segmentation
# would be a second thing that can be wrong, and every frame in this corpus is
# a landscape with sky at the top. Frames where that is false should not be
# graded with this tool.
SKY_BAND = 0.15
# The ground band, taken from the bottom half for the same reason.
GROUND_BAND = 0.50

# Analysis is done on a downscale. The measurements are all population
# statistics over hundreds of thousands of pixels, so full resolution buys
# nothing and costs a minute per frame.
LONG_EDGE = 1400


def srgb_to_linear(x: np.ndarray) -> np.ndarray:
    return np.where(x <= 0.04045, x / 12.92, ((x + 0.055) / 1.055) ** 2.4)


def load(path: Path) -> np.ndarray:
    image = Image.open(path).convert("RGB")
    image.thumbnail((LONG_EDGE, LONG_EDGE), Image.LANCZOS)
    return srgb_to_linear(np.asarray(image).astype(np.float64) / 255.0)


def green_deficit(rgb: np.ndarray) -> float:
    """How far green sits below the brightest channel, as a fraction.

    This is the lavender-magenta signature. On a neutral or a natural blue sky
    it is small; the blown A7C skies ran 28-78% before the near-white anchor
    rule landed. Averaged over the population first, then reduced, so a few
    saturated pixels cannot dominate.
    """
    if rgb.size == 0:
        return float("nan")
    mean = rgb.reshape(-1, 3).mean(axis=0)
    peak = mean.max()
    if peak <= 0:
        return float("nan")
    return float(1.0 - mean[1] / peak)


def measure(rendered: Path, reference: Path) -> dict:
    ours, theirs = load(rendered), load(reference)
    height = min(ours.shape[0], theirs.shape[0])
    ours, theirs = ours[:height], theirs[:height]

    sky = slice(0, max(1, int(height * SKY_BAND)))
    ground = slice(int(height * GROUND_BAND), height)

    def ev(a: np.ndarray, b: np.ndarray) -> float:
        la = (a * LUMA).sum(axis=2).mean()
        lb = (b * LUMA).sum(axis=2).mean()
        return float(np.log2(max(la, 1e-6) / max(lb, 1e-6)))

    ours_lum = (ours * LUMA).sum(axis=2)
    bright = ours_lum > np.percentile(ours_lum, 90)

    # Pixels the eye reads as the magenta failure: brighter than mid, with both
    # red and blue clearly above green. Thresholds are in linear light and
    # deliberately loose; the number is for comparing settings, not for a gate.
    r, g, b = ours[..., 0], ours[..., 1], ours[..., 2]
    purple = float(((r > g * 1.06) & (b > g * 1.06) & (ours_lum > 0.18)).mean())

    return {
        "frame": rendered.stem,
        "sky_ev_vs_camera": ev(ours[sky], theirs[sky]),
        "ground_ev_vs_camera": ev(ours[ground], theirs[ground]),
        "sky_green_deficit": green_deficit(ours[sky]),
        "sky_green_deficit_camera": green_deficit(theirs[sky]),
        "bright_green_deficit": green_deficit(ours[bright]),
        "purple_fraction": purple,
    }


def index_reference(reference_dir: Path) -> dict[str, Path]:
    index: dict[str, Path] = {}
    for path in sorted(reference_dir.iterdir()):
        if path.suffix.lower() in {".jpg", ".jpeg"}:
            index.setdefault(path.stem.lower(), path)
    return index


def stem_of(rendered: Path) -> str:
    """`_DSC1283_auto.jpg` and `_DSC1283.jpg` both key on `_dsc1283`."""
    stem = rendered.stem
    for suffix in ("_auto", "_baseline", "_neutral", "_punchy"):
        if stem.lower().endswith(suffix):
            stem = stem[: -len(suffix)]
            break
    return stem.lower()


def rendered_frames(rendered_dir: Path) -> list[Path]:
    """Return finished JPEG/PNG renders below a directory in stable order.

    `raw-autotune --recursive` preserves the input directory structure below
    its output root.  Looking only at `iterdir()` silently yields no pairs for
    that ordinary invocation, so the grader must follow that layout too.
    """
    return sorted(
        path
        for path in rendered_dir.rglob("*")
        if path.is_file() and path.suffix.lower() in {".jpg", ".jpeg", ".png"}
    )


def grade(rendered_dir: Path, index: dict[str, Path]) -> tuple[list[dict], int]:
    rows, unpaired = [], 0
    for path in rendered_frames(rendered_dir):
        match = index.get(stem_of(path))
        if match is None:
            unpaired += 1
            continue
        rows.append(measure(path, match))
    return rows, unpaired


def summarize(rows: list[dict]) -> dict:
    if not rows:
        return {}
    keys = [k for k in rows[0] if k != "frame"]
    return {k: float(np.nanmedian([r[k] for r in rows])) for k in keys}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("rendered", nargs="+", type=Path)
    parser.add_argument("--reference", type=Path, default=Path("raw/jpeg"))
    parser.add_argument("--json", type=Path, default=None)
    parser.add_argument(
        "--per-frame", action="store_true", help="print every frame, not just the median"
    )
    args = parser.parse_args()

    if not args.reference.is_dir():
        return f"reference directory not found: {args.reference}"
    index = index_reference(args.reference)
    if not index:
        return f"no JPEGs in {args.reference}"

    report = {}
    header = (
        f"{'setting':<28} {'n':>4} {'sky EV':>8} {'ground EV':>10} "
        f"{'sky Gdef':>9} {'cam Gdef':>9} {'purple':>8}"
    )
    print(header)
    print("-" * len(header))
    for directory in args.rendered:
        if not directory.is_dir():
            print(f"{directory}: not a directory, skipped")
            continue
        rows, unpaired = grade(directory, index)
        report[str(directory)] = {"frames": rows, "median": summarize(rows)}
        if not rows:
            print(f"{directory.name:<28} {0:>4}  (no paired frames)")
            continue
        m = summarize(rows)
        print(
            f"{directory.name:<28} {len(rows):>4} "
            f"{m['sky_ev_vs_camera']:>+8.2f} {m['ground_ev_vs_camera']:>+10.2f} "
            f"{m['sky_green_deficit']:>8.1%} {m['sky_green_deficit_camera']:>8.1%} "
            f"{m['purple_fraction']:>7.2%}"
        )
        if unpaired:
            print(f"{'':<28} {unpaired} frame(s) had no paired camera JPEG")
        if args.per_frame:
            for row in rows:
                print(
                    f"    {row['frame']:<26} "
                    f"{row['sky_ev_vs_camera']:>+8.2f} {row['ground_ev_vs_camera']:>+10.2f} "
                    f"{row['sky_green_deficit']:>8.1%} {row['sky_green_deficit_camera']:>8.1%} "
                    f"{row['purple_fraction']:>7.2%}"
                )

    print()
    print("sky EV / ground EV : ours minus the camera's. 0.00 is a match.")
    print("sky Gdef           : how far green sits below the brightest channel in")
    print("                     the sky band. The lavender failure ran 28-78%.")
    print("cam Gdef           : the same measure on the camera's JPEG, for scale.")
    print("purple             : fraction of the frame reading as the magenta failure.")

    if args.json:
        args.json.write_text(json.dumps(report, indent=2), encoding="utf-8")
        print(f"\nwrote {args.json}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
