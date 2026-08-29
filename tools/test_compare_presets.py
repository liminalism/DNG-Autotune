"""Regression tests for the preset comparison helper."""

import importlib.util
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import numpy as np

MODULE_PATH = Path(__file__).with_name("compare_presets.py")
SPEC = importlib.util.spec_from_file_location("compare_presets", MODULE_PATH)
assert SPEC and SPEC.loader
compare_presets = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(compare_presets)


class LabTest(unittest.TestCase):
    def test_mid_grey_is_neutral(self):
        grey = np.full((2, 2, 3), 128, dtype=np.uint8)
        lab = compare_presets.srgb_to_lab(grey)
        chroma, _ = compare_presets.chroma_hue(lab)
        self.assertLess(float(chroma.max()), 0.01)
        self.assertAlmostEqual(float(lab[..., 0].mean()), 53.6, places=1)

    def test_hue_families_land_where_named(self):
        """Each named colour must fall in the family it is named for.

        The whole comparison keys hue families off these angles, so a wrong
        boundary would silently report sky numbers in the magenta column --
        which is exactly what a 300-degree cut did on the first attempt.
        """
        cases = {
            "blue": [(0, 64, 255), (90, 140, 220), (40, 90, 190)],
            "warm": [(255, 128, 0), (230, 90, 60), (255, 220, 0), (220, 170, 140)],
            "green": [(80, 150, 60), (40, 90, 40)],
            "cyan": [(0, 255, 255)],
            "magenta": [(220, 60, 200), (200, 40, 80)],
        }
        for family, pixels in cases.items():
            array = np.array([pixels], dtype=np.uint8)
            _, hue = compare_presets.chroma_hue(compare_presets.srgb_to_lab(array))
            member = compare_presets.in_family(hue, compare_presets.FAMILIES[family])
            self.assertTrue(member.all(), f"{family}: {hue.ravel()}")

    def test_every_hue_angle_belongs_to_exactly_one_family(self):
        hue = np.arange(0.0, 360.0, 0.25)
        counts = sum(
            compare_presets.in_family(hue, bounds).astype(int)
            for bounds in compare_presets.FAMILIES.values()
        )
        self.assertTrue((counts == 1).all())

    def test_hue_delta_takes_the_short_arc(self):
        self.assertAlmostEqual(compare_presets.hue_delta(np.array(350.0), np.array(10.0)), -20.0)
        self.assertAlmostEqual(compare_presets.hue_delta(np.array(10.0), np.array(350.0)), 20.0)


class PairingTest(unittest.TestCase):
    def test_stem_drops_every_preset_suffix(self):
        for suffix in ("auto", "standard", "vivid", "punchy", "neutral", "baseline"):
            self.assertEqual(
                compare_presets.stem_of(Path(f"_DSC1283_{suffix}.jpg")), "_dsc1283"
            )

    def test_reference_index_spans_several_directories(self):
        """Sony pairs live both in `raw/jpeg` and beside their own ARWs."""
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "jpeg").mkdir()
            (root / "beside").mkdir()
            (root / "jpeg" / "_DSC1283.JPG").touch()
            (root / "beside" / "_DSC1236.JPG").touch()
            index = compare_presets.index_images([root / "jpeg", root / "beside"])
        self.assertEqual(sorted(index), ["_dsc1236", "_dsc1283"])

    def test_measure_is_called_on_recursive_render_output(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            nested = root / "vivid" / "raw_3rd_batch"
            nested.mkdir(parents=True)
            frame = nested / "_DSC1283_vivid.jpg"
            frame.touch()
            (root / "vivid" / "summary.json").touch()
            self.assertEqual(compare_presets.rendered_frames(root / "vivid"), [frame])

    def test_match_splits_one_tree_by_camera(self):
        """A Sony and a Samsung frame in the same output directory must separate."""
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            root.joinpath("_DSC1283_vivid.jpg").touch()
            root.joinpath("20260729_114901_vivid.jpg").touch()
            sony = compare_presets.rendered_frames(root, r"^_dsc")
            samsung = compare_presets.rendered_frames(root, r"^\d")
        self.assertEqual([p.name for p in sony], ["_DSC1283_vivid.jpg"])
        self.assertEqual([p.name for p in samsung], ["20260729_114901_vivid.jpg"])

    def test_align_crops_to_the_common_shape(self):
        one = np.zeros((10, 12, 3))
        two = np.zeros((11, 11, 3))
        left, right = compare_presets.align(one, two)
        self.assertEqual(left.shape, (10, 11, 3))
        self.assertEqual(right.shape, (10, 11, 3))


class MeasureTest(unittest.TestCase):
    def test_identical_renders_score_a_perfect_match(self):
        pixels = np.random.default_rng(0).integers(0, 256, (24, 24, 3), dtype=np.uint8)
        lab = compare_presets.srgb_to_lab(pixels)
        with patch.object(compare_presets, "load_lab", return_value=lab):
            row = compare_presets.measure(Path("a.jpg"), Path("b.jpg"))
        self.assertAlmostEqual(row["chroma_ratio"], 1.0, places=6)
        self.assertAlmostEqual(row["hue_err"], 0.0, places=6)
        self.assertEqual(row["neutralised"], 0.0)

    def test_a_desaturated_render_is_counted_as_neutralised(self):
        camera = np.zeros((8, 8, 3))
        camera[..., 0] = 60.0
        camera[..., 1] = 60.0  # a* = 60, so C* = 60, well over the threshold
        ours = camera.copy()
        ours[..., 1] = 20.0
        with patch.object(compare_presets, "load_lab", side_effect=[ours, camera]):
            row = compare_presets.measure(Path("ours.jpg"), Path("camera.jpg"))
        self.assertEqual(row["neutralised"], 1.0)
        self.assertEqual(row["overshot"], 0.0)
        self.assertAlmostEqual(row["C_ratio_saturated"], 1.0 / 3.0, places=6)
        self.assertAlmostEqual(row["chroma_ratio"], 1.0 / 3.0, places=6)

    def test_a_frame_can_overshoot_the_camera_where_it_is_boldest(self):
        """The saturated-pixel ratio must be able to disagree with the frame's.

        `standard` overshoots the camera on the frame average while falling
        short on the camera's boldest pixels, so one number cannot serve both.
        """
        camera = np.zeros((2, 2, 3))
        camera[0, :, 1] = 60.0  # two strongly saturated pixels, C* = 60
        camera[1, :, 1] = 4.0  # two near-neutral ones
        ours = camera.copy()
        ours[0, :, 1] = 30.0  # half the chroma where the camera is boldest
        ours[1, :, 1] = 40.0  # far more of it where the camera is quiet
        with patch.object(compare_presets, "load_lab", side_effect=[ours, camera]):
            row = compare_presets.measure(Path("ours.jpg"), Path("camera.jpg"))
        self.assertGreater(row["chroma_ratio"], 1.0)
        self.assertAlmostEqual(row["C_ratio_saturated"], 0.5, places=6)
        self.assertEqual(row["neutralised"], 1.0)


if __name__ == "__main__":
    unittest.main()
