"""
Headless Blender script: merge multiple Mixamo FBX files into one glTF.

Usage:
    blender --background --python tools/mixamo_to_gltf.py -- \
        --input-dir path/to/fbx_folder \
        --output path/to/output/model.gltf \
        [--base "Exo Red.fbx"] \
        [--scale 0.01] \
        [--yaw 90] \
        [--no-tag] \
        [--strip-face] \
        [--fix-eyes]

The --base file is treated as the base mesh + armature.  If omitted, the
first FBX alphabetically is used (which may be an animation-only file —
pass --base to be explicit).  All other FBXs contribute their animation
as named Actions (derived from the filename, e.g. "Idle.fbx" -> "idle").
Actions the base file ships beyond its own rest pose (e.g. control-rig
MCH_* driver tracks) are dropped; real animation comes from the clips.

Use --yaw when a model was exported facing a non-standard direction (it
rotates about the vertical axis on top of the standard facing flip).

After export the script tags the Mixamo skeleton with engine extras
(poseMask, aimBendWeight, socket, hitZone) and sets the skin skeleton root.
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

# Skeletal hit zones (bone -> `hitZone` tag). A descriptor's `zoneMultipliers`
# key matches a joint's zone tag by string, so without this tag the head/leg
# multipliers are silently dead. Radius is omitted; the engine applies its
# DEFAULT_ZONE_RADIUS (see scripting/systems/hit_zones.rs). Feet are left
# untagged (small, ground-level); the thigh+shin carry the "leg" zone.
HIT_ZONES = {
    "Head": "head",
    "LeftUpLeg": "leg",
    "LeftLeg": "leg",
    "RightUpLeg": "leg",
    "RightLeg": "leg",
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
        "--yaw", type=float, default=0.0,
        metavar="DEGREES",
        help="Extra rotation about the vertical axis, applied with the "
             "standard 180-deg facing flip. Use when a model was exported "
             "facing a different way than the engine's +Z-forward convention "
             "(e.g. --yaw 90 for a model turned a quarter-turn). Default 0 "
             "(no extra rotation), correct for standard Mixamo exports."
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
        "--fix-eyes", action="store_true",
        help="Rotate each eyeball 180 deg about its own vertical axis so the "
             "iris faces the front of the head. Mixamo exo eyes are authored "
             "with the pupil on the rear hemisphere (visible only from inside "
             "the skull). Eyes are skinned 100%% to the Head joint, so a "
             "whole-model flip cannot correct this — the eye sphere must be "
             "spun in place."
    )
    parser.add_argument(
        "--eye-inset", type=float, default=0.01,
        metavar="METERS",
        help="With --fix-eyes, push each eyeball this far back into the socket "
             "(meters, along the head's backward axis) so the cornea does not "
             "bulge past the eyelids. Eye radius is ~0.014m; default 0.01. "
             "Use 0 to disable, or raise it for deeper-set eyes."
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


def strip_non_bone_fcurves(action):
    """Remove object-level (non-`pose.bones`) F-curves from an action.

    FBX exports from Blender control rigs (Rigify/MCH, etc.) frequently
    animate the armature OBJECT's own transform (location/rotation/scale)
    alongside the pose bones. The engine consumes only skeletal (`pose.bones`)
    animation, so these channels are dead weight — and, critically,
    EVALUATING an object-scale channel during re-bake overwrites the FBX
    importer's unit conversion on the armature (e.g. resets the 0.01 cm->m
    object scale to 1.0), which corrupts the baked output size. Vanilla Mixamo
    downloads carry no such channels, so this is a no-op for them.

    Returns the number of F-curves removed.
    """
    fcurves = get_action_fcurves(action)
    doomed = [fc for fc in list(fcurves) if fcurve_bone_name(fc.data_path) is None]
    removed = 0
    for fc in doomed:
        data_path = fc.data_path
        try:
            fcurves.remove(fc)
            removed += 1
        except (RuntimeError, ReferenceError, TypeError) as exc:
            # A survivor here is not benign: an object-scale channel that
            # outlives this pass gets evaluated in Phase 2 and clobbers the
            # armature's import scale, corrupting the baked output size. Never
            # swallow it silently.
            print(f"  WARNING: could not strip object-level channel "
                  f"'{data_path}' from '{action.name}' ({exc}); "
                  f"baked output size may be corrupted")
    return removed


def merge_animations(input_dir, scale=None, base=None, exclude_meshes=None, yaw=0.0):
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
    base_action = None
    if base_armature.animation_data and base_armature.animation_data.action:
        base_action = base_armature.animation_data.action
        base_action.name = action_name_from_filename(base_file)
        base_action.use_fake_user = True
        seen_names.add(base_action.name)
        print(f"  Action: {base_action.name}")

    # Drop any OTHER actions the base file shipped. The base file contributes
    # geometry plus its single rest/bind pose (kept above as `base_action`);
    # real animation comes from the clip files. Blender control-rig exports
    # leave extra driver-baked actions here (e.g. MCH_* mechanism tracks) that
    # would otherwise export as junk clips. A vanilla Mixamo base has only its
    # one action, so this removes nothing for clean input.
    for action in list(bpy.data.actions):
        if action is not base_action:
            print(f"  Dropping base-file leftover action: '{action.name}'")
            bpy.data.actions.remove(action)

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

    # --- Cleanup: strip control-rig leftovers ------------------------------
    # Blender-rig FBX exports (Rigify/MCH, etc.) leave two artifacts that
    # corrupt the bake. Neither exists in a vanilla Mixamo download, so every
    # step below is a no-op for clean input. This must run BEFORE Phase 2:
    # once Phase 2 assigns an action and steps frames, an object-scale channel
    # would clobber the armature's import scale (see strip_non_bone_fcurves).
    #
    # 1. Non-deforming control objects left beside the character (helper
    #    empties, mechanism armatures). The mesh and the chosen base armature
    #    are the only objects the engine consumes.
    for obj in list(bpy.data.objects):
        if obj is base_armature or obj.type == "MESH":
            continue
        print(f"  Removing non-deforming object: {obj.type} '{obj.name}'")
        bpy.data.objects.remove(obj, do_unlink=True)

    # 2. Object-level F-curves that would clobber the armature's import scale,
    #    then any action left with no channels targeting the base skeleton
    #    (e.g. clips authored purely against absent control bones).
    for action in list(bpy.data.actions):
        stripped = strip_non_bone_fcurves(action)
        if stripped:
            print(f"  Stripped {stripped} object-level channel(s) from '{action.name}'")
    for action in list(bpy.data.actions):
        fcurves = get_action_fcurves(action)
        if not any(fcurve_bone_name(fc.data_path) in base_bones for fc in fcurves):
            print(f"  Dropping action with no base-skeleton channels: '{action.name}'")
            bpy.data.actions.remove(action)

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
    # A 180-deg Z flip is also needed: as imported, the character faces the
    # wrong way for the engine's +Z-forward convention (pose_modifier.rs,
    # mesh_pass.rs). Confirmed empirically: without this flip the whole model
    # (body and feet) renders facing away from its movement direction.
    #
    # `--yaw` adds a further rotation about the same vertical (Z) axis for
    # models exported facing a non-standard direction; it folds into flip_z so
    # every downstream use (armature bake, mesh bake, root-bone re-bake in
    # Phase 4) stays consistent. Default yaw 0 leaves the flip unchanged.
    flip_z = Matrix.Rotation(math.pi + math.radians(yaw), 4, 'Z')
    if yaw:
        print(f"  Applying extra yaw: {yaw} deg (about vertical axis)")

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
    # matrix itself, which needs the flip_z rotation applied (the rest pose
    # was flipped, so the animated pose must be too).
    #
    # We compute pose deltas (Blender's pbone TRS = offset from rest):
    #   child:  pose_delta = old_rest_local.inv() @ old_anim_local
    #   root:   pose_delta = new_rest_local.inv() @ (flip_z_rot @ old_anim_local)
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
            # glTF clip duration is its final key time. Rebase imported Mixamo
            # frame numbers so a clip beginning at frame 1 starts at time zero
            # instead of adding a one-frame dwell when it loops.
            rebaked_frame = frame - frame_start

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
                    # Convert from old armature space (cm, original orientation)
                    # to new armature space (meters, flipped) via baked_transform.
                    # baked_transform carries arm_world's 0.01 scale, but
                    # transform_apply normalized that out of rest poses — so
                    # strip scale to match.
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

                pbone.keyframe_insert(data_path="location", frame=rebaked_frame)
                pbone.keyframe_insert(data_path="rotation_quaternion", frame=rebaked_frame)
                pbone.keyframe_insert(data_path="scale", frame=rebaked_frame)

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


def flip_eye_geometry(inset=0.0):
    """Rotate each eyeball 180 deg about its own vertical axis so the iris
    faces the front of the head.

    Mixamo's exo eyeballs are authored with the iris/pupil on the REAR
    hemisphere of the eye sphere: the pupil vertex sits at the into-skull
    extreme while the face points the other way, so the iris is only visible
    from inside the head. The eyes are skinned 100% to the Head joint, so no
    whole-model transform can correct this — the iris is 180 deg off relative
    to the face regardless of which way the body points. We spin each eye
    sphere in place about the vertical (Blender Z) axis through its own
    center: the pupil swings from back to front, the eye stays in its socket,
    and its Head skinning is untouched. Run this AFTER the meshes are joined
    so all eye material slots live on one object.

    `inset` (meters) then pushes the eyeballs backward into the sockets so the
    now-forward-facing cornea does not bulge past the eyelids.
    """
    mesh_obj = get_mesh()
    if not mesh_obj:
        print("\n  --fix-eyes: no mesh found, skipping")
        return
    mesh = mesh_obj.data

    eye_slots = {
        i for i, slot in enumerate(mesh_obj.material_slots)
        if slot.material and "eye" in slot.material.name.lower()
    }
    if not eye_slots:
        print("\n  --fix-eyes: no 'Eye' material on the mesh, skipping")
        return

    eye_loop_idx = []
    eye_vert_idx = set()
    for poly in mesh.polygons:
        if poly.material_index in eye_slots:
            for li in poly.loop_indices:
                eye_loop_idx.append(li)
                eye_vert_idx.add(mesh.loops[li].vertex_index)
    if not eye_vert_idx:
        print("\n  --fix-eyes: no eye geometry found, skipping")
        return

    verts = mesh.vertices

    # Capture the existing (computed) split normals so the eye ones can be
    # rotated too; a 180 deg turn about vertical maps (nx,ny,nz)->(-nx,-ny,nz).
    # API differs across Blender versions; failure is non-fatal (positions are
    # the visible fix, and recomputed normals on a rotated sphere are correct).
    loop_normals = None
    try:
        mesh.calc_normals_split()
        loop_normals = [list(loop.normal) for loop in mesh.loops]
    except Exception:
        try:
            loop_normals = [list(cn.vector) for cn in mesh.corner_normals]
        except Exception as exc:
            print(f"  --fix-eyes: split-normal capture skipped ({exc})")

    # Split into left/right eyeballs by X sign about the mean, so each sphere
    # rotates about its OWN center — no socket swap, no per-eye texture swap.
    mean_x = sum(verts[i].co.x for i in eye_vert_idx) / len(eye_vert_idx)
    clusters = {"L": [], "R": []}
    for i in eye_vert_idx:
        clusters["L" if verts[i].co.x < mean_x else "R"].append(i)

    total = 0
    for name, idxs in clusters.items():
        if not idxs:
            continue
        cx = sum(verts[i].co.x for i in idxs) / len(idxs)
        cy = sum(verts[i].co.y for i in idxs) / len(idxs)
        for i in idxs:
            co = verts[i].co
            # 180 deg about vertical (Z) through (cx, cy): negate the X and Y
            # offsets from the eye center, keep Z (height) unchanged.
            co.x = 2.0 * cx - co.x
            co.y = 2.0 * cy - co.y
        total += len(idxs)
        print(f"  --fix-eyes: spun {name} eye ({len(idxs)} verts) about "
              f"(x={cx:.3f}, y={cy:.3f})")

    # Push the eyeballs back into their sockets. After the spin, the sphere's
    # rounded front hemisphere sits proud of the eyelids; a small backward
    # translation seats the cornea behind the lids. The head faces Blender +Y
    # at this stage (the flip_z bake maps the face to glTF -Z / engine +Z), so
    # "into the skull" is -Y. Translation does not affect normals.
    if inset:
        for i in eye_vert_idx:
            verts[i].co.y -= inset
        print(f"  --fix-eyes: inset eyes {inset:.3f}m into sockets (-Y)")

    if loop_normals is not None:
        for li in eye_loop_idx:
            nx, ny, nz = loop_normals[li]
            loop_normals[li] = (-nx, -ny, nz)
        try:
            if hasattr(mesh, "use_auto_smooth"):
                mesh.use_auto_smooth = True
            mesh.normals_split_custom_set(loop_normals)
        except Exception as exc:
            print(f"  --fix-eyes: could not write split normals ({exc}); "
                  f"relying on recomputed normals")

    mesh.update()
    print(f"  --fix-eyes: done ({total} eye verts; iris now faces front)")


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

        if short in HIT_ZONES:
            extras["hitZone"] = HIT_ZONES[short]

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
    hit_zone_counts = {}
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
        hz = extras.get("hitZone")
        if hz:
            hit_zone_counts[hz] = hit_zone_counts.get(hz, 0) + 1
    for mask, count in sorted(mask_counts.items()):
        print(f"  poseMask '{mask}': {count} bone(s)")
    if socket_count:
        print(f"  sockets: {socket_count}")
    for zone, count in sorted(hit_zone_counts.items()):
        print(f"  hitZone '{zone}': {count} bone(s)")
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
    merge_animations(args.input_dir, args.scale, args.base, args.exclude_meshes, args.yaw)
    if args.fix_eyes:
        flip_eye_geometry(inset=args.eye_inset)
    if args.strip_face:
        strip_facial_bones()
    validate_model()
    export_gltf(args.output)
    if not args.no_tag:
        tag_gltf_skeleton(args.output)


if __name__ == "__main__":
    main()
