#!/usr/bin/env python3
"""Baseline vs toe-experiment: what moved, and did we get closer to the camera."""
import json, os
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
base = {(r["set"], r["stem"]): r
        for r in json.load(open(os.path.join(HERE, "renders-percentiles.json")))}
toe = {(r["set"], r["stem"]): r
       for r in json.load(open(os.path.join(HERE, "renders-toe-percentiles.json")))}
keys = sorted(set(base) & set(toe))
KEYS = ["p05_ev", "p25_ev", "p50_ev", "p75_ev", "p95_ev"]

print(f"{len(keys)} joined pairs")
print("\nShift of OUR render (toe - baseline), display EV:")
print("  " + "".join(f"{k[:3]:>10}" for k in KEYS))
for stat, fn in (("median", np.median), ("mean", np.mean),
                 ("min", np.min), ("max", np.max)):
    vals = [fn([toe[k]["ours"][kk] - base[k]["ours"][kk] for k in keys])
            for kk in KEYS]
    print(f"  {stat:<7}" + "".join(f"{v:+10.2f}" for v in vals))

print("\n|ours - camera| median, baseline -> toe:")
for kk in KEYS:
    b = np.median([abs(base[k]["ours"][kk] - base[k]["cam"][kk]) for k in keys])
    t = np.median([abs(toe[k]["ours"][kk] - toe[k]["cam"][kk]) for k in keys])
    print(f"  {kk[:3]}: {b:.2f} -> {t:.2f}")

print("\nSigned (ours - camera) median, baseline -> toe:")
for kk in KEYS:
    b = np.median([base[k]["ours"][kk] - base[k]["cam"][kk] for k in keys])
    t = np.median([toe[k]["ours"][kk] - toe[k]["cam"][kk] for k in keys])
    print(f"  {kk[:3]}: {b:+.2f} -> {t:+.2f}")

# Frames where the toe binary moved p05 the most, and any frame that got
# further from the camera at any percentile by more than 0.15 EV.
print("\nLargest p05 moves (toe - baseline):")
for k in sorted(keys, key=lambda k: toe[k]["ours"]["p05_ev"] - base[k]["ours"]["p05_ev"])[:8]:
    d = toe[k]["ours"]["p05_ev"] - base[k]["ours"]["p05_ev"]
    print(f"  {k[0]}/{k[1]}: {d:+.2f}")

print("\nFrames further from camera by > 0.15 EV at any percentile:")
worse = 0
for k in keys:
    for kk in KEYS:
        b = abs(base[k]["ours"][kk] - base[k]["cam"][kk])
        t = abs(toe[k]["ours"][kk] - toe[k]["cam"][kk])
        if t - b > 0.15:
            print(f"  {k[0]}/{k[1]} {kk[:3]}: |d| {b:.2f} -> {t:.2f}"
                  f"  (class={base[k]['meta'].get('tonal_class')},"
                  f" lls={base[k]['meta'].get('low_light_score', -1):.2f})")
            worse += 1
print(f"  total: {worse}")
