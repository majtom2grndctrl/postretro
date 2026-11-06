"""Tests for the Mixamo converter's Blender-free logic: the post-export pass and
bone-prefix matching."""

import copy
import importlib.util
import json
import sys
import tempfile
import types
import unittest
from pathlib import Path
from unittest import mock


def load_converter_module():
    # The post-export pass is pure JSON and bytes work, but the converter imports
    # Blender APIs for its FBX handling. Stub those so this runs under plain Python.
    sys.modules.setdefault("bpy", types.ModuleType("bpy"))
    mathutils = sys.modules.setdefault("mathutils", types.ModuleType("mathutils"))
    mathutils.Vector = getattr(mathutils, "Vector", tuple)
    mathutils.Matrix = getattr(mathutils, "Matrix", tuple)

    script = Path(__file__).parents[1] / "mixamo_to_gltf.py"
    spec = importlib.util.spec_from_file_location("mixamo_to_gltf_test", script)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


CONVERTER = load_converter_module()

LIBRARY = "../clip_libraries/mixamo_rifle/library.gltf"

# Each view's bytes are unique, so a survivor can be identified after compaction.
POSITIONS = b"POSITIONS---"
INDICES = b"INDXXX"  # 6 bytes: the next view needs padding to stay aligned.
CLIP_TIMES = b"CLIPTIMES-------"
BIND_MATRICES = b"INVERSEBINDS"
CLIP_ROTATIONS = b"CLIPROTATIONS---"
SHARED_TIMES = b"SHAREDTIMES-"
EMBEDDED_IMAGE = b"IMAGEBYTES"


def fake_document(prefix="mixamorig:"):
    """Build a skinned model whose clip data interleaves its mesh data.

    Accessor and view indices are deliberately not grouped, so a pass that drops
    clip entries without rewriting survivors reads the wrong data.
    """
    chunks = [POSITIONS, INDICES, CLIP_TIMES, BIND_MATRICES, CLIP_ROTATIONS,
              SHARED_TIMES, EMBEDDED_IMAGE]
    data = bytearray()
    views = []
    for chunk in chunks:
        data.extend(bytes(-len(data) % 4))
        views.append({"buffer": 0, "byteOffset": len(data), "byteLength": len(chunk)})
        data.extend(chunk)
    gltf = {
        "asset": {"version": "2.0"},
        "nodes": [
            {"name": "Armature", "children": [1, 4]},
            {"name": f"{prefix}Hips", "children": [2, 3]},
            {"name": f"{prefix}RightHand"},
            {"name": f"{prefix}Head", "extras": {"keep": "value"}},
            {"name": "Body", "mesh": 0, "skin": 0},
        ],
        "meshes": [{"primitives": [{
            "attributes": {"POSITION": 0, "TEXCOORD_0": 5},
            "indices": 1,
            "targets": [{"POSITION": 0}],
        }]}],
        "skins": [{"inverseBindMatrices": 3, "joints": [1, 2, 3]}],
        "images": [{"bufferView": 6, "mimeType": "image/png"}],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 1, "type": "VEC3"},
            {"bufferView": 1, "componentType": 5123, "count": 3, "type": "SCALAR"},
            {"bufferView": 2, "componentType": 5126, "count": 4, "type": "SCALAR"},
            {"bufferView": 3, "componentType": 5126, "count": 1, "type": "MAT4"},
            {"bufferView": 4, "componentType": 5126, "count": 1, "type": "VEC4"},
            {"bufferView": 5, "componentType": 5126, "count": 3, "type": "SCALAR"},
        ],
        "bufferViews": views,
        "buffers": [{"byteLength": len(data), "uri": "model.bin"}],
        "animations": [
            {
                "name": "walk",
                "channels": [{"sampler": 0, "target": {"node": 1, "path": "rotation"}}],
                "samplers": [{"input": 2, "output": 4}],
            },
            {
                # Accessor 5 is also the mesh's TEXCOORD_0, so it must survive.
                "name": "aim",
                "channels": [{"sampler": 0, "target": {"node": 1, "path": "rotation"}}],
                "samplers": [{"input": 5, "output": 4}],
            },
        ],
    }
    return gltf, bytes(data)


def accessor_bytes(gltf, data, index):
    view = gltf["bufferViews"][gltf["accessors"][index]["bufferView"]]
    start = view.get("byteOffset", 0)
    return data[start:start + view["byteLength"]]


