#!/usr/bin/env python3
"""Summarize observational scene evidence into bounded RAW and encoder hints.

This tool does not apply policy.  It consumes one or more raw-autotune batch summary
files and emits deterministic evidence intended for review and later policy design.
"""

from __future__ import annotations

import argparse
import json
import math
import statistics
from collections import defaultdict
from pathlib import Path
from typing import Any, Iterable


REGION_METRICS = (
    "area_fraction",
    "mean_confidence",
    "p50_ev",
    "clipped_fraction",
    "reconstruction_uncertainty",
    "mean_chroma",
    "sharpness",
)
ANALYSIS_METRICS = (
    "low_light_score",
    "near_white_fraction",
    "clipped_3_fraction",
    "mean_chroma",
    "measured_dynamic_range_ev",
)


def finite_number(value: Any) -> float | None:
    if isinstance(value, (int, float)) and math.isfinite(value):
        return float(value)
    return None


def percentile(values: Iterable[float], fraction: float) -> float | None:
    ordered = sorted(values)
    if not ordered:
        return None
    position = fraction * (len(ordered) - 1)
    lower = math.floor(position)
    upper = math.ceil(position)
    if lower == upper:
        return ordered[lower]
    weight = position - lower
    return ordered[lower] * (1.0 - weight) + ordered[upper] * weight


def distribution(values: Iterable[float]) -> dict[str, float | int | None]:
    clean = [value for value in values if math.isfinite(value)]
    return {
        "count": len(clean),
        "min": min(clean) if clean else None,
        "p25": percentile(clean, 0.25),
        "median": percentile(clean, 0.5),
        "p75": percentile(clean, 0.75),
        "max": max(clean) if clean else None,
    }


def pearson(pairs: Iterable[tuple[float, float]]) -> dict[str, float | int | None]:
    clean = [(x, y) for x, y in pairs if math.isfinite(x) and math.isfinite(y)]
    if len(clean) < 3:
        return {"count": len(clean), "pearson_r": None}
    xs, ys = zip(*clean)
    x_mean = statistics.fmean(xs)
    y_mean = statistics.fmean(ys)
    numerator = sum((x - x_mean) * (y - y_mean) for x, y in clean)
    x_norm = math.sqrt(sum((x - x_mean) ** 2 for x in xs))
    y_norm = math.sqrt(sum((y - y_mean) ** 2 for y in ys))
    coefficient = numerator / (x_norm * y_norm) if x_norm and y_norm else None
    return {"count": len(clean), "pearson_r": coefficient}


def group_name(input_path: str) -> str:
    parts = Path(input_path).parts
    for name in ("raw_at_night", "raw_extremely_bright", "raw_3rd_batch"):
        if name in parts:
            return name
    return Path(input_path).parent.name or "ungrouped"


def active_regions(file_entry: dict[str, Any]) -> list[dict[str, Any]]:
    scene = file_entry.get("scene") or {}
    return [
        region
        for region in scene.get("regions", [])
        if (finite_number(region.get("area_fraction")) or 0.0) > 0.0
    ]


def summarize_regions(files: list[dict[str, Any]]) -> dict[str, Any]:
    by_kind: dict[str, list[tuple[dict[str, Any], dict[str, Any]]]] = defaultdict(list)
    known_kinds: set[str] = set()
    for entry in files:
        for region in (entry.get("scene") or {}).get("regions", []):
            kind = region.get("kind")
            if not isinstance(kind, str):
                continue
            known_kinds.add(kind)
            if (finite_number(region.get("area_fraction")) or 0.0) > 0.0:
                by_kind[kind].append((entry, region))

    result: dict[str, Any] = {}
    for kind in sorted(known_kinds):
        samples = by_kind[kind]
        metrics = {
            metric: distribution(
                value
                for _, region in samples
                if (value := finite_number(region.get(metric))) is not None
            )
            for metric in REGION_METRICS
        }
        correlations: dict[str, Any] = {}
        for region_metric in ("area_fraction", "clipped_fraction", "mean_chroma"):
            for analysis_metric in ANALYSIS_METRICS:
                pairs = []
                for entry, region in samples:
                    x = finite_number(region.get(region_metric))
                    y = finite_number((entry.get("analysis") or {}).get(analysis_metric))
                    if x is not None and y is not None:
                        pairs.append((x, y))
                correlations[f"{region_metric}_vs_{analysis_metric}"] = pearson(pairs)
        result[kind] = {
            "detected_files": len(samples),
            "detection_rate": len(samples) / len(files) if files else 0.0,
            "metrics": metrics,
            "correlations": correlations,
        }
    return result


