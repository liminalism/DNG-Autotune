#!/usr/bin/env python3
"""Build an ours-vs-camera contact sheet from a raw-autotune batch summary.

The numbers come from the summary the program already writes, so this script never
re-renders anything and never re-measures anything: it resizes images and lays
them out. If a number here disagrees with the program, the program is right.

Why it exists: `docs/PLAN.md` records that "the camera JPEG is a reference, not
ground truth" — the scene classes where this controller is weakest are the same
ones where the vendor is weakest, so the metrics are least trustworthy exactly
where they matter most. Read the number, then open the pair. This is how you open
the pair.

Usage:
    raw-autotune raw/pairs --output out --format jpeg --reference \\
        --summary out/summary.json
    tools/contact-sheet.py out/summary.json --output out/sheet.html

Sorted worst-first by default, because triage is looking for the failures.
Requires Pillow, which is the only dependency outside the standard library.
"""

import argparse
import html
import json
import os
import sys

try:
    from PIL import Image
except ImportError:  # pragma: no cover - environment problem, not a code path
    sys.exit("this script needs Pillow: pip install pillow")

Image.MAX_IMAGE_PIXELS = None


def deviation(entry):
    """How far this file sits from its camera JPEG, worst measure wins.

    Exposure is in EV and saturation is a ratio, so they are put on a common
    footing before being compared: a third of a stop is treated as about as
    wrong as being 10% off on saturation. The point is only to sort, so the
    exchange rate just has to be defensible, not exact.
    """
    reference = entry.get("reference")
    if not reference:
        return -1.0
    delta = reference.get("delta", {})
    score = abs(delta.get("center_weighted_key_display_ev", 0.0)) / 0.33
    ratio = delta.get("saturation_ratio")
    if ratio:
        score = max(score, abs(ratio - 1.0) / 0.10)
    # Hue joined the sort in 0.1.18. Five degrees of median hue error is treated
    # as about as wrong as a third of a stop, on the same reasoning as above: the
    # exchange rate only has to be defensible enough to rank, not exact.
    hue = (delta.get("hue") or {}).get("median_degrees")
    if hue:
        score = max(score, hue / 5.0)
    return score


def thumbnail(source, destination, width):
    if not source or not os.path.exists(source):
        return None
    if not os.path.exists(destination):
        image = Image.open(source)
        image.thumbnail((width, width * 4), Image.LANCZOS)
        image.convert("RGB").save(destination, "JPEG", quality=88)
    return destination