def write_model(directory, gltf, data):
    path = Path(directory) / "model.gltf"
    path.write_text(json.dumps(gltf), encoding="utf-8")
    (Path(directory) / "model.bin").write_bytes(data)
    return path


class StripClipsTests(unittest.TestCase):
    def strip(self):
        gltf, data = fake_document()
        return gltf, CONVERTER.strip_clips(gltf, data)

    def test_removes_animations_and_bytes_only_clips_used(self):
        gltf, data = self.strip()

        self.assertNotIn("animations", gltf)
        self.assertEqual(len(gltf["accessors"]), 4)
        self.assertEqual(len(gltf["bufferViews"]), 5)
        self.assertNotIn(CLIP_TIMES, data)
        self.assertNotIn(CLIP_ROTATIONS, data)

    def test_buffer_holds_only_surviving_views_and_their_padding(self):
        gltf, data = self.strip()

        self.assertEqual(gltf["buffers"][0]["byteLength"], len(data))
        padded = [POSITIONS, INDICES + bytes(2), BIND_MATRICES, SHARED_TIMES, EMBEDDED_IMAGE]
        self.assertEqual(data, b"".join(padded))

    def test_mesh_and_skin_read_their_own_bytes_through_rewritten_indices(self):
        gltf, data = self.strip()
        primitive = gltf["meshes"][0]["primitives"][0]

        self.assertEqual(accessor_bytes(gltf, data, primitive["attributes"]["POSITION"]), POSITIONS)
        self.assertEqual(accessor_bytes(gltf, data, primitive["targets"][0]["POSITION"]), POSITIONS)
        self.assertEqual(accessor_bytes(gltf, data, primitive["indices"]), INDICES)
        self.assertEqual(
            accessor_bytes(gltf, data, gltf["skins"][0]["inverseBindMatrices"]),
            BIND_MATRICES,
        )

    def test_accessor_shared_with_a_clip_survives(self):
        gltf, data = self.strip()
        texcoord = gltf["meshes"][0]["primitives"][0]["attributes"]["TEXCOORD_0"]

        self.assertEqual(accessor_bytes(gltf, data, texcoord), SHARED_TIMES)

    def test_view_shared_by_a_clip_accessor_and_a_mesh_accessor_survives(self):
        gltf, data = fake_document()
        gltf["accessors"][2]["bufferView"] = 0

        stripped = CONVERTER.strip_clips(gltf, data)

        position = gltf["meshes"][0]["primitives"][0]["attributes"]["POSITION"]
        self.assertEqual(accessor_bytes(gltf, stripped, position), POSITIONS)

    def test_embedded_image_view_is_rewritten(self):
        gltf, data = self.strip()
        view = gltf["bufferViews"][gltf["images"][0]["bufferView"]]

        self.assertEqual(
            data[view["byteOffset"]:view["byteOffset"] + view["byteLength"]],
            EMBEDDED_IMAGE,
        )

    def test_sparse_accessor_views_survive_and_are_rewritten(self):
        gltf, data = fake_document()
        gltf["accessors"][0]["sparse"] = {
            "count": 1,
            "indices": {"bufferView": 1, "componentType": 5123},
            "values": {"bufferView": 3},
        }

        stripped = CONVERTER.strip_clips(gltf, data)

        sparse = gltf["accessors"][0]["sparse"]
        for slot, expected in ((sparse["indices"], INDICES), (sparse["values"], BIND_MATRICES)):
            view = gltf["bufferViews"][slot["bufferView"]]
            start = view["byteOffset"]
            self.assertEqual(stripped[start:start + view["byteLength"]], expected)

    def test_doomed_sparse_accessor_frees_only_views_nothing_else_reads(self):
        gltf, data = fake_document()
        rotations = gltf["accessors"][4]
        del rotations["bufferView"]
        rotations["sparse"] = {
            "count": 1,
            "indices": {"bufferView": 3, "componentType": 5123},
            "values": {"bufferView": 4},
        }

        stripped = CONVERTER.strip_clips(gltf, data)

        self.assertNotIn(CLIP_ROTATIONS, stripped)
        self.assertEqual(
            accessor_bytes(gltf, stripped, gltf["skins"][0]["inverseBindMatrices"]),
            BIND_MATRICES,
        )

    def test_refuses_extensions_that_hide_accessor_references(self):
        for extension in ("EXT_mesh_gpu_instancing", "KHR_draco_mesh_compression"):
            with self.subTest(extension=extension):
                gltf, data = fake_document()
                gltf["extensionsUsed"] = ["KHR_materials_unlit", extension]

                with self.assertRaisesRegex(ValueError, extension):
                    CONVERTER.strip_clips(gltf, data)

    def test_other_extensions_are_allowed(self):
        gltf, data = fake_document()
        gltf["extensionsUsed"] = ["KHR_materials_unlit"]

        self.assertNotIn(CLIP_ROTATIONS, CONVERTER.strip_clips(gltf, data))

    def test_accessor_offset_in_a_misaligned_view_stays_aligned(self):
        kept = b"xxKEPT--"
        gltf = {
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 1, "type": "SCALAR"},
                {"bufferView": 1, "byteOffset": 2, "componentType": 5126, "count": 1,
                 "type": "SCALAR"},
            ],
            "bufferViews": [
                {"buffer": 0, "byteOffset": 0, "byteLength": 6},
                {"buffer": 0, "byteOffset": 6, "byteLength": len(kept)},
            ],
            "buffers": [{"byteLength": 6 + len(kept), "uri": "model.bin"}],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 1}}]}],
            "animations": [{"samplers": [{"input": 0, "output": 0}]}],
        }

        stripped = CONVERTER.strip_clips(gltf, b"DOOMED" + kept)

        view = gltf["bufferViews"][0]
        self.assertEqual((view["byteOffset"] + gltf["accessors"][0]["byteOffset"]) % 4, 0)
        self.assertEqual(stripped[view["byteOffset"]:], kept)

    def test_views_stay_four_byte_aligned(self):
        gltf, _ = self.strip()

        for view in gltf["bufferViews"]:
            self.assertEqual(view["byteOffset"] % 4, 0)

    def test_nodes_mesh_skin_and_images_are_otherwise_untouched(self):
        gltf, _ = self.strip()
        original, _ = fake_document()

        self.assertEqual(gltf["nodes"], original["nodes"])
        self.assertEqual(gltf["skins"][0]["joints"], original["skins"][0]["joints"])
        self.assertEqual(gltf["images"][0]["mimeType"], "image/png")

    def test_second_strip_changes_nothing(self):
        gltf, data = self.strip()
        before = copy.deepcopy(gltf)

        self.assertEqual(CONVERTER.strip_clips(gltf, data), data)
        self.assertEqual(gltf, before)

    def test_refuses_a_buffer_that_does_not_match_the_document(self):
        gltf, data = fake_document()

        with self.assertRaisesRegex(ValueError, "declares"):
            CONVERTER.strip_clips(gltf, data[:-1])

    def test_refuses_more_than_one_buffer(self):
        gltf, data = fake_document()
        gltf["buffers"].append({"byteLength": 0})

        with self.assertRaisesRegex(ValueError, "one buffer"):
            CONVERTER.strip_clips(gltf, data)


