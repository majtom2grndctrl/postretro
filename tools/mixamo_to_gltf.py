"""
Headless Blender script: merge multiple Mixamo FBX files into one glTF.

Usage:
    blender --background --python tools/mixamo_to_gltf.py -- \
        --input-dir path/to/fbx_folder \
        --output path/to/output/model.gltf \
        [--base "Exo Red.fbx"] \
        [--scale 0.01] \
        [--no-tag] \
        [--strip-face]

The --base file is treated as the base mesh + armature.  If omitted, the
first FBX alphabetically is used (which may be an animation-only file —
pass --base to be explicit).  All other FBXs contribute their animation
as named Actions (derived from the filename, e.g. "Idle.fbx" -> "idle").

After export the script tags the Mixamo skeleton with engine extras
(poseMask, aimBendWeight, socket) and sets the skin skeleton root.
Pass --no-tag to skip.

Output is glTF Separate (.gltf + .bin + textures) ready for the engine.
"""

import bpy
import sys
import json
import argparse
from pathlib import Path
import math
from mathutils import Matrix, Vector


# ---------------------------------------------------------------------------
# Mixamo bone -> engine extras mapping (bone names without "mixamorig:" prefix)
# ---------------------------------------------------------------------------

MIXAMO_PREFIX = "mixamorig:"

UPPER_BODY = {
    "Spine", "Spine1", "Spine2",
    "Neck", "Head", "HeadTop_End",
    "LeftShoulder", "LeftArm", "LeftForeArm", "LeftHand",
    "RightShoulder", "RightArm", "RightForeArm", "RightHand",
    "LeftHandThumb1", "LeftHandThumb2", "LeftHandThumb3", "LeftHandThumb4",
    "LeftHandIndex1", "LeftHandIndex2", "LeftHandIndex3", "LeftHandIndex4",
    "LeftHandMiddle1", "LeftHandMiddle2", "LeftHandMiddle3", "LeftHandMiddle4",
    "LeftHandRing1", "LeftHandRing2", "LeftHandRing3", "LeftHandRing4",
    "LeftHandPinky1", "LeftHandPinky2", "LeftHandPinky3", "LeftHandPinky4",
    "RightHandThumb1", "RightHandThumb2", "RightHandThumb3", "RightHandThumb4",
    "RightHandIndex1", "RightHandIndex2", "RightHandIndex3", "RightHandIndex4",
    "RightHandMiddle1", "RightHandMiddle2", "RightHandMiddle3", "RightHandMiddle4",
    "RightHandRing1", "RightHandRing2", "RightHandRing3", "RightHandRing4",
    "RightHandPinky1", "RightHandPinky2", "RightHandPinky3", "RightHandPinky4",
}

LOWER_BODY = {
    "Hips",
    "LeftUpLeg", "LeftLeg", "LeftFoot", "LeftToeBase", "LeftToe_End",
    "RightUpLeg", "RightLeg", "RightFoot", "RightToeBase", "RightToe_End",
}

AIM_SPINE_WEIGHTS = {
    "Spine": 0.4,
    "Spine1": 0.7,
    "Spine2": 1.0,
}

LEG_L = {"LeftUpLeg", "LeftLeg", "LeftFoot"}
LEG_R = {"RightUpLeg", "RightLeg", "RightFoot"}
FOOT_L = {"LeftFoot"}
FOOT_R = {"RightFoot"}

SOCKETS = {
    "RightHand": "hand_r",
    "LeftHand": "hand_l",
}


