"""Export the Cyberpunk Guns Asset Pack Blender scene as engine-ready props.

Usage:
    blender --background "C:/Users/danhi/Downloads/12-cyberpunk-weapons.blend" \
      --python tools/extract_cyberpunk_weapons.py -- \
      --source "C:/Users/danhi/Downloads/12-cyberpunk-weapons.blend" \
      --output-root content/dev/models/cyberpunk_weapons

Each source weapon is evaluated in its armature's local rest frame, converted
to one rigid mesh node, and written as an external glTF with its packed base
color texture.  This deliberately preserves the pack's authored metre scale
and grip-relative origin while discarding animation rig data: PostRetro uses
these as rigid third-person props and viewmodels.
"""

import argparse
import json
import sys
from pathlib import Path

import bpy


WEAPONS = {
    "c4": "c4",
    "grenade": "grenade",
    "grenade_launcher": "grenade launcher",
    "melee_baton": "meele weapon",
    "pistol": "pistol",
    "revolver": "revolver",
    "rifle": "rifle mag",
    "rpg": "rpg ammo",
    "sci_fi_weapon": "sci-fi weapon",
    "shotgun": "shotgun",
    "smg": "smg",
    "sniper": "sniper",
}

# Authoring-space muzzle points for the firearm models.  The source scene uses
# Blender coordinates while emitted glTF uses +Y up and -Z forward, so these
# values intentionally stay in the final glTF mesh-node-local frame.  Each
# point is at the centre of the forward barrel face, measured from the
# extracted rigid mesh; it is not a geometric-extents guess at regeneration
# time.  Consumable explosives and the melee baton have no barrel and therefore
# deliberately have no muzzle socket.
MUZZLE_OFFSETS = {
    "grenade_launcher": (0.0, 0.474, -0.842),
    "pistol": (0.0, 0.225, -0.574),
    "revolver": (0.0, 0.301, -0.439),
    "rifle": (0.0, 0.350, -1.006),
    "rpg": (0.0, 0.372, -0.984),
    "sci_fi_weapon": (0.0, 0.276, -0.834),
    "shotgun": (0.0, 0.340, -1.137),
    "smg": (0.0, 0.274, -0.567),
    "sniper": (0.0, 0.224, -1.124),
}

LICENSE_TEXT = """Model Information:
* title: Cyberpunk Guns Asset Pack — {weapon}
* source: https://ab8b.itch.io/cyberpunk-guns-asset-pack
* author: ab8b (https://ab8b.itch.io)

Model License:
* license type: itch.io asset pack license
* requirements: Free to use in personal and commercial projects.
                Do not redistribute or resell the raw assets.
                See asset pack page for full terms.

Packaging notes (PostRetro):
* Weapon model extracted from 12-cyberpunk-weapons.blend and converted to an
  external glTF layout (model.gltf + model.bin + texture) by
  tools/extract_cyberpunk_weapons.py for the engine's rigid model pipeline.
* Geometry is evaluated in the source weapon armature's rest frame, keeping
  the authored grip-relative origin and metre scale.
"""


def parse_args():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--output-root", type=Path, required=True)
    return parser.parse_args(argv)


def select_only(*objects):
    for candidate in bpy.context.view_layer.objects:
        candidate.select_set(False)
    for obj in objects:
        obj.select_set(True)
    bpy.context.view_layer.objects.active = objects[0]


def image_for_mesh(mesh):
    images = set()
    for material in mesh.data.materials:
        if material is None or not material.use_nodes:
            continue
        for node in material.node_tree.nodes:
            if node.type != "BSDF_PRINCIPLED":
                continue
            base_color = node.inputs.get("Base Color")
            if base_color is None:
                continue
            for link in base_color.links:
                if link.from_node.type == "TEX_IMAGE" and link.from_node.image is not None:
                    images.add(link.from_node.image)
    if len(images) != 1:
        names = sorted(image.name for image in images)
        raise RuntimeError(
            f"{mesh.name!r} must use exactly one base-color image; found {names}"
        )
    return images.pop()


def prepare_texture(image, output_dir, weapon):
    """Save the source's packed texture and point the material at that copy."""
    texture_path = output_dir / "texture.png"
    image.name = f"{weapon}_texture"
    image.filepath_raw = str(texture_path)
    image.filepath = str(texture_path)
    image.file_format = "PNG"
    image.save()
    return texture_path


def duplicate_as_rigid_mesh(source, depsgraph):
    """Evaluate a mesh, then express it relative to its armature's origin."""
    if source.parent is None or source.parent.type != "ARMATURE":
        raise RuntimeError(f"{source.name!r} must be parented to an armature")

    evaluated = source.evaluated_get(depsgraph)
    evaluated_mesh = evaluated.to_mesh(preserve_all_data_layers=True, depsgraph=depsgraph)
    try:
        mesh = evaluated_mesh.copy()
    finally:
        evaluated.to_mesh_clear()

    # The source scene lays weapons out in a grid.  The parent armature's
    # transform is that grid offset; remove it while retaining its local rest
    # coordinates, whose origin is the authored weapon grip.
    mesh.transform(source.parent.matrix_world.inverted() @ evaluated.matrix_world)
    rigid = bpy.data.objects.new(source.name, mesh)
    bpy.context.collection.objects.link(rigid)
    return rigid