class StripSkinsTests(unittest.TestCase):
    def test_library_keeps_joints_and_clips_but_no_skin_data(self):
        gltf, data = fake_document()
        del gltf["meshes"]
        del gltf["images"]
        gltf["nodes"].pop()
        gltf["nodes"][0]["children"] = [1]

        stripped = CONVERTER.strip_skins(gltf, data)

        self.assertNotIn("skins", gltf)
        self.assertNotIn(BIND_MATRICES, stripped)
        self.assertEqual([node["name"] for node in gltf["nodes"]][1],
                         "mixamorig:Hips")
        walk, aim = gltf["animations"]
        self.assertEqual(accessor_bytes(gltf, stripped, walk["samplers"][0]["input"]), CLIP_TIMES)
        self.assertEqual(accessor_bytes(gltf, stripped, walk["samplers"][0]["output"]), CLIP_ROTATIONS)
        self.assertEqual(accessor_bytes(gltf, stripped, aim["samplers"][0]["input"]), SHARED_TIMES)

    def test_clears_skin_references_from_nodes(self):
        gltf, data = fake_document()

        CONVERTER.strip_skins(gltf, data)

        self.assertNotIn("skin", gltf["nodes"][4])


class ClipLibraryStampTests(unittest.TestCase):
    def test_stamps_a_document_with_no_extras(self):
        gltf, _ = fake_document()

        CONVERTER.stamp_clip_library(gltf, LIBRARY)

        self.assertEqual(gltf["extras"], {"clipLibrary": LIBRARY})

    def test_preserves_existing_extras_keys(self):
        gltf, _ = fake_document()
        gltf["extras"] = {"tags": ["enemy"]}

        CONVERTER.stamp_clip_library(gltf, LIBRARY)

        self.assertEqual(gltf["extras"], {"tags": ["enemy"], "clipLibrary": LIBRARY})

    def test_refuses_extras_that_are_not_an_object(self):
        gltf, _ = fake_document()
        gltf["extras"] = "note"

        with self.assertRaisesRegex(ValueError, "extras"):
            CONVERTER.stamp_clip_library(gltf, LIBRARY)

    def test_rejects_a_reference_that_does_not_decode_to_utf8(self):
        with self.assertRaises(ValueError):
            CONVERTER.check_clip_library_path("library%FF.gltf")

    def test_accepts_relative_paths_including_parent_segments(self):
        for value in ("library.gltf", "../lib/library.gltf", "a%20b/library.gltf"):
            with self.subTest(value=value):
                CONVERTER.check_clip_library_path(value)

    def test_rejects_values_the_engine_reads_as_malformed(self):
        for value in ("", "/lib.gltf", "data:x", "file:lib.gltf", "http://x/lib.gltf",
                      "C:lib.gltf", "C%3Alib.gltf", "%2Flib.gltf", "..\\lib.gltf", None):
            with self.subTest(value=value), self.assertRaises(ValueError):
                CONVERTER.check_clip_library_path(value)


