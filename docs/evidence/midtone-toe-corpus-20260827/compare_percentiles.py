#!/usr/bin/env python3
"""Ours-vs-camera display-luminance percentiles across the paired corpus.

Reads each render set's automatic batch summary.json, pairs every rendered
frame with its camera JPEG through the summary's `input` stem, computes
Rec.709 luminance percentiles of both 8-bit images (code values + display EV
relative to middle grey), and joins analysis/parameters/reference fields.
Writes percentiles.json and prints a table.
"""
import json, os, glob
import numpy as np
from PIL import Image

ROOT = "/mnt/Samsung980_1TB/Rust-projects/raw-autotune"
import sys
RENDERS = os.path.join(ROOT, ".agent/scratch/midtone-eval", sys.argv[1] if len(sys.argv) > 1 else "renders")
PCTS = [5, 25, 50, 75, 95]
MIDDLE_GREY = 0.1845865  # linear luminance of sRGB 118/255

CAM_DIR = {
    "raw_3rd_batch": "raw/jpeg",
    "arw_better": "raw/arw_better",
    "indoor_tungsten": "raw/indoor_tungsten",
    "raw_backlit": "raw/raw_backlit",
    "raw_extremely_bright": "raw/raw_extremely_bright",
    "raw_at_night": "raw/raw_at_night",
    "raw_better": "raw/raw_better",
}


def srgb_to_linear(x):
    x = x / 255.0
    return np.where(x <= 0.04045, x / 12.92, ((x + 0.055) / 1.055) ** 2.4)


def luma_stats(path):
    im = Image.open(path)
    im.thumbnail((1200, 1200))
    a = np.asarray(im.convert("RGB"), dtype=np.float32)
    lin = srgb_to_linear(a)
    y = 0.2126 * lin[..., 0] + 0.7152 * lin[..., 1] + 0.0722 * lin[..., 2]
    code = 0.2126 * a[..., 0] + 0.7152 * a[..., 1] + 0.0722 * a[..., 2]
    out = {}
    for p in PCTS:
        out[f"p{p:02d}"] = float(np.percentile(code, p))
        yv = max(float(np.percentile(y, p)), 1e-6)
        out[f"p{p:02d}_ev"] = float(np.log2(yv / MIDDLE_GREY))
    return out


def find_camera_jpeg(cam_dir, stem):
    for ext in (".JPG", ".jpg", ".JPEG", ".jpeg"):
        p = os.path.join(ROOT, cam_dir, stem + ext)
        if os.path.exists(p):
            return p
    return None


def find_summary(set_dir):
    hits = glob.glob(os.path.join(set_dir, "summary.json")) + glob.glob(
        os.path.join(set_dir, "*", "summary.json"))
    return hits[0] if hits else None


rows = []
for set_name, cam_dir in CAM_DIR.items():
    sdir = os.path.join(RENDERS, set_name)
    summary = find_summary(sdir)
    if summary is None:
        print(f"# {set_name}: no summary.json yet, skipped")
        continue
    with open(summary) as f:
        entries = json.load(f)
    if isinstance(entries, dict):
        entries = entries.get("files") or entries.get("entries") or []
    base = os.path.dirname(summary)
    for e in entries:
        if e.get("status") not in (None, "completed", "ok"):
            continue
        out_rel = e.get("output")
        if not out_rel:
            continue
        rj = out_rel if os.path.isabs(out_rel) else os.path.join(
            ROOT, out_rel)
        if not os.path.exists(rj):
            rj = os.path.join(base, os.path.basename(out_rel))
        if not os.path.exists(rj):
            continue
        stem = os.path.splitext(os.path.basename(e["input"]))[0]
        cam = find_camera_jpeg(cam_dir, stem)
        if cam is None:
            continue
        meta = {"guidance_mode": e.get("guidance_mode")}
        an = e.get("analysis") or {}
        for k in ("tonal_class", "measured_dynamic_range_ev", "key_score",
                  "center_median_ev", "target_median_ev", "low_light_score",
                  "p50_ev", "near_white_fraction"):
            if k in an:
                meta[k] = an[k]
        pr = e.get("parameters") or {}
        for k in ("black_input_ev", "white_input_ev", "black_output_ev",
                  "white_output_ev", "exposure_ev", "contrast",
                  "shadow_power", "highlight_power"):
            if k in pr:
                meta[k] = pr[k]
        ref = e.get("reference") or {}
        for k in ("center_weighted_key_display_ev", "p05_display_ev",
                  "p50_display_ev", "p95_display_ev"):
            if k in ref:
                meta["ref_" + k] = ref[k]
        if isinstance(ref.get("delta"), dict):
            meta["delta_key_ev"] = ref["delta"].get(
                "center_weighted_key_display_ev")
        rows.append({
            "set": set_name, "stem": stem,
            "ours": luma_stats(rj), "cam": luma_stats(cam), "meta": meta,
        })

out = os.path.join(RENDERS, "..", os.path.basename(RENDERS.rstrip("/")) + "-percentiles.json")
with open(out, "w") as f:
    json.dump(rows, f, indent=1)

print(f"{len(rows)} pairs -> {os.path.normpath(out)}")
hdr = f"{'set':<20}{'stem':<13}" + "".join(
    f"{k + ' o/c':>12}" for k in ("p05", "p25", "p50", "p75", "p95"))
print(hdr + "  class      dEV(p50)")
for r in rows:
    o, c = r["ours"], r["cam"]
    cells = "".join(
        f"{o[k]:5.0f}/{c[k]:<5.0f} " for k in ("p05", "p25", "p50", "p75", "p95"))
    dev = o["p50_ev"] - c["p50_ev"]
    print(f"{r['set']:<20}{r['stem']:<13}{cells} {str(r['meta'].get('tonal_class','?')):<10} {dev:+.2f}")
