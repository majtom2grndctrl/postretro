"""Guard: dev characters take their clips from the shared library.

Every model named here ships as mesh and skeleton. A clip name that stops
resolving falls back to clip 0 with no error, so descriptors are checked against
the library's clip list.
"""

import json
import re
import unittest
from pathlib import Path

CONTENT = Path(__file__).parents[2] / "content" / "dev"
MODELS = CONTENT / "models"
LIBRARY = MODELS / "clip_libraries" / "mixamo_rifle" / "library.gltf"
REFERENCE = "../clip_libraries/mixamo_rifle/library.gltf"

# Model folder -> budget for `.gltf` plus `.bin`, in bytes.
BOUND_MODELS = {
    "exo_red": 1_500_000,
    "limitator": 300_000,
    "rodin_sci-fi_trooper_m": 400_000,
    "cyberpunk_warrior_f": 400_000,
}
# `rifle-squads.ts` inherits the limitator's animations, so it names no clip.
DESCRIPTORS = [
    "player.ts",
    "limitator.ts",
    "sci-fi-trooper.ts",
    "cyberpunk-warrior.ts",
]
CLIP = re.compile(r'clip:\s*"([^"]+)"')


def load(path):
    with open(path, encoding="utf-8") as handle:
        return json.load(handle)


class DevClipLibraryTests(unittest.TestCase):
    def test_bound_models_embed_no_clips_and_name_the_library(self):
        for name, budget in BOUND_MODELS.items():
            with self.subTest(model=name):
                folder = MODELS / name
                gltf = load(folder / "model.gltf")
                self.assertTrue(gltf.get("skins"))
                self.assertFalse(gltf.get("animations"))
                self.assertEqual(gltf["extras"]["clipLibrary"], REFERENCE)
                size = sum((folder / f"model.{ext}").stat().st_size for ext in ("gltf", "bin"))
                self.assertLess(size, budget)

    def test_new_characters_dropped_their_static_meshes(self):
        for name in ("rodin_sci-fi_trooper_m", "cyberpunk_warrior_f"):
            with self.subTest(model=name):
                folder = MODELS / name
                self.assertTrue((folder / "license.txt").exists())
                self.assertFalse((folder / "scene.gltf").exists())
                self.assertFalse((folder / "scene.bin").exists())
                self.assertFalse((folder / "textures").exists())

    def test_library_has_no_mesh_and_clips_start_at_time_zero(self):
        library = load(LIBRARY)
        self.assertFalse(library.get("meshes"))
        self.assertFalse(library.get("skins"))
        self.assertTrue(library["animations"])
        for animation in library["animations"]:
            with self.subTest(clip=animation["name"]):
                first = min(
                    library["accessors"][sampler["input"]]["min"][0]
                    for sampler in animation["samplers"]
                )
                self.assertEqual(first, 0.0)

    def test_every_descriptor_clip_resolves_in_the_library(self):
        clips = {animation["name"] for animation in load(LIBRARY)["animations"]}
        for descriptor in DESCRIPTORS:
            text = (CONTENT / "scripts" / descriptor).read_text(encoding="utf-8")
            names = CLIP.findall(text)
            self.assertTrue(names, descriptor)
            for name in names:
                with self.subTest(descriptor=descriptor, clip=name):
                    self.assertIn(name, clips)


if __name__ == "__main__":
    unittest.main()