class BindExistingModelTests(unittest.TestCase):
    def bind(self, gltf, data):
        temp_dir = tempfile.TemporaryDirectory()
        self.addCleanup(temp_dir.cleanup)
        path = write_model(temp_dir.name, gltf, data)
        CONVERTER.bind_gltf_to_clip_library(path, LIBRARY)
        return path

    def test_strips_stamps_and_compacts_on_disk(self):
        gltf, data = fake_document()
        path = self.bind(gltf, data)

        result = json.loads(path.read_bytes().decode("utf-8"))
        written = path.with_suffix(".bin").read_bytes()
        self.assertNotIn("animations", result)
        self.assertEqual(result["extras"]["clipLibrary"], LIBRARY)
        self.assertEqual(result["buffers"][0]["byteLength"], len(written))
        self.assertNotIn(CLIP_ROTATIONS, written)

    def test_leaves_a_committed_models_tags_alone(self):
        gltf, data = fake_document()
        gltf["nodes"][3]["extras"]["hitZone"] = "custom"
        path = self.bind(gltf, data)

        result = json.loads(path.read_text(encoding="utf-8"))
        self.assertEqual(result["nodes"][3]["extras"], {"keep": "value", "hitZone": "custom"})
        self.assertNotIn("extras", result["nodes"][2])
        self.assertNotIn("skeleton", result["skins"][0])

    def test_tags_a_numbered_rig_the_old_tool_left_untagged(self):
        gltf, data = fake_document("mixamorig1:")
        path = self.bind(gltf, data)

        result = json.loads(path.read_text(encoding="utf-8"))
        self.assertEqual(result["nodes"][1]["name"], "mixamorig:Hips")
        self.assertEqual(result["nodes"][2]["extras"]["socket"], "hand_r")
        self.assertEqual(result["nodes"][3]["extras"]["hitZone"], "head")
        self.assertEqual(result["nodes"][3]["extras"]["keep"], "value")
        self.assertEqual(result["skins"][0]["skeleton"], 1)

    def test_tags_a_canonical_rig_with_no_engine_tags(self):
        gltf, data = fake_document()
        path = self.bind(gltf, data)

        result = json.loads(path.read_text(encoding="utf-8"))
        self.assertEqual(result["nodes"][2]["extras"]["socket"], "hand_r")
        self.assertEqual(result["skins"][0]["skeleton"], 1)

    def test_writes_utf8_without_bom_with_tabs_and_lf(self):
        gltf, data = fake_document()
        gltf["nodes"][0]["name"] = "Armatüre"
        raw = self.bind(gltf, data).read_bytes()

        self.assertFalse(raw.startswith(b"\xef\xbb\xbf"))
        self.assertIn("Armatüre".encode("utf-8"), raw)
        self.assertIn(b"\n\t", raw)
        self.assertNotIn(b"\r", raw)

    def test_percent_encoded_buffer_uri_resolves(self):
        gltf, data = fake_document()
        gltf["buffers"][0]["uri"] = "my%20model.bin"
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "model.gltf"
            path.write_text(json.dumps(gltf), encoding="utf-8")
            bin_path = Path(directory) / "my model.bin"
            bin_path.write_bytes(data)

            CONVERTER.bind_gltf_to_clip_library(path, LIBRARY)

            self.assertNotIn(CLIP_TIMES, bin_path.read_bytes())

    def test_embedded_buffer_is_refused_and_nothing_is_written(self):
        gltf, data = fake_document()
        gltf["buffers"][0]["uri"] = "data:application/octet-stream;base64,AAAA"
        with tempfile.TemporaryDirectory() as directory:
            path = write_model(directory, gltf, data)
            original = path.read_bytes()

            with self.assertRaisesRegex(ValueError, "external buffer"):
                CONVERTER.bind_gltf_to_clip_library(path, LIBRARY)

            self.assertEqual(path.read_bytes(), original)


