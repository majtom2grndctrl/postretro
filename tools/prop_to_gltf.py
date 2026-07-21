"""
Headless Blender script: convert a downloaded prop/weapon model to engine glTF.

Usage:
    blender --background --python tools/prop_to_gltf.py -- \
        --input path/to/model.glb \
        --output path/to/output/model.gltf \
        [--grip 0.0 -0.05 0.12] \
        [--scale 0.01] \
        [--socket muzzle=BarrelTip] \
        [--socket optic_rail=ScopeMount]

Imports the model (glTF, GLB, FBX, or OBJ), joins all mesh objects into a
single mesh node, relocates the origin to the grip point, strips tangents,
validates against the engine model contract, and exports glTF Separate.

After export the script post-processes the .gltf JSON: removes extensions
the engine rejects from extensionsRequired, and optionally adds socket
extras tags for named attachment points on child nodes.

Output is glTF Separate (.gltf + .bin + textures) ready for the engine.
"""

import bpy
import sys
import json
import argparse
from pathlib import Path
from mathutils import Vector


REJECTED_EXTENSIONS = {
    "KHR_materials_pbrSpecularGlossiness",
}


def parse_args():
    argv = sys.argv
    if "--" in argv:
        argv = argv[argv.index("--") + 1:]
    else:
        argv = []

    parser = argparse.ArgumentParser(
        description="Convert a prop/weapon model to engine-ready glTF"
    )
    parser.add_argument(
        "--input", required=True,
        help="Path to the source model file (.gltf, .glb, .fbx, .obj)"
    )
    parser.add_argument(
        "--output", required=True,
        help="Output path for the .gltf file"
    )
    parser.add_argument(
        "--grip", type=float, nargs=3, metavar=("X", "Y", "Z"),
        help="World-space coordinates of the grip point. The model origin "
             "is relocated so this point becomes (0,0,0). Units are in the "
             "model's current coordinate space (applied before --scale)."
    )
    parser.add_argument(
        "--scale", type=float, default=None,
        help="Uniform scale factor applied after import "
             "(e.g. 0.01 to convert centimeters to meters)"
    )
    parser.add_argument(
        "--socket", action="append", metavar="NAME=NODE",
        help="Add a socket extras tag to a named node. Can be repeated. "
             "Example: --socket muzzle=BarrelTip --socket optic_rail=ScopeMount"
    )
    parser.add_argument(
        "--up", choices=["Y", "Z"], default=None,
        help="Override the model's up axis (for OBJ/FBX imports). "
             "glTF is always Y-up. If omitted, Blender's importer default is used."
    )
    return parser.parse_args(argv)


def clean_scene():
    bpy.ops.wm.read_factory_settings(use_empty=True)


def detect_format(filepath):
    suffix = Path(filepath).suffix.lower()
    formats = {
        ".gltf": "GLTF",
        ".glb": "GLTF",
        ".fbx": "FBX",
        ".obj": "OBJ",
    }
    fmt = formats.get(suffix)
    if not fmt:
        print(f"ERROR: Unsupported file format '{suffix}'")
        print(f"  Supported: {', '.join(sorted(formats.keys()))}")
        sys.exit(1)
    return fmt


def import_model(filepath, fmt, scale=None, up_axis=None):
    filepath = str(filepath)

    if fmt == "GLTF":
        bpy.ops.import_scene.gltf(filepath=filepath)
    elif fmt == "FBX":
        kwargs = {"filepath": filepath}
        if scale is not None:
            kwargs["global_scale"] = scale
        if up_axis == "Z":
            kwargs["use_manual_orientation"] = True
            kwargs["axis_up"] = "Z"
        bpy.ops.import_scene.fbx(**kwargs)
    elif fmt == "OBJ":
        kwargs = {"filepath": filepath}
        if up_axis == "Z":
            kwargs["up_axis"] = "Z"
        bpy.ops.import_scene.obj(**kwargs)


def get_meshes():
    return [obj for obj in bpy.data.objects if obj.type == "MESH"]


