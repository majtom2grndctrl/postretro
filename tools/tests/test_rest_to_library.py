"""Tests for tools/rest_to_library.py: re-resting a bound character onto its
clip library's rest rotations."""

import importlib.util
import json
import math
import struct
import tempfile
import unittest
from pathlib import Path

import numpy as np

SCRIPT = Path(__file__).parents[1] / "rest_to_library.py"
SPEC = importlib.util.spec_from_file_location("rest_to_library_test", SCRIPT)
TOOL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(TOOL)

# The character's arm is bent 30 degrees about Z at rest; the library's is straight.
BENT = [0.0, 0.0, math.sin(math.radians(15)), math.cos(math.radians(15))]
STRAIGHT = [0.0, 0.0, 0.0, 1.0]
HIPS_T = [0.0, 1.0, 0.0]
ARM_T = [0.0, 0.5, 0.0]
VERTEX_IN_ARM = np.array([0.0, 0.4, 0.0])


def world(rotation):
    hips = np.eye(4)
    hips[:3, 3] = HIPS_T
    arm = np.eye(4)
    arm[:3, :3] = TOOL.quat_to_matrix(rotation)
    arm[:3, 3] = ARM_T
    return hips, hips @ arm


def character(directory, library_ref):
    hips_w, arm_w = world(BENT)
    vertex = (arm_w @ np.append(VERTEX_IN_ARM, 1.0))[:3]
    chunks = [
        struct.pack("<3f", *vertex),
        struct.pack("<3f", 1.0, 0.0, 0.0),
        struct.pack("<4B", 1, 0, 0, 0),
        struct.pack("<4f", 1.0, 0.0, 0.0, 0.0),
        b"".join(struct.pack("<16f", *np.linalg.inv(m).T.reshape(-1)) for m in (hips_w, arm_w)),
    ]
    data, views = bytearray(), []
    for chunk in chunks:
        views.append({"buffer": 0, "byteOffset": len(data), "byteLength": len(chunk)})
        data.extend(chunk)
    gltf = {
        "asset": {"version": "2.0"},
        "extras": {"clipLibrary": library_ref},
        "scenes": [{"nodes": [0]}],
        "nodes": [
            {"name": "Armature", "children": [1, 3]},
            {"name": "mixamorig:Hips", "translation": HIPS_T, "children": [2]},
            {"name": "mixamorig:LeftArm", "translation": ARM_T, "rotation": BENT},
            {"name": "Body", "mesh": 0, "skin": 0},
        ],
        "skins": [{"joints": [1, 2], "skeleton": 1, "inverseBindMatrices": 4}],
        "meshes": [{"primitives": [{"attributes": {
            "POSITION": 0, "NORMAL": 1, "JOINTS_0": 2, "WEIGHTS_0": 3}}]}],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 1, "type": "VEC3",
             "min": list(vertex), "max": list(vertex)},
            {"bufferView": 1, "componentType": 5126, "count": 1, "type": "VEC3"},
            {"bufferView": 2, "componentType": 5121, "count": 1, "type": "VEC4"},
            {"bufferView": 3, "componentType": 5126, "count": 1, "type": "VEC4"},
            {"bufferView": 4, "componentType": 5126, "count": 2, "type": "MAT4"},
        ],
        "bufferViews": views,
        "buffers": [{"byteLength": len(data), "uri": "model.bin"}],
    }
    path = Path(directory) / "model.gltf"
    path.write_text(json.dumps(gltf), encoding="utf-8")
    (Path(directory) / "model.bin").write_bytes(bytes(data))
    return path


def library(directory):
    lib = {
        "asset": {"version": "2.0"},
        "nodes": [
            {"name": "Armature", "children": [1]},
            {"name": "mixamorig:Hips", "translation": [0.0, 0.9, 0.0], "children": [2]},
            {"name": "mixamorig:LeftArm", "translation": [0.0, 0.3, 0.0], "rotation": STRAIGHT},
        ],
    }
    folder = Path(directory) / "lib"
    folder.mkdir()
    (folder / "library.gltf").write_text(json.dumps(lib), encoding="utf-8")
    return "lib/library.gltf"


class RestToLibraryTests(unittest.TestCase):
    def run_tool(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        path = character(directory.name, library(directory.name))
        matched, total, _ = TOOL.rest_to_library(path)
        gltf = json.loads(path.read_text(encoding="utf-8"))
        acc = TOOL.Accessors(gltf, bytearray(path.with_suffix(".bin").read_bytes()))
        return matched, total, gltf, acc

    def test_joints_take_the_library_rotation_and_keep_their_offsets(self):
        matched, total, gltf, _ = self.run_tool()

        self.assertEqual((matched, total), (2, 2))
        arm = gltf["nodes"][2]
        self.assertEqual(arm["rotation"], STRAIGHT)
        self.assertEqual(arm["translation"], ARM_T, "bone lengths stay the character's")
        self.assertEqual(gltf["nodes"][1]["translation"], HIPS_T)

    def test_the_mesh_follows_its_joint_into_the_new_rest(self):
        _, _, gltf, acc = self.run_tool()

        _, arm_w = world(STRAIGHT)
        expected = (arm_w @ np.append(VERTEX_IN_ARM, 1.0))[:3]
        np.testing.assert_allclose(acc.read(0)[0], expected, atol=1e-6)
        np.testing.assert_allclose(gltf["accessors"][0]["min"], expected, atol=1e-6)
        normal = acc.read(1)[0]
        np.testing.assert_allclose(np.linalg.norm(normal), 1.0, atol=1e-6)
        undo_bend = TOOL.quat_to_matrix(STRAIGHT) @ TOOL.quat_to_matrix(BENT).T
        np.testing.assert_allclose(normal, undo_bend @ [1.0, 0.0, 0.0], atol=1e-6,
                                   err_msg="the normal turns with the straightened arm")

    def test_inverse_binds_match_the_new_rest(self):
        _, _, _, acc = self.run_tool()

        binds = acc.read(4).reshape(-1, 4, 4).transpose(0, 2, 1)
        for bind, joint_world in zip(binds, world(STRAIGHT)):
            np.testing.assert_allclose(bind @ joint_world, np.eye(4), atol=1e-6)

    def test_a_model_without_a_library_is_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            path = character(directory, "unused")
            gltf = json.loads(path.read_text(encoding="utf-8"))
            del gltf["extras"]
            path.write_text(json.dumps(gltf), encoding="utf-8")
            with self.assertRaises(ValueError):
                TOOL.rest_to_library(path)


if __name__ == "__main__":
    unittest.main()