class PostprocessTests(unittest.TestCase):
    def run_pass(self, prefix="mixamorig:", **kwargs):
        gltf, data = fake_document(prefix)
        with tempfile.TemporaryDirectory() as directory:
            path = write_model(directory, gltf, data)
            CONVERTER.postprocess_gltf(path, **kwargs)
            return json.loads(path.read_text(encoding="utf-8"))

    def test_character_pass_normalizes_tags_strips_and_stamps(self):
        result = self.run_pass("mixamorig1:", clip_library=LIBRARY)

        self.assertEqual(result["nodes"][1]["name"], "mixamorig:Hips")
        self.assertEqual(result["nodes"][2]["extras"]["socket"], "hand_r")
        self.assertEqual(result["skins"][0]["skeleton"], 1)
        self.assertNotIn("animations", result)
        self.assertEqual(result["extras"]["clipLibrary"], LIBRARY)

    def test_numbered_rig_matches_canonical_rig(self):
        for tag in (True, False):
            with self.subTest(tag=tag):
                self.assertEqual(
                    self.run_pass("mixamorig12:", tag=tag),
                    self.run_pass("mixamorig:", tag=tag),
                )

    def test_tagging_off_still_normalizes_and_stamps(self):
        result = self.run_pass("mixamorig1:", tag=False, clip_library=LIBRARY)

        self.assertEqual(result["nodes"][1]["name"], "mixamorig:Hips")
        self.assertEqual(result["extras"]["clipLibrary"], LIBRARY)
        self.assertNotIn("extras", result["nodes"][2])

    def test_without_a_library_clips_stay(self):
        result = self.run_pass()

        self.assertEqual(len(result["animations"]), 2)
        self.assertNotIn("extras", result)

    def test_library_pass_drops_skin_and_keeps_clips(self):
        gltf, data = fake_document("mixamorig1:")
        del gltf["meshes"]
        del gltf["images"]
        gltf["nodes"].pop()
        gltf["nodes"][0]["children"] = [1]
        with tempfile.TemporaryDirectory() as directory:
            path = write_model(directory, gltf, data)
            CONVERTER.postprocess_gltf(path, tag=False, library=True)
            result = json.loads(path.read_text(encoding="utf-8"))
            written = path.with_suffix(".bin").read_bytes()

        self.assertNotIn("skins", result)
        self.assertEqual([a["name"] for a in result["animations"]], ["walk", "aim"])
        self.assertEqual(result["nodes"][1]["name"], "mixamorig:Hips")
        self.assertIn(CLIP_ROTATIONS, written)

    def test_library_pass_refuses_a_mesh_or_no_clips(self):
        gltf, data = fake_document()
        no_clips = copy.deepcopy(gltf)
        del no_clips["meshes"]
        del no_clips["animations"]
        for document, message in ((gltf, "mesh"), (no_clips, "no clips")):
            with self.subTest(message=message), tempfile.TemporaryDirectory() as directory:
                path = write_model(directory, document, data)
                with self.assertRaisesRegex(ValueError, message):
                    CONVERTER.postprocess_gltf(path, tag=False, library=True)


