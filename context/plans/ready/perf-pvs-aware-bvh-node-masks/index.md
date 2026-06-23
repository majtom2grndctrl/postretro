# PVS-Aware BVH Node Masks

## Goal

Make the runtime BVH cull pass skip invisible portal regions before it reaches
BVH leaves. Keep the single global BVH and material-bucket draw layout. Remove
the large-map landmine where the GPU walks most of the map even after CPU portal
visibility has already rejected it.

## Scope

### In scope

- Add compact visibility masks to the existing global BVH node array.
- Reuse the current fixed 4096-cell visibility model.
- Let the camera BVH cull shader skip internal BVH subtrees that contain no
  currently visible cell group.
- Support measured mask widths of 4, 8, 16, and 32 `u32` words per BVH node.
- Add `prl-build --bvh-node-mask-words {0|4|8|16|32}`. `0` means legacy
  no-mask output; `4`, `8`, `16`, and `32` mean masked output.
- Preserve the current flat global BVH for runtime and bake paths. Lightmap,
  SH/direct/delta-SH, chunk-light-list, and animated-weight-map bakes consume
  the live `bvh::Bvh` plus `BvhPrimitive` list; SDF and navmesh are
  geometry/BSP-derived and unaffected.
- Keep the existing material-bucket leaf order and indirect draw ranges.
- Use the committed stress-warren maps as the performance harness.
- Add behavior-neutral measurement instrumentation when existing logs do not
  expose required baseline counters. Instrumentation must not change cull
  traversal, command encoding, shader inputs, or measured GPU work.

### Out of scope

- Per-region BVHs.
- Quake-style baked leaf-to-leaf PVS.
- Runtime BVH refit for kinematic brushes or entities.
- Changing portal traversal semantics.
- Changing the 4096-cell visible-cell bitmask capacity.
- Changing the BVH build algorithm or replacing the `bvh` crate.
- Optimizing the spot-shadow cone cull path beyond compatibility with the new
  shared shader layout.

## Acceptance Criteria

- [ ] `stress-warren.prl` shows at least a 30% `cull` GPU-time reduction on a
      representative camera view where fewer than half of drawable cells are
      visible, using the narrowest mask width that clears the gate.
- [ ] Baseline measurements report `visible_cell_count`,
      synthetic `visible_cell_group_count` for widths 4, 8, 16, and 32, and old
      `cull` time before node masks.
- [ ] Post-change measurements report `cull` time for masked widths 4, 8, 16,
      and 32.
- [ ] `stress-warren-lit.prl` and `stress-warren-crates.prl` still produce the
      same visible world-geometry set as the current leaf-level cell test,
      verified by the Task 7 CPU mirror or serialized indirect-slot comparison.
- [ ] A small representative map does not regress reported GPU pass-time sum by
      more than 2% averaged over the same `POSTRETRO_GPU_TIMING=1` window. Sum
      the existing labels: `cull`, `animated_lm_compose`, `depth_prepass`,
      `sdf_shadow`, `forward`, `sh_compose`, and `smoke`. If a new timing label
      is added for the camera indirect clear, include it in this sum.
- [ ] Camera cull output is deterministic against the logical leaf-level
      result after exact frustum plus cell tests: no visible leaf is lost and
      no nonvisible leaf is submitted.
- [ ] Old BVH sections with no node masks still load and use the current global
      traversal behavior. Regression coverage proves existing writer output has
      zero header padding and `node_visibility_mask_words == 0`.
- [ ] At least one committed legacy PRL or generated legacy fixture loads with
      `node_visibility_mask_words == 0` and normalizes to the runtime all-ones
      fallback.
- [ ] The `Bvh` section bytes intentionally change for masked output, while
      section-level bytes for non-BVH PRL sections remain unchanged for
      unchanged inputs. Warm-cache outputs for the selected fixture remain
      byte-identical for cache entries that exist before and after the change;
      document the comparison command in Task 7 output.
- [ ] With node masks present, `ShadowCullPipeline` writes all-ones visible
      groups, binds the all-ones exact visible-cell buffer at binding 3, and
      produces the same cone-only occluder set as the current path.
- [ ] `context/lib/build_pipeline.md` and
      `context/lib/rendering_pipeline.md` are updated after implementation with
      node masks, fallback loading, camera cull flow, and shadow all-ones
      behavior.
