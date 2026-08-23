#!/usr/bin/env python3
"""Measure renderer-exact sky checkpoints without reducing them to 8 bits.

The stage dumps are lossless 16-bit RGB PNGs.  Pillow exposes those files as
8-bit ``RGB`` images, which is convenient for a contact sheet but not enough
for a boundary measurement.  This evaluator reads the raw 16-bit sample stream
through ImageMagick, then compares the production checkpoints in linear light
and OKLab.

Expected input layout::

    ROOT/default/stages/_DSC1289-tone-after-curve.png
    ROOT/default/stages/_DSC1289-tone-after-chroma.png
    ROOT/default/stages/_DSC1289-gamut-pre.png
    ROOT/default/stages/_DSC1289-gamut-post.png
    ROOT/default/stages/_DSC1289-final.png
    ROOT/default/stages/_DSC1289-highlight-{pre,clipmap,post}.png
    ROOT/{default,reconstruction-off,local-tone-035,hdr-050}/_DSC1289_auto.png

The ``final`` diagnostic is written after optional output sharpening and is
the byte-identical normal output. ``gamut-post`` remains the checkpoint after
gamut compression and is the stage used for attribution.
"""

from __future__ import annotations

import argparse
import json
import subprocess
from pathlib import Path

import numpy as np


SRGB_BREAK = 0.04045


