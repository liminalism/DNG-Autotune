#!/usr/bin/env python3
"""Census clipped_cfa_sites vs DEFAULT_SPATIAL_CLIPPED_FLOOR (1e-5)."""
from __future__ import annotations

import json
import math
from collections import Counter
from pathlib import Path

FLOOR = 1.0e-5
SUMMARY = Path("cfa-corpus.json")
DIMS = Path("raw-dimensions.json")


def percentile(sorted_vals, p):
    if not sorted_vals:
        return None
    if len(sorted_vals) == 1:
        return sorted_vals[0]
    k = (len(sorted_vals) - 1) * p / 100.0
    lo = int(math.floor(k))
    hi = int(math.ceil(k))
    if lo == hi:
        return sorted_vals[lo]
    return sorted_vals[lo] * (hi - k) + sorted_vals[hi] * (k - lo)


def main():
    summary = json.loads(SUMMARY.read_text())
    dims = {r["path"]: r for r in json.loads(DIMS.read_text())}
    files = summary.get("files", [])
    rows = []
    missing = []
    for entry in files:
        path = entry["input"]
        status = entry.get("status")
        color = entry.get("color") or {}
        hl = color.get("highlight_reconstruction") or {}
        d = dims.get(path) or {}
        pixels = d.get("pixels")
        sites = hl.get("clipped_cfa_sites")
        method = hl.get("method")
        c1 = hl.get("clipped_1_pixels")
        c2 = hl.get("clipped_2_pixels")
        c3 = hl.get("clipped_3_pixels")
        if pixels is None or sites is None:
            missing.append(path)
            continue
        frac = sites / pixels if pixels else 0.0
        two_plus = ((c2 or 0) + (c3 or 0)) / pixels if pixels else 0.0
        rows.append(
            {
                "input": path,
                "status": status,
                "pixels": pixels,
                "clipped_cfa_sites": sites,
                "frac": frac,
                "method": method,
                "clipped_1": c1,
                "clipped_2": c2,
                "clipped_3": c3,
                "two_plus_frac": two_plus,
                "photo": d.get("photo_name"),
                "elapsed_ms": entry.get("elapsed_ms"),
            }
        )

    fracs = sorted(r["frac"] for r in rows)
    nonzero = sorted(r["frac"] for r in rows if r["frac"] > 0)
    print(f"files {len(files)} usable {len(rows)} missing {len(missing)}")
    print(f"status {Counter(r['status'] for r in rows)}")
    print(f"method {Counter(r['method'] for r in rows)}")
    print(f"photo {Counter(r['photo'] for r in rows)}")
    print(
        "frac all: min {0:.3e} p50 {1:.3e} p90 {2:.3e} max {3:.3e}".format(
            fracs[0], percentile(fracs, 50), percentile(fracs, 90), fracs[-1]
        )
    )
    if nonzero:
        print(
            "frac >0: n {0} min {1:.3e} p50 {2:.3e} p90 {3:.3e} max {4:.3e}".format(
                len(nonzero),
                nonzero[0],
                percentile(nonzero, 50),
                percentile(nonzero, 90),
                nonzero[-1],
            )
        )

    zero = sum(1 for r in rows if r["frac"] == 0)
    below = [r for r in rows if 0 < r["frac"] < FLOOR]
    above = [r for r in rows if r["frac"] >= FLOOR]
    print(f"zero {zero}  (0, floor) {len(below)}  >=floor {len(above)}")

    # empty bands on the positive fraction line
    pos = sorted({r["frac"] for r in rows if r["frac"] > 0})
    if pos:
        print(f"smallest positive {pos[0]:.6e}  floor {FLOOR:.6e}  gap {pos[0] - 0:.6e}")
        in_band = [f for f in pos if f < FLOOR]
        print(f"positive fractions strictly below floor: {len(in_band)}")
        if in_band:
            print("  below-floor frames:")
            for r in sorted(below, key=lambda x: x["frac"]):
                print(
                    f"    {r['frac']:.6e}  sites={r['clipped_cfa_sites']:7d}  "
                    f"px={r['pixels']}  {r['input']}"
                )
        # largest gap below 1e-3
        seq = [0.0] + pos
        gaps = []
        for a, b in zip(seq, seq[1:]):
            gaps.append((b - a, a, b))
        gaps.sort(reverse=True)
        print("largest empty bands (top 5):")
        for g, a, b in gaps[:5]:
            print(f"  {a:.6e} .. {b:.6e}  width {g:.6e}")

    # statistic mismatch: CFA sites vs two-or-more-channel demosaiced pixels
    comparable = [r for r in rows if r["clipped_2"] is not None]
    if comparable:
        ratios = []
        for r in comparable:
            if r["frac"] == 0 and r["two_plus_frac"] == 0:
                continue
            ratios.append((r["frac"], r["two_plus_frac"], r["input"]))
        print(f"frames with any clip on either statistic: {len(ratios)}")
        cfa_gt = sum(1 for a, b, _ in ratios if a > b)
        print(f"CFA-site frac > two-plus pixel frac: {cfa_gt}/{len(ratios)}")

    out = Path("census.json")
    out.write_text(json.dumps(rows, indent=1))
    print("wrote", out)


if __name__ == "__main__":
    main()