- [ ] `cargo test -p postretro-level-format`, `cargo test -p postretro-level-compiler`,
      and `cargo test -p postretro` pass.
- [ ] No new `unsafe`; verify as a grep/review gate with `rg '\bunsafe\b'` and
      review any approved existing exceptions.

## Tasks

### Task 1: Measure and Capture Baselines

Run the existing stress fixtures before changing the cull path. If the required
baseline counters are not already exposed, first add logging or an offline
inspection helper that is behavior-neutral: it may read CPU/runtime structures
and report synthetic group counts, but it must not change BVH traversal,
command encoding, shader inputs, or measured GPU work. Capture
`POSTRETRO_GPU_TIMING=1` output for `stress-warren.prl`,
`stress-warren-lit.prl`, `stress-warren-crates.prl`, and one small
representative map. Record adapter, build mode, map, camera position, visible
cell count, synthetic visible group counts for 4-, 8-, 16-, and 32-word masks,
BVH node count, BVH leaf count, and averaged old `cull` time in
`context/plans/in-progress/perf-pvs-aware-bvh-node-masks/research.md` when
orchestrated. If this task runs before the plan moves to `in-progress/`, write
the same `research.md` file in the current plan folder.

The camera setup must be reproducible: record exact map path, camera transform,
FOV, resolution, build profile, timing window, and whether the position came
from a scripted placement or a manual debug-camera capture.

### Task 2: Split BVH Loading Out of `prl.rs`

Move BVH section conversion and BVH-specific loader tests out of the oversized
PRL loader file before extending the loaded BVH shape. Keep the public
`LevelWorld` contract unchanged. The split should be behavior-preserving and
small: `load_prl` still reads the `Bvh` section, but delegates format-to-runtime
conversion to a focused module.

### Task 3: Add Node Visibility Masks to the BVH Section

Extend the level-format BVH section with an optional per-node mask table. The
existing node and leaf records keep their byte layout. The compiler computes
one mask per flattened node while building the existing global `BvhSection`.
Leaf masks are derived from `cell_id`; internal masks are the union of child
masks. Existing bake consumers keep using the live `bvh::Bvh` returned by
`build_bvh`. The compiler already rejects `cell_id >= 4096`; masked `Bvh`
sections must also reject any leaf with `cell_id >= 4096`. Legacy no-mask
sections keep the old exact-leaf shader behavior.

Preserve the existing BVH serialization seam in
`crates/level-compiler/src/pack.rs::serialize_bvh_with_chunk_ranges`. Masked
serialization must still stamp `chunk_range_start` and `chunk_range_count` into
`BvhLeaf` records immediately before `BvhSection::to_bytes()`.

`BvhSection::from_bytes` must reject non-zero `node_visibility_mask_words`
values other than 4, 8, 16, and 32. It must validate masked payload size using
the decoded mask stride before reading nodes and leaves.

Mask width is a compiler-selected output setting. Initial masked candidates are
4, 8, 16, and 32 words per node. Add
`prl-build --bvh-node-mask-words {0|4|8|16|32}`. Before Task 7 selects a final
default, `0` or an absent flag emits legacy no-mask output with
`node_visibility_mask_words == 0`; non-zero values emit masked output. Invalid
values fail with a compiler error. Measurement builds start with an explicit
`--bvh-node-mask-words 16`, then Task 7 selects the final default. If Task 7
changes unflagged compiler output to a non-zero width, explicit
`--bvh-node-mask-words 0` remains the deliberate legacy/no-mask path for
regression fixtures and compatibility tests.

Add a regression fixture or test proving legacy writer output still encodes
zero header padding and `node_visibility_mask_words == 0` for no-mask BVH
sections. Include either a committed legacy PRL or a generated legacy fixture in
loader coverage so compatibility is tested against bytes with no mask table.

### Task 4: Load and Upload Node Masks

Add the mask table to the runtime `BvhTree` and upload it beside the existing
node and leaf storage buffers. If a loaded PRL has no node masks, fill the
runtime mask array with all bits set so the shader takes the old path without a
branch or special bind-group layout. Legacy sections with
`node_visibility_mask_words == 0` normalize at load time to 32 runtime words per
node, all ones. Masked sections preserve their loaded word count so shader
indexing matches the uploaded table stride.