class NumberedPrefixTests(unittest.TestCase):
    def test_canonical_bone_name(self):
        cases = {
            "mixamorig1:Hips": "mixamorig:Hips",
            "mixamorig12:LeftHand": "mixamorig:LeftHand",
            "mixamorig:Hips": "mixamorig:Hips",
            "mixamorigX:Hips": "mixamorigX:Hips",
            "Armature": "Armature",
        }
        for name, expected in cases.items():
            with self.subTest(name=name):
                self.assertEqual(CONVERTER.canonical_bone_name(name), expected)


class StripFacialBonesTests(unittest.TestCase):
    def test_finds_head_on_a_numbered_rig(self):
        def bone(name, children=()):
            return types.SimpleNamespace(name=name, children_recursive=list(children))

        jaw = bone("mixamorig7:Jaw")
        head_top = bone("mixamorig7:HeadTop_End")
        head = bone("mixamorig7:Head", [jaw, head_top])
        edit_bones = {b.name: types.SimpleNamespace() for b in (jaw, head_top, head)}
        armature = types.SimpleNamespace(
            type="ARMATURE",
            data=types.SimpleNamespace(bones=[head, jaw, head_top], edit_bones=edit_bones),
        )
        bpy = mock.MagicMock()
        bpy.data.objects = [armature]

        with mock.patch.object(CONVERTER, "bpy", bpy):
            CONVERTER.strip_facial_bones()

        self.assertTrue(edit_bones["mixamorig7:Jaw"].select)
        self.assertFalse(hasattr(edit_bones["mixamorig7:HeadTop_End"], "select"))
        self.assertFalse(hasattr(edit_bones["mixamorig7:Head"], "select"))
        bpy.ops.armature.delete.assert_called_once()


class ResetPoseTests(unittest.TestCase):
    def test_clears_every_pose_bone(self):
        bones = [types.SimpleNamespace(matrix_basis="old pose") for _ in range(2)]
        armature = types.SimpleNamespace(pose=types.SimpleNamespace(bones=bones))
        matrix = types.SimpleNamespace(Identity=lambda size: ("identity", size))

        with mock.patch.object(CONVERTER, "Matrix", matrix):
            CONVERTER.reset_pose_to_rest(armature)

        self.assertEqual([b.matrix_basis for b in bones], [("identity", 4)] * 2)


class StaleOutputTests(unittest.TestCase):
    def setUp(self):
        temp_dir = tempfile.TemporaryDirectory()
        self.addCleanup(temp_dir.cleanup)
        self.directory = Path(temp_dir.name) / "models"
        self.directory.mkdir()

    def touch(self, *names):
        for name in names:
            (self.directory / name).write_bytes(b"old")

    def names(self):
        return sorted(path.name for path in self.directory.iterdir())

    def export(self, output, **kwargs):
        with mock.patch.object(CONVERTER, "bpy", mock.MagicMock()):
            CONVERTER.export_gltf(self.directory / output, **kwargs)

    def write_old_document(self, output, images):
        document = {"images": images}
        (self.directory / output).write_text(json.dumps(document), encoding="utf-8")

    def test_binding_keeps_the_library_beside_the_character(self):
        self.touch("library.gltf", "library.bin", "model.bin", "model_base.png", "other.png")
        self.write_old_document("model.gltf", [{"uri": "model_base.png"}])

        self.export("model.gltf", keep_siblings=True)

        self.assertEqual(self.names(), ["library.bin", "library.gltf", "other.png"])

    def test_library_keeps_a_character_beside_it(self):
        self.touch("library.bin", "model.gltf", "model.bin", "model_base.png")
        self.write_old_document("library.gltf", [])

        self.export("library.gltf", keep_siblings=True)

        self.assertEqual(self.names(), ["model.bin", "model.gltf", "model_base.png"])

    def test_old_image_outside_the_folder_or_embedded_stays(self):
        outside = self.directory.parent / "shared.png"
        outside.write_bytes(b"old")
        self.touch("a b.png")
        self.write_old_document("model.gltf", [
            {"uri": "../shared.png"},
            {"uri": "data:image/png;base64,AAAA"},
            {"bufferView": 0},
            {"uri": "a%20b.png"},
            {"uri": "a%20b.png"},
        ])

        self.export("model.gltf", keep_siblings=True)

        self.assertTrue(outside.exists())
        self.assertEqual(self.names(), [])

    def test_unreadable_old_document_keeps_images(self):
        self.touch("model.gltf", "model.bin", "model_base.png")

        self.export("model.gltf", keep_siblings=True)

        self.assertEqual(self.names(), ["model_base.png"])

    def test_plain_conversion_still_clears_the_folder(self):
        self.touch("old.gltf", "old.glb", "old.bin", "old.png", "old.jpg", "old.jpeg",
                   "license.txt")

        self.export("model.gltf")

        self.assertEqual(self.names(), ["license.txt"])

    def test_missing_folder_is_created(self):
        with mock.patch.object(CONVERTER, "bpy", mock.MagicMock()):
            CONVERTER.export_gltf(self.directory / "new" / "model.gltf", keep_siblings=True)

        self.assertTrue((self.directory / "new").is_dir())


