"""
Headless Blender script: merge multiple Mixamo FBX files into one glTF.

Usage:
    blender --background --python tools/mixamo_to_gltf.py -- \
        --input-dir path/to/fbx_folder \
        --output path/to/output/model.gltf \
        [--scale 0.01]

The first FBX (alphabetically) is treated as the base mesh + armature.
All other FBXs contribute their animation as named Actions (derived from
the filename, e.g. "Idle.fbx" -> action named "idle").

Output is glTF Separate (.gltf + .bin + textures) ready for the engine.
"""

import bpy
import sys
import os
import argparse
from pathlib import Path


def parse_args():
    argv = sys.argv
    # Blender passes everything after "--" to the script
    if "--" in argv:
        argv = argv[argv.index("--") + 1:]
    else:
        argv = []

    parser = argparse.ArgumentParser(description="Merge Mixamo FBX files into one glTF")
    parser.add_argument(
        "--input-dir", required=True,
        help="Directory containing Mixamo .fbx files"
    )
    parser.add_argument(
        "--output", required=True,
        help="Output path for the .gltf file"
    )
    parser.add_argument(
        "--scale", type=float, default=None,
        help="Import scale override (default: let Blender auto-convert units). "
             "Use 0.01 if your model appears 100x too large after import."
    )
    return parser.parse_args(argv)


def clean_scene():
    bpy.ops.wm.read_factory_settings(use_empty=True)


def import_fbx(filepath, scale=None):
    kwargs = {
        "filepath": str(filepath),
        "automatic_bone_orientation": True,
        "use_anim": True,
    }
    if scale is not None:
        kwargs["global_scale"] = scale
    bpy.ops.import_scene.fbx(**kwargs)


def get_armature():
    for obj in bpy.data.objects:
        if obj.type == "ARMATURE":
            return obj
    return None


def get_mesh():
    for obj in bpy.data.objects:
        if obj.type == "MESH":
            return obj
    return None


def action_name_from_filename(filepath):
    """Derive a clean action name from the FBX filename."""
    stem = Path(filepath).stem
    # Mixamo filenames often look like "Walking.fbx" or "Idle_Breathing.fbx"
    # Convert to lowercase, replace spaces/dots with underscores
    name = stem.lower().replace(" ", "_").replace(".", "_")
    # Strip common Mixamo prefixes/suffixes
    for prefix in ("mixamo_com_", "mixamo_"):
        if name.startswith(prefix):
            name = name[len(prefix):]
    return name


def merge_animations(input_dir, scale=None):
    """Import all FBX files, keeping one armature+mesh and collecting actions."""
    fbx_files = sorted(Path(input_dir).glob("*.fbx"), key=lambda p: p.name.lower())
    if not fbx_files:
        # Try case-insensitive
        fbx_files = sorted(
            [f for f in Path(input_dir).iterdir() if f.suffix.lower() == ".fbx"],
            key=lambda p: p.name.lower()
        )
    if not fbx_files:
        print(f"ERROR: No .fbx files found in {input_dir}")
        sys.exit(1)

    print(f"Found {len(fbx_files)} FBX file(s):")
    for f in fbx_files:
        print(f"  {f.name}")

    # Import the first file as the base (mesh + armature + first animation)
    base_file = fbx_files[0]
    print(f"\nImporting base: {base_file.name}")
    import_fbx(base_file, scale)

    base_armature = get_armature()
    if not base_armature:
        print("ERROR: No armature found after importing base FBX")
        sys.exit(1)

    # Rename the base action
    if base_armature.animation_data and base_armature.animation_data.action:
        base_action = base_armature.animation_data.action
        base_action.name = action_name_from_filename(base_file)
        print(f"  Action: {base_action.name}")

    # Store base bone names for validation
    base_bones = set(bone.name for bone in base_armature.data.bones)

    # Apply transforms on base mesh
    base_mesh = get_mesh()
    if base_mesh:
        bpy.context.view_layer.objects.active = base_mesh
        base_mesh.select_set(True)
        bpy.ops.object.transform_apply(location=True, rotation=True, scale=True)
        base_mesh.select_set(False)

    bpy.context.view_layer.objects.active = base_armature
    base_armature.select_set(True)
    bpy.ops.object.transform_apply(location=True, rotation=True, scale=True)
    base_armature.select_set(False)

    # Import remaining files for their animations
    for fbx_file in fbx_files[1:]:
        clip_name = action_name_from_filename(fbx_file)
        print(f"\nImporting clip: {fbx_file.name} -> '{clip_name}'")

        import_fbx(fbx_file, scale)

        # Find the newly imported armature (not the base one)
        new_armature = None
        for obj in bpy.data.objects:
            if obj.type == "ARMATURE" and obj != base_armature:
                new_armature = obj
                break

        if not new_armature:
            print(f"  WARNING: No new armature found, skipping {fbx_file.name}")
            continue

        # Grab the action from the new armature
        if new_armature.animation_data and new_armature.animation_data.action:
            action = new_armature.animation_data.action
            action.name = clip_name

            # Validate bone names match
            new_bones = set(bone.name for bone in new_armature.data.bones)
            missing = base_bones - new_bones
            if missing:
                print(f"  WARNING: New armature missing bones: {missing}")
        else:
            print(f"  WARNING: No animation data in {fbx_file.name}")

        # Delete the duplicate armature and any meshes that came with it
        new_meshes = [
            obj for obj in bpy.data.objects
            if obj.type == "MESH" and obj != base_mesh
        ]
        for mesh_obj in new_meshes:
            bpy.data.objects.remove(mesh_obj, do_unlink=True)
        bpy.data.objects.remove(new_armature, do_unlink=True)

    # Summary
    print(f"\n--- Result ---")
    print(f"Actions in file:")
    for action in bpy.data.actions:
        frame_range = action.frame_range
        print(f"  {action.name}: frames {int(frame_range[0])}-{int(frame_range[1])}")