def read_rgb16_png(path: Path, crop: tuple[int, int, int, int] | None = None) -> np.ndarray:
    """Read raw 16-bit RGB samples through ImageMagick without an 8-bit cast.

    The repository already uses ImageMagick for lossless inspection of the
    generated PNGs.  Its raw ``RGB`` writer is little-endian for 16-bit samples
    on this host; using that stream avoids Pillow's silent 16-bit-RGB-to-8-bit
    conversion and avoids maintaining a second PNG decoder in the evaluation
    tool.
    """

    command = ["convert", str(path)]
    if crop is not None:
        x0, y0, width, height = crop
        if x0 < 0 or y0 < 0 or width <= 0 or height <= 0:
            raise ValueError(f"invalid crop {crop}")
        command.extend(["-crop", f"{width}x{height}+{x0}+{y0}", "+repage"])
    else:
        identify = subprocess.run(
            ["identify", "-format", "%w %h", str(path)],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.split()
        width, height = (int(value) for value in identify)
    command.extend(["-depth", "16", "RGB:-"])
    try:
        raw = subprocess.run(command, check=True, capture_output=True).stdout
    except FileNotFoundError as error:
        raise RuntimeError("evaluate_sky_stages.py requires ImageMagick's convert command") from error
    expected = width * height * 3 * 2
    if len(raw) != expected:
        raise ValueError(f"{path}: convert emitted {len(raw)} bytes, expected {expected}")
    return np.frombuffer(raw, dtype="<u2").reshape(height, width, 3).copy()


def srgb_to_linear(encoded: np.ndarray) -> np.ndarray:
    value = encoded.astype(np.float64) / 65535.0
    return np.where(value <= SRGB_BREAK, value / 12.92, ((value + 0.055) / 1.055) ** 2.4)


def oklab(rgb: np.ndarray) -> np.ndarray:
    r, g, b = np.moveaxis(rgb, -1, 0)
    l = 0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b
    m = 0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b
    s = 0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b
    l_, m_, s_ = np.cbrt(l), np.cbrt(m), np.cbrt(s)
    return np.stack(
        (
            0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_,
            1.9779984951 * l_ - 2.4285922050 * m_ + 0.4505937099 * s_,
            0.0259040371 * l_ + 0.7827717662 * m_ - 0.8086757660 * s_,
        ),
        axis=-1,
    )


def summary(values: np.ndarray) -> dict[str, float | int]:
    values = np.asarray(values, dtype=np.float64)
    if values.size == 0:
        return {"n": 0, "p50": None, "p90": None, "p99": None}
    p50, p90, p99 = np.percentile(values, (50, 90, 99))
    return {"n": int(values.size), "p50": float(p50), "p90": float(p90), "p99": float(p99)}


def adjacent_steps(
    lab: np.ndarray,
    boundary_h: np.ndarray,
    boundary_v: np.ndarray,
    same_h: np.ndarray,
    same_v: np.ndarray,
) -> dict[str, dict[str, float | int]]:
    horizontal = lab[:, 1:] - lab[:, :-1]
    vertical = lab[1:] - lab[:-1]

    def collect(mask_h: np.ndarray, mask_v: np.ndarray) -> np.ndarray:
        return np.concatenate(
            (
                np.hypot(horizontal[..., 1], horizontal[..., 2])[mask_h],
                np.hypot(vertical[..., 1], vertical[..., 2])[mask_v],
            )
        )

    def collect_luma(mask_h: np.ndarray, mask_v: np.ndarray) -> np.ndarray:
        return np.concatenate((np.abs(horizontal[..., 0])[mask_h], np.abs(vertical[..., 0])[mask_v]))

    return {
        "boundary_chroma": summary(collect(boundary_h, boundary_v)),
        "same_state_chroma": summary(collect(same_h, same_v)),
        "boundary_lightness": summary(collect_luma(boundary_h, boundary_v)),
        "same_state_lightness": summary(collect_luma(same_h, same_v)),
    }


def make_masks(pre_lab: np.ndarray, classes: np.ndarray) -> tuple[np.ndarray, ...]:
    bright = pre_lab[..., 0] > 0.72
    dlh = np.abs(pre_lab[:, 1:, 0] - pre_lab[:, :-1, 0])
    dlv = np.abs(pre_lab[1:, :, 0] - pre_lab[:-1, :, 0])
    bright_h = bright[:, 1:] & bright[:, :-1]
    bright_v = bright[1:] & bright[:-1]
    changed_h = classes[:, 1:] != classes[:, :-1]
    changed_v = classes[1:, :] != classes[:-1, :]
    boundary_h = bright_h & (dlh < 0.012) & changed_h
    boundary_v = bright_v & (dlv < 0.012) & changed_v
    same_h = bright_h & (dlh < 0.012) & ~changed_h
    same_v = bright_v & (dlv < 0.012) & ~changed_v
    return boundary_h, boundary_v, same_h, same_v


def make_transition_masks(
    pre_lab: np.ndarray,
    classes: np.ndarray,
    low: int,
    high: int,
) -> tuple[np.ndarray, ...]:
    """Select one adjacent clip-state transition and its local control edges."""

    bright = pre_lab[..., 0] > 0.72
    dlh = np.abs(pre_lab[:, 1:, 0] - pre_lab[:, :-1, 0])
    dlv = np.abs(pre_lab[1:, :, 0] - pre_lab[:-1, :, 0])
    bright_h = bright[:, 1:] & bright[:, :-1]
    bright_v = bright[1:] & bright[:-1]
    smooth_h = bright_h & (dlh < 0.012)
    smooth_v = bright_v & (dlv < 0.012)

    left, right = classes[:, :-1], classes[:, 1:]
    top, bottom = classes[:-1, :], classes[1:, :]
    boundary_h = smooth_h & (
        ((left == low) & (right == high)) | ((left == high) & (right == low))
    )
    boundary_v = smooth_v & (
        ((top == low) & (bottom == high)) | ((top == high) & (bottom == low))
    )
    same_h = smooth_h & (left == right) & ((left == low) | (left == high))
    same_v = smooth_v & (top == bottom) & ((top == low) | (top == high))
    return boundary_h, boundary_v, same_h, same_v


def p90_ratio(step: dict[str, dict[str, float | int]]) -> float | None:
    boundary = step["boundary_chroma"]["p90"]
    same = step["same_state_chroma"]["p90"]
    if boundary is None or same is None or same <= 0:
        return None
    return float(boundary / same)


def stage_file(root: Path, name: str) -> Path:
    return root / "default" / "stages" / f"_DSC1289-{name}.png"


def final_file(root: Path, name: str) -> Path:
    return root / name / "_DSC1289_auto.png"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path, help="root of the four current-frame render arms")
    parser.add_argument("--json", type=Path, default=None, help="write the report as JSON")
    parser.add_argument(
        "--stages-only",
        action="store_true",
        help="measure the default stage dump without requiring the three ablation renders",
    )
    parser.add_argument(
        "--roi",
        default="1000,0,4600,1400",
        help="x,y,width,height bright upper-sky crop (default: %(default)s)",
    )
    args = parser.parse_args()
    crop = tuple(int(value) for value in args.roi.split(","))
    if len(crop) != 4:
        raise SystemExit("--roi must be x,y,width,height")

    clipmap_full = read_rgb16_png(stage_file(args.root, "highlight-clipmap"))
    clip_counts = np.sum(clipmap_full > 32767, axis=2).astype(np.uint8)
    counts = {str(state): int(np.count_nonzero(clip_counts == state)) for state in range(4)}
    del clipmap_full

    clipmap = read_rgb16_png(stage_file(args.root, "highlight-clipmap"), crop)
    classes = np.sum(clipmap > 32767, axis=2).astype(np.uint8)
    del clipmap

    pre = srgb_to_linear(read_rgb16_png(stage_file(args.root, "highlight-pre"), crop))
    pre_lab = oklab(pre)
    masks = make_masks(pre_lab, classes)

    paths = {
        "highlight_pre": stage_file(args.root, "highlight-pre"),
        "highlight_post": stage_file(args.root, "highlight-post"),
        "tone_after_curve": stage_file(args.root, "tone-after-curve"),
        "tone_after_chroma": stage_file(args.root, "tone-after-chroma"),
        # gamut-pre is the checkpoint after the highlight white mix and before
        # compress_gamut; keep the established filename for compatibility.
        "after_highlight_white_mix": stage_file(args.root, "gamut-pre"),
        "after_gamut_compression": stage_file(args.root, "gamut-post"),
        "final_diagnostic": stage_file(args.root, "final"),
        "final_default": final_file(args.root, "default"),
    }
    if not args.stages_only:
        paths.update(
            {
                "final_reconstruction_off": final_file(args.root, "reconstruction-off"),
                "final_local_tone_035": final_file(args.root, "local-tone-035"),
                "final_hdr_050": final_file(args.root, "hdr-050"),
            }
        )

    measurements: dict[str, dict] = {}
    clip_transitions: dict[str, dict[str, dict]] = {}
    stage_arrays: dict[str, np.ndarray] = {"highlight_pre": pre}
    for name, path in paths.items():
        if name == "highlight_pre":
            continue
        stage_arrays[name] = srgb_to_linear(read_rgb16_png(path, crop))

    for name, rgb in stage_arrays.items():
        lab = oklab(rgb)
        measurements[name] = adjacent_steps(lab, *masks)
        clip_transitions[name] = {}
        for low, high in ((0, 1), (1, 2), (2, 3)):
            label = f"{low}<->{high}"
            transition_masks = make_transition_masks(pre_lab, classes, low, high)
            step = adjacent_steps(lab, *transition_masks)
            step["boundary_to_same_p90_ratio"] = p90_ratio(step)
            clip_transitions[name][label] = step

    transitions = {}
    ordered = [
        ("highlight_pre", "highlight_post"),
        ("highlight_post", "tone_after_curve"),
        ("tone_after_curve", "tone_after_chroma"),
        ("tone_after_chroma", "after_highlight_white_mix"),
        ("after_highlight_white_mix", "after_gamut_compression"),
    ]
    for before, after in ordered:
        delta = np.linalg.norm(oklab(stage_arrays[after]) - oklab(stage_arrays[before]), axis=2)
        transitions[f"{before}_to_{after}"] = summary(delta)

    # Compare the encoded 16-bit pixels, not compressed PNG bytes.  The latter
    # include independent PNG metadata/chunking even when the pixels match.
    diagnostic = read_rgb16_png(paths["final_diagnostic"])
    normal = read_rgb16_png(paths["final_default"])
    identity = {
        "pixel_bytes_identical": bool(np.array_equal(diagnostic, normal)),
        "max_absolute_code_delta": int(np.max(np.abs(diagnostic.astype(np.int32) - normal.astype(np.int32)))),
    }

    report = {
        "root": str(args.root),
        "roi": list(crop),
        "clip_state_counts_full_frame": counts,
        "identity": identity,
        "steps": measurements,
        "clip_transitions": clip_transitions,
        "transitions": transitions,
    }

    print(f"final diagnostic == normal render (16-bit pixels): {identity['pixel_bytes_identical']}")
    print(f"clip-state counts (0/1/2/3): {counts['0']}/{counts['1']}/{counts['2']}/{counts['3']}")
    print("stage                                      boundary p90     same-state p90     boundary n  same n")
    for name, result in measurements.items():
        boundary = result["boundary_chroma"]
        same = result["same_state_chroma"]
        print(
            f"{name:<42} {boundary['p90'] or 0:>14.5f} {same['p90'] or 0:>18.5f}"
            f" {boundary['n']:>12} {same['n']:>8}"
        )
    print(
        "clip-state transition              stage                         ratio"
        "     boundary n  same n"
    )
    for stage in ("highlight_post", "tone_after_curve"):
        for label, result in clip_transitions[stage].items():
            ratio = result["boundary_to_same_p90_ratio"]
            ratio_text = "n/a" if ratio is None else f"{ratio:.5f}"
            print(
                f"{label:<34} {stage:<28} {ratio_text:>8}"
                f" {result['boundary_chroma']['n']:>12} {result['same_state_chroma']['n']:>7}"
            )

    if args.json is not None:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        args.json.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        print(f"wrote {args.json}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