class MainTests(unittest.TestCase):
    def run_main(self, gltf, data, *flags, delete_bin=False):
        """Run main() with Blender's stages stubbed and the export already on disk."""
        with tempfile.TemporaryDirectory() as directory:
            path = write_model(directory, gltf, data)
            if delete_bin:
                path.with_suffix(".bin").unlink()
            argv = ["blender", "--", "--input-dir", "clips", "--base", "rig.fbx",
                    "--output", str(path), *flags]
            with mock.patch.object(sys, "argv", argv), \
                    mock.patch.object(CONVERTER, "clean_scene"), \
                    mock.patch.object(CONVERTER, "merge_animations"), \
                    mock.patch.object(CONVERTER, "validate_model"), \
                    mock.patch.object(CONVERTER, "export_gltf") as self.export:
                CONVERTER.main()
            return json.loads(path.read_text(encoding="utf-8"))

    def library_document(self):
        gltf, data = fake_document()
        del gltf["meshes"]
        del gltf["images"]
        gltf["nodes"].pop()
        gltf["nodes"][0]["children"] = [1]
        return gltf, data

    def test_library_is_never_tagged(self):
        for flags in (("--library",), ("--library", "--no-tag")):
            with self.subTest(flags=flags):
                gltf, data = self.library_document()

                result = self.run_main(gltf, data, *flags)

                self.assertFalse(CONVERTER.has_engine_tags(result))
                self.assertEqual(result["nodes"][3]["extras"], {"keep": "value"})

    def test_character_is_tagged(self):
        gltf, data = fake_document()

        result = self.run_main(gltf, data, "--clip-library", LIBRARY)

        self.assertEqual(result["nodes"][2]["extras"]["socket"], "hand_r")

    def test_no_tag_binding_stays_untagged(self):
        gltf, data = fake_document("mixamorig1:")

        result = self.run_main(gltf, data, "--no-tag", "--clip-library", LIBRARY)

        self.assertFalse(CONVERTER.has_engine_tags(result))
        self.assertEqual(result["extras"]["clipLibrary"], LIBRARY)

    def test_only_library_modes_keep_siblings(self):
        for flags, expected in (((), False), (("--library",), True),
                                (("--clip-library", LIBRARY), True)):
            with self.subTest(flags=flags):
                gltf, data = fake_document()
                del gltf["meshes"]
                self.run_main(gltf, data, *flags)

                self.assertEqual(self.export.call_args.kwargs["keep_siblings"], expected)

    def test_missing_buffer_exits_nonzero(self):
        gltf, data = fake_document()

        with self.assertRaises(SystemExit) as caught:
            self.run_main(gltf, data, "--clip-library", LIBRARY, delete_bin=True)

        self.assertEqual(caught.exception.code, 1)

    def test_refused_document_exits_nonzero(self):
        gltf, data = fake_document()

        with self.assertRaises(SystemExit) as caught:
            self.run_main(gltf, data, "--library")

        self.assertEqual(caught.exception.code, 1)


def fcurve(bone, channel="rotation_quaternion"):
    return types.SimpleNamespace(data_path=f'pose.bones["{bone}"].{channel}')


