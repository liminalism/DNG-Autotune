#!/usr/bin/env python3
"""Aggregate percentiles.json: per-set and per-class EV deltas by percentile."""
import json, os
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
import sys
rows = json.load(open(os.path.join(HERE, sys.argv[1] if len(sys.argv) > 1 else "percentiles.json")))
KEYS = ["p05_ev", "p25_ev", "p50_ev", "p75_ev", "p95_ev"]


def summarize(label, sel):
    if not sel:
        return
    print(f"\n{label}  (n={len(sel)})")
    print("  " + "".join(f"{k[:3]:>10}" for k in KEYS) + "   (ours - camera, display EV)")
    for stat, fn in (("median", np.median), ("mean", np.mean),
                     ("p10", lambda a: np.percentile(a, 10)),
                     ("p90", lambda a: np.percentile(a, 90))):
        vals = [fn([r["ours"][k] - r["cam"][k] for r in sel]) for k in KEYS]
        print(f"  {stat:<7}" + "".join(f"{v:+10.2f}" for v in vals))


summarize("ALL PAIRS", rows)
for s in sorted({r["set"] for r in rows}):
    summarize(f"set {s}", [r for r in rows if r["set"] == s])
for c in sorted({str(r["meta"].get("tonal_class")) for r in rows}):
    summarize(f"class {c}",
              [r for r in rows if str(r["meta"].get("tonal_class")) == c])

# Scene dependence: correlation of per-frame p50 delta with analyzer stats.
print("\nCorrelation of dEV with analyzer fields (Pearson r, n where field present):")
for field in ("measured_dynamic_range_ev", "key_score", "low_light_score",
              "center_median_ev", "near_white_fraction"):
    for k in ("p05_ev", "p50_ev", "p95_ev"):
        pts = [(r["meta"][field], r["ours"][k] - r["cam"][k])
               for r in rows if field in r["meta"]]
        if len(pts) > 5:
            x, y = zip(*pts)
            r_ = np.corrcoef(x, y)[0, 1]
            print(f"  {field:<28} vs d({k[:3]}): r={r_:+.2f} (n={len(pts)})")

# The frames with the largest |p50| disagreement, for eyeballing.
print("\nLargest |d p50_ev| frames:")
for r in sorted(rows, key=lambda r: -abs(r["ours"]["p50_ev"] - r["cam"]["p50_ev"]))[:10]:
    d = {k[:3]: r["ours"][k] - r["cam"][k] for k in KEYS}
    print(f"  {r['set']}/{r['stem']:<14}" +
          "".join(f" {k}{v:+.2f}" for k, v in d.items()) +
          f"  class={r['meta'].get('tonal_class')}"
          f" lls={r['meta'].get('low_light_score', float('nan')):.2f}")
