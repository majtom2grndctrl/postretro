# Billboard Sprite PRM Baking

## Goal

Move billboard sprite-collection textures off the runtime PNG-load-and-stitch
path and onto the `prl-build` → `.prm` baking pipeline that already serves world
and model textures. Sprites gain a Mitchell-Netravali mip chain and are sampled
with mipmaps at runtime, eliminating the distance shimmer that single-mip strips
produce. Sprite collections become aesthetically consistent with world surfaces:
same filter, same linear-space downsample, same `.prm` sidecar addressing.

## Scope

### In scope

- Compile-time stitching: `prl-build` discovers sprite collections from the
  map's `billboard_emitter` placements, stitches each collection's frames into a
  single horizontal strip, bakes a **per-frame-independent** mip chain, and
  writes a diffuse-only `.prm` sidecar — content-addressed exactly like a
  world/model diffuse sidecar.
- Per-frame mip independence: each animation frame is downsampled in isolation
  (edge-clamped) and the per-level results are re-stitched, so no mip level
  bleeds one frame's texels into the next. This is what makes the strip layout
  safe to mip.
- Runtime load: the renderer content-hashes the collection's frames the same way
  the compiler does, opens `<key>.prm` directly, and uploads the baked mip chain.
  No PRL section carries sprite keys — the prop_mesh "no-section, runtime re-hash"
  pattern.
- Mip-aware sprite sampler: `mipmap_filter` flips from `Nearest` to `Linear`;
  `lod_max_clamp` is set per collection from the baked chain depth.
- Frame count stays runtime-derived from the PNG file count (unchanged source of
  truth); it continues to reach the shader through `SpriteDrawParams`.
- Fallback parity: a collection that fails to resolve, hash, or load degrades to
  the existing 1×1 white-frame placeholder, load continues.

### Out of scope

- **Texture-array layers per frame.** Would force `D2Array` bind-group/shader
  changes and rewrite the strip UV math. The strip + per-frame-independent mips
  reaches the same anti-bleed result without touching the WGSL `SpriteInstance`
  layout or the `u = (frame_idx + cd.z) / frame_count` convention. See *Design
  decision 3*.
- **A PRL sprite section** (TextureNames/TextureCacheKeys analog for sprites).
  Sprite collection names are not fully known at compile time. See *Design
  decision 2*.
- **Frame count in the PRM header or a baked KVP.** See *Design decision 1*.
- Specular/normal slots for sprites. Sprites bake diffuse-only, matching the
  model path.
- The billboard pass blend mode, depth state, lighting math, and the
  `SpriteInstance` storage-buffer layout.
- FGD changes. No new KVPs. `billboard_emitter.sprite` is unchanged.
- Sprite collections that only ever spawn from data-script descriptors or the
  hardcoded weapon-impact effect — the compiler cannot see these. They keep
  working via the runtime fallback (see *Open questions*); baking them is a
  follow-up, not this slice.

## Design decisions

### Decision 1 — Frame count stays runtime-derived from the PNG count

**Chosen:** the runtime keeps counting `<collection>_NN.png` files to derive
`frame_count`, exactly as `load_collection_frames` does today. Stitching moves to
compile time; counting does not.

**Why:** `frame_count` already flows runtime → `SpriteDrawParams.params.x` →
shader UV math, and that path is untouched. Persisting the count in the PRM
header or a baked KVP would add a wire-format surface and a second source of
truth for a value the runtime can recover for free by listing the same directory
it already lists to compute the content hash. The shader's frame-count sourcing
stays byte-for-byte unchanged, satisfying the invariant. The compiler and runtime
must agree on frame **order and count** so the baked strip's column layout
matches the runtime's UV math — both derive it from the same sorted-by-numeric-
suffix file scan, so the order is already shared by construction (lift the scan
into a shared helper; see *Rough sketch*).

### Decision 2 — No PRL section; runtime re-hash (prop_mesh pattern)

**Chosen:** sprite `.prm` sidecars carry **no** PRL key section. The runtime
content-hashes the collection's frame bytes at load time and opens
`<key>.prm` directly, mirroring `prop_mesh` model textures.

