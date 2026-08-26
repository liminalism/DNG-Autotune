#!/usr/bin/env python3
"""Score scene-proxy renders with a trained CamSDD model (phases 3c/3e).

Input is a directory of `*-scene-proxy.png` files produced by
`raw-autotune --semantic --dump-scene-proxy <dir>` — the exact pixels the
production classifier will see. Outputs, into --out:

  scores.jsonl   one line per image: full top-3, softmax entropy, source path
                 (the phase-3e observational record)
  review.html    contact sheet grouped by predicted class, confidence-sorted,
                 for the human hand-review pass the plan requires
  pseudo.csv     path,class rows at confidence >= --min-conf, consumable by
                 train_camsdd.py --stage adapt (phase 3c)

Scores are observational evidence only; nothing here moves pixels.
"""

from __future__ import annotations

import argparse
import html
import json
import os
from pathlib import Path


def letterbox(img, hw):
    """Aspect-preserving resize onto a black canvas of (H, W)."""
    from PIL import Image

    h, w = hw
    scale = min(w / img.width, h / img.height)
    nw, nh = max(1, round(img.width * scale)), max(1, round(img.height * scale))
    resized = img.resize((nw, nh), Image.BILINEAR)
    canvas = Image.new("RGB", (w, h), (0, 0, 0))
    canvas.paste(resized, ((w - nw) // 2, (h - nh) // 2))
    return canvas


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run", required=True, help="training run dir holding best.pt")
    parser.add_argument("--proxies", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--min-conf", type=float, default=0.90)
    parser.add_argument("--batch", type=int, default=32)
    args = parser.parse_args()

    import sys

    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import torch
    import train_camsdd as t
    from PIL import Image
    from torchvision import transforms as T

    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
    ckpt = torch.load(Path(args.run) / "best.pt", map_location=device, weights_only=True)
    classes = ckpt["classes"]
    input_hw = tuple(ckpt.get("input_hw", t.INPUT_HW))
    model = t.build_model(arch=ckpt.get("arch", "resnet50")).to(device)
    model.load_state_dict(ckpt["model"])
    model.eval()
    normalize = T.Compose([T.ToTensor(), T.Normalize(t.MEAN, t.STD)])

    paths = sorted(args.proxies.rglob("*-scene-proxy.png"))
    if not paths:
        raise SystemExit(f"no *-scene-proxy.png under {args.proxies}")
    args.out.mkdir(parents=True, exist_ok=True)

    rows = []
    with torch.no_grad():
        for start in range(0, len(paths), args.batch):
            chunk = paths[start : start + args.batch]
            batch = torch.stack(
                [normalize(letterbox(Image.open(p).convert("RGB"), input_hw)) for p in chunk]
            ).to(device)
            probs = torch.softmax(model(batch), dim=1).cpu()
            for path, p in zip(chunk, probs):
                top = p.topk(3)
                entropy = float(-(p * p.clamp(min=1e-9).log()).sum())
                rows.append(
                    {
                        "file": str(path),
                        "top3": [
                            [classes[i], round(float(v), 4)]
                            for v, i in zip(top.values, top.indices)
                        ],
                        "entropy": round(entropy, 4),
                    }
                )
    with (args.out / "scores.jsonl").open("w") as fh:
        for row in rows:
            fh.write(json.dumps(row) + "\n")

    keep = [(r["file"], r["top3"][0][0]) for r in rows if r["top3"][0][1] >= args.min_conf]
    with (args.out / "pseudo.csv").open("w") as fh:
        fh.write("path,label\n")
        for path, label in keep:
            fh.write(f"{path},{label}\n")

    by_class: dict[str, list] = {}
    for r in rows:
        by_class.setdefault(r["top3"][0][0], []).append(r)
    parts = [
        "<meta charset='utf-8'><title>CamSDD pseudo-label review</title>",
        "<style>body{font-family:sans-serif;background:#181818;color:#ddd}"
        "figure{display:inline-block;margin:4px;text-align:center}"
        "img{max-width:280px;display:block}figcaption{font-size:11px}"
        ".lo{outline:3px solid #c33}</style>",
        f"<h1>{len(rows)} proxies — {len(keep)} at conf ≥ {args.min_conf}</h1>",
        "<p>Red outline = below the pseudo-label confidence floor. "
        "Review: does the class name match the photo?</p>",
    ]
    for name in sorted(by_class, key=lambda n: -len(by_class[n])):
        group = sorted(by_class[name], key=lambda r: -r["top3"][0][1])
        parts.append(f"<h2>{html.escape(name)} ({len(group)})</h2>")
        for r in group:
            rel = os.path.relpath(r["file"], args.out)
            conf = r["top3"][0][1]
            cls = "" if conf >= args.min_conf else " class='lo'"
            runners = ", ".join(f"{c} {v:.2f}" for c, v in r["top3"][1:])
            parts.append(
                f"<figure{cls}><img src='{html.escape(rel)}' loading='lazy'>"
                f"<figcaption>{html.escape(Path(r['file']).name)}<br>"
                f"{conf:.3f} (then {html.escape(runners)})</figcaption></figure>"
            )
    (args.out / "review.html").write_text("\n".join(parts))

    print(f"{len(rows)} scored -> {args.out}/scores.jsonl")
    print(f"{len(keep)} pseudo-labels at conf >= {args.min_conf} -> {args.out}/pseudo.csv")
    print(f"review sheet: {args.out}/review.html")
    dist = sorted(by_class.items(), key=lambda kv: -len(kv[1]))
    for name, group in dist:
        print(f"  {name:24s} {len(group)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