def validate_model():
    """Check the model meets engine requirements."""
    mesh = get_mesh()
    armature = get_armature()

    if not mesh:
        print("WARNING: No mesh found in scene")
        return
    if not armature:
        print("WARNING: No armature found in scene")
        return

    # Check height (should be ~1.8-2.0m for a human character)
    dims = mesh.dimensions
    height = dims.z  # Blender Z-up after FBX import with auto bone orientation
    print(f"\nModel height: {height:.2f}m")
    if height < 0.5:
        print("  WARNING: Model seems very small. Consider adjusting --scale.")
        print("  Engine expects models authored at final world size (~2m for a human).")
    elif height > 5.0:
        print("  WARNING: Model seems very large. Consider adjusting --scale.")
        print("  Engine expects models authored at final world size (~2m for a human).")
    else:
        print("  OK (engine expects ~2m for a human)")

    # Check mesh count (engine loads one mesh node only)
    mesh_count = sum(1 for obj in bpy.data.objects if obj.type == "MESH")
    if mesh_count > 1:
        print(f"\n  WARNING: {mesh_count} mesh objects found. Engine loads only one mesh node.")
        print("  Consider joining meshes (Ctrl+J) before export.")


def export_gltf(output_path):
    """Export as glTF Separate format."""
    output = Path(output_path)
    output.parent.mkdir(parents=True, exist_ok=True)

    # Ensure we're exporting with the right settings for the engine
    export_kwargs = {
        "filepath": str(output),
        "export_format": "GLTF_SEPARATE",  # .gltf + .bin + textures
        "export_texcoords": True,
        "export_normals": True,
        "export_tangents": False,  # Engine: "omit tangents rather than ship near-degenerate ones"
        "export_materials": "EXPORT",
        "export_colors": False,
        "export_cameras": False,
        "export_lights": False,
        "use_selection": False,
        "export_animations": True,
        "export_nla_strips": True,  # Each NLA track / action as a named clip
        "export_animation_mode": "ACTIONS",
        "export_anim_single_armature": True,
    }

    # Push all actions to NLA tracks so they export as separate clips
    armature = get_armature()
    if armature:
        if not armature.animation_data:
            armature.animation_data_create()
        ad = armature.animation_data

        # Clear existing NLA tracks
        for track in list(ad.nla_tracks):
            ad.nla_tracks.remove(track)

        # Create one NLA track per action
        for action in bpy.data.actions:
            track = ad.nla_tracks.new()
            track.name = action.name
            strip = track.strips.new(action.name, int(action.frame_range[0]), action)
            strip.name = action.name

        # Clear the active action so it doesn't double-export
        ad.action = None

    print(f"\nExporting to: {output}")
    bpy.ops.export_scene.gltf(**export_kwargs)
    print("Done!")

    # List output files
    output_dir = output.parent
    output_stem = output.stem
    print(f"\nOutput files:")
    for f in sorted(output_dir.iterdir()):
        if f.stem.startswith(output_stem) or f.suffix in (".bin", ".png", ".jpg"):
            size_kb = f.stat().st_size / 1024
            print(f"  {f.name} ({size_kb:.1f} KB)")


def main():
    args = parse_args()
    print("=" * 60)
    print("Mixamo FBX -> glTF Converter (Postretro Engine)")
    print("=" * 60)

    clean_scene()
    merge_animations(args.input_dir, args.scale)
    validate_model()
    export_gltf(args.output)


if __name__ == "__main__":
    main()