**Why:** world textures get a PRL section because their full set is known at
compile time (from `TextureNames`). Sprite collections are **not** fully known at
compile time — names arrive from three runtime sources:
`billboard_emitter.sprite` map KVPs (compiler-visible via MapEntity), data-script
descriptor archetypes (runtime-only), and the hardcoded weapon-impact collection
(`weapon::impact_sprite_collection()` → `"impact"`). A PRL section could only
cover the first source, leaving the runtime to re-hash for the other two anyway —
so the section earns nothing. The content-hash-at-load pattern already exists,
is tested, and shares the addressing contract (`cache_filename_for_key`). The
compiler bakes whatever it can discover from MapEntity; anything it misses
degrades to the runtime fallback with no correctness loss.

### Decision 3 — Single-row strip with per-frame-independent mips (no MAX_DIMENSION change)

**Chosen:** keep the single horizontal strip (`N·W × H`). Bake the mip chain by
downsampling **each frame independently** (edge-clamped, Mitchell-Netravali) and
re-stitching the per-frame results at each level, then **truncate the chain** at
the level where a single frame would drop below 4 px on its shorter axis.

**Why:** the strip layout is the only one that preserves the shader's
`u = (frame_idx + cd.z) / frame_count` UV math and the `frame_count`-from-
draw-params sourcing — both hard invariants. Naively mipping a stitched strip
bleeds neighbouring frames at coarse levels (the canonical atlas-mip artifact);
per-frame-independent downsampling with edge clamp is the standard fix and keeps
the strip safe. Truncating before frames go sub-4px bounds residual bleed to
levels no longer selected at the distances where shimmer occurs — the shimmer the
chain exists to kill is gone after the first few levels.

**MAX_DIMENSION:** `prm.rs` caps each axis at `u16` 4096. A 64px×64-frame strip
is exactly 4096 wide — at the limit, not over it. The cap stays. The bake path
**rejects** (warns, emits no sidecar → runtime placeholder) any collection whose
stitched strip would exceed 4096 on either axis, rather than silently truncating
frames or raising the cap. A hard, logged per-sprite-size frame-count ceiling is
the documented contract: `floor(4096 / frame_width)` frames. At the common 64px
that is 64 frames; smaller frames allow more. This is a content constraint, not a
format change, so no `MAX_DIMENSION` bump and no PRM version bump.

## Acceptance criteria

- [ ] Compiling a map containing a `billboard_emitter` whose `sprite` resolves to
      a multi-frame collection writes one diffuse-only `.prm` under
      `.build-caches/prm-cache/` whose filename is the blake3 the runtime computes
      for the same frames. The sidecar's diffuse slot has `level_count > 1`.
- [ ] At runtime the collection's sprite texture is created with
      `mip_level_count == baked level_count` and the sprite sampler uses
      `mipmap_filter: Linear` with `lod_max_clamp == level_count - 1`.
- [ ] A sprite viewed at distance no longer shimmers: the coarse mips are present
      and selected. Verify visually on `content/dev/maps/campaign-test.prl` (which
      has smoke emitters) — distant smoke is stable frame-to-frame under camera
      motion where it previously crawled.
- [ ] No mip level bleeds one frame into an adjacent frame: a fixture collection
      of solid-color frames (frame 0 red, frame 1 blue) bakes a chain where every
      level's frame-0 region stays pure red and frame-1 region stays pure blue
      (within filter tolerance at the frame interior; edges may soften inward
      only).
- [ ] A collection that fails to resolve (missing directory) or whose strip would
      exceed 4096 px on an axis emits **no** sidecar, logs one warning, and the
      runtime falls back to the 1×1 white placeholder without panicking.
- [ ] The WGSL `SpriteInstance` struct stride still equals `SPRITE_INSTANCE_SIZE`
      (existing test `billboard_wgsl_sprite_instance_stride_matches_cpu` passes
      unchanged). `frame_count` still reaches the shader via `SpriteDrawParams`
      (`draw_params_layout` passes unchanged).
- [ ] `load_collection_frames` and `SpriteFrame` are retired from the runtime
      draw path: the renderer no longer stitches at upload time and no longer
      uploads a single-mip `Rgba8UnormSrgb` strip.

## Tasks

### Task 1: Shared frame-scan + stitch helpers in level-format