Expose the uploaded node-mask buffer and runtime mask-word stride through the
same renderer-owned plumbing that lets `ShadowCullPipeline` share the camera
cull node and leaf buffers. Camera and shadow cull must bind the same shared
shader layout.

Preserve existing no-world-geometry behavior. A zero-node BVH may either skip
BVH cull setup as today or allocate the existing minimum-size placeholder
buffers, but it must not require a real node-mask table.

### Task 5: Make Camera Cull PVS-Aware

Extend the cull uniforms with a compact visible-cell-group mask. The camera
path derives that mask from `VisibleCells` each frame. The WGSL traversal tests
the current node mask before its AABB test and jumps to `skip_index` when the
node contains no visible group. The leaf-level `cell_is_visible` test remains
in place as the exact final gate.

Carry the shared shader layout explicitly. Keep existing bindings 0..5 and add
node masks at binding 6. `CullUniforms` keeps the six `vec4<f32>` frustum
planes first, then appends `visible_cell_groups: array<vec4<u32>, 8>` and
`node_visibility_mask_words: u32`; serialized size is 240 bytes including
16-byte tail padding. Node-mask storage is a flat `array<vec4<u32>>`; node `i`
starts at `i * (node_visibility_mask_words / 4)`. Widths below 32 zero unused
visible-group lanes.

Do not enable internal-node PVS skips unless Task 6's camera indirect clear is
already present. If Task 5 lands first during manual development, keep the skip
disabled behind a local test gate until the clear exists.

Group derivation is based on the returned `VisibleCells`: portal, solid-leaf,
exterior-camera, and no-portals fallback paths that return
`VisibleCells::Culled(cells)` set groups from those cells. `DrawAll` sets all
groups.

### Task 6: Clear Indirect Slots Before Camera Traversal

Clear the camera indirect buffer before dispatch. This makes skipped invisible
subtrees safe: leaves the shader never visits remain zero instead of retaining
an earlier frame's draw command. Clear only the camera world-geometry indirect
range `0..total_leaves * DRAW_INDIRECT_SIZE`, before camera BVH dispatch and
before shadow cull writes. Do not clear shadow indirect regions here. Keep the
existing cull-status clear. It writes `0`, the existing portal-culled /
not-submitted enum value, so descendants skipped by node-mask traversal inherit
that debug state.

Encode the clear outside the existing `cull` compute pass timing unless this
task also adds a dedicated timing pair for it. Task 7 owns clear-cost reporting.

### Task 7: Verify Rendering and Performance

Add unit tests for mask serialization, fallback loading, mask derivation, and
visible-group derivation. Automated cargo tests must use a CPU mirror of shader
traversal; tests run without a GPU context. Compare the CPU mirror against
logical leaf-level frustum plus exact-cell results for representative masked
trees and visibility sets. GPU readback, serialized indirect-slot inspection,
and the BVH overlay are manual diagnostics only. Run the baseline maps again
and compare before/after `cull` timing plus reported GPU pass-time sum. Use the
BVH overlay to spot-check that invisible subtrees disappear without hiding
visible cells.

Compare 4-, 8-, 16-, and 32-word masks on the stress fixtures. Pick the
narrowest width that clears the performance gate without regressing the small
representative map. Rebuild stress fixture PRLs once per width.

Set or intentionally leave the unflagged `prl-build` default after measuring.
Document the decision in `research.md`. Explicit `--bvh-node-mask-words 0`
must remain the legacy/no-mask path either way.

Report the camera indirect-buffer clear cost separately when GPU timing makes
that practical. If the clear is encoded outside the timed cull compute pass,
either add a dedicated timing pair or explicitly note that `cull` timing does
not include the clear.

Verify unchanged inputs keep identical section-level bytes for non-BVH PRL
sections. For cache output, use a warm cached build of the selected fixture and
compare `.build-caches/prl-cache/` entries that exist both before and after the
change. Document the exact command and fixture in `research.md`.

Verify shadows with masked maps: `ShadowCullPipeline` writes all-ones visible
groups, binds the all-ones exact visible-cell buffer at binding 3, and produces
the same cone-only occluder set as the current path.

### Task 8: Update Context Docs

