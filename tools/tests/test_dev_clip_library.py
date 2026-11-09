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
# Library folder -> the models bound to it (folder -> budget for `.gltf` plus
# `.bin`, in bytes) and the descriptors whose clips it must resolve.
# `rifle-squads.ts` inherits the limitator's animations, so it names no clip.
LIBRARIES = {
    "mixamo_rifle": {
        "models": {
            "exo_red": 1_500_000,
            "limitator": 300_000,
            "rodin_sci-fi_trooper_m": 400_000,
            "cyberpunk_warrior_f": 400_000,
        },
        "descriptors": [
            "player.ts",
            "limitator.ts",
            "sci-fi-trooper.ts",
            "cyberpunk-warrior.ts",
        ],
    },
    "mixamo_brute": {
        "models": {"mutant": 700_000},
        "descriptors": [
            "reference-enemy.ts",
            "reference-enemy.luau",
            "coop-presentation-entities.ts",
        ],
    },
}
CLIP = re.compile(r'clip\s*[:=]\s*"([^"]+)"')


def library_path(name):
    return MODELS / "clip_libraries" / name / "library.gltf"


def load(path):
    with open(path, encoding="utf-8") as handle:
        return json.load(handle)


class DevClipLibraryTests(unittest.TestCase):
    def test_bound_models_embed_no_clips_and_name_the_library(self):
        for library, entry in LIBRARIES.items():
            reference = f"../clip_libraries/{library}/library.gltf"
            for name, budget in entry["models"].items():
                with self.subTest(model=name):
                    folder = MODELS / name
                    gltf = load(folder / "model.gltf")
                    self.assertTrue(gltf.get("skins"))
                    self.assertFalse(gltf.get("animations"))
                    self.assertEqual(gltf["extras"]["clipLibrary"], reference)
                    size = sum((folder / f"model.{ext}").stat().st_size
                               for ext in ("gltf", "bin"))
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
        for name in LIBRARIES:
            library = load(library_path(name))
            self.assertFalse(library.get("meshes"))
            self.assertFalse(library.get("skins"))
            self.assertTrue(library["animations"])
            for animation in library["animations"]:
                with self.subTest(library=name, clip=animation["name"]):
                    first = min(
                        library["accessors"][sampler["input"]]["min"][0]
                        for sampler in animation["samplers"]
                    )
                    self.assertEqual(first, 0.0)

    def test_every_descriptor_clip_resolves_in_the_library(self):
        for library, entry in LIBRARIES.items():
            clips = {animation["name"] for animation in load(library_path(library))["animations"]}
            for descriptor in entry["descriptors"]:
                text = (CONTENT / "scripts" / descriptor).read_text(encoding="utf-8")
                names = CLIP.findall(text)
                self.assertTrue(names, descriptor)
                for name in names:
                    with self.subTest(descriptor=descriptor, clip=name):
                        self.assertIn(name, clips)


if __name__ == "__main__":
    unittest.main()