Lift the frame-discovery and strip-stitch logic into shared, runtime-and-compiler
code so both sides agree on frame order, count, strip dimensions, and content
hash. Add to `postretro-level-format` (the crate both `prl-build` and the runtime
already depend on for `prm`): a `collection_frame_paths(texture_root, collection)`
that returns frame PNG paths in numeric-suffix order, and a
`stitch_frames_to_strip` that produces `(rgba, strip_w, strip_h, frame_count)`.
The content hash for a collection is `blake3` over the concatenated raw PNG bytes
in frame order — define this once here (`sprite_collection_filename_key`) so the
compiler's sidecar filename and the runtime's lookup key are computed by the same
function, the way `cache_filename_for_key` already unifies addressing.

### Task 2: Per-frame-independent mip bake entry in `texture_mips.rs`

Add `bake_sprite_collection(texture_root, collection, cache_root) -> Option<[u8;
32]>`. It resolves frames via the Task 1 helper, rejects (warn + `None`) an empty
or over-4096 strip, then bakes the diffuse mip chain by downsampling **each frame
independently** with the existing Mitchell-Netravali path (edge-clamped at frame
borders), re-stitching the per-level frame results, and truncating the chain at
the per-frame sub-4px level. Emits a diffuse-only `PrmFile`
(`PrmSlots::DIFFUSE`, `Rgba8UnormSrgb`) addressed by the Task 1 content hash,
written through the existing `atomic_write` + cache-hit-check path so re-bakes are
idempotent and cross-collection dedupe is preserved. Reuse `build_diffuse_chain`'s
filter primitives; the only new logic is the per-frame tiling and re-stitch around
them.

### Task 3: Compiler discovery + bake pass in `main.rs`

Add `billboard_sprite_collections(entities) -> Vec<&str>`: walk MapEntity records,
select `classname == "billboard_emitter"`, read the last-wins `sprite` KVP
(default `"smoke"` when absent, matching the FGD default and the runtime
component default), dedupe in map order — structurally the twin of
`prop_mesh_model_handles`. Add `bake_sprite_textures(entities, texture_root,
prm_cache_root)` that calls `bake_sprite_collection` per discovered collection and
warns-and-continues on failure — the twin of `bake_model_textures`. Wire the call
next to the existing `bake_model_textures` call site so sprite sidecars are
produced in the same pass.

### Task 4: Runtime PRM load path for sprite collections

Replace `register_collection`'s stitch-and-upload body. Given a collection name
and `texture_root`, compute the Task 1 content hash, open `<key>.prm`, parse it,
and upload the diffuse slot's mip chain via the shared
`upload_texture_data`/`slot_levels` helpers in `loaded_texture.rs` (expose them
`pub(crate)` if not already). Build the sprite sampler with `mipmap_filter:
Linear` and `lod_max_clamp = level_count - 1`. Frame count is still derived from
the PNG file count (Task 1 helper) and packed into `SpriteDrawParams` unchanged.
On any failure (no frames, missing/corrupt `.prm`), upload the 1×1 white
placeholder and continue. Update the `register_smoke_collection` wrapper signature
in `render/mod.rs` (it no longer takes `&[SpriteFrame]`; it takes the collection
name + texture root + the spec/lifetime params it already passes) and update the
three `main.rs` call sites accordingly.

### Task 5: Retire `SpriteFrame` / `load_collection_frames`; share the WGSL UV path

Remove `load_collection_frames` and the runtime `SpriteFrame` upload usage from
`fx/smoke.rs` and `render/smoke.rs` once Task 4 no longer consumes them (keep the
GPU-layout pins `MAX_SPRITES`, `SPRITE_INSTANCE_SIZE`, and `frame_duration`). The
billboard WGSL is unchanged except the comment block describing the strip's
origin (it is now compile-stitched, not upload-stitched). Confirm the existing
naga-validation and stride tests still pass.

## Sequencing

**Phase 1 (sequential):** Task 1 — defines the frame-scan, stitch, and content-hash
contract every other task consumes. Blocks 2, 3, 4.
**Phase 2 (concurrent):** Task 2 (compiler bake entry) and Task 4 (runtime load)
— they meet only at the Task 1 hash + the `.prm` wire format, so they parallelize
against a hand-built fixture sidecar.
**Phase 3 (sequential):** Task 3 — wires the compiler discovery to the Task 2 bake
entry; consumes Task 2's `bake_sprite_collection`.
**Phase 4 (sequential):** Task 5 — retires the dead runtime path after Task 4 owns
loading; consumes Task 4's call-site updates.

## Rough sketch