def join_meshes():
    """Join all mesh objects into a single mesh. Returns the resulting mesh object."""
    meshes = get_meshes()
    if not meshes:
        print("ERROR: No mesh objects found after import")
        sys.exit(1)

    if len(meshes) == 1:
        print(f"  Single mesh object: {meshes[0].name}")
        return meshes[0]

    print(f"  Joining {len(meshes)} mesh objects into one...")
    bpy.ops.object.select_all(action="DESELECT")
    for obj in meshes:
        obj.select_set(True)
    bpy.context.view_layer.objects.active = meshes[0]
    bpy.ops.object.join()

    result = bpy.context.view_layer.objects.active
    print(f"  Joined mesh: {result.name}")
    return result


def clear_parents_keep_transform():
    """Clear parent relationships while keeping world transforms."""
    for obj in bpy.data.objects:
        if obj.type == "MESH" and obj.parent:
            bpy.ops.object.select_all(action="DESELECT")
            obj.select_set(True)
            bpy.context.view_layer.objects.active = obj
            bpy.ops.object.parent_clear(type="CLEAR_KEEP_TRANSFORM")


def apply_scale(mesh, scale_factor):
    """Apply a uniform scale factor to the mesh."""
    bpy.ops.object.select_all(action="DESELECT")
    mesh.select_set(True)
    bpy.context.view_layer.objects.active = mesh
    mesh.scale = (scale_factor, scale_factor, scale_factor)
    bpy.ops.object.transform_apply(location=False, rotation=False, scale=True)
    print(f"  Applied scale factor: {scale_factor}")


def relocate_origin(mesh, grip_point):
    """Move all geometry so that grip_point becomes the new origin (0,0,0)."""
    offset = Vector(grip_point)
    bpy.ops.object.select_all(action="DESELECT")
    mesh.select_set(True)
    bpy.context.view_layer.objects.active = mesh

    saved_cursor = bpy.context.scene.cursor.location.copy()
    bpy.context.scene.cursor.location = mesh.matrix_world @ offset
    bpy.ops.object.origin_set(type="ORIGIN_CURSOR")
    bpy.context.scene.cursor.location = saved_cursor

    mesh.location = (0, 0, 0)
    bpy.ops.object.transform_apply(location=True, rotation=False, scale=False)

    print(f"  Origin relocated to grip point: ({grip_point[0]}, {grip_point[1]}, {grip_point[2]})")


def apply_all_transforms(mesh):
    """Bake the mesh's object transform into vertex data."""
    bpy.ops.object.select_all(action="DESELECT")
    mesh.select_set(True)
    bpy.context.view_layer.objects.active = mesh
    bpy.ops.object.transform_apply(location=True, rotation=True, scale=True)


def strip_skin(mesh):
    """Remove armature modifiers and vertex groups so the mesh exports as rigid."""
    removed_mods = 0
    for mod in list(mesh.modifiers):
        if mod.type == "ARMATURE":
            bpy.ops.object.select_all(action="DESELECT")
            mesh.select_set(True)
            bpy.context.view_layer.objects.active = mesh
            bpy.ops.object.modifier_apply(modifier=mod.name)
            removed_mods += 1

    removed_groups = len(mesh.vertex_groups)
    mesh.vertex_groups.clear()

    if removed_mods or removed_groups:
        print(f"  Stripped skin: {removed_mods} armature modifier(s), "
              f"{removed_groups} vertex group(s)")


def remove_armatures():
    """Remove any armature objects (props are rigid)."""
    armatures = [obj for obj in bpy.data.objects if obj.type == "ARMATURE"]
    if armatures:
        print(f"  Removing {len(armatures)} armature(s) (prop models are rigid)")
        for arm in armatures:
            bpy.data.objects.remove(arm, do_unlink=True)


def remove_non_mesh():
    """Remove cameras, lights, empties, and other non-mesh objects."""
    remove_types = {"CAMERA", "LIGHT", "EMPTY", "CURVE", "SURFACE", "SPEAKER"}
    removed = 0
    for obj in list(bpy.data.objects):
        if obj.type in remove_types:
            bpy.data.objects.remove(obj, do_unlink=True)
            removed += 1
    if removed:
        print(f"  Removed {removed} non-mesh object(s)")