def image_hints(entry: dict[str, Any]) -> dict[str, Any]:
    analysis = entry.get("analysis") or {}
    regions = {region["kind"]: region for region in active_regions(entry)}
    raw_hints: list[str] = []
    encoder_hints: list[str] = []
    review_flags: list[str] = []

    sky = regions.get("sky")
    if sky:
        area = finite_number(sky.get("area_fraction")) or 0.0
        confidence = finite_number(sky.get("mean_confidence")) or 0.0
        clipped = finite_number(sky.get("clipped_fraction")) or 0.0
        if area >= 0.05 and confidence >= 0.75 and clipped >= 0.10:
            raw_hints.append("protect_sky_highlights_luma_only")
            encoder_hints.append("prefer_10bit_highlight_headroom")

    low_light = finite_number(analysis.get("low_light_score")) or 0.0
    if low_light >= 0.5:
        encoder_hints.append("retain_grain_avoid_aggressive_denoise")

    confident_regions = [
        region
        for region in regions.values()
        if (finite_number(region.get("mean_confidence")) or 0.0) >= 0.70
    ]
    non_sky_chroma_subject = any(
        region.get("kind") != "sky"
        and (finite_number(region.get("area_fraction")) or 0.0) >= 0.05
        and (finite_number(region.get("mean_chroma")) or 0.0) >= 0.12
        for region in confident_regions
    )
    if (finite_number(analysis.get("mean_chroma")) or 0.0) >= 0.08 and non_sky_chroma_subject:
        encoder_hints.append("prefer_chroma_fidelity_444_or_high_quality_422")

    for kind in ("face", "person"):
        region = regions.get(kind)
        if region and (finite_number(region.get("mean_confidence")) or 0.0) >= 0.75:
            review_flags.append(f"verify_{kind}_detection_before_roi_or_exposure_policy")

    encoder_profile_hint: dict[str, Any] = {}
    if "prefer_10bit_highlight_headroom" in encoder_hints:
        encoder_profile_hint["minimum_bit_depth"] = 10
    if "retain_grain_avoid_aggressive_denoise" in encoder_hints:
        encoder_profile_hint["grain_preservation"] = "prefer"
    if "prefer_chroma_fidelity_444_or_high_quality_422" in encoder_hints:
        encoder_profile_hint["chroma_sampling_preference"] = ["4:4:4", "high-quality 4:2:2"]

    return {
        "input": entry.get("input"),
        "group": group_name(str(entry.get("input", ""))),
        "raw_hints": raw_hints,
        "encoder_hints": encoder_hints,
        "encoder_profile_hint": encoder_profile_hint,
        "review_flags": review_flags,
        "policy_status": "observational_only",
    }