- **Shared scan.** `load_collection_frames` already sorts `<collection>_NN.png`
  by parsed numeric suffix. Lift exactly that ordering into Task 1's
  `collection_frame_paths` so the compiler's column order and the runtime's
  `frame_count` agree by construction. The runtime stops decoding frames to
  pixels for upload; it decodes them only to hash (or hashes raw PNG bytes —
  cheaper and what Decision 1/Task 1 specifies).
- **Per-frame mip.** For each frame, run the existing separable
  Mitchell-Netravali downsample (`build_diffuse_chain` internals) on that frame's
  `W×H` pixels alone with edge clamp; collect `levels[frame][n]`. At level `n` the
  strip is `(frame_count · (W>>n)) × (H>>n)`; re-stitch `levels[*][n]`
  horizontally. Stop at the first `n` where `min(W>>n, H>>n) < 4`. The truncated
  `level_count` is what the `.prm` slot records and what `lod_max_clamp` keys on.
- **Strip-width guard.** `frame_count · W > 4096` (or `H > 4096`) → reject. Log
  `floor(4096 / W)` as the max frames for that frame width so the content author
  sees the actual ceiling.
- **Addressing.** Sprite sidecar filename = `cache_filename_for_key(
  sprite_collection_filename_key(frames))`. A diffuse-only sprite strip and a
  world/model diffuse share the `.prm` shape, so the existing richer-world-bundle
  preservation in `bake_diffuse_texture` applies if hashes ever collide (they
  won't in practice — a stitched strip's bytes differ from any single PNG).
- **Sampler.** Today `register_collection` builds one shared `Nearest`-mip
  sampler in `SmokePass::new`. Either rebuild a per-collection sampler keyed on
  `level_count` (cheap, few collections) or reuse the world path's
  `mip_count_aniso_samplers` pool idea. Per-collection is simplest at sprite
  scale.

## Boundary inventory

| Name | Rust | Wire / serde | FGD KVP |
|---|---|---|---|
| Sprite collection name | `BillboardEmitterComponent.sprite` / MapEntity `sprite` KVP | n/a (not in PRL) | `billboard_emitter.sprite` (default `"smoke"`) |
| Sprite sidecar key | `sprite_collection_filename_key(&[png_bytes]) -> [u8;32]` | `.prm` filename stem (hex) via `cache_filename_for_key` | n/a |
| Sprite `.prm` slot mask | `PrmSlots::DIFFUSE` | PRM header `slot_mask` bit 0 | n/a |
| Frame count | runtime PNG-count → `SpriteDrawParams.params.x` | bitcast `f32` in draw-params UBO | implied by `<collection>_NN.png` count |

## Wire format

No new binary surface. Sprite collections reuse the existing `.prm` wire format
(`postretro-level-format::prm`) unchanged — a diffuse-only bundle, byte-identical
in shape to a model diffuse sidecar. No PRL section is added, so `pack.rs` and the
PRL header version are untouched. The PRM `STAGE_VERSION` does **not** bump: the
format is unchanged; only a new producer/consumer of the existing format is added.

## Open questions

- **Descriptor-spawned and weapon-impact collections.** The compiler sees only
  `billboard_emitter` map placements. Collections introduced by data-script
  descriptors or the hardcoded `"impact"` effect bake no sidecar and fall back to
  the runtime placeholder (or, if we want them mipped too, the runtime could bake
  on first load — but runtime baking is an explicit engine non-goal). Recommended
  follow-up: have the compiler also bake a fixed allowlist of engine-built-in
  sprite collections (`"smoke"`, `"impact"`) regardless of map content, the way
  built-in handlers are engine-closed. Out of scope for this slice; the fallback
  keeps them rendering.
- **Truncation level vs. visible smallest mip.** Truncating at sub-4px-per-frame
  is a conservative anti-bleed bound. If profiling shows distant sprites still
  want one coarser level, the per-frame downsample can go to 1px with the edge
  clamp absorbing the bleed — revisit only if AC's shimmer check fails at the
  truncated depth.

## Sources

- [Kyle Halladay — Minimizing Mip Map Artifacts In Atlassed Textures](https://kylehalladay.com/blog/tutorial/2016/11/04/Texture-Atlassing-With-Mips.html)
- [0 FPS — Texture atlases, wrapping and mip mapping](https://0fps.net/2013/07/09/texture-atlases-wrapping-and-mip-mapping/)
