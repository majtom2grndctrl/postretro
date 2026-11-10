"""
Re-rest a skinned glTF character onto its clip library's rest pose.

A clip library stores each clip as a change from the library's rest pose, and
the engine applies that change on top of the character's own rest pose
(`docs/character-animation.md`). When the two rest poses differ — an A-pose
character on a T-pose library — every limb lands off by the difference, so
arms cross the body. This script gives each joint the library's rest
rotation, keeps the character's own bone lengths and offsets, and moves the
mesh into the new rest pose with its existing skin weights (linear blend
skinning, as Blender's "apply pose as rest pose" does). Inverse binds are
rewritten to match, so the character renders unchanged until a clip plays,
and then plays the library's clips as authored.

Joints match by exact name. Joints the library lacks keep their rotation.
Both documents must use the same bone convention (Mixamo rigs do: each child
offset lies along its parent's +Y); a mismatch shows as a distorted skeleton.

Usage (plain Python with numpy; no Blender):

    python3 tools/rest_to_library.py content/mymod/models/ghoul/model.gltf

The library is the one the model names in `extras.clipLibrary`. The model's
`.gltf` and `.bin` are rewritten in place.
"""

import argparse
import json
import struct
import sys
from pathlib import Path
from urllib.parse import unquote

import numpy as np

WIDTH = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4, "MAT4": 16}
FORMAT = {5126: ("f", 4), 5121: ("B", 1), 5123: ("H", 2)}


def quat_to_matrix(q):
    x, y, z, w = q
    return np.array([
        [1 - 2 * (y * y + z * z), 2 * (x * y - z * w), 2 * (x * z + y * w)],
        [2 * (x * y + z * w), 1 - 2 * (x * x + z * z), 2 * (y * z - x * w)],
        [2 * (x * z - y * w), 2 * (y * z + x * w), 1 - 2 * (x * x + y * y)],
    ])


def local_matrix(node):
    m = np.eye(4)
    m[:3, :3] = quat_to_matrix(node.get("rotation", [0, 0, 0, 1])) * np.array(node.get("scale", [1, 1, 1]))
    m[:3, 3] = node.get("translation", [0, 0, 0])
    return m


class Accessors:
    """Read and overwrite dense accessors of a single-buffer document."""

    def __init__(self, gltf, data):
        self.gltf, self.data = gltf, data

    def _layout(self, index):
        accessor = self.gltf["accessors"][index]
        if "sparse" in accessor:
            raise ValueError(f"accessor {index} is sparse")
        code, size = FORMAT[accessor["componentType"]]
        width = WIDTH[accessor["type"]]
        view = self.gltf["bufferViews"][accessor["bufferView"]]
        start = view.get("byteOffset", 0) + accessor.get("byteOffset", 0)
        stride = view.get("byteStride", size * width)
        return accessor, f"<{width}{code}", start, stride

    def read(self, index):
        accessor, fmt, start, stride = self._layout(index)
        rows = [struct.unpack_from(fmt, self.data, start + i * stride) for i in range(accessor["count"])]
        values = np.array(rows, dtype=np.float64)
        if accessor.get("normalized") and accessor["componentType"] != 5126:
            values /= 255.0 if accessor["componentType"] == 5121 else 65535.0
        return values

    def write(self, index, values):
        accessor, fmt, start, stride = self._layout(index)
        for i, row in enumerate(values):
            struct.pack_into(fmt, self.data, start + i * stride, *row)
        if "min" in accessor or "max" in accessor:
            accessor["min"] = values.min(axis=0).tolist()
            accessor["max"] = values.max(axis=0).tolist()


def node_parents(gltf):
    return {child: i for i, node in enumerate(gltf["nodes"]) for child in node.get("children", [])}


def world_matrices(gltf, locals_by_node):
    parents = node_parents(gltf)
    cache = {}

    def world(i):
        if i not in cache:
            parent = parents.get(i)
            cache[i] = locals_by_node[i] if parent is None else world(parent) @ locals_by_node[i]
        return cache[i]

    return world


def rest_to_library(model_path):
    model_path = Path(model_path)
    gltf = json.loads(model_path.read_text(encoding="utf-8"))
    library_ref = gltf.get("extras", {}).get("clipLibrary")
    if not library_ref:
        raise ValueError(f"{model_path} names no clip library")
    library = json.loads((model_path.parent / library_ref).read_text(encoding="utf-8"))
    library_rotations = {n["name"]: n.get("rotation", [0, 0, 0, 1]) for n in library["nodes"] if "name" in n}

    bin_path = model_path.parent / unquote(gltf["buffers"][0]["uri"])
    data = bytearray(bin_path.read_bytes())
    acc = Accessors(gltf, data)
    skin = gltf["skins"][0]
    joints = skin["joints"]

    # Old and new rest poses, as world matrices per node. Only joint rotations
    # change; non-joint ancestors keep their transforms.
    old_locals = {i: local_matrix(n) for i, n in enumerate(gltf["nodes"])}
    matched = 0
    for j in joints:
        node = gltf["nodes"][j]
        rotation = library_rotations.get(node.get("name"))
        if rotation is not None:
            node["rotation"] = list(rotation)
            matched += 1
    new_locals = {i: local_matrix(n) for i, n in enumerate(gltf["nodes"])}
    old_world, new_world = world_matrices(gltf, old_locals), world_matrices(gltf, new_locals)

    old_ibm = acc.read(skin["inverseBindMatrices"]).reshape(-1, 4, 4).transpose(0, 2, 1)
    # Each joint's move from the old bind to the new rest, in model space.
    deltas = np.array([new_world(j) @ old_ibm[k] for k, j in enumerate(joints)])

    for mesh in gltf["meshes"]:
        for primitive in mesh["primitives"]:
            attributes = primitive["attributes"]
            weights = acc.read(attributes["WEIGHTS_0"])
            indices = acc.read(attributes["JOINTS_0"]).astype(int)
            skinning = np.einsum("vk,vkij->vij", weights, deltas[indices])
            positions = acc.read(attributes["POSITION"])
            moved = np.einsum("vij,vj->vi", skinning[:, :3, :3], positions) + skinning[:, :3, 3]
            acc.write(attributes["POSITION"], moved)
            if "NORMAL" in attributes:
                normals = np.einsum("vij,vj->vi", skinning[:, :3, :3], acc.read(attributes["NORMAL"]))
                normals /= np.linalg.norm(normals, axis=1, keepdims=True)
                acc.write(attributes["NORMAL"], normals)
            if "TANGENT" in attributes:
                del attributes["TANGENT"]  # stale after the move; the loader defaults them

    new_ibm = np.array([np.linalg.inv(new_world(j)) for j in joints])
    acc.write(skin["inverseBindMatrices"], new_ibm.transpose(0, 2, 1).reshape(-1, 16))

    bin_path.write_bytes(bytes(data))
    with open(model_path, "w", encoding="utf-8", newline="\n") as handle:
        json.dump(gltf, handle, indent="\t", ensure_ascii=False)
    old_root = old_world(joints[0])[:3, 3]
    return matched, len(joints), old_root


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("model", help="skinned .gltf bound to a clip library")
    args = parser.parse_args(argv)
    try:
        matched, total, _ = rest_to_library(args.model)
    except (ValueError, OSError, KeyError) as exc:
        print(f"ERROR: {exc}")
        return 1
    print(f"Re-rested {matched} of {total} joints onto the library's rest pose")
    return 0


if __name__ == "__main__":
    sys.exit(main())