def candidate_actions(files: list[dict[str, Any]], regions: dict[str, Any]) -> list[dict[str, Any]]:
    hints = [image_hints(entry) for entry in files]

    def count_hint(section: str, hint: str) -> int:
        return sum(hint in item[section] for item in hints)

    face_reviews = sum(
        any(flag.startswith("verify_face") for flag in item["review_flags"]) for item in hints
    )
    person_reviews = sum(
        any(flag.startswith("verify_person") for flag in item["review_flags"]) for item in hints
    )
    unsupported = [
        kind
        for kind in ("water", "snow_or_sand", "text_or_document", "salient_foreground")
        if regions.get(kind, {}).get("detected_files", 0) == 0
    ]
    return [
        {
            "target": "raw.highlight_tone",
            "candidate": "protect_sky_highlights_luma_only",
            "triggered_files": count_hint("raw_hints", "protect_sky_highlights_luma_only"),
            "status": "report_only_requires_visual_ablation",
            "guardrail": "Must not alter chroma or conceal highlight-reconstruction hue errors.",
        },
        {
            "target": "encoder.bit_depth_transfer",
            "candidate": "prefer_10bit_highlight_headroom",
            "triggered_files": count_hint("encoder_hints", "prefer_10bit_highlight_headroom"),
            "status": "candidate_profile_hint",
            "guardrail": (
                "Encoder capability and output transfer function remain explicit caller choices."
            ),
        },
        {
            "target": "encoder.denoise_grain",
            "candidate": "retain_grain_avoid_aggressive_denoise",
            "triggered_files": count_hint("encoder_hints", "retain_grain_avoid_aggressive_denoise"),
            "status": "usable_raw_analysis_hint_not_semantic_added_value",
            "guardrail": (
                "Derived from RAW low-light analysis; semantics currently add context, "
                "not the decision."
            ),
        },
        {
            "target": "encoder.chroma_sampling",
            "candidate": "prefer_chroma_fidelity_444_or_high_quality_422",
            "triggered_files": count_hint(
                "encoder_hints", "prefer_chroma_fidelity_444_or_high_quality_422"
            ),
            "status": "candidate_profile_hint_requires_encoder_ablation",
            "guardrail": "Subject to codec, delivery bandwidth, and hardware decode constraints.",
        },
        {
            "target": "raw.skin_exposure_and_encoder.roi",
            "candidate": "face_or_person_priority",
            "triggered_files": face_reviews + person_reviews,
            "face_review_files": face_reviews,
            "person_review_files": person_reviews,
            "raw_detection_files": regions.get("face", {}).get("detected_files", 0)
            + regions.get("person", {}).get("detected_files", 0),
            "status": "blocked_on_false_positive_review",
            "guardrail": (
                "No exposure or ROI policy may use these detections without visual ground truth."
            ),
        },
        {
            "target": "semantic_classes",
            "candidate": "expand_model_or_mapping",
            "unsupported_classes": unsupported,
            "status": "model_gap",
            "guardrail": "Zero detections are not evidence that these subjects are absent.",
        },
        {
            "target": "encoder.temporal_scene_change",
            "candidate": "scene_cut_or_gop_hint",
            "triggered_files": 0,
            "status": "not_evaluated_on_still_image_corpus",
            "guardrail": "Semantic still-image evidence cannot establish temporal cut boundaries.",
        },
    ]


def semantic_value_assessment(
    regions: dict[str, Any], actions: list[dict[str, Any]]
) -> list[dict[str, Any]]:
    by_candidate = {action["candidate"]: action for action in actions}
    return [
        {
            "signal": "sky_highlight_localization",
            "verdict": "useful_for_bounded_raw_ablation_and_highlight_profile_hint",
            "evidence": {
                "sky_detected_files": regions.get("sky", {}).get("detected_files", 0),
                "high_confidence_clipped_sky_files": by_candidate[
                    "protect_sky_highlights_luma_only"
                ]["triggered_files"],
            },
            "limit": (
                "Apply only to luma/highlight allocation; semantic masks must not hide "
                "reconstruction hue defects."
            ),
        },
        {
            "signal": "vegetation_and_building_region_statistics",
            "verdict": "useful_as_measurement_not_yet_as_render_policy",
            "evidence": {
                "vegetation_detected_files": regions.get("vegetation", {}).get("detected_files", 0),
                "building_detected_files": regions.get("building_or_interior", {}).get(
                    "detected_files", 0
                ),
            },
            "limit": "Needs feature-off visual ablation before local tone or colour changes.",
        },
        {
            "signal": "face_person_priority",
            "verdict": "not_useful_on_this_corpus",
            "evidence": {
                "face_detected_files": regions.get("face", {}).get("detected_files", 0),
                "person_detected_files": regions.get("person", {}).get("detected_files", 0),
            },
            "limit": (
                "Coverage is absent or below the policy confidence gate; no exposure or ROI action."
            ),
        },
        {
            "signal": "night_grain_profile",
            "verdict": "useful_but_not_semantic_added_value",
            "evidence": {
                "triggered_files": by_candidate["retain_grain_avoid_aggressive_denoise"][
                    "triggered_files"
                ]
            },
            "limit": "The existing RAW low-light score is sufficient for this hint.",
        },
        {
            "signal": "semantic_chroma_profile",
            "verdict": "candidate_requires_encoder_ablation",
            "evidence": {
                "triggered_files": by_candidate[
                    "prefer_chroma_fidelity_444_or_high_quality_422"
                ]["triggered_files"]
            },
            "limit": "Do not infer codec support or bandwidth budget from scene evidence.",
        },
    ]


