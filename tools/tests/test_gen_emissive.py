"""Tests for gen_emissive.py's bright-texel cutoff.

Without `--cutoff` every texture takes one default, whatever its name's prefix:
glow strength is the mod's `materials.toml`, so no prefix tunes the generator.

The cutoff test stubs Pillow when it is absent, so it runs with the standard
library. The pixel test needs Pillow (`uv run --with pillow python3 -m unittest
discover -s tools/tests -p test_gen_emissive.py`) and skips without it.
"""

import contextlib
import importlib.util
import io
import sys
import tempfile
import types
import unittest
from pathlib import Path

SCRIPT = Path(__file__).parents[1] / "gen_emissive.py"

try:
    import PIL  # noqa: F401

    HAVE_PIL = True
except ImportError:
    HAVE_PIL = False


def load_module():
    """Import the script, standing in for Pillow when it is not installed."""
    if not HAVE_PIL:
        pil = types.ModuleType("PIL")
        for name in ("Image", "ImageOps", "PngImagePlugin"):
            submodule = types.ModuleType(f"PIL.{name}")
            setattr(pil, name, submodule)
            sys.modules[f"PIL.{name}"] = submodule
        sys.modules["PIL.PngImagePlugin"].PngInfo = lambda: None
        sys.modules["PIL"] = pil
    spec = importlib.util.spec_from_file_location("gen_emissive_test", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


GEN = load_module()


class _FakeImage:
    def __init__(self):
        self.info = {}

    def __enter__(self):
        return self

    def __exit__(self, *_):
        return False

    def save(self, *_args, **_kwargs):
        pass


class DefaultCutoffTest(unittest.TestCase):
    def cutoff_used_for(self, filename, cutoff=None):
        seen = []
        original_open = GEN.Image.__dict__.get("open")
        original_emissive = GEN.emissive_from_diffuse
        GEN.Image.open = lambda _path: _FakeImage()

        def record(_image, used):
            seen.append(used)
            return _FakeImage()

        GEN.emissive_from_diffuse = record
        try:
            with tempfile.TemporaryDirectory() as tmp:
                out = Path(tmp) / "out_e.png"
                with contextlib.redirect_stdout(io.StringIO()):
                    ok = GEN.process_image(Path(tmp) / filename, out, cutoff, force=True)
                self.assertTrue(ok)
        finally:
            GEN.emissive_from_diffuse = original_emissive
            if original_open is None:
                del GEN.Image.open
            else:
                GEN.Image.open = original_open
        self.assertEqual(len(seen), 1)
        return seen[0]

    def test_every_prefix_takes_the_one_default_cutoff(self):
        for filename in ("neon_sign.png", "NEON_sign.png", "metal_panel.png", "panel.png"):
            self.assertEqual(self.cutoff_used_for(filename), GEN.DEFAULT_CUTOFF, filename)

    def test_the_default_is_the_conservative_highlight_cutoff(self):
        self.assertEqual(GEN.DEFAULT_CUTOFF, 0.75)

    def test_an_explicit_cutoff_wins_over_the_default(self):
        self.assertEqual(self.cutoff_used_for("neon_sign.png", cutoff=0.35), 0.35)


@unittest.skipUnless(HAVE_PIL, "needs Pillow")
class EmissivePixelsTest(unittest.TestCase):
    def test_a_former_neon_texture_keeps_only_texels_past_the_default(self):
        from PIL import Image

        with tempfile.TemporaryDirectory() as tmp:
            # Mid grey (0.5) sits under the default cutoff; white sits over it.
            source = Image.new("RGBA", (2, 1))
            source.putpixel((0, 0), (128, 128, 128, 255))
            source.putpixel((1, 0), (255, 255, 255, 255))
            for name in ("neon_sign", "sign"):
                path = Path(tmp) / f"{name}.png"
                source.save(path)
                out = Path(tmp) / f"{name}_e.png"
                with contextlib.redirect_stdout(io.StringIO()):
                    self.assertTrue(GEN.process_image(path, out, force=True))
                with Image.open(out) as result:
                    rgba = result.convert("RGBA")
                    self.assertEqual(rgba.getpixel((0, 0)), (0, 0, 0, 255), name)
                    self.assertEqual(rgba.getpixel((1, 0)), (255, 255, 255, 255), name)


if __name__ == "__main__":
    unittest.main()