After implementation, update `context/lib/build_pipeline.md` and
`context/lib/rendering_pipeline.md` with the BVH node masks, fallback loading,
camera cull flow, and shadow all-ones behavior.

## Sequencing

**Phase 1 (sequential):** Task 1 — establishes the baseline and exact test views.
**Phase 2 (sequential):** Task 2 — split-before-extend for the oversized loader.
**Phase 3 (concurrent):** Task 3, Task 4 — format/compiler and runtime data plumbing share the new section contract.
**Phase 4 (sequential):** Task 6, Task 5 — clear indirect slots before enabling shader skips.
**Phase 5 (sequential):** Task 7, Task 8 — consumes all implementation output.

## Rough Sketch

### Current Bottleneck Shape

`visibility::determine_visible_cells` produces `VisibleCells`. The camera
`ComputeCullPipeline` writes that to the 128-word visible-cell bitmask. The
current `bvh_cull.wgsl` walks the global BVH with one invocation and only tests
`BvhLeaf.cell_id` after reaching a leaf. Internal `BvhNode` records know AABB
and `skip_index`, but know nothing about which cells their subtrees contain.

This plan makes internal nodes PVS-addressable without partitioning the tree.

### Mask Granularity

Use a coarse cell-group mask per BVH node. Mask width is measured, not guessed.

| Mask words | Groups | Cells per group | Storage per node |
|---:|---:|---:|---:|
| 4 | 128 | 32 | 16 bytes |
| 8 | 256 | 16 | 32 bytes |
| 16 | 512 | 8 | 64 bytes |
| 32 | 1024 | 4 | 128 bytes |

For a selected width:

- `group_count = node_visibility_mask_words * 32`
- `cells_per_group = 4096 / group_count`
- `cell_group_id = cell_id / cells_per_group`
- `mask_word = cell_group_id >> 5`
- `mask_bit = 1u32 << (cell_group_id & 31)`

This reuses the existing fixed 4096-cell budget and avoids a new clustering
algorithm. It is conservative: a group bit can keep a subtree alive even when
only a different cell in the same group is visible. Finer widths reduce false
positive groups on room-heavy and multi-layer maps. The leaf-level
`cell_is_visible` test remains exact.

Per-region BVH and compiler-derived clustering stay fallback options if 32-word
masks are still too blunt on real content.

### Compiler

`bvh_build::build_bvh` still returns the live `Bvh<f32, 3>`,
`Vec<BvhPrimitive>`, and flattened `BvhSection`. `BvhPrimitive.cell_id` is the
only input needed for visibility masks.

`flatten` should compute masks for the flattened DFS node order:

- leaf node: one bit for `leaf.cell_id / cells_per_group`
- internal node: bitwise OR of child masks
- empty tree: empty mask table

The mask table is parallel to `BvhSection.nodes`, so node `i` reads mask `i`.
It does not depend on post-sort leaf order. The mask table stride is
`node_visibility_mask_words`.

### Runtime

`BvhTree` gains the loaded node masks. `ComputeCullPipeline::new` uploads them
once at level load, next to the node and leaf buffers.

Runtime mask width is non-zero for any uploaded BVH. Legacy no-mask sections
upload one 32-word all-ones mask per node and set the shader-visible width to
32.

`ComputeCullPipeline::dispatch` derives two visibility products:

- existing 128-word exact visible-cell bitmask
- new visible-cell-group mask with `node_visibility_mask_words` active words

For `VisibleCells::DrawAll`, the group mask is all ones. For
`VisibleCells::Culled(cells)`, set one group bit for each visible cell.
Portal, solid-leaf, exterior-camera, and no-portals fallback paths follow this
same returned-`VisibleCells` rule.

`ShadowCullPipeline` shares `CULL_SHADER_SOURCE`, so it must bind the node-mask
buffer and write all-ones group masks in its per-slot uniforms. It must also
continue binding the all-ones exact visible-cell buffer at binding 3 because the
shared shader still keeps the exact leaf-level `cell_is_visible` gate. This
neutralizes PVS skipping for shadows, preserving the current cone-only shadow
behavior.

### Shader