def make_report(paths: list[Path]) -> dict[str, Any]:
    files: list[dict[str, Any]] = []
    source_runs: list[dict[str, Any]] = []
    for path in paths:
        document = json.loads(path.read_text(encoding="utf-8"))
        run_files = document.get("files", [])
        if not isinstance(run_files, list):
            raise ValueError(f"{path}: 'files' must be a list")
        files.extend(
            entry
            for entry in run_files
            if entry.get("status") in {"analyzed", "written", "completed"}
        )
        source_runs.append(
            {
                "path": str(path),
                "total": document.get("total"),
                "completed": document.get("completed"),
                "failed": document.get("failed"),
                "skipped": document.get("skipped"),
            }
        )

    backends = sorted(
        {
            model.get("backend")
            for entry in files
            for model in (entry.get("scene") or {}).get("models", [])
            if model.get("backend")
        }
    )
    fallback_files = [
        entry.get("input")
        for entry in files
        if not (entry.get("scene") or {}).get("models")
        or any(
            "fallback" in str(model.get("backend", "")).lower()
            or "4060" not in str(model.get("backend", ""))
            for model in (entry.get("scene") or {}).get("models", [])
        )
    ]
    policy_adjustment_files = [
        entry.get("input")
        for entry in files
        if (entry.get("scene") or {}).get("policy_adjustments")
    ]
    grouped: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for entry in files:
        grouped[group_name(str(entry.get("input", "")))].append(entry)

    regions = summarize_regions(files)
    actions = candidate_actions(files, regions)
    return {
        "schema_version": 1,
        "policy_status": "observational_only",
        "source_runs": source_runs,
        "validation": {
            "completed_files": len(files),
            "groups": {name: len(entries) for name, entries in sorted(grouped.items())},
            "model_backends": backends,
            "non_rtx_4060_or_fallback_files": fallback_files,
            "files_with_policy_adjustments": policy_adjustment_files,
        },
        "global_analysis": {
            metric: distribution(
                value
                for entry in files
                if (value := finite_number((entry.get("analysis") or {}).get(metric))) is not None
            )
            for metric in ANALYSIS_METRICS
        },
        "regions": regions,
        "groups": {
            name: {
                "files": len(entries),
                "analysis": {
                    metric: distribution(
                        value
                        for entry in entries
                        if (value := finite_number((entry.get("analysis") or {}).get(metric)))
                        is not None
                    )
                    for metric in ANALYSIS_METRICS
                },
                "regions": summarize_regions(entries),
            }
            for name, entries in sorted(grouped.items())
        },
        "semantic_value_assessment": semantic_value_assessment(regions, actions),
        "candidate_actions": actions,
        "per_image_hints": [
            image_hints(entry) for entry in sorted(files, key=lambda item: item["input"])
        ],
    }


def markdown(report: dict[str, Any]) -> str:
    validation = report["validation"]
    lines = [
        "# Scene evidence usefulness report",
        "",
        f"Policy status: **{report['policy_status']}**",
        "",
        f"Completed images: {validation['completed_files']}",
        f"Groups: {json.dumps(validation['groups'], sort_keys=True)}",
        f"Backends: {', '.join(validation['model_backends']) or 'none'}",
        f"Non-RTX-4060/fallback files: {len(validation['non_rtx_4060_or_fallback_files'])}",
        f"Files with applied semantic policy: {len(validation['files_with_policy_adjustments'])}",
        "",
        "## Candidate actions",
        "",
    ]
    for action in report["candidate_actions"]:
        count = action.get("triggered_files")
        suffix = f" ({count} files)" if count is not None else ""
        lines.append(
            f"- `{action['candidate']}`: {action['status']}{suffix}. {action['guardrail']}"
        )
    lines.extend(["", "## Region coverage", ""])
    for kind, summary in report["regions"].items():
        confidence = summary["metrics"]["mean_confidence"]["median"]
        confidence_text = "n/a" if confidence is None else f"{confidence:.3f}"
        lines.append(
            f"- `{kind}`: {summary['detected_files']} files "
            f"({summary['detection_rate']:.1%}), median confidence {confidence_text}"
        )
    lines.append("")
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("summaries", type=Path, nargs="+")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--format", choices=("json", "markdown"), default="json")
    args = parser.parse_args()
    report = make_report(args.summaries)
    rendered = (
        markdown(report)
        if args.format == "markdown"
        else json.dumps(report, indent=2, sort_keys=True) + "\n"
    )
    args.output.write_text(rendered, encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