def validate_model(mesh):
    """Validate the model against the engine contract."""
    print("\n--- Validation ---")
    ok = True

    dims = mesh.dimensions
    max_dim = max(dims.x, dims.y, dims.z)
    print(f"Dimensions: {dims.x:.3f} x {dims.y:.3f} x {dims.z:.3f} m")

    if max_dim < 0.01:
        print("  WARNING: Model is very small (<1cm). Is the scale correct?")
        print("  Pistol ~0.22m, rifle ~0.8m, wrench ~0.4m")
        ok = False
    elif max_dim > 3.0:
        print("  WARNING: Model is very large (>3m). Is the scale correct?")
        print("  Use --scale to convert (e.g. --scale 0.01 for cm to meters)")
        ok = False
    else:
        print("  Size OK")

    mesh_count = sum(1 for obj in bpy.data.objects if obj.type == "MESH")
    if mesh_count > 1:
        print(f"  ERROR: {mesh_count} mesh objects remain. Engine loads one mesh node.")
        ok = False
    else:
        print(f"  Mesh count OK (1)")

    mat_count = len(mesh.data.materials)
    prim_count = len(mesh.data.polygons)
    print(f"  Materials: {mat_count}, Polygons: {prim_count}")

    return ok


def export_gltf(output_path):
    output = Path(output_path)
    output.parent.mkdir(parents=True, exist_ok=True)

    export_kwargs = {
        "filepath": str(output),
        "export_format": "GLTF_SEPARATE",
        "export_texcoords": True,
        "export_normals": True,
        "export_tangents": False,
        "export_materials": "EXPORT",
        "export_colors": False,
        "export_cameras": False,
        "export_lights": False,
        "use_selection": False,
        "export_animations": False,
    }

    valid_props = set(bpy.ops.export_scene.gltf.get_rna_type().properties.keys())
    export_kwargs = {k: v for k, v in export_kwargs.items() if k in valid_props}

    print(f"\nExporting to: {output}")
    bpy.ops.export_scene.gltf(**export_kwargs)
    print("Done!")

    output_dir = output.parent
    output_stem = output.stem
    print(f"\nOutput files:")
    for f in sorted(output_dir.iterdir()):
        if f.stem.startswith(output_stem):
            size_kb = f.stat().st_size / 1024
            print(f"  {f.name} ({size_kb:.1f} KB)")