def parse_args():
    argv = sys.argv
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
        "--base",
        help="Filename of the FBX to use as the base mesh+armature "
             "(e.g. 'Exo Red.fbx'). If omitted, the first file "
             "alphabetically is used."
    )
    parser.add_argument(
        "--scale", type=float, default=None,
        help="Import scale override (default: let Blender auto-convert units). "
             "Use 0.01 if your model appears 100x too large after import."
    )
    parser.add_argument(
        "--no-tag", action="store_true",
        help="Skip auto-tagging the Mixamo skeleton with engine extras"
    )
    parser.add_argument(
        "--strip-face", action="store_true",
        help="Remove facial bones (eyelids, jaw, tongue, brows, etc.) "
             "and merge their vertex weights into the Head bone"
    )
    parser.add_argument(
        "--exclude-meshes", nargs="+", default=[],
        metavar="NAME",
        help="Drop named mesh objects before join (e.g. --exclude-meshes EXO_Body). "
             "Names are matched case-insensitively against Blender object names. "
             "Use to remove hidden geometry that causes z-fighting."
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
    stem = Path(filepath).stem
    name = stem.lower().replace(" ", "_").replace(".", "_")
    for prefix in ("mixamo_com_", "mixamo_"):
        if name.startswith(prefix):
            name = name[len(prefix):]
    return name


def get_action_fcurves(action):
    """Get fcurves from an action, handling both legacy and layered (Blender 4.4+) APIs."""
    if hasattr(action, 'fcurves') and action.fcurves is not None:
        try:
            iter(action.fcurves)
            return action.fcurves
        except (TypeError, AttributeError):
            pass
    if hasattr(action, 'layers'):
        for layer in action.layers:
            for strip in layer.strips:
                if hasattr(strip, 'channelbags'):
                    for channelbag in strip.channelbags:
                        if hasattr(channelbag, 'fcurves'):
                            return channelbag.fcurves
    return []


def fcurve_bone_name(data_path):
    prefix = 'pose.bones["'
    if not data_path.startswith(prefix):
        return None
    return data_path[len(prefix):data_path.index('"]')]


def merge_animations(input_dir, scale=None, base=None, exclude_meshes=None):
    """Import all FBX files, keeping one armature+mesh and collecting actions."""
    if not Path(input_dir).is_dir():
        print(f"ERROR: Input directory not found: {input_dir}")
        sys.exit(1)

    fbx_files = sorted(
        [f for f in Path(input_dir).iterdir() if f.suffix.lower() == ".fbx"],
        key=lambda p: p.name.lower()
    )
    if not fbx_files:
        print(f"ERROR: No .fbx files found in {input_dir}")
        sys.exit(1)

    if base:
        base_file = None
        for f in fbx_files:
            if f.name == base or f.stem == base:
                base_file = f
                break
        if not base_file:
            print(f"ERROR: Base file '{base}' not found in {input_dir}")
            print(f"  Available files: {[f.name for f in fbx_files]}")
            sys.exit(1)
        fbx_files.remove(base_file)
        fbx_files.insert(0, base_file)

    print(f"Found {len(fbx_files)} FBX file(s):")
    for f in fbx_files:
        print(f"  {f.name}")

    base_file = fbx_files[0]
    print(f"\nImporting base: {base_file.name}")
    import_fbx(base_file, scale)

    base_armature = get_armature()
    if not base_armature:
        print("ERROR: No armature found after importing base FBX")
        sys.exit(1)

    seen_names = set()
    if base_armature.animation_data and base_armature.animation_data.action:
        base_action = base_armature.animation_data.action
        base_action.name = action_name_from_filename(base_file)
        base_action.use_fake_user = True
        seen_names.add(base_action.name)
        print(f"  Action: {base_action.name}")

    base_bones = set(bone.name for bone in base_armature.data.bones)

    # --- Phase 1: Import all clip FBXes and collect actions ----------------
    # Clips must be imported BEFORE the armature transform so we can sample
    # their world-space bone poses against the original (FBX-imported) rest
    # pose.  After transform_apply the rest pose changes, and F-curve values
    # authored for the old rest pose produce wrong results.

    base_objects = set(bpy.data.objects[:])

    for fbx_file in fbx_files[1:]:
        clip_name = action_name_from_filename(fbx_file)
        print(f"\nImporting clip: {fbx_file.name} -> '{clip_name}'")

        import_fbx(fbx_file, scale)

        new_armature = None
        for obj in bpy.data.objects:
            if obj.type == "ARMATURE" and obj != base_armature:
                new_armature = obj
                break

        if not new_armature:
            print(f"  WARNING: No new armature found, skipping {fbx_file.name}")
            continue

        if new_armature.animation_data and new_armature.animation_data.action:
            action = new_armature.animation_data.action
            if clip_name in seen_names:
                print(f"  WARNING: Clip name '{clip_name}' collides with an earlier clip; "
                      f"Blender will auto-suffix it (e.g. '{clip_name}.001')")
            seen_names.add(clip_name)
            action.name = clip_name
            action.use_fake_user = True

            orphaned_channels = []
            matched_any = False
            fcurves = get_action_fcurves(action)
            for fcurve in fcurves:
                bone_name = fcurve_bone_name(fcurve.data_path)
                if bone_name is None:
                    continue
                if bone_name in base_bones:
                    matched_any = True
                else:
                    orphaned_channels.append(fcurve.data_path)
            if not matched_any and fcurves:
                print(f"  ERROR: No F-curves in '{clip_name}' target a base armature bone; clip is useless")
            elif orphaned_channels:
                print(f"  WARNING: Channels in '{clip_name}' target bones not in the base armature: "
                      f"{sorted(set(orphaned_channels))}")
        else:
            print(f"  WARNING: No animation data in {fbx_file.name}")

        for obj in list(bpy.data.objects):
            if obj not in base_objects:
                bpy.data.objects.remove(obj, do_unlink=True)

    # --- Phase 2: Sample armature-space bone poses for every action --------
    # Before we transform the armature, evaluate each action on the base
    # armature and record each bone's LOCAL transform (relative to parent)
    # at every keyframe.  We sample in armature space (pbone.matrix), NOT
    # world space, to avoid entangling the armature's cm-to-m scale with
    # the bone poses.  The local transforms are scale-free and transfer
    # directly to the new rest pose.

    all_actions = [a for a in bpy.data.actions if a.users > 0 or a.use_fake_user]
    action_poses = {}

    if not base_armature.animation_data:
        base_armature.animation_data_create()

    # Cache old rest-pose local transforms (before transform_apply)
    old_rest_local = {}
    for pbone in base_armature.pose.bones:
        if pbone.parent:
            old_rest_local[pbone.name] = pbone.parent.bone.matrix_local.inverted() @ pbone.bone.matrix_local
        else:
            old_rest_local[pbone.name] = pbone.bone.matrix_local.copy()

    for action in all_actions:
        base_armature.animation_data.action = action
        frame_start = int(action.frame_range[0])
        frame_end = int(action.frame_range[1])
        poses_by_frame = {}
        for frame in range(frame_start, frame_end + 1):
            bpy.context.scene.frame_set(frame)
            bpy.context.view_layer.update()
            bone_locals = {}
            for pbone in base_armature.pose.bones:
                # pbone.matrix is in armature space.  Derive local transform.
                if pbone.parent:
                    local = pbone.parent.matrix.inverted() @ pbone.matrix
                else:
                    local = pbone.matrix.copy()
                bone_locals[pbone.name] = local
            poses_by_frame[frame] = bone_locals
        action_poses[action.name] = (frame_start, frame_end, poses_by_frame)

    base_armature.animation_data.action = None
    bpy.context.scene.frame_set(0)

    print(f"\n  Sampled {len(action_poses)} action(s) for re-bake")

    # --- Phase 3: Apply armature + mesh transforms ------------------------
    # The engine never reads the Armature node (only skin joints), so ALL
    # transforms must be baked into bone rest positions AND mesh vertices.
    #
    # No forward-facing rotation is applied. Mixamo characters face Blender
    # -Y, which the glTF exporter (Z-up -> Y-up) maps directly to +Z, exactly
    # the forward the engine expects (pose_modifier.rs, mesh_pass.rs). An
    # earlier 180-deg Z flip here was based on a wrong premise (that the
    # source faces Blender +Y) and left the whole model reversed: brows, eyes,
    # cornea, and toes all pointed -Z. Identity keeps the model facing +Z.
    flip_z = Matrix.Identity(4)

    arm_world = base_armature.matrix_world.copy()
    baked_transform = flip_z @ arm_world

    mesh_objects = [obj for obj in bpy.data.objects if obj.type == "MESH"]
    mesh_worlds = {obj.name: obj.matrix_world.copy() for obj in mesh_objects}

    print(f"\n  Armature scale: {[round(x, 4) for x in arm_world.to_scale()]}"
          f"  rotation: {[round(math.degrees(x), 1) for x in arm_world.to_euler()]}")
    print(f"  Mesh objects to transform: {len(mesh_objects)}"
          f" [{', '.join(obj.name for obj in mesh_objects)}]")

    base_armature.matrix_basis = baked_transform
    bpy.ops.object.select_all(action='DESELECT')
    bpy.context.view_layer.objects.active = base_armature
    base_armature.select_set(True)
    bpy.ops.object.transform_apply(location=False, rotation=True, scale=True)
    base_armature.select_set(False)

    if mesh_objects:
        for mesh_obj in mesh_objects:
            obj_world = mesh_worlds[mesh_obj.name]
            mesh_bake = flip_z @ obj_world

            verts = mesh_obj.data.vertices
            if verts:
                zs = [v.co.z for v in verts]
                print(f"  {mesh_obj.name}: {len(verts)} verts,"
                      f" Z[{min(zs):.2f}, {max(zs):.2f}] before bake")

            mesh_obj.matrix_parent_inverse = Matrix.Identity(4)
            mesh_obj.matrix_basis = mesh_bake

            bpy.ops.object.select_all(action='DESELECT')
            bpy.context.view_layer.objects.active = mesh_obj
            mesh_obj.select_set(True)
            bpy.ops.object.transform_apply(location=True, rotation=True, scale=True)
            mesh_obj.select_set(False)

        all_verts_z = []
        for mesh_obj in mesh_objects:
            for v in mesh_obj.data.vertices:
                all_verts_z.append(v.co.z)
        if all_verts_z:
            print(f"  All meshes AFTER bake: Z[{min(all_verts_z):.4f}, {max(all_verts_z):.4f}]")
            print(f"  (expected: Z ~ [0.0, 1.8])")
    else:
        print("  WARNING: No mesh in base file — output will have no geometry.")
        print("  If the character mesh is in a different FBX, use --base to select it.")

    # --- Phase 4: Re-bake animations into the new rest pose ---------------
    # Phase 2 sampled each bone's LOCAL transform (parent.inv() @ bone) in
    # the old armature space.  For child bones, local transforms are
    # independent of the armature's world transform — they represent joint
    # angles/positions relative to the parent.  After transform_apply, the
    # same local transforms produce the correct pose.
    #
    # For the ROOT bone (no parent), the "local" is the armature-space
    # matrix itself, which is carried through baked_transform (the cm->m
    # scale) into the new rest space. flip_z is identity now, so no rotation
    # is introduced here — only the scale conversion.
    #
    # We compute pose deltas (Blender's pbone TRS = offset from rest):
    #   child:  pose_delta = old_rest_local.inv() @ old_anim_local
    #   root:   pose_delta = new_rest_local.inv() @ (baked_transform @ old_anim_local, scale-stripped)
    #
    # No depsgraph round-trip — pure math, no stale-parent corruption.

    for pbone in base_armature.pose.bones:
        pbone.rotation_mode = 'QUATERNION'

    # Cache the NEW rest-pose local transforms (after transform_apply)
    new_rest_local = {}
    for pbone in base_armature.pose.bones:
        if pbone.parent:
            new_rest_local[pbone.name] = pbone.parent.bone.matrix_local.inverted() @ pbone.bone.matrix_local
        else:
            new_rest_local[pbone.name] = pbone.bone.matrix_local.copy()

    for action in all_actions:
        if action.name not in action_poses:
            continue
        frame_start, frame_end, poses_by_frame = action_poses[action.name]

        base_armature.animation_data.action = action
        fcurves = get_action_fcurves(action)
        for fc in list(fcurves):
            fcurves.remove(fc)

        scale_ok = True
        for frame in range(frame_start, frame_end + 1):
            bpy.context.scene.frame_set(frame)
            bone_locals = poses_by_frame[frame]

            for pbone in base_armature.pose.bones:
                if pbone.name not in bone_locals:
                    continue

                old_local = bone_locals[pbone.name]

                if pbone.parent:
                    # Child bone: local transform is armature-independent.
                    # Pose delta = change from old rest to old animated.
                    old_rest = old_rest_local[pbone.name]
                    pose_delta = old_rest.inverted() @ old_local
                else:
                    # Root bone: "local" IS the armature-space matrix.
                    # Convert from old armature space (cm) to new armature
                    # space (meters) via baked_transform. baked_transform
                    # carries arm_world's 0.01 scale, but transform_apply
                    # normalized that out of rest poses — so strip scale to
                    # match. (flip_z is identity, so no rotation is applied.)
                    raw = baked_transform @ old_local
                    new_loc = raw.to_translation()
                    new_rot = raw.to_quaternion()
                    new_local = Matrix.Translation(new_loc) @ new_rot.to_matrix().to_4x4()
                    new_rest = new_rest_local[pbone.name]
                    pose_delta = new_rest.inverted() @ new_local

                loc = pose_delta.to_translation()
                rot = pose_delta.to_quaternion()
                scl = pose_delta.to_scale()

                pbone.location = loc
                pbone.rotation_quaternion = rot
                pbone.scale = scl

                pbone.keyframe_insert(data_path="location", frame=frame)
                pbone.keyframe_insert(data_path="rotation_quaternion", frame=frame)
                pbone.keyframe_insert(data_path="scale", frame=frame)

                if abs(scl[0] - 1.0) > 0.05 or abs(scl[1] - 1.0) > 0.05 or abs(scl[2] - 1.0) > 0.05:
                    if scale_ok:
                        print(f"  WARNING: non-unit scale on '{pbone.name}' frame {frame}: "
                              f"({scl[0]:.4f}, {scl[1]:.4f}, {scl[2]:.4f})")
                        scale_ok = False

        print(f"  Re-baked '{action.name}': {frame_end - frame_start + 1} frames"
              + ("" if scale_ok else " (scale warnings)"))

    base_armature.animation_data.action = None
    bpy.context.scene.frame_set(0)

    # --- Phase 5: Exclude meshes, then join ---------------------------------

    if exclude_meshes:
        exclude_lower = {n.lower() for n in exclude_meshes}
        mesh_objects = [obj for obj in bpy.data.objects if obj.type == "MESH"]
        for obj in mesh_objects:
            if obj.name.lower() in exclude_lower:
                print(f"  Excluding mesh: {obj.name} ({len(obj.data.vertices)} verts)")
                bpy.data.objects.remove(obj, do_unlink=True)

    mesh_objects = [obj for obj in bpy.data.objects if obj.type == "MESH"]
    if len(mesh_objects) > 1:
        print(f"\nJoining {len(mesh_objects)} mesh objects into one...")
        bpy.ops.object.select_all(action='DESELECT')
        for obj in mesh_objects:
            obj.select_set(True)
        bpy.context.view_layer.objects.active = mesh_objects[0]
        bpy.ops.object.join()

    print(f"\n--- Result ---")
    print(f"Actions in file:")
    for action in bpy.data.actions:
        frame_range = action.frame_range
        print(f"  {action.name}: frames {int(frame_range[0])}-{int(frame_range[1])}")


def strip_facial_bones():
    """Remove facial bones and merge their vertex weights to the Head bone."""
    armature = get_armature()
    if not armature:
        print("\nWARNING: No armature found, skipping facial bone strip")
        return

    head_bone = armature.data.bones.get(MIXAMO_PREFIX + "Head")
    if not head_bone:
        print("\nWARNING: No 'mixamorig:Head' bone found, skipping facial bone strip")
        return

    keep = {MIXAMO_PREFIX + "HeadTop_End"}
    face_bone_names = set()
    for child in head_bone.children_recursive:
        if child.name not in keep:
            face_bone_names.add(child.name)

    if not face_bone_names:
        print("\nNo facial bones found to strip")
        return

    print(f"\nStripping {len(face_bone_names)} facial bones...")

    mesh = get_mesh()
    if mesh:
        head_group = mesh.vertex_groups.get(MIXAMO_PREFIX + "Head")
        if not head_group:
            head_group = mesh.vertex_groups.new(name=MIXAMO_PREFIX + "Head")

        face_group_indices = set()
        for bone_name in face_bone_names:
            group = mesh.vertex_groups.get(bone_name)
            if group:
                face_group_indices.add(group.index)

        if face_group_indices:
            reassigned = 0
            for v in mesh.data.vertices:
                face_weight = 0.0
                for g in v.groups:
                    if g.group in face_group_indices:
                        face_weight += g.weight
                if face_weight > 0:
                    head_group.add([v.index], face_weight, 'ADD')
                    reassigned += 1

            for bone_name in face_bone_names:
                group = mesh.vertex_groups.get(bone_name)
                if group:
                    mesh.vertex_groups.remove(group)

            print(f"  Reassigned weights on {reassigned} vertices to Head")

    bpy.context.view_layer.objects.active = armature
    bpy.ops.object.mode_set(mode='EDIT')
    bpy.ops.armature.select_all(action='DESELECT')

    deleted = 0
    for bone_name in face_bone_names:
        bone = armature.data.edit_bones.get(bone_name)
        if bone:
            bone.select = True
            bone.select_head = True
            bone.select_tail = True
            deleted += 1

    bpy.ops.armature.delete()
    bpy.ops.object.mode_set(mode='OBJECT')

    remaining = len(armature.data.bones)
    print(f"  Deleted {deleted} bones, {remaining} remain")


def validate_model():
    mesh = get_mesh()
    armature = get_armature()

    if not mesh:
        print("WARNING: No mesh found in scene")
        return
    if not armature:
        print("WARNING: No armature found in scene")
        return

    dims = mesh.dimensions
    height = dims.z
    print(f"\nModel height: {height:.2f}m")
    if height < 0.5:
        print("  WARNING: Model seems very small. Consider adjusting --scale.")
        print("  Engine expects models authored at final world size (~2m for a human).")
    elif height > 5.0:
        print("  WARNING: Model seems very large. Consider adjusting --scale.")
        print("  Engine expects models authored at final world size (~2m for a human).")
    else:
        print("  OK (engine expects ~2m for a human)")

    min_z = min((mesh.matrix_world @ Vector(corner)).z for corner in mesh.bound_box)
    print(f"  Bounding box min Z: {min_z:.2f}m")
    if abs(min_z) > 0.1:
        print("  WARNING: Origin may not be at feet level. Engine expects the origin "
              "between the feet at ground level (z=0).")

    mesh_count = sum(1 for obj in bpy.data.objects if obj.type == "MESH")
    if mesh_count > 1:
        print(f"\n  ERROR: {mesh_count} mesh objects remain after join. Engine loads only one mesh node.")
        sys.exit(1)


def export_gltf(output_path):
    output = Path(output_path)
    output_dir = output.parent
    if output_dir.exists():
        for f in output_dir.iterdir():
            if f.suffix.lower() in (".gltf", ".glb", ".bin", ".png", ".jpg", ".jpeg"):
                f.unlink()
                print(f"  Removed stale: {f.name}")
    output_dir.mkdir(parents=True, exist_ok=True)

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
        "export_animations": True,
        "export_nla_strips": True,
        "export_animation_mode": "ACTIONS",
        "export_anim_single_armature": True,
    }

    armature = get_armature()
    if armature:
        if not armature.animation_data:
            armature.animation_data_create()
        ad = armature.animation_data

        for track in list(ad.nla_tracks):
            ad.nla_tracks.remove(track)

        for action in bpy.data.actions:
            track = ad.nla_tracks.new()
            track.name = action.name
            strip = track.strips.new(action.name, int(action.frame_range[0]), action)
            strip.name = action.name

        ad.action = None

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