def cell(entry, reference, sheet_dir, thumbs, width):
    stem = os.path.splitext(os.path.basename(entry["input"]))[0]
    ours = thumbnail(entry.get("output"), os.path.join(thumbs, stem + "-ours.jpg"), width)
    theirs = thumbnail(
        reference.get("path") if reference else None,
        os.path.join(thumbs, stem + "-camera.jpg"),
        width,
    )

    def img(path, label):
        if not path:
            return f'<div class="missing">{label}: not available</div>'
        relative = os.path.relpath(path, sheet_dir)
        return (
            f'<figure><img src="{html.escape(relative)}" alt="{label}">'
            f"<figcaption>{label}</figcaption></figure>"
        )

    measured = entry.get("measured") or {}
    delta = (reference or {}).get("delta", {})
    theirs_measured = (reference or {}).get("measured", {})

    def row(name, ours_value, theirs_value, difference, fmt="{:.3f}"):
        blank = "&mdash;"
        form = lambda value: blank if value is None else fmt.format(value)
        return (
            f"<tr><th>{name}</th><td>{form(ours_value)}</td>"
            f"<td>{form(theirs_value)}</td><td>{form(difference)}</td></tr>"
        )

    analysis = entry.get("analysis") or {}
    parameters = entry.get("parameters") or {}
    shot = entry.get("shot") or {}
    facts = " &middot; ".join(
        part
        for part in (
            html.escape(entry.get("camera", "")),
            f"ISO {shot['iso']}" if shot.get("iso") else "",
            html.escape(str(analysis.get("tonal_class", ""))),
            f"exposure {parameters.get('exposure_ev', 0.0):+.2f} EV",
        )
        if part
    )

    table = "".join(
        [
            row(
                "key EV",
                (reference or {}).get("center_weighted_key_display_ev", 0.0)
                + delta.get("center_weighted_key_display_ev", 0.0)
                if reference
                else None,
                (reference or {}).get("center_weighted_key_display_ev"),
                delta.get("center_weighted_key_display_ev"),
                "{:+.2f}",
            ),
            row(
                "saturation",
                measured.get("mean_saturation"),
                theirs_measured.get("mean_saturation"),
                delta.get("mean_saturation"),
            ),
            row(
                "highlight sat.",
                measured.get("mean_saturation_highlight"),
                theirs_measured.get("mean_saturation_highlight"),
                delta.get("highlight_saturation_ratio"),
            ),
            # Hue is not a difference of two per-frame numbers, so it does not fit
            # the ours/theirs/delta shape: it is a per-pixel comparison on a
            # canonical grid, summarised. Shown in the delta column alone.
            row(
                "hue delta (deg)",
                None,
                None,
                (delta.get("hue") or {}).get("median_degrees"),
                "{:.2f}",
            ),
            row(
                "hue p90 (deg)",
                None,
                None,
                (delta.get("hue") or {}).get("p90_degrees"),
                "{:.2f}",
            ),
            row(
                "colourfulness",
                measured.get("colourfulness"),
                theirs_measured.get("colourfulness"),
                delta.get("colourfulness"),
                "{:.1f}",
            ),
            row(
                "mean level",
                measured.get("mean_level"),
                theirs_measured.get("mean_level"),
                delta.get("mean_level"),
                "{:.1f}",
            ),
            row(
                "near-white %",
                (measured.get("near_white_fraction") or 0) * 100 if measured else None,
                (theirs_measured.get("near_white_fraction") or 0) * 100
                if theirs_measured
                else None,
                (delta.get("near_white_fraction") or 0) * 100
                if delta.get("near_white_fraction") is not None
                else None,
                "{:.2f}",
            ),
            row(
                "crushed %",
                (measured.get("crushed_fraction") or 0) * 100 if measured else None,
                (theirs_measured.get("crushed_fraction") or 0) * 100
                if theirs_measured
                else None,
                (delta.get("crushed_fraction") or 0) * 100
                if delta.get("crushed_fraction") is not None
                else None,
                "{:.2f}",
            ),
        ]
    )

    return f"""<section>
  <h2>{html.escape(stem)}</h2>
  <p class="facts">{facts}</p>
  <div class="pair">{img(ours, "raw-autotune")}{img(theirs, "camera JPEG")}</div>
  <table><thead><tr><th></th><th>ours</th><th>camera</th><th>delta</th></tr></thead>
  <tbody>{table}</tbody></table>
</section>"""