def postprocess_gltf(gltf_path, sockets=None):
    """Post-process the exported glTF JSON: clean extensions, add socket tags."""
    gltf_path = Path(gltf_path)
    with open(gltf_path, "r") as f:
        gltf = json.load(f)

    modified = False

    ext_required = gltf.get("extensionsRequired", [])
    rejected = [e for e in ext_required if e in REJECTED_EXTENSIONS]
    if rejected:
        gltf["extensionsRequired"] = [
            e for e in ext_required if e not in REJECTED_EXTENSIONS
        ]
        if not gltf["extensionsRequired"]:
            del gltf["extensionsRequired"]
        print(f"\nRemoved rejected extensionsRequired: {rejected}")

        ext_used = gltf.get("extensionsUsed", [])
        gltf["extensionsUsed"] = [
            e for e in ext_used if e not in REJECTED_EXTENSIONS
        ]
        if not gltf["extensionsUsed"]:
            del gltf["extensionsUsed"]
        modified = True

    if ext_required and not rejected:
        remaining = gltf.get("extensionsRequired", [])
        if remaining:
            print(f"\n  NOTE: extensionsRequired still contains: {remaining}")
            print("  If the engine rejects the model, these may need conversion.")

    nodes = gltf.get("nodes", [])

    if sockets:
        print(f"\nAdding socket tags:")
        for socket_spec in sockets:
            if "=" not in socket_spec:
                print(f"  ERROR: Invalid socket spec '{socket_spec}' — expected NAME=NODE")
                continue
            socket_name, node_name = socket_spec.split("=", 1)

            found = False
            for node in nodes:
                if node.get("name") == node_name:
                    extras = node.get("extras") or {}
                    extras["socket"] = socket_name
                    node["extras"] = extras
                    print(f"  socket '{socket_name}' -> node '{node_name}'")
                    found = True
                    modified = True
                    break
            if not found:
                print(f"  WARNING: Node '{node_name}' not found for socket '{socket_name}'")
                node_names = [n.get("name", "(unnamed)") for n in nodes]
                print(f"  Available nodes: {node_names}")

    accessors = gltf.get("accessors", [])
    meshes = gltf.get("meshes", [])
    has_tangent = False
    for mesh in meshes:
        for prim in mesh.get("primitives", []):
            attrs = prim.get("attributes", {})
            if "TANGENT" in attrs:
                has_tangent = True
                del attrs["TANGENT"]
                modified = True
    if has_tangent:
        print("\nStripped TANGENT attributes from mesh primitives")

    has_skin_attrs = False
    for mesh in meshes:
        for prim in mesh.get("primitives", []):
            attrs = prim.get("attributes", {})
            for attr in list(attrs.keys()):
                if attr.startswith("JOINTS_") or attr.startswith("WEIGHTS_"):
                    del attrs[attr]
                    has_skin_attrs = True
                    modified = True
    if "skins" in gltf:
        del gltf["skins"]
        modified = True
        has_skin_attrs = True
    for node in nodes:
        if "skin" in node:
            del node["skin"]
            modified = True
    if has_skin_attrs:
        print("Stripped skin data (JOINTS, WEIGHTS, skins)")

    print(f"\n--- glTF Summary ---")
    print(f"Nodes: {len(nodes)}")
    print(f"Meshes: {len(meshes)}")
    print(f"Accessors: {len(accessors)}")
    print(f"Materials: {len(gltf.get('materials', []))}")
    ext_req = gltf.get("extensionsRequired", [])
    print(f"extensionsRequired: {ext_req if ext_req else '(none)'}")

    mesh_node = None
    for node in nodes:
        if "mesh" in node:
            mesh_node = node
            break
    if mesh_node:
        print(f"Mesh node: '{mesh_node.get('name', '(unnamed)')}'")
        prim_count = 0
        mesh_idx = mesh_node["mesh"]
        if mesh_idx < len(meshes):
            prim_count = len(meshes[mesh_idx].get("primitives", []))
        print(f"  Primitives: {prim_count}")

    for acc in accessors:
        if acc.get("type") == "VEC3":
            mn = acc.get("min")
            mx = acc.get("max")
            if mn and mx:
                height = mx[1] - mn[1]
                width = mx[0] - mn[0]
                depth = mx[2] - mn[2]
                print(f"  POSITION bounds: {width:.3f} x {height:.3f} x {depth:.3f}")
                break

    if modified:
        with open(gltf_path, "w") as f:
            json.dump(gltf, f, indent="\t")
        print("\nglTF file updated.")


def main():
    args = parse_args()
    print("=" * 60)
    print("Prop/Weapon Model -> glTF Converter (Postretro Engine)")
    print("=" * 60)

    input_path = Path(args.input)
    if not input_path.exists():
        print(f"ERROR: Input file not found: {input_path}")
        sys.exit(1)

    fmt = detect_format(input_path)
    print(f"\nSource: {input_path.name} ({fmt})")

    clean_scene()

    print(f"\nImporting {fmt}...")
    import_model(input_path, fmt, up_axis=args.up)

    remove_non_mesh()
    clear_parents_keep_transform()

    mesh = join_meshes()

    strip_skin(mesh)
    remove_armatures()

    apply_all_transforms(mesh)

    if args.scale:
        apply_scale(mesh, args.scale)

    if args.grip:
        relocate_origin(mesh, args.grip)

    validate_model(mesh)
    export_gltf(args.output)
    postprocess_gltf(args.output, sockets=args.socket)


if __name__ == "__main__":
    main()