class RetargetFcurveBonesTests(unittest.TestCase):
    def test_numbered_clip_on_canonical_rig_keeps_its_clip(self):
        curves = [fcurve("mixamorig1:Hips", "location"), fcurve("mixamorig1:Spine")]

        matched, orphaned = CONVERTER.retarget_fcurve_bones(
            curves, {"mixamorig:Hips", "mixamorig:Spine"})

        self.assertTrue(matched)
        self.assertEqual(orphaned, [])
        self.assertEqual(
            [curve.data_path for curve in curves],
            ['pose.bones["mixamorig:Hips"].location',
             'pose.bones["mixamorig:Spine"].rotation_quaternion'],
        )

    def test_canonical_clip_on_numbered_rig_targets_the_rigs_names(self):
        curves = [fcurve("mixamorig:Hips")]

        matched, _ = CONVERTER.retarget_fcurve_bones(curves, {"mixamorig4:Hips"})

        self.assertTrue(matched)
        self.assertEqual(curves[0].data_path,
                         'pose.bones["mixamorig4:Hips"].rotation_quaternion')

    def test_differently_numbered_clip_and_rig_match(self):
        curves = [fcurve("mixamorig2:Hips")]

        matched, _ = CONVERTER.retarget_fcurve_bones(curves, {"mixamorig4:Hips"})

        self.assertTrue(matched)
        self.assertEqual(curves[0].data_path,
                         'pose.bones["mixamorig4:Hips"].rotation_quaternion')

    def test_exact_match_is_left_alone(self):
        curves = [fcurve("mixamorig:Hips")]
        before = curves[0].data_path

        matched, orphaned = CONVERTER.retarget_fcurve_bones(curves, {"mixamorig:Hips"})

        self.assertTrue(matched)
        self.assertEqual(orphaned, [])
        self.assertEqual(curves[0].data_path, before)

    def test_unknown_bone_is_orphaned_and_unchanged(self):
        curves = [fcurve("mixamorig1:Hips"), fcurve("mixamorig1:Tail")]

        matched, orphaned = CONVERTER.retarget_fcurve_bones(curves, {"mixamorig:Hips"})

        self.assertTrue(matched)
        self.assertEqual(orphaned, [curves[1].data_path])
        self.assertIn("mixamorig1:Tail", curves[1].data_path)

    def test_clip_with_no_base_bone_does_not_match(self):
        matched, _ = CONVERTER.retarget_fcurve_bones(
            [fcurve("MCH_spine")], {"mixamorig:Hips"})

        self.assertFalse(matched)

    def test_object_level_channels_are_ignored(self):
        curve = types.SimpleNamespace(data_path="location")

        matched, orphaned = CONVERTER.retarget_fcurve_bones([curve], {"mixamorig:Hips"})

        self.assertFalse(matched)
        self.assertEqual(orphaned, [])
        self.assertEqual(curve.data_path, "location")


class ConverterArgumentTests(unittest.TestCase):
    def parse(self, *args):
        old_argv = sys.argv
        try:
            sys.argv = ["blender", "--", "--input-dir", "clips", "--output", "out.gltf", *args]
            return CONVERTER.parse_args()
        finally:
            sys.argv = old_argv

    def test_authoring_surface_commands_parse(self):
        library = self.parse("--base", "rig.fbx", "--library")
        character = self.parse("--base", "rig.fbx", "--clip-library", LIBRARY)

        self.assertTrue(library.library)
        self.assertIsNone(library.clip_library)
        self.assertFalse(character.library)
        self.assertEqual(character.clip_library, LIBRARY)

    def test_rejects_library_with_clip_library(self):
        with self.assertRaises(SystemExit):
            self.parse("--base", "rig.fbx", "--library", "--clip-library", LIBRARY)

    def test_library_requires_a_base(self):
        with self.assertRaises(SystemExit):
            self.parse("--library")

    def test_rejects_a_malformed_clip_library_path(self):
        with self.assertRaises(SystemExit):
            self.parse("--clip-library", "file:library.gltf")

    def test_rejects_a_clip_library_that_is_the_output(self):
        for value in ("out.gltf", "./out.gltf", "x/../out.gltf", "%6Fut.gltf"):
            with self.subTest(value=value), self.assertRaises(SystemExit):
                self.parse("--clip-library", value)

    def test_accepts_a_library_beside_the_output(self):
        self.assertEqual(self.parse("--clip-library", "library.gltf").clip_library,
                         "library.gltf")


if __name__ == "__main__":
    unittest.main()