def tag_gltf_skeleton(gltf_path):
    """Post-process the exported glTF to add engine extras to Mixamo bones."""
    gltf_path = Path(gltf_path)
    with open(gltf_path, 'r') as f:
        gltf = json.load(f)

    nodes = gltf.get("nodes", [])
    skins = gltf.get("skins", [])
    tagged = 0

    for i, node in enumerate(nodes):
        name = node.get("name", "")
        if not name.startswith(MIXAMO_PREFIX):
            continue
        short = name[len(MIXAMO_PREFIX):]

        extras = node.get("extras") or {}

        masks = []
        if short in UPPER_BODY:
            masks.append("upperBody")
        if short in LOWER_BODY:
            masks.append("lowerBody")
        if short in AIM_SPINE_WEIGHTS:
            masks.append("aimSpine")
            extras["aimBendWeight"] = AIM_SPINE_WEIGHTS[short]
        if short in LEG_L:
            masks.append("legL")
        if short in LEG_R:
            masks.append("legR")
        if short in FOOT_L:
            masks.append("footL")
        if short in FOOT_R:
            masks.append("footR")

        if masks:
            extras["poseMask"] = masks if len(masks) > 1 else masks[0]

        if short in SOCKETS:
            extras["socket"] = SOCKETS[short]

        if extras:
            node["extras"] = extras
            tagged += 1

    hips_idx = None
    for i, node in enumerate(nodes):
        if node.get("name") == MIXAMO_PREFIX + "Hips":
            hips_idx = i
            break
    if hips_idx is not None and skins:
        skins[0]["skeleton"] = hips_idx

    with open(gltf_path, 'w') as f:
        json.dump(gltf, f, indent='\t')

    print(f"\nTagged {tagged} bones with engine extras")
    mask_counts = {}
    socket_count = 0
    for node in nodes:
        extras = node.get("extras")
        if not extras:
            continue
        pm = extras.get("poseMask")
        if pm:
            for m in (pm if isinstance(pm, list) else [pm]):
                mask_counts[m] = mask_counts.get(m, 0) + 1
        if "socket" in extras:
            socket_count += 1
    for mask, count in sorted(mask_counts.items()):
        print(f"  poseMask '{mask}': {count} bone(s)")
    if socket_count:
        print(f"  sockets: {socket_count}")
    weights_str = ", ".join(f"{n}={w}" for n, w in sorted(AIM_SPINE_WEIGHTS.items()))
    print(f"  aimBendWeight: {weights_str}")
    if hips_idx is not None:
        print(f"  skin skeleton root: node {hips_idx} (Hips)")


def main():
    args = parse_args()
    print("=" * 60)
    print("Mixamo FBX -> glTF Converter (Postretro Engine)")
    print("=" * 60)

    clean_scene()
    merge_animations(args.input_dir, args.scale, args.base, args.exclude_meshes)
    if args.strip_face:
        strip_facial_bones()
    validate_model()
    export_gltf(args.output)
    if not args.no_tag:
        tag_gltf_skeleton(args.output)


if __name__ == "__main__":
    main()
