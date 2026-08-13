import json
import tempfile
import unittest
from pathlib import Path

from tools.scene_policy_report import make_report, pearson, percentile


def entry(path: str, *, low_light: float, sky_clip: float, backend: str = "lege-gpu-wgpu:RTX 4060"):
    return {
        "input": path,
        "status": "analyzed",
        "analysis": {
            "low_light_score": low_light,
            "near_white_fraction": sky_clip,
            "clipped_3_fraction": sky_clip / 2,
            "mean_chroma": 0.2,
            "measured_dynamic_range_ev": 10.0,
        },
        "scene": {
            "models": [{"backend": backend}],
            "policy_adjustments": [],
            "regions": [
                {
                    "kind": "sky",
                    "area_fraction": 0.25,
                    "mean_confidence": 0.9,
                    "p50_ev": 1.5,
                    "clipped_fraction": sky_clip,
                    "reconstruction_uncertainty": 0.1,
                    "mean_chroma": 0.25,
                    "sharpness": 0.02,
                },
                {"kind": "face", "area_fraction": 0.0, "mean_confidence": 0.0},
            ],
        },
    }


class ScenePolicyReportTest(unittest.TestCase):
    def test_percentile_and_pearson(self):
        self.assertEqual(percentile([1.0, 3.0], 0.5), 2.0)
        self.assertAlmostEqual(pearson([(1, 2), (2, 4), (3, 6)])["pearson_r"], 1.0)

    def test_report_groups_gpu_and_hints(self):
        document = {
            "total": 3,
            "completed": 3,
            "failed": 0,
            "skipped": 0,
            "files": [
                entry("raw/raw_3rd_batch/a.dng", low_light=0.0, sky_clip=0.2),
                entry("raw/raw_at_night/b.ARW", low_light=0.8, sky_clip=0.0),
                entry("raw/raw_extremely_bright/c.ARW", low_light=0.0, sky_clip=0.4),
            ],
        }
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "summary.json"
            path.write_text(json.dumps(document), encoding="utf-8")
            report = make_report([path])

        self.assertEqual(report["validation"]["completed_files"], 3)
        self.assertEqual(report["validation"]["groups"]["raw_at_night"], 1)
        self.assertEqual(report["validation"]["non_rtx_4060_or_fallback_files"], [])
        self.assertEqual(report["regions"]["sky"]["detected_files"], 3)
        self.assertEqual(report["candidate_actions"][0]["triggered_files"], 2)
        self.assertEqual(
            report["semantic_value_assessment"][0]["verdict"],
            "useful_for_bounded_raw_ablation_and_highlight_profile_hint",
        )
        night = next(item for item in report["per_image_hints"] if item["group"] == "raw_at_night")
        self.assertIn("retain_grain_avoid_aggressive_denoise", night["encoder_hints"])
        self.assertEqual(night["encoder_profile_hint"]["grain_preservation"], "prefer")
        self.assertEqual(night["policy_status"], "observational_only")


if __name__ == "__main__":
    unittest.main()
