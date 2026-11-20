"""Tests for gen_emissive_stress.py: the variants differ only in volumes.

Run with `python3 -m unittest discover -s tools/tests -p test_gen_emissive_stress.py`.
"""

import contextlib
import importlib.util
import io
import tempfile
import unittest
from pathlib import Path


def load_module():
    script = Path(__file__).parents[1] / "gen_emissive_stress.py"
    spec = importlib.util.spec_from_file_location("gen_emissive_stress_test", script)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


GEN = load_module()


def generate(tmp, *args):
    out = Path(tmp) / "fixture.map"
    script = Path(tmp) / "scripts" / "stress.ts"
    with contextlib.redirect_stdout(io.StringIO()):
        GEN.main([*args, "-o", str(out), "--script", str(script)])
    return out.read_text(), script


class GenEmissiveStressTest(unittest.TestCase):
    def test_volume_counts_tags_and_script(self):
        with tempfile.TemporaryDirectory() as tmp:
            text, script = generate(tmp, "--volumes", "256", "--animate", "16")
            self.assertEqual(text.count('"classname" "emissive_volume"'), 256)
            self.assertEqual(text.count('"_tags" "stress_anim"'), 16)
            self.assertIn('"data_script" "../scripts/stress.ts"', text)
            self.assertIn("pulse", script.read_text())
            self.assertEqual(text.count("{"), text.count("}"))

    def test_static_variants_name_no_script_and_share_every_other_line(self):
        with tempfile.TemporaryDirectory() as tmp:
            none, _ = generate(tmp, "--volumes", "0")
            one, _ = generate(tmp, "--volumes", "1")
            many, _ = generate(tmp, "--volumes", "256")
        for text in (none, one, many):
            self.assertNotIn("data_script", text)
            self.assertEqual(text.count(f" {GEN.GLOW_TEXTURE} "), 16 * 16)
        self.assertNotIn("emissive_volume", none)
        self.assertEqual(one.count('"classname" "emissive_volume"'), 1)
        # Volume entities come last, so the rest of each map is the 0-volume map.
        body = none.split("// Emissive-volume measurement fixture")[1].split("\n", 1)[1]
        for text in (one, many):
            self.assertIn(body.rstrip("\n"), text)

    def test_baseline_texture_swaps_only_the_glow_faces(self):
        with tempfile.TemporaryDirectory() as tmp:
            current, _ = generate(tmp, "--volumes", "0")
            baseline, _ = generate(tmp, "--volumes", "0", "--glow-texture", "neon/neon_glow_panel")
        self.assertEqual(baseline.count(" neon/neon_glow_panel "), 256)
        self.assertEqual(
            baseline.replace("neon/neon_glow_panel", GEN.GLOW_TEXTURE), current
        )


if __name__ == "__main__":
    unittest.main()
