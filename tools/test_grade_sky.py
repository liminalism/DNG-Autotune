"""Regression tests for the standalone sky-grading helper."""

import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import importlib.util

MODULE_PATH = Path(__file__).with_name("grade_sky.py")
SPEC = importlib.util.spec_from_file_location("grade_sky", MODULE_PATH)
assert SPEC and SPEC.loader
grade_sky = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(grade_sky)


class GradeSkyTest(unittest.TestCase):
    def test_grade_finds_recursive_render_output(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            nested = root / "rendered" / "raw_3rd_batch"
            nested.mkdir(parents=True)
            frame = nested / "_DSC1283_auto.jpg"
            frame.touch()
            (root / "rendered" / "notes.txt").touch()
            reference = root / "_DSC1283.JPG"
            reference.touch()

            with patch.object(grade_sky, "measure", return_value={"frame": "_DSC1283_auto"}) as measure:
                rows, unpaired = grade_sky.grade(root / "rendered", {"_dsc1283": reference})

            self.assertEqual(rows, [{"frame": "_DSC1283_auto"}])
            self.assertEqual(unpaired, 0)
            measure.assert_called_once_with(frame, reference)


if __name__ == "__main__":
    unittest.main()
