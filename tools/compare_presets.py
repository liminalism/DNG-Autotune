#!/usr/bin/env python3
"""Compare rendered presets against the camera's JPEG in CIELAB.

Why this exists
---------------
`tools/grade_sky.py` answers the questions the purple-sky work asked: is the
sky the right *brightness*, and does green sit below the other channels. It
says nothing about how saturated a render is or which way its hues moved, and
those are exactly the two axes that separate `--preset standard` from
`--preset vivid` -- vivid is standard's tone grade plus a calibrated DCP
HueSatMap, so the whole difference lives in hue and chroma.

It measures three things, all in CIELAB against the paired camera JPEG:

1. Chroma level. `C_ratio` is our mean chroma over the camera's. 1.00 means we
   are as saturated as the camera's own JPEG engine.
2. Hue placement. `hue_err` is the chroma-weighted mean absolute hue error in
   degrees, and the per-family columns say *where* the error is, using the
   camera's hue to assign families so a preset cannot move a pixel into a
   different bucket and flatter itself.
3. Neutralisation. `neutralised` is the fraction of the camera's genuinely
   saturated pixels (C* > 40) where our render loses more than a quarter of
   that chroma -- the failure the vivid table has to be checked against, since
   a hue/sat LUT can pull down a legitimately saturated blue or sunset as
   easily as it can lift a dull one.

With `--against` it also reports each preset's *direct* difference from a
reference preset, frame by frame, which is the number that says whether vivid
adds or removes chroma rather than whether it lands near the camera.

Usage
-----
    python tools/compare_presets.py OUT/standard OUT/vivid \
        --reference raw/jpeg raw/arw_better raw/raw_backlit2 \
        --against OUT/standard --json out.json

Requires only Pillow and numpy.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

try:
    import numpy as np
    from PIL import Image
except ImportError:  # pragma: no cover - a setup problem, not a logic one
    sys.exit("compare_presets.py needs Pillow and numpy: pip install pillow numpy")

# Analysis downscale. Every measure here is a population statistic over
# hundreds of thousands of pixels, so full resolution buys nothing.
LONG_EDGE = 1400

# sRGB primaries to XYZ, D65, and the D65 white point Lab is referenced to.
SRGB_TO_XYZ_D65 = np.array(
    [
        [0.4123908, 0.3575843, 0.1804808],
        [0.2126390, 0.7151687, 0.0721923],
        [0.0193308, 0.1191948, 0.9505322],
    ],
    dtype=np.float64,
)
WHITE_D65 = np.array([0.9504559, 1.0, 1.0890578], dtype=np.float64)

# A pixel the camera rendered above this chroma is "genuinely saturated": a
# blue sky, a sunset, a painted wall. Below it, a chroma ratio is noise.
SATURATED_C = 40.0
# How much of that chroma a render may lose before it counts as neutralised.
NEUTRALISE_LOSS = 0.25
# Chroma floor for hue statistics. Hue angle is meaningless near the axis.
HUE_MIN_C = 8.0

# Hue families, by the *camera's* hue angle in CIELAB degrees. The boundaries
# are measured, not guessed: sunset red sits at 40 and orange at 60, yellow at
# 93, grass at 135, cyan at 196, a mid sky blue at 279 and a fully saturated
# blue at 306. So the two things this comparison has to protect -- blue sky and
# warm sunset -- each land wholly inside one column. `magenta` wraps through 0
# because the lavender failure mode straddles it.
FAMILIES = {
    "warm": (20.0, 100.0),  # red, orange, yellow: sunset, skin, autumn foliage
    "green": (100.0, 180.0),
    "cyan": (180.0, 225.0),
    "blue": (225.0, 312.0),  # sky
    "magenta": (312.0, 20.0),  # wraps through 0
}


def in_family(hue: "np.ndarray", bounds: tuple[float, float]) -> "np.ndarray":
    """Membership mask for one hue family, honouring a range that wraps 0."""
    low, high = bounds
    if low < high:
        return (hue >= low) & (hue < high)
    return (hue >= low) | (hue < high)


def srgb_to_lab(image8: np.ndarray) -> np.ndarray:
    """8-bit sRGB -> CIELAB (D65), shape (..., 3)."""
    srgb = image8.astype(np.float64) / 255.0
    linear = np.where(srgb <= 0.04045, srgb / 12.92, ((srgb + 0.055) / 1.055) ** 2.4)
    xyz = linear @ SRGB_TO_XYZ_D65.T / WHITE_D65
    eps = (6.0 / 29.0) ** 3
    kappa = 1.0 / (3.0 * (6.0 / 29.0) ** 2)
    f = np.where(xyz > eps, np.cbrt(xyz), kappa * xyz + 4.0 / 29.0)
    return np.stack(
        [
            116.0 * f[..., 1] - 16.0,
            500.0 * (f[..., 0] - f[..., 1]),
            200.0 * (f[..., 1] - f[..., 2]),
        ],
        axis=-1,
    )


def load_lab(path: Path) -> np.ndarray:
    image = Image.open(path).convert("RGB")
    image.thumbnail((LONG_EDGE, LONG_EDGE), Image.LANCZOS)
    return srgb_to_lab(np.asarray(image))


def chroma_hue(lab: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    a, b = lab[..., 1], lab[..., 2]
    chroma = np.hypot(a, b)
    hue = np.degrees(np.arctan2(b, a)) % 360.0
    return chroma, hue


def align(one: np.ndarray, two: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    """Crop two analysis-scale images to a common shape.

    The renders and the camera JPEG agree on aspect ratio but can differ by a
    pixel after independent Lanczos downscales, and a whole-frame statistic
    must not depend on which one was larger.
    """
    height = min(one.shape[0], two.shape[0])
    width = min(one.shape[1], two.shape[1])
    return one[:height, :width], two[:height, :width]


def hue_delta(ours: np.ndarray, theirs: np.ndarray) -> np.ndarray:
    """Signed shortest-arc hue difference in degrees, ours minus theirs."""
    return (ours - theirs + 180.0) % 360.0 - 180.0


def measure(rendered: Path, reference: Path) -> dict:
    ours_lab, cam_lab = align(load_lab(rendered), load_lab(reference))
    ours_c, ours_h = chroma_hue(ours_lab)
    cam_c, cam_h = chroma_hue(cam_lab)

    row: dict[str, float | str] = {"frame": rendered.stem}
    row["chroma_mean"] = float(ours_c.mean())
    row["chroma_mean_camera"] = float(cam_c.mean())
    row["chroma_ratio"] = float(ours_c.mean() / max(cam_c.mean(), 1e-6))

    # Hue error, weighted by the camera's chroma so a grey wall cannot vote.
    weight = np.where(cam_c >= HUE_MIN_C, cam_c, 0.0)
    delta = hue_delta(ours_h, cam_h)
    total = weight.sum()
    row["hue_err"] = float((np.abs(delta) * weight).sum() / total) if total > 0 else float("nan")
    row["hue_bias"] = float((delta * weight).sum() / total) if total > 0 else float("nan")

    for name, bounds in FAMILIES.items():
        family = in_family(cam_h, bounds) & (cam_c >= HUE_MIN_C)
        row[f"share_{name}"] = float(family.mean())
        if not family.any():
            row[f"C_ratio_{name}"] = float("nan")
            row[f"hue_bias_{name}"] = float("nan")
            continue
        w = cam_c[family]
        row[f"C_ratio_{name}"] = float(ours_c[family].mean() / max(w.mean(), 1e-6))
        row[f"hue_bias_{name}"] = float((delta[family] * w).sum() / w.sum())

    # The camera's boldest pixels, treated separately. A whole-frame chroma
    # ratio pools them with the dull majority, and a preset can overshoot the
    # frame while still falling short exactly where the camera is boldest --
    # which is what `standard` does, so the two numbers must stay apart.
    saturated = cam_c > SATURATED_C
    row["saturated_share"] = float(saturated.mean())
    if saturated.any():
        row["C_ratio_saturated"] = float(
            ours_c[saturated].mean() / max(cam_c[saturated].mean(), 1e-6)
        )
        row["neutralised"] = float(
            (ours_c[saturated] < cam_c[saturated] * (1.0 - NEUTRALISE_LOSS)).mean()
        )
        row["overshot"] = float(
            (ours_c[saturated] > cam_c[saturated] * (1.0 + NEUTRALISE_LOSS)).mean()
        )
    else:
        row["C_ratio_saturated"] = float("nan")
        row["neutralised"] = float("nan")
        row["overshot"] = float("nan")
    return row


def measure_pair(rendered: Path, baseline: Path) -> dict:
    """Direct difference between two of our own renders of the same frame."""
    ours_lab, base_lab = align(load_lab(rendered), load_lab(baseline))
    ours_c, ours_h = chroma_hue(ours_lab)
    base_c, base_h = chroma_hue(base_lab)
    weight = np.where(base_c >= HUE_MIN_C, base_c, 0.0)
    delta = hue_delta(ours_h, base_h)
    total = weight.sum()
    row: dict[str, float | str] = {
        "frame": rendered.stem,
        "d_chroma_ratio": float(ours_c.mean() / max(base_c.mean(), 1e-6)),
        "d_hue": float((np.abs(delta) * weight).sum() / total) if total > 0 else float("nan"),
    }
    for name, bounds in FAMILIES.items():
        family = in_family(base_h, bounds) & (base_c >= HUE_MIN_C)
        if not family.any():
            row[f"d_C_{name}"] = float("nan")
            row[f"d_h_{name}"] = float("nan")
            continue
        w = base_c[family]
        row[f"d_C_{name}"] = float(ours_c[family].mean() / max(w.mean(), 1e-6))
        row[f"d_h_{name}"] = float((delta[family] * w).sum() / w.sum())
    strong = base_c > SATURATED_C
    row["desaturated_strong"] = (
        float((ours_c[strong] < base_c[strong] * (1.0 - NEUTRALISE_LOSS)).mean())
        if strong.any()
        else float("nan")
    )
    return row


PRESET_SUFFIXES = ("_auto", "_baseline", "_neutral", "_punchy", "_standard", "_vivid")


def stem_of(path: Path) -> str:
    stem = path.stem
    for suffix in PRESET_SUFFIXES:
        if stem.lower().endswith(suffix):
            stem = stem[: -len(suffix)]
            break
    return stem.lower()


def index_images(directories: list[Path]) -> dict[str, Path]:
    """Map frame stem -> image path, first directory listed winning."""
    index: dict[str, Path] = {}
    for directory in directories:
        for path in sorted(directory.rglob("*")):
            if path.is_file() and path.suffix.lower() in {".jpg", ".jpeg", ".png"}:
                index.setdefault(stem_of(path), path)
    return index


def rendered_frames(directory: Path, pattern: str | None = None) -> list[Path]:
    """Renders below `directory`, optionally kept to stems matching `pattern`.

    The filter exists because one output tree can hold two cameras: the third
    Sony batch renders beside the Samsung DNGs shot the same day, and the
    bundled HueSatMap is calibrated for exactly one of them, so a pooled number
    would hide the thing worth knowing.
    """
    frames = sorted(
        path
        for path in directory.rglob("*")
        if path.is_file() and path.suffix.lower() in {".jpg", ".jpeg", ".png"}
    )
    if pattern is None:
        return frames
    keep = re.compile(pattern, re.IGNORECASE)
    return [path for path in frames if keep.search(stem_of(path))]


def median_of(rows: list[dict]) -> dict:
    if not rows:
        return {}
    keys = [k for k in rows[0] if k != "frame"]
    return {k: float(np.nanmedian([r[k] for r in rows])) for k in keys}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("rendered", nargs="+", type=Path)
    parser.add_argument(
        "--reference",
        nargs="+",
        type=Path,
        default=[Path("raw/jpeg")],
        help="directories holding the camera's own JPEGs",
    )
    parser.add_argument(
        "--against",
        type=Path,
        default=None,
        help="also report each preset's direct difference from this render directory",
    )
    parser.add_argument("--json", type=Path, default=None)
    parser.add_argument("--per-frame", action="store_true")
    parser.add_argument(
        "--match",
        default=None,
        help="only grade frames whose stem matches this regex (e.g. '^_dsc' for the Sony half)",
    )
    args = parser.parse_args()

    missing = [d for d in args.reference if not d.is_dir()]
    if missing:
        print(f"reference directory not found: {missing[0]}", file=sys.stderr)
        return 1
    camera = index_images(args.reference)
    if not camera:
        print(f"no JPEGs under {args.reference}", file=sys.stderr)
        return 1
    baseline = index_images([args.against]) if args.against else {}

    report: dict[str, dict] = {}
    head = (
        f"{'preset':<12} {'n':>4} {'C ratio':>8} {'hue err':>8} {'hue bias':>9} "
        f"{'C blue':>7} {'C warm':>7} {'C green':>8} {'h blue':>7} {'h warm':>7} "
        f"{'C sat':>7} {'neutral':>8} {'overshot':>9}"
    )
    print(head)
    print("-" * len(head))
    for directory in args.rendered:
        if not directory.is_dir():
            print(f"{directory}: not a directory, skipped")
            continue
        rows, pairs, unpaired = [], [], 0
        for path in rendered_frames(directory, args.match):
            stem = stem_of(path)
            match = camera.get(stem)
            if match is None:
                unpaired += 1
                continue
            rows.append(measure(path, match))
            partner = baseline.get(stem)
            if partner is not None and partner.resolve() != path.resolve():
                pairs.append(measure_pair(path, partner))
        report[directory.name] = {
            "frames": rows,
            "median": median_of(rows),
            "vs_baseline_frames": pairs,
            "vs_baseline_median": median_of(pairs),
            "unpaired": unpaired,
        }
        if not rows:
            print(f"{directory.name:<12} {0:>4}  (no paired frames)")
            continue
        m = median_of(rows)
        print(
            f"{directory.name:<12} {len(rows):>4} {m['chroma_ratio']:>8.3f} "
            f"{m['hue_err']:>8.2f} {m['hue_bias']:>+9.2f} "
            f"{m['C_ratio_blue']:>7.3f} {m['C_ratio_warm']:>7.3f} {m['C_ratio_green']:>8.3f} "
            f"{m['hue_bias_blue']:>+7.2f} {m['hue_bias_warm']:>+7.2f} "
            f"{m['C_ratio_saturated']:>7.3f} {m['neutralised']:>7.2%} {m['overshot']:>8.2%}"
        )
        if unpaired:
            print(f"{'':<12} {unpaired} frame(s) had no paired camera JPEG")
        if args.per_frame:
            for row in sorted(rows, key=lambda r: r["frame"]):
                print(
                    f"    {row['frame']:<20} {row['chroma_ratio']:>8.3f} "
                    f"{row['hue_err']:>8.2f} {row['hue_bias']:>+9.2f} "
                    f"{row['C_ratio_blue']:>7.3f} {row['C_ratio_warm']:>7.3f} "
                    f"{row['C_ratio_green']:>8.3f} {row['hue_bias_blue']:>+7.2f} "
                    f"{row['hue_bias_warm']:>+7.2f} {row['C_ratio_saturated']:>7.3f} "
                    f"{row['neutralised']:>7.2%} {row['overshot']:>8.2%}"
                )

    if args.against:
        head2 = (
            f"{'vs ' + args.against.name:<12} {'n':>4} {'dC all':>8} {'d hue':>8} "
            f"{'dC blue':>8} {'dC warm':>8} {'dC green':>9} {'dh blue':>8} {'dh warm':>8} {'desat':>8}"
        )
        print()
        print(head2)
        print("-" * len(head2))
        for name, entry in report.items():
            pairs = entry["vs_baseline_frames"]
            if not pairs:
                continue
            m = median_of(pairs)
            print(
                f"{name:<12} {len(pairs):>4} {m['d_chroma_ratio']:>8.3f} {m['d_hue']:>8.2f} "
                f"{m['d_C_blue']:>8.3f} {m['d_C_warm']:>8.3f} {m['d_C_green']:>9.3f} "
                f"{m['d_h_blue']:>+8.2f} {m['d_h_warm']:>+8.2f} {m['desaturated_strong']:>7.2%}"
            )
            if args.per_frame:
                for row in sorted(pairs, key=lambda r: r["frame"]):
                    print(
                        f"    {row['frame']:<20} {row['d_chroma_ratio']:>8.3f} {row['d_hue']:>8.2f} "
                        f"{row['d_C_blue']:>8.3f} {row['d_C_warm']:>8.3f} {row['d_C_green']:>9.3f} "
                        f"{row['d_h_blue']:>+8.2f} {row['d_h_warm']:>+8.2f} "
                        f"{row['desaturated_strong']:>7.2%}"
                    )

    print()
    print("C ratio  : our mean CIELAB chroma over the camera JPEG's. 1.00 matches.")
    print("hue err  : chroma-weighted mean |hue error| vs the camera, degrees.")
    print("hue bias : the same, signed, so a systematic rotation is visible.")
    print("C blue/warm/green : chroma ratio inside one hue family of the camera's.")
    print("C sat    : chroma ratio restricted to the camera's own C*>40 pixels.")
    print("neutral  : share of the camera's C*>40 pixels where we lose >25% of it.")
    print("overshot : the same share where we exceed it by >25%.")
    if args.against:
        print("dC / dh  : the same measures taken directly against the reference preset.")
        print("desat    : share of the reference's C*>40 pixels this preset drops >25% below.")

    if args.json:
        args.json.write_text(json.dumps(report, indent=2), encoding="utf-8")
        print(f"\nwrote {args.json}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