STYLE = """
:root { color-scheme: light dark; }
body { font: 14px/1.5 system-ui, sans-serif; margin: 0 auto; max-width: 1400px; padding: 24px; }
h1 { font-size: 20px; }
section { border-top: 1px solid rgba(128,128,128,.35); padding: 20px 0; }
h2 { font-size: 16px; margin: 0; font-family: ui-monospace, monospace; }
.facts { margin: 4px 0 12px; opacity: .7; }
.pair { display: flex; gap: 12px; flex-wrap: wrap; }
figure { margin: 0; flex: 1 1 380px; }
figure img { width: 100%; height: auto; display: block; border-radius: 3px; }
figcaption { font-size: 12px; opacity: .6; padding-top: 4px; }
.missing { flex: 1 1 380px; display: grid; place-items: center; opacity: .5;
           border: 1px dashed rgba(128,128,128,.5); border-radius: 3px; min-height: 120px; }
table { border-collapse: collapse; margin-top: 12px; font-variant-numeric: tabular-nums; }
th, td { text-align: right; padding: 2px 14px 2px 0; }
thead th { font-weight: 600; opacity: .6; }
tbody th { text-align: left; font-weight: 400; opacity: .7; }
.summary { opacity: .8; }
"""


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("summary", help="summary JSON written by --summary")
    parser.add_argument("--output", help="HTML file to write (default: sheet.html beside the summary)")
    parser.add_argument("--width", type=int, default=760, help="thumbnail width in pixels")
    parser.add_argument(
        "--sort",
        choices=("deviation", "name"),
        default="deviation",
        help="worst-first (default) or by filename",
    )
    parser.add_argument("--limit", type=int, help="only include the first N files after sorting")
    parser.add_argument(
        "--guidance",
        choices=("all", "independent", "preview_guided"),
        default="all",
        help="only include files developed in this guidance mode. 'independent' is "
        "the column that measures the controller's own judgement rather than its "
        "ability to follow the camera's preview (see docs/PLAN.md 4b)",
    )
    arguments = parser.parse_args()

    with open(arguments.summary) as handle:
        summary = json.load(handle)

    sheet = arguments.output or os.path.join(os.path.dirname(arguments.summary) or ".", "sheet.html")
    sheet_dir = os.path.dirname(os.path.abspath(sheet))
    thumbs = os.path.join(sheet_dir, "thumbs")
    os.makedirs(thumbs, exist_ok=True)

    files = [entry for entry in summary["files"] if entry.get("status") == "completed"]
    if arguments.guidance != "all":
        files = [
            entry for entry in files if entry.get("guidance_mode") == arguments.guidance
        ]
    if arguments.sort == "deviation":
        files.sort(key=deviation, reverse=True)
    else:
        files.sort(key=lambda entry: entry["input"])
    if arguments.limit:
        files = files[: arguments.limit]

    sections = [
        cell(entry, entry.get("reference"), sheet_dir, thumbs, arguments.width) for entry in files
    ]

    def median(key, source=None):
        distribution = (source if source is not None else summary).get(key)
        return f"{distribution['median']:+.3f}" if distribution else "n/a"

    # The split scorecard, printed as two rows rather than one pooled number: a
    # pooled median cannot distinguish the controller getting better at judging
    # scenes from more files happening to carry a usable preview.
    def scorecard_row(name):
        block = summary.get(name) or {}
        if not block.get("files"):
            return ""
        return (
            f"<br><b>{name.replace('_', '-')}</b>: {block['files']} file(s), "
            f"{block.get('reference_pairs', 0)} paired &middot; "
            f"key EV {median('center_weighted_key_ev_delta', block)} &middot; "
            f"saturation {median('saturation_ratio', block)} &middot; "
            f"hue {median('hue_median_degrees', block)}&deg; "
            f"({block.get('hue_pairs', 0)} comparable)"
        )

    colour = summary.get("raw_color_path", "rawler")
    clip = summary.get("clip_altered_fraction")
    clip_note = (
        f"<br>rawler's Calibrate would have rewritten a median "
        f"{clip['median'] * 100:.3f}% of pixels per frame (max {clip['max'] * 100:.3f}%)"
        if clip
        else ""
    )

    head = (
        f"<p class='summary'>raw-autotune {summary['application_version']} &middot; "
        f"preset {summary['preset']} &middot; controller "
        f"{summary.get('controller_version', '?')} &middot; colour path {colour}"
        f"/{summary.get('working_space', 'srgb')} &middot; {len(files)} file(s) &middot; "
        f"{summary.get('reference_pairs', 0)} paired<br>"
        f"median key EV delta {median('reference_center_weighted_key_ev_delta')} &middot; "
        f"saturation ratio {median('reference_saturation_ratio')} &middot; "
        f"highlight {median('reference_highlight_saturation_ratio')} &middot; "
        f"hue {median('reference_hue_median_degrees')}&deg; "
        f"({summary.get('reference_hue_pairs', 0)} of "
        f"{summary.get('reference_pairs', 0)} comparable) &middot; "
        f"colourfulness delta {median('reference_colourfulness_delta')}"
        f"{scorecard_row('independent')}{scorecard_row('preview_guided')}{clip_note}</p>"
    )

    with open(sheet, "w") as handle:
        handle.write(
            f"<!doctype html><meta charset=utf-8><title>raw-autotune contact sheet</title>"
            f"<style>{STYLE}</style><h1>raw-autotune vs camera JPEG</h1>{head}"
            + "".join(sections)
        )
    print(f"{sheet} ({len(files)} file(s))")


if __name__ == "__main__":
    main()
