# Research — weapon-mount-frame-solver

Investigation notes. Decisions live in `index.md`.

## The mount math (grounded)

Runtime mount (`crates/postretro/src/scripting/systems/attachments.rs`,
`emit_for_holder`): a skinned attachment draws at `holder_transform *
socket_matrix`, where `socket_matrix = world_pose[joint]` — the socket joint's
world matrix. There is **no per-socket offset**. Rigid bindings use their
load-resolved rest matrix directly.

`socket_matrix` comes from the standard glTF forward sweep
(`crates/model/src/anim/compose.rs`, `compose_world_pose`): `world[i] =
world[parent] * local`. Spec-correct, no reorientation. `SocketBinding`
(`crates/model/src/gltf_loader.rs`) is `SkinnedJoint(usize)` for skinned holders
(the `hand_r` socket → RightHand joint, tagged by `tools/mixamo_to_gltf.py`
`SOCKETS = {"RightHand": "hand_r", "LeftHand": "hand_l"}`) or `RigidRest(Mat4)` for rigid.

Consequence: to make a weapon's barrel point along the character's forward, the
weapon's **own local frame** must pre-compensate for the socket joint's frame.
The compensation is a function of `(weapon local geometry, socket joint frame at
some pose)`. Because the engine reads raw vertices with no import transform
(`resource_management.md` §7: "no import-time normalization, no node-transform
bake into vertices"), the compensation must be baked into the weapon's vertices
— it cannot live in the loader.

## Why not the loader / not runtime

- Loader is deliberately raw (`resource_management.md` §7). Changing it to
  reorient bones/axes would break every other-tool asset and the skinning math
  (inverse-bind composition in `compose_palette`). Explicitly out of scope.
- Engine mount contract (`holder * socket`, zero offset) is deliberate and
  documented. A per-attachment corrective rotation in the descriptor
  (`mesh.attachments`) would fork the mount path and push a coordinate-frame
  concern into every consumer/descriptor. Rejected — see index Direction.
- `build_pipeline.md` (Supported Map Formats): "each input format translates its
  own coordinate axes, angle encoding, and units to engine convention before
  reaching shared compiler logic. Format-specific helpers belong in the format
  adapter, not shared code." This is the precedent for placing authoring-frame →
  engine-frame conversion at a **tooling/compile boundary**, not the runtime.

## The existing solve in socket_dump.rs

`crates/model/examples/socket_dump.rs` (merged to `main`, present in-tree) already:

1. Loads a model via `postretro_model::gltf_loader::load_model` — the SAME
   loader the engine uses (no drift).
2. Samples the socket joint's world matrix at `(clip, time)` via
   `sample_clip_looped_world_modified` — the SAME sampler the attachment path
   uses.
3. Geometrically identifies the weapon's barrel: long axis from the extreme
   vertex pair refined by end-region centroids; **muzzle = the thin end** (smaller
   cross-section radius); **up = the direction opposite the mean mass offset**
   (grip/mag/stock hang below the bore). Builds a right-handed frame
   `G = [side up barrel]`.
4. Computes a corrective delta in glTF space: `D = S^T · G^T`, with
   `S` = the per-column-normalized socket rotation (`s3` — each column of
   `Mat3::from_mat4(socket_matrix)` normalized; the un-normalized `Mat3::from_mat4`
   feeds only the verify metrics), so that `S · D` maps `barrel → +Z`, `up → +Y`.
5. Maps to Blender frame via `C: (x,y,z) -> (x,-z,y)` (`c_map`), then decomposes
   to Blender XYZ euler (`R = Rz·Ry·Rx`) → the `--rotate-euler` degrees.
6. Optionally composes with the current bake's euler (args 6-8) to print the
   TOTAL euler for re-baking from raw source.
7. Reports verify metrics: `barrel·+Z` (1.0 = forward), `barrel·+Y` (0 = level),
   `up·+Y` (1.0 = not rolled).

Compose semantics follow from step 3. Because `socket_dump` detects the barrel
GEOMETRICALLY off the loaded (already-baked) mesh, its delta `D` is a RESIDUAL, so
step 6 composes it onto the current bake's euler to print the TOTAL for re-baking
from raw. The promoted tool's DECLARED-axes path builds `G` from author-declared
axes rather than measured geometry, so its `D` is a FULL from-raw solve emitted
directly — no compose (stateless). Compose is thus the geometric-ASSIST path's
behavior only; see `index.md` Task 2 and AC #6. The change of basis
`C: (x,y,z) -> (x,-z,y)` (`c_map`, `C = [X, Z, -Y]` columns) is a two-sided
similarity on the corrective rotation (`d_b = C·d_gltf·Cᵀ`); check mode inverts it
(`R_gltf = Cᵀ·R_blender·C`) to bring the applied Blender euler back to glTF frame.

This is the solve to promote. Split the ENGINE-frame parts (steps 1-4, 7) from
the BLENDER-adapter parts (steps 5-6): the Blender-XYZ mapping is authoring-tool
specific and belongs at the tool layer, per the format-adapter philosophy above.

## The false-positive history

Geometric muzzle detection is a heuristic. The task brief records an earlier bug:
the muzzle was mis-identified, producing a mount that read as "correct" but faced
backward. Root cause shape: self-consistent geometry solve (barrel maps to +Z)
does not prove the barrel was the real barrel. Mitigation in index: let the
author **declare** the weapon-local barrel/up axes (intent they can see), use
geometry only as an assist, and verify against the declared forward — not just
self-consistency.

## The "how many bakes" question (settled — index Decision 3)

`content/dev/scripts/limitator.ts` comment: the AR_4 rifle mounts "with its
grip-relative orientation re-tuned for this skeleton bake." The correction is
computed per `(weapon, socket joint frame at reference pose)`. Whether one weapon
bake serves multiple characters depends on whether their socket joint frames
(in model space) agree. All humanoids share the Mixamo rig (memory:
character-model-provenance), but `mixamo_to_gltf.py --yaw` rotates the whole
skeleton about vertical Z (including the hand joint's model-space frame), so two
characters baked at different yaws could need different corrections. Decision 3
settles the design call: default to one bake per weapon, keep it empirically
verifiable (solve against each target socket and compare the emitted eulers), and
document a per-character corrective — a separate baked weapon file per target, each
with its own `extras.mount.euler` — as the escape hatch if socket frames diverge.

Also pose-dependence: `socket_matrix` varies per `(clip, time)`. A rigid bake is
exact only at the reference pose. Poses that reorient the wrist (the limitator's
crouching reload clip) tip the rigidly-mounted weapon — `limitator.ts` documents
this and leaves reload unwired. Fixing wrist-reorienting poses needs a skinned
weapon, not a better bake. Out of scope.

## Compiled-asset precedents

- **xtask subcommand shape:** `crates/xtask/src/main.rs` `try_main()` dispatches
  subcommands via a flat `if command == "..."` chain (~lines 33-66). Existing:
  `run`, `observe`, `capture`, `mint-identity`, `bake-model-textures`,
  `crate-graph`. A new subcommand = one branch + a handler fn + a `parse_*_args`
  helper + a `print_help()` line (~543). `bake-model-textures` (lines ~68-171) is
  the closest precedent: a standalone `<scene.gltf>`-consuming step that reaches
  into `postretro-level-compiler`'s lib.
- **Binary/manual model:** `prl-build` (crate `postretro-level-compiler`,
  `crates/level-compiler/`) is a standalone CLI, invoked manually
  (`cargo run -p postretro-level-compiler -- input.map -o out.prl`), not a
  `build.rs` step. Model-texture baking (`bake-model-textures`) is likewise a
  manual xtask, content-driven, no PRL section. Precedent: design-time asset
  tooling is a manual command, not an automatic build hook.
- **The Blender converter:** `tools/prop_to_gltf.py` (under `tools/`, not
  `crates/`) already joins meshes to one node, relocates origin to `--grip`,
  applies `--scale`, bakes `--rotate-euler` into vertices (`rotate_mesh`, applied
  AFTER grip so rotation pivots at the grip), strips tangents/skin, validates the
  engine contract, exports glTF Separate, and writes `--socket NAME=NODE` extras.
  It is the mesh-vertex-baking authority (Blender owns the transform apply). The
  solver should feed it a computed `--rotate-euler`, not reimplement vertex
  rotation + re-export in Rust.

## `extras` persistence grounding (index Decision 1)

The single-source-of-truth `extras.mount` contract in Decision 1 reuses two
existing mechanisms, verified in source:

- **Write side — `tools/prop_to_gltf.py` `postprocess_gltf`.** After the glTF
  export, it re-opens the `.gltf` JSON and, for each `--socket NAME=NODE`, finds
  the node by name and merges the tag into that node's `extras`
  (`extras = node.get("extras") or {}; extras["socket"] = socket_name; node["extras"] = extras`),
  then rewrites the file. This is exactly the node-`extras` write Decision 1
  extends with a `--mount-axes` flag writing `extras.mount = {barrel, up, euler}`
  onto the mesh node; `euler` is the `--rotate-euler` value the same run applied
  (`rotate_mesh`). No new export machinery — one more key in the same post-process.
- **Read side — `crates/model/src/gltf_extras.rs`.** Per-node metadata readers
  (`read_socket_name`, `read_joint_zone`, `read_pose_masks`) each take a node's
  `gltf::json::Extras`, deserialize with `serde_json`, and degrade to `None`/
  default on any error — author metadata never fails the load. `mixamo_to_gltf.py`
  writes the skeleton's `socket`/`hitZone`/`poseMask` extras the same way. The new
  `read_mount_axes` mirrors `read_socket_name`: read the mesh node's `extras.mount`,
  return raw-source-frame `barrel`/`up` plus optional `euler`, degrade to `None`.
  Granularity note: `barrel`/`up` are the core pair (either absent/malformed →
  whole `mount` `None`); `euler` degrades independently to its own `None`.
- **Reader visibility / crate-boundary access (verified).** All `gltf_extras.rs`
  readers are `pub(crate)` (`read_socket_name` at `gltf_extras.rs`, callers only in
  `gltf_loader.rs`), so xtask cannot call `read_mount_axes` directly. The pattern to
  mirror: `read_socket_name` feeds the PUBLIC `LoadedModel.sockets` field
  (`gltf_loader.rs`: `pub sockets: HashMap<..>` on `LoadedModel`; populated inside
  `load_model` via `build_skinned_sockets`/`build_rigid_sockets`, which call
  `read_socket_name`, then placed in the returned `LoadedModel` struct literal).
  So the mount reader is surfaced the same way: a new `pub mount: Option<MountAxes>`
  field populated inside `load_model` from `SelectedModel.mesh_node`'s `extras`,
  read by xtask across the boundary while `read_mount_axes` stays `pub(crate)`.
  `sample_clip_looped_world_modified` lives in `postretro_model::anim`
  (`anim/mod.rs`), not `gltf_loader`.

Frame note: the persisted `barrel`/`up` are raw-source-frame (the intent),
frame-invariant across re-bakes, so they persist correctly even though the baked
weapon they live on is rotated; CHECK composes the persisted `euler` onto them to
reach the baked frame. The CLI `--barrel`/`--up`/`--current-euler` remain as the
first-author bootstrap (extras absent) and override path (CLI wins, re-persisted at
the next bake).

## Fixtures

- `content/dev/models/limitator/model.gltf` — skinned holder, carries
  `extras.socket` `hand_l` / `hand_r`.
- `content/dev/models/ar_4/model.gltf` — the AR_4 weapon/prop, node `AR_4`, no
  socket tags (it is the attached prop).
- Verify path today: `cargo run -p postretro-model --example socket_dump --
  content/dev/models/limitator/model.gltf idle_aiming hand_r 0
  content/dev/models/ar_4/model.gltf`.

## Doc capture (at promotion, not now)

`resource_management.md` §7 already documents socket-mount verification against
the engine frame and names `socket_dump` + `prop_to_gltf.py --rotate-euler`. At
promotion, update §7 to name the new solver tool and the declare-axes authoring
loop, and demote the raw-euler trial-and-error description.