def add_muzzle_socket(parent, muzzle_offset):
    """Create an export-only empty at a glTF-frame muzzle point.

    Blender's glTF exporter maps Blender ``(x, y, z)`` to glTF
    ``(x, z, -y)``.  Transform the final engine-facing point back before
    parenting it to the rigid mesh, so the metadata and descriptor values use
    precisely the same frame as the exported vertices.
    """
    gltf_x, gltf_y, gltf_z = muzzle_offset
    socket = bpy.data.objects.new("muzzle", None)
    socket.empty_display_type = "PLAIN_AXES"
    socket.parent = parent
    socket.location = (gltf_x, -gltf_z, gltf_y)
    bpy.context.collection.objects.link(socket)
    return socket


def strip_engine_unsupported_extensions(gltf_path, has_muzzle_socket):
    with gltf_path.open(encoding="utf-8") as file:
        gltf = json.load(file)

    for material in gltf.get("materials", []):
        material.pop("extensions", None)
    gltf.pop("extensionsUsed", None)
    gltf.pop("extensionsRequired", None)

    for mesh in gltf.get("meshes", []):
        for primitive in mesh.get("primitives", []):
            primitive.get("attributes", {}).pop("TANGENT", None)

    images = gltf.get("images", [])
    if len(images) != 1:
        raise RuntimeError(
            f"{gltf_path} must export exactly one texture image; found {len(images)}"
        )
    images[0]["uri"] = "texture.png"

    if has_muzzle_socket:
        muzzle_nodes = [node for node in gltf.get("nodes", []) if node.get("name") == "muzzle"]
        if len(muzzle_nodes) != 1:
            raise RuntimeError(
                f"{gltf_path} must export exactly one muzzle node; found {len(muzzle_nodes)}"
            )
        extras = muzzle_nodes[0].get("extras") or {}
        extras["socket"] = "muzzle"
        muzzle_nodes[0]["extras"] = extras

    with gltf_path.open("w", encoding="utf-8", newline="\n") as file:
        json.dump(gltf, file, indent="\t")
        file.write("\n")


def export_weapon(source, output_dir, weapon, depsgraph):
    output_dir.mkdir(parents=True, exist_ok=True)
    rigid = duplicate_as_rigid_mesh(source, depsgraph)
    muzzle_socket = None
    try:
        texture_path = prepare_texture(image_for_mesh(rigid), output_dir, weapon)
        if muzzle_offset := MUZZLE_OFFSETS.get(weapon):
            muzzle_socket = add_muzzle_socket(rigid, muzzle_offset)
            select_only(rigid, muzzle_socket)
        else:
            select_only(rigid)
        bpy.ops.export_scene.gltf(
            filepath=str(output_dir / "model.gltf"),
            export_format="GLTF_SEPARATE",
            use_selection=True,
            export_animations=False,
            export_cameras=False,
            export_lights=False,
            export_normals=True,
            export_tangents=False,
            export_texcoords=True,
            export_materials="EXPORT",
            export_image_format="AUTO",
            export_keep_originals=True,
        )
        strip_engine_unsupported_extensions(
            output_dir / "model.gltf", has_muzzle_socket=muzzle_socket is not None
        )

        # `export_keep_originals` preserves the packed PNG at its authored
        # filename. Normalize it to the stable, documented texture name.
        exported_images = [
            path for path in output_dir.iterdir()
            if path.suffix.lower() == ".png" and path != texture_path
        ]
        for exported in exported_images:
            exported.unlink()
        if not texture_path.exists():
            raise RuntimeError(f"glTF export did not retain {texture_path}")

        (output_dir / "license.txt").write_text(
            LICENSE_TEXT.format(weapon=weapon), encoding="utf-8", newline="\n"
        )
    finally:
        if muzzle_socket is not None:
            bpy.data.objects.remove(muzzle_socket, do_unlink=True)
        bpy.data.objects.remove(rigid, do_unlink=True)


def main():
    args = parse_args()
    source_path = args.source.resolve()
    output_root = args.output_root.resolve()
    if not source_path.is_file():
        raise SystemExit(f"Source Blender file not found: {source_path}")

    loaded_path = Path(bpy.data.filepath).resolve()
    if loaded_path != source_path:
        raise SystemExit(
            "Launch Blender with --background <source.blend> before --python; "
            f"loaded {loaded_path}, expected {source_path}"
        )
    depsgraph = bpy.context.evaluated_depsgraph_get()

    source_meshes = {obj.name: obj for obj in bpy.context.scene.objects if obj.type == "MESH"}
    missing = sorted(source_name for source_name in WEAPONS.values() if source_name not in source_meshes)
    if missing:
        raise SystemExit(f"Expected source weapon meshes are missing: {missing}")

    for weapon, source_name in WEAPONS.items():
        print(f"Exporting {weapon} from source mesh {source_name!r}")
        export_weapon(source_meshes[source_name], output_root / weapon, weapon, depsgraph)

    print(f"Exported {len(WEAPONS)} weapon models to {output_root}")


if __name__ == "__main__":
    main()