`bvh_cull.wgsl` adds one read-only node-mask binding and extends `CullUniforms`
with the visible-group words. `CullUniforms` keeps the six `vec4<f32>` frustum
planes first, then appends `visible_cell_groups: array<vec4<u32>, 8>` and
`node_visibility_mask_words: u32`. The fixed uniform layout supports the
maximum planned width: 32 words = 8 `vec4<u32>` lanes. Widths below 32 leave
unused lanes zeroed. Serialized uniform size is 240 bytes, including tail
padding to 16-byte alignment. Node-mask storage is a flat `array<vec4<u32>>`;
node `i` starts at `i * (node_visibility_mask_words / 4)`.

Keep the existing bindings 0..5 and add node masks at binding 6. Camera and
shadow bind group layouts must mirror this shared shader.

At the top of the traversal loop:

1. Read current node mask.
2. If mask intersects the visible-group mask, continue.
3. Otherwise jump to `node.skip_index`.

Run this before AABB testing. Invisible portal regions skip without paying
frustum math. The exact leaf cell test remains after leaf AABB testing.

### Indirect Buffer Correctness

The current traversal can leave stale draw commands for subtrees skipped by
internal-node frustum rejection, but those commands clip outside the current
projection. PVS-based internal skips are different: skipped leaves can be inside
the camera frustum but outside the portal-visible set. They must not retain old
draw commands.

Clear the camera indirect buffer before the cull dispatch. Then the shader only
needs to write positive draw commands for leaves that survive all tests. Existing
zero writes can remain for clarity and cull-status updates.

Clear only `0..total_leaves * DRAW_INDIRECT_SIZE` in the camera world-geometry
indirect buffer range. Run it before camera BVH dispatch and before shadow cull
writes. Do not clear shadow indirect ranges. The cull-status clear writes `0`,
the existing portal-culled / not-submitted enum value; skipped descendants keep
that status.

## Wire Format

The existing `Bvh` PRL section keeps section ID 19 and little-endian encoding.

Header:

| Field | Type | Meaning |
|---|---|---|
| `node_count` | `u32` | Existing flat node count |
| `leaf_count` | `u32` | Existing flat leaf count |
| `root_node_index` | `u32` | Existing root node index |
| `node_visibility_mask_words` | `u32` | Reuses existing header padding at bytes 12..16; `0` for legacy/no masks; `4`, `8`, `16`, or `32` for masked output |

Payload when `node_visibility_mask_words == 0`:

1. `BvhNode * node_count`
2. `BvhLeaf * leaf_count`

Expected size: `HEADER_SIZE + node_count * 40 + leaf_count * 48`.

Payload when `node_visibility_mask_words > 0`:

1. `u32 * node_count * node_visibility_mask_words` node visibility masks
2. `BvhNode * node_count`
3. `BvhLeaf * leaf_count`

Expected size:
`HEADER_SIZE + node_count * node_visibility_mask_words * 4 + node_count * 40 + leaf_count * 48`.

`HEADER_SIZE` remains 16. `BvhNode` remains 40 bytes. `BvhLeaf` remains 48
bytes. Unknown non-zero `node_visibility_mask_words` values reject the section.
Only 4, 8, 16, and 32 are valid non-zero values. Masked sections reject any leaf
with `cell_id >= 4096`; legacy no-mask sections keep the old exact-leaf shader
behavior.

Mask bit mapping:

| Value | Meaning |
|---|---|
| group count | `node_visibility_mask_words * 32` |
| cells per group | `4096 / group_count` |
| group id | `cell_id / cells_per_group` |
| mask word | `group_id >> 5` |
| mask bit | `1u32 << (group_id & 31)` |

Old maps keep working because the old header padding decodes as zero mask words.
New maps require an updated engine.

Regression coverage must prove existing writer output keeps zero header padding
and `node_visibility_mask_words == 0` for legacy BVH sections.

## Measurement Questions

- The cell-group mask may be too coarse if raw BSP leaf IDs are poorly
  correlated with portal locality. Measure `visible_cell_count` against
  `visible_cell_group_count` for 4, 8, 16, and 32 words. If 32-word masks still
  spend too much time in `cull`, follow up with compiler-derived visibility
  clusters or revive the per-region BVH draft.
- Clearing the camera indirect buffer adds a per-frame GPU clear. The clear is
  required for correctness once internal-node PVS skips exist, but the before
  and after measurements should report its cost separately if GPU timing makes
  that practical.
