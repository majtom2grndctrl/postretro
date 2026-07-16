"""Headless Blender pipeline: turn a high-poly source glTF into an engine-ready
low-poly character/prop with a freshly baked texture atlas.

Postretro's model loader draws exactly ONE glTF mesh node and samples only the
base-color slot (see context/lib/resource_management.md §7). Downloaded assets
(Sketchfab, Rodin, etc.) are usually the opposite: many mesh nodes, many
materials, and a poly count far above the retro budget. Aggressive polygon
reduction alone corrupts the original UVs, so the texture stops lining up.

This script fixes all of that in one Blender session:

  1. Import the source and JOIN every mesh into a single object.
  2. Weld micro-cracks, then Collapse-decimate to a target triangle count
     (cleaner topology than gltfpack's aggressive mode — no holes/slivers).
  3. Smart-UV-unwrap the decimated mesh (fresh, non-overlapping layout).
  4. Cycles selected-to-active bake of the diffuse albedo from the high-poly
     onto a new atlas — consolidating many source materials into one.
  5. Export a loader-ready glTF: single mesh, one material, Y-up baked into the
     vertices (identity node → correct feet-at-origin), tangents omitted (the
     loader defaults them and rejects degenerate ones; base color is all the
     engine uses).

Author the source feet-at-origin at final ~2 m scale before running.

Requires Blender 4.5 LTS (Intel-Mac compatible; 5.0 dropped x86-64). Run:

    blender --background --python tools/blender_model_rebake.py -- \
        <source.gltf> <out_atlas.png> <out.gltf> <atlas_res> <target_tris>

The source glTF's textures must resolve from its own directory (stage the file
where its `textures/` siblings live). After export, bake the atlas into the
runtime `.prm` cache:

    cargo run -p xtask -- bake-model-textures <out.gltf>
"""
import bpy, sys, math

argv = sys.argv[sys.argv.index("--") + 1:]
HI, OUT_PNG, OUT_GLTF, RES, TARGET = argv[0], argv[1], argv[2], int(argv[3]), int(argv[4])

bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.preferences.addon_enable(module='io_scene_gltf2')

before = set(bpy.data.objects)
bpy.ops.import_scene.gltf(filepath=HI)
hi_objs = [o for o in bpy.data.objects if o not in before and o.type == 'MESH']

# Join all source meshes into a single object (the color source).
bpy.ops.object.select_all(action='DESELECT')
for o in hi_objs:
    o.select_set(True)
bpy.context.view_layer.objects.active = hi_objs[0]
if len(hi_objs) > 1:
    bpy.ops.object.join()
hi = bpy.context.view_layer.objects.active
hi.name = "hi_src"
print(f"[rebake] source tris={len(hi.data.polygons)}")

# Duplicate -> the decimation target.
bpy.ops.object.select_all(action='DESELECT')
hi.select_set(True)
bpy.context.view_layer.objects.active = hi
bpy.ops.object.duplicate()
lo = bpy.context.view_layer.objects.active
lo.name = "lo_target"

# Weld micro-cracks so decimation can't tear them into holes.
bpy.ops.object.mode_set(mode='EDIT')
bpy.ops.mesh.select_all(action='SELECT')
bpy.ops.mesh.remove_doubles(threshold=0.0008)  # 0.8 mm
bpy.ops.mesh.normals_make_consistent(inside=False)
bpy.ops.object.mode_set(mode='OBJECT')

# Collapse-decimate to the target triangle count.
faces_now = len(lo.data.polygons)
ratio = min(1.0, TARGET / max(1, faces_now))
mod = lo.modifiers.new("dec", 'DECIMATE')
mod.decimate_type = 'COLLAPSE'
mod.ratio = ratio
mod.use_collapse_triangulate = True
bpy.context.view_layer.objects.active = lo
bpy.ops.object.modifier_apply(modifier="dec")
print(f"[rebake] decimated {faces_now} -> {len(lo.data.polygons)} tris (ratio {ratio:.5f})")

# Fresh UV unwrap on the low-poly.
while lo.data.uv_layers:
    lo.data.uv_layers.remove(lo.data.uv_layers[0])
lo.data.uv_layers.new(name="UVMap")
bpy.ops.object.select_all(action='DESELECT')
lo.select_set(True)
bpy.context.view_layer.objects.active = lo
bpy.ops.object.mode_set(mode='EDIT')
bpy.ops.mesh.select_all(action='SELECT')
bpy.ops.uv.smart_project(angle_limit=math.radians(66.0), island_margin=0.02)
bpy.ops.object.mode_set(mode='OBJECT')

# New atlas + material with it as the active (bake target) node.
img = bpy.data.images.new("rebake_atlas", RES, RES, alpha=True)
mat = bpy.data.materials.new("rebaked")
mat.use_nodes = True
nt = mat.node_tree
bsdf = nt.nodes.get("Principled BSDF")
tex = nt.nodes.new("ShaderNodeTexImage")
tex.image = img
nt.links.new(tex.outputs["Color"], bsdf.inputs["Base Color"])
nt.nodes.active = tex
lo.data.materials.clear()
lo.data.materials.append(mat)

# Cycles albedo bake, selected-to-active (high-poly -> low-poly).
scn = bpy.context.scene
scn.render.engine = 'CYCLES'
scn.cycles.device = 'CPU'
scn.cycles.samples = 4
bake = scn.render.bake
bake.use_selected_to_active = True
bake.use_pass_direct = False
bake.use_pass_indirect = False
bake.use_pass_color = True
bake.cage_extrusion = 0.06  # raise if faces bake black (rays missing the surface)
bake.max_ray_distance = 0.0
bake.margin = 8

bpy.ops.object.select_all(action='DESELECT')
hi.select_set(True)
lo.select_set(True)
bpy.context.view_layer.objects.active = lo
print("[rebake] baking...")
bpy.ops.object.bake(type='DIFFUSE', pass_filter={'COLOR'},
                    use_selected_to_active=True, cage_extrusion=0.06, margin=8)

alpha = list(img.pixels)[3::4]
covered = sum(1 for a in alpha if a > 0.5)
print(f"[rebake] atlas coverage: {100 * covered / len(alpha):.1f}%")

img.filepath_raw = OUT_PNG
img.file_format = 'PNG'
img.save()
bsdf.inputs["Metallic"].default_value = 0.0
bsdf.inputs["Roughness"].default_value = 1.0

# Export only the low-poly. No tangents: the loader defaults them and rejects
# degenerate ones, and only base color is consumed.
bpy.ops.object.select_all(action='DESELECT')
lo.select_set(True)
bpy.context.view_layer.objects.active = lo
bpy.ops.export_scene.gltf(
    filepath=OUT_GLTF, export_format='GLTF_SEPARATE', use_selection=True,
    export_apply=True, export_texcoords=True, export_normals=True,
    export_tangents=False, export_materials='EXPORT', export_yup=True)
print("[rebake] exported", OUT_GLTF)
