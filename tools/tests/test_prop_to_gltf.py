"""Metadata tests for the Blender converter's post-export glTF pass."""

import importlib.util
import json
import sys
import tempfile
import types
import unittest
from pathlib import Path


def load_converter_module():
    # postprocess_gltf is pure JSON work, but the converter imports Blender APIs
    # for its actual mesh conversion. Stub those imports so this test runs in a
    # normal Python environment as well as Blender.
    sys.modules.setdefault("bpy", types.ModuleType("bpy"))
    mathutils = sys.modules.setdefault("mathutils", types.ModuleType("mathutils"))
    mathutils.Vector = getattr(mathutils, "Vector", tuple)

    script = Path(__file__).parents[1] / "prop_to_gltf.py"
    spec = importlib.util.spec_from_file_location("prop_to_gltf_test", script)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


CONVERTER = load_converter_module()


class PostprocessGltfMountMetadataTests(unittest.TestCase):
    def write_fixture(self):
        temp_dir = tempfile.TemporaryDirectory()
        path = Path(temp_dir.name) / "model.gltf"
        path.write_text(json.dumps({
            "nodes": [
                {"name": "MuzzleNode"},
                {"name": "WeaponMesh", "mesh": 0, "extras": {"keep": "value"}},
                {"name": "IgnoredMesh", "mesh": 1},
            ],
            "meshes": [{"primitives": []}, {"primitives": []}],
        }))
        return temp_dir, path

    def test_writes_mount_metadata_to_first_mesh_node_and_preserves_socket_extras(self):
        temp_dir, path = self.write_fixture()
        with temp_dir:
            CONVERTER.postprocess_gltf(
                path,
                sockets=["muzzle=MuzzleNode"],
                rotate_euler=[10.0, 20.0, 30.0],
                mount_axes=[0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            )
            result = json.loads(path.read_text())

        self.assertEqual(result["nodes"][0]["extras"]["socket"], "muzzle")
        self.assertEqual(result["nodes"][1]["extras"]["keep"], "value")
        self.assertEqual(
            result["nodes"][1]["extras"]["mount"],
            {
                "barrel": [0.0, 1.0, 0.0],
                "up": [0.0, 0.0, 1.0],
                "euler": [10.0, 20.0, 30.0],
            },
        )
        self.assertNotIn("extras", result["nodes"][2])

    def test_writes_zero_euler_when_axes_are_supplied_without_rotation(self):
        temp_dir, path = self.write_fixture()
        with temp_dir:
            CONVERTER.postprocess_gltf(
                path,
                mount_axes=[1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            )
            result = json.loads(path.read_text())

        self.assertEqual(
            result["nodes"][1]["extras"]["mount"]["euler"],
            [0, 0, 0],
        )

    def test_omitted_mount_axes_do_not_create_declared_mount_metadata(self):
        # Regression: geometric-assist rebakes must remain undeclared so later
        # checks cannot promote an axis guess to VERIFIED metadata.
        temp_dir, path = self.write_fixture()
        with temp_dir:
            CONVERTER.postprocess_gltf(
                path,
                rotate_euler=[10.0, 20.0, 30.0],
            )
            result = json.loads(path.read_text())

        self.assertEqual(result["nodes"][1]["extras"], {"keep": "value"})


if __name__ == "__main__":
    unittest.main()
