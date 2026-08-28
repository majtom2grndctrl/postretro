# Billboard Sprite PRM Baking

## Goal

Move billboard sprite-collection textures — specifically the multi-frame `_NN`
collections discovered from a map's `billboard_emitter` placements — off the
runtime PNG-load-and-stitch path and onto the `prl-build` → `.prm` baking
pipeline that already serves world and model textures. These collections gain a
Mitchell-Netravali mip chain and are sampled with mipmaps at runtime,
eliminating the distance shimmer that single-mip strips produce. They become
aesthetically consistent with world surfaces: same filter, same linear-space
downsample, same `.prm` sidecar addressing. A collection that ships companion
specular or normal frames bakes them into extra slots of the same sidecar,
giving the downstream billboard-specular-shimmer work the per-texel data it
samples; a collection without companions stays diffuse-only.

This spec does **not** retire the runtime PNG decode path. Every sprite source
that is not a map-placed `billboard_emitter` `_NN` collection — direct-`.png`
single-frame references, data-script descriptor-spawned sprites, and the
hardcoded weapon-impact effect — stays on the existing runtime decode-and-upload
path, untouched. The baked `.prm` path becomes the *preferred* path for
map-emitter collections; the runtime decode path *remains* as the fallback for
everything not baked. Full PNG-decode retirement and baking of every sprite
source is owned by the separate `sprite-png-retirement` spec (see *Cross-spec
coordination*).

## Scope

### In scope

- Compile-time stitching: `prl-build` discovers sprite collections from the
  map's `billboard_emitter` placements, stitches each collection's frames into a
  single horizontal strip, bakes a **per-frame-independent** mip chain, and
  writes a `.prm` sidecar — content-addressed exactly like a world/model
  sidecar. The sidecar always carries the diffuse slot; it carries specular
  and/or normal slots when the collection ships companion frames for them.
- **Optional per-collection specular/normal slots.** A collection MAY provide
  companion `<collection>_NN_spec.png` and/or `<collection>_NN_normal.png` frames
  alongside its `<collection>_NN.png` diffuse frames. When present, they bake
  into the SPECULAR / NORMAL slots of the same `.prm` bundle; when absent, the
  sidecar stays diffuse-only. A collection with no companions bakes a diffuse-only
  bundle whose content key equals the key computed with the spec/normal sets
  empty. This spec only bakes and uploads the extra slots — the shader-side
  consumption is owned by the separate billboard-specular-shimmer spec. See
  *Design decision 4*.
- Per-frame mip independence: each animation frame is downsampled in isolation
  (edge-clamped) and the per-level results are re-stitched, so no mip level
  bleeds one frame's texels into the next. This is what makes the strip layout
  safe to mip.
- Runtime load: for a map-emitter collection the renderer content-hashes the
  collection's frames through the same shared function the compiler uses and, when
  that key resolves a `<key>.prm`, opens it directly and uploads the baked mip
  chain (the new *preferred* path). When no sidecar resolves, the renderer falls
  back to the existing decode-and-upload single-mip path exactly as today. No PRL
  section carries sprite keys — the prop_mesh "no-section, runtime re-hash"
  pattern.
- Mip-aware sprite sampler: for a baked collection `mipmap_filter` flips from
  `Nearest` to `Linear` and `lod_max_clamp` is set per collection from the baked
  chain depth. The decode fallback keeps its existing single-mip sampler.
- Frame count stays runtime-derived from the PNG file count (unchanged source of
  truth); it continues to reach the shader through `SpriteDrawParams`.
- Fallback parity: a collection whose sidecar fails to resolve, hash, or load
  degrades to the existing runtime decode-and-upload path (which itself falls to
  the 1×1 white-frame placeholder if even decoding fails); load continues.

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
- The billboard pass blend mode, depth state, lighting math, and the
  `SpriteInstance` storage-buffer layout.
- FGD changes. No new KVPs. `billboard_emitter.sprite` is unchanged.
- **Retiring the runtime PNG decode path.** `load_collection_frames`,
  `load_sprite_frames`, and `SpriteFrame` stay — they power the decode fallback
  this spec deliberately preserves. Retiring the decode path and migrating every
  sprite source to `.prm` is owned by `sprite-png-retirement` (see *Cross-spec
  coordination*).
- Every sprite source that is not a map-placed `billboard_emitter` `_NN`
  collection: direct-`.png` single-frame references, data-script
  descriptor-spawned sprites, and the hardcoded weapon-impact effect
  (`weapon::impact_sprite_collection()` → `"impact"`). The compiler cannot see the
  latter two, and single-frame `.png` references are not this slice's target.
  They all keep working on the runtime decode fallback (see *Open questions*);
  their PRM migration is owned by `sprite-png-retirement`, not this spec.

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
degrades to the runtime decode fallback with no correctness loss.

This spec bakes only the first source (map `billboard_emitter` `_NN`
collections). The descriptor-only and weapon-impact collections — and direct
single-frame `.png` references — stay on the runtime decode fallback here; baking
them (and retiring the decode path once every source is baked) is owned by
`sprite-png-retirement`. See *Cross-spec coordination*.

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
**rejects** (warns, emits no sidecar → runtime decode fallback) any collection whose
stitched strip would exceed 4096 on either axis, rather than silently truncating
frames or raising the cap. A hard, logged per-sprite-size frame-count ceiling is
the documented contract: `floor(4096 / frame_width)` frames. At the common 64px
that is 64 frames; smaller frames allow more. This is a content constraint, not a
format change, so no `MAX_DIMENSION` bump and no PRM version bump.

### Decision 4 — Optional specular/normal slots, discovered per collection, complete-or-absent

**Chosen:** the bake discovers companion frames by the same numeric-suffix scan
that finds diffuse frames — `<collection>_NN_spec.png` for the SPECULAR slot and
`<collection>_NN_normal.png` for the NORMAL slot — and stitches each companion
set into its own strip baked into an extra slot of the same `.prm` bundle. Each
slot is **complete-or-absent**: for a given slot the companion set must cover
every `<collection>_NN` diffuse frame or none. A collection missing even one
frame's companion for a slot warns and omits that slot; the diffuse slot (and any
complete companion slot) still bakes. The two slots are independent — a collection
may ship spec-only, normal-only, both, or neither.

**Why complete-or-absent, not per-frame:** the strip is a single stitched image
whose column layout is `frame_count` tiles wide, and the runtime UV math indexes
it by `frame_idx`. A partial companion set would leave undefined tiles in the
strip and give the mip re-stitch a non-uniform per-frame tile count. Requiring the
set complete keeps every slot's strip the same tile geometry as diffuse, so the
per-frame-independent bake and the strip-width guard (Decision 3) apply to each
slot unchanged. This mirrors the world path, which rejects an emissive companion
whose dimensions do not match diffuse rather than baking a mismatched bundle.

**Per-frame geometry, not just count.** Complete-or-absent is a frame-*count*
check; it does not by itself keep `stitch_frames_to_strip` safe, which has no
uniform-geometry precondition. A ragged diffuse set (one frame a different
`W×H`), or a same-count companion set with one differently-sized frame, produces
a corrupt or undefined strip. The bake therefore also enforces geometry
per slot: all frames *within* a slot must share one `W×H`, or the slot is
malformed; and each companion slot's per-frame geometry must equal the diffuse
frame geometry, or that companion is a mismatch. Ragged **diffuse** frames reject
the whole collection (no sidecar → runtime decode fallback).
A companion slot whose frames are same-count but a different resolution warns and
is **omitted** (the diffuse slot, and any geometry-matching companion, still
bakes) — the world path's dimension check, now actually delivered per frame
rather than assumed by a count.

**Colorspaces (mirroring the world/model bake):** SPECULAR bakes to
`PrmFormat::R8Unorm` — linear, single-channel, the R channel of the decoded PNG,
through `build_specular_chain`, exactly as the world specular map bakes. NORMAL
bakes to `PrmFormat::Bc5RgUnorm` — linear, never sRGB, through
`build_normal_bc5_chain`, exactly as the world/model tangent-space normal map
bakes. BC5 needs both axes ≥ 4 px; the per-frame sub-4px truncation (Decision 3)
already stops a frame's chain before it drops below that, so the normal slot's
`level_count` is `bc5_level_count(frame_w, frame_h)` and a collection whose frames
are below 4×4 omits the normal slot (the runtime substitutes its neutral-normal
placeholder, matching the world path).

**Uniform bundle depth.** `build_specular_chain` runs a frame's chain down to
1×1, deeper than the diffuse truncation (Decision 3, sub-4px on the shorter axis).
Left uneven, the bundle would carry three different `level_count`s and the
per-collection sampler's single `lod_max_clamp = diffuse level_count - 1` would
leave the extra specular mips unsamplable. The bake therefore **truncates the
specular chain to the same `level_count` as diffuse**, so every slot in the bundle
shares one depth and one sampler clamp serves them all. The normal slot's
`bc5_level_count(frame_w, frame_h)` already coincides with the diffuse shorter-axis
rule, so it matches without a separate truncation.

**Content hash folds every slot's bytes, with per-slot framing.** The sidecar
filename addresses the full bundle. `sprite_collection_filename_key` mirrors
`bundle_hash_for`'s mask+tag scheme (not `filename_key_for`'s diffuse-only
special case) and adds two things world textures do not need: a **sprite-domain
discriminator** and a **per-slot frame count**. It folds, in fixed slot order
(diffuse, then spec, then normal): a leading sprite-domain tag byte distinct from
any world/model key, the present-slot mask, then per present slot its slot tag
byte, that slot's frame *count*, then each frame's raw PNG bytes in numeric order.

The discriminator is required for correctness, not tidiness. `filename_key_for`
hashes a **diffuse-only** bundle as bare `blake3(diffuse_bytes)` with no
mask/tag prefix (so a model diffuse-only sidecar can dedupe against it). Without a
sprite discriminator, a single-frame diffuse-only sprite collection's key would be
`blake3(that_one_PNG)` and would collide with any world/model diffuse-only sidecar
of the same PNG — both carry `slot_mask == DIFFUSE`, so the richer-bundle guard
`matching_diffuse_only` (`texture_mips.rs`) would treat it as a valid hit and
cross-contaminate chains. The leading sprite-domain tag byte makes a sprite key
unreachable from any world/model key, for every frame count.

The per-slot frame count keeps slot boundaries visible. A bare
diffuse-then-spec-then-normal byte concatenation makes them invisible: moving one
frame from the diffuse set into the spec set yields an identical byte stream and
an identical key even though the bundle changed. Folding each slot's present-mask,
tag byte, and frame *count* before its bytes means such a move addresses a
distinct `.prm`. Two collections that differ only in their spec or normal maps
likewise address distinct sidecars instead of colliding on a diffuse-only key.

## Acceptance criteria

- [ ] Compiling a map containing a `billboard_emitter` whose `sprite` resolves to
      a multi-frame collection with no spec/normal companions writes one `.prm`
      under `.build-caches/prm-cache/` whose filename equals the runtime-computed
      key for the same frames. A no-companion collection's key equals the key
      computed with the spec/normal sets empty; its `slot_mask` is diffuse-only;
      its diffuse slot has `level_count > 1`.
- [ ] A collection that ships complete `<collection>_NN_spec.png` and/or
      `<collection>_NN_normal.png` companion sets bakes a `.prm` whose `slot_mask`
      has the SPECULAR and/or NORMAL bit set. The specular slot is `R8Unorm` with
      `specular level_count == diffuse level_count`; the normal slot is `Bc5RgUnorm`
      with `level_count == bc5_level_count(frame_w, frame_h)`.
- [ ] Removing, adding, or changing any spec/normal companion frame changes the
      sidecar filename (the content hash folds every slot's bytes): a collection
      differing from another only in its spec or normal maps addresses a distinct
      `.prm`.
- [ ] A collection that moves one frame from the diffuse set into the spec set
      addresses a distinct `.prm` from the all-diffuse collection with the same
      bytes (the per-slot frame-count fold makes the slot boundary hash-visible).
- [ ] A collection whose spec (or normal) companion set is incomplete — present
      for some frames, missing for others — omits that slot with one warning and
      still bakes the diffuse slot (and any complete companion slot).
- [ ] A collection whose spec (or normal) companion set is complete in count but
      has one frame at a different resolution than the diffuse frames omits that
      slot with one warning and still bakes the diffuse slot; a collection whose
      **diffuse** frames are ragged (not all one `W×H`) bakes no sidecar, logs one
      warning, and the runtime falls back.
- [ ] The runtime key for a companion-bearing collection equals the compiler's
      sidecar filename (round-trip), verified against the Phase-2 hand-built
      fixture — so a runtime that scans only diffuse can never silently miss a
      companion-bearing sidecar.
- [ ] At runtime the collection's diffuse sprite texture is created with
      `mip_level_count == baked level_count` and the sprite sampler uses
      `mipmap_filter: Linear` with `lod_max_clamp == level_count - 1`.
- [ ] When the loaded sidecar carries a SPECULAR or NORMAL slot, the runtime
      creates and uploads its mip chain and **retains** the texture view (plus the
      parsed `slot_mask`) on the sprite sheet, such that the billboard pass can
      retrieve the specular/normal views and the slot mask — not merely that the
      textures were uploaded. Wiring those views into the bind-group layout and
      sampling them is owned by the downstream billboard-specular-shimmer spec, not
      this one.
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
      exceed 4096 px on an axis emits **no** sidecar and logs one warning; the
      runtime, finding no sidecar for the key, falls back to the existing decode
      path (which itself reaches the 1×1 white placeholder when there are no
      frames to decode) without panicking.
- [ ] The WGSL `SpriteInstance` struct stride still equals `SPRITE_INSTANCE_SIZE`
      (existing test `billboard_wgsl_sprite_instance_stride_matches_cpu` passes
      unchanged). `frame_count` still reaches the shader via `SpriteDrawParams`
      (`draw_params_layout` passes unchanged).

## Tasks

### Task 1: Shared frame-scan + stitch helpers in level-format

Lift the frame-discovery and strip-stitch logic into shared, runtime-and-compiler
code so both sides agree on frame order, count, strip dimensions, and content
hash. Add to `postretro-level-format` (the crate both `prl-build` and the runtime
already depend on for `prm`): a `collection_frame_paths(texture_root, collection)`
that returns frame PNG paths in numeric-suffix order for a given slot suffix (the
diffuse `<collection>_NN.png` set, the `<collection>_NN_spec.png` set, and the
`<collection>_NN_normal.png` set — one call per slot), and a
`stitch_frames_to_strip` that produces `(rgba, strip_w, strip_h, frame_count)`.

The lifted diffuse scan keeps the exact **pure-integer suffix gate** the runtime
uses today — `suffix.parse::<u32>()` from `load_collection_frames`
(`crates/render-cpu/src/fx/smoke.rs`) — so a `_NN_spec` / `_NN_normal` companion
file is never miscounted as a diffuse frame; the companion scan parses `NN` from
the `NN_spec` / `NN_normal` stem.

Define the content key once here as
`sprite_collection_filename_key(texture_root, collection)`, and have it perform
the diffuse + spec + normal scan **internally** — so the compiler and the runtime
call one function and cannot diverge on which slots the key covers (a runtime that
scanned only diffuse while the compiler folded companions would mismatch every
companion-bearing key and silently fall to the fallback). It mirrors
`bundle_hash_for`'s mask+tag scheme rather than `filename_key_for`'s diffuse-only
special case, and adds two things world textures do not have: a leading
sprite-domain discriminator byte (distinct from any world/model key, so a sprite
sidecar can never collide with a world/model one at any frame count) and a per-slot
frame *count*. The exact `blake3` stream, in fixed slot order (diffuse, spec,
normal): the sprite-domain tag byte, the present-slot mask, then per present slot
its slot tag byte (`0x00` diffuse / `0x01` spec / `0x02` normal), that slot's frame
count, then each frame's raw PNG bytes in numeric order. Per-slot count + tag
keeps slot boundaries hash-visible (moving a frame diffuse→spec changes the key),
which a bare concatenation would not. This is defined once here so the compiler's
sidecar filename and the runtime's lookup key are the same function, the way
`cache_filename_for_key` already unifies addressing and `bundle_hash_for` folds the
world path's mask+tag+bytes.

`bc5_level_count(width: u16, height: u16)` takes `u16`; the 4096 strip-width guard
(Decision 3) keeps stitched dims in range for that call.

### Task 2: Per-frame-independent mip bake entry in `texture_mips.rs`

Add `bake_sprite_collection(texture_root, collection, cache_root) -> Option<[u8;
32]>`. It resolves the diffuse frames via the Task 1 helper, rejects (warn +
`None`) an empty or over-4096 strip, and rejects (warn + `None`) a **ragged**
diffuse set whose frames do not all share one `W×H` — a per-frame geometry check,
since `stitch_frames_to_strip` has no uniform-geometry precondition and a
count-only check cannot keep it safe. It then bakes the diffuse mip chain by
downsampling **each frame independently** with the existing Mitchell-Netravali
path (edge-clamped at frame borders), re-stitching the per-level frame results,
and truncating the chain at the per-frame sub-4px level.

Then discover the optional companion sets — `<collection>_NN_spec.png` and
`<collection>_NN_normal.png` — via the same Task 1 helper. Each companion slot is
**complete-or-absent** and **geometry-matched**: if present it must cover every
diffuse frame (else warn and omit that slot) and each of its frames' `W×H` must
equal the diffuse frame geometry (else warn and omit that slot); diffuse and any
valid companion still bake. For a present SPECULAR set, flatten each frame to its
R channel and run the per-frame-independent bake through `build_specular_chain`,
then **truncate the specular chain to the diffuse `level_count`** so the whole
bundle shares one depth (Decision 4, "uniform bundle depth"), emitting an
`R8Unorm` slot; for a present NORMAL set, run the per-frame-independent bake
through `build_normal_bc5_chain`, emitting a `Bc5RgUnorm` slot whose `level_count`
is `bc5_level_count(frame_w, frame_h)` (omit the normal slot when frames are below
BC5's 4×4 minimum). The strip-width guard and the per-frame tiling/re-stitch are
identical to diffuse — only the filter primitive and slot format differ per slot.

Emit one `PrmFile` carrying `PrmSlots::DIFFUSE` plus whichever companion slots
baked (`SPECULAR`, `NORMAL`), addressed by the Task 1 content hash (which folds
every slot's bytes), written through the existing `atomic_write` + cache-hit-check
path so re-bakes are idempotent and cross-collection dedupe is preserved. A
collection with no companions emits a diffuse-only bundle. Reuse the existing
filter primitives (`build_diffuse_chain`, `build_specular_chain`,
`build_normal_bc5_chain`); the only new logic is the per-frame tiling and
re-stitch around them.

Build the anti-bleed fixture here (a two-frame collection, frame 0 solid red /
frame 1 solid blue) so the per-frame-independent re-stitch has a test — the
no-cross-frame-bleed AC — asserting no bleed at every level.

Note (pre-existing, not solved here): `atomic_write` writes sidecars straight into
the prm-cache and bypasses the `StageCache` LRU, so a companion edit that changes
the content key leaves the old content-addressed sidecar as an unpruned orphan.
This mirrors the world/model bake's existing behavior; this spec does not add
pruning.

### Task 3: Compiler discovery + bake pass in `main.rs`

Add `billboard_sprite_collections(entities) -> Vec<&str>`: walk MapEntity records,
select `classname == "billboard_emitter"`, read the last-wins `sprite` KVP
(default `"smoke"` when absent, matching the FGD default and the runtime
component default), dedupe in map order — structurally the twin of
`prop_mesh_model_handles`. Add `bake_sprite_textures(entities, texture_root,
prm_cache_root)` that calls `bake_sprite_collection` per discovered collection and
warns-and-continues on failure — the twin of `bake_model_textures`. The optional
spec/normal companion frames are discovered inside `bake_sprite_collection`
(Task 2) from the collection name, so this pass needs no extra discovery — it
bakes whatever companion slots the collection ships. Wire the call next to the
existing `bake_model_textures` call site so sprite sidecars are produced in the
same pass.

### Task 4: Runtime PRM load path for sprite collections (baked-preferred, decode fallback)

Add a baked-preferred branch to `register_collection`; keep its existing
stitch-and-upload body as the fallback rather than removing it. Given a collection
name and `texture_root`, compute the key through the shared Task 1
`sprite_collection_filename_key(texture_root, collection)` (which scans diffuse +
spec + normal internally, so the runtime cannot fold a different slot set than the
compiler did). When that key resolves a `<key>.prm`, parse it and upload the
diffuse slot's mip chain via the shared `upload_texture_data`/`slot_levels`
helpers in `loaded_texture.rs` (expose them `pub(crate)` if not already), and
build the sprite sampler with `mipmap_filter: Linear` and
`lod_max_clamp = level_count - 1`. When **no** sidecar resolves (or the `.prm`
fails to parse), fall through to the existing decode-and-upload single-mip path
exactly as today — `load_collection_frames` / `SpriteFrame` stay for this purpose
— which itself uploads the 1×1 white placeholder when there are no frames at all.

When the parsed sidecar's `slot_mask` also carries the SPECULAR and/or NORMAL
bits, create and upload those slots' mip chains through the same helpers (they
already handle `R8Unorm` and `Bc5RgUnorm`), and **retain** their texture views —
add optional `specular_view` / `normal_view` fields plus the parsed `slot_mask`
to `SpriteSheet` (`crates/renderer/src/render/smoke.rs`, today only `bind_group`
and `frame_count`) so the views survive past this function and the billboard pass
can retrieve them. A wgpu texture/view with no retained owner is dropped on
return, so without this the "resident for the billboard bind group" property is
not actually produced. Wiring those views into the billboard bind-group layout and
sampling them is a downstream consumer owned by the billboard-specular-shimmer
spec — this task uploads and retains the views, it does not change the shader or
the existing bind-group bindings.

Frame count is still derived from the PNG file count and packed into
`SpriteDrawParams` unchanged; after the wrapper stops taking `&[SpriteFrame]`, the
call site derives it from `collection_frame_paths(texture_root, collection).len()`
(Task 1) for both `register_smoke_collection` and
`resolve_sprite_collection_draw_contract`
(`crates/postretro/src/startup/lifecycle.rs`).

Update the `register_smoke_collection` wrapper — a `Renderer` method in
`crates/renderer/src/render/renderer_resources.rs` (not `render/mod.rs`) — so it no
longer takes `&[SpriteFrame]`; it takes the collection name + texture root + the
spec/lifetime params it already passes. Update the **two** call sites, both in
`crates/postretro/src/startup/lifecycle.rs` (the emitter/projectile-loop call and
the `weapon::impact_sprite_collection()` call — not `main.rs`), accordingly.

Finally, update the comment block in the billboard WGSL describing the strip's
origin: a baked collection's strip is now compile-stitched, not upload-stitched
(the decode fallback still upload-stitches). No WGSL logic changes; confirm the
existing naga-validation and stride tests still pass. Keep the GPU-layout pins
`MAX_SPRITES`, `SPRITE_INSTANCE_SIZE`, and `frame_duration` untouched.

## Sequencing

**Phase 1 (sequential):** Task 1 — defines the frame-scan, stitch, and content-hash
contract every other task consumes. Blocks 2, 3, 4.
**Phase 2 (concurrent):** Task 2 (compiler bake entry) and Task 4 (runtime load)
— they meet only at the Task 1 hash + the `.prm` wire format, so they parallelize
against a hand-built fixture sidecar. The fixture **must include companion (spec
and/or normal) slots** and assert the round-trip that the runtime's
`sprite_collection_filename_key` for that companion-bearing collection equals the
fixture's filename. Without a companion slot in the fixture, a Blocker-class
compiler/runtime divergence over which slots the key folds goes unsurfaced until
integration.
**Phase 3 (sequential):** Task 3 — wires the compiler discovery to the Task 2 bake
entry; consumes Task 2's `bake_sprite_collection`.

Task 4 folds in the (retained) decode fallback and the billboard WGSL comment
update, so there is no separate retirement phase — the decode path is kept, not
retired, by this spec.

## Cross-spec coordination

> **Upstream prerequisite — `prm-array-layers` (lands first).** The sprite
> texture representation is being migrated from a single stitched strip to
> per-frame `texture_2d_array` layers by the `prm-array-layers` spec, which
> extends the PRM format with a file-header `layer_count`, adds
> `upload_texture_array_data`, and flips `billboard.wgsl` / the g1b0 bind layout
> / the decode-fallback upload to D2Array end-to-end (sprite path renders from
> the array-based decode fallback, single-mip, no baked mips). This spec is the
> **payoff layer** on top of that migration: it bakes the mipped array `.prm`
> and, in `register_collection`'s baked-preferred branch, uploads the layered
> mip chain via `upload_texture_array_data` and flips the sampler to
> `Linear`/`lod_max_clamp`. **Decision 3 (single-row strip + per-frame-
> independent re-stitch + sub-4px truncation) is superseded** — each frame is
> baked as an independent standard image into its own array layer with a normal
> full mip chain, which the PRM reader validates as-is; the strip UV math, the
> `N·W` 4096 ceiling, and the truncation are gone. A full revision of the
> sections below to the array-layer representation is pending the
> `prm-array-layers` draft; until then, read Decision 3 and every "strip"/
> "stitch" reference as replaced by "per-frame array layer." This spec must not
> land before `prm-array-layers`.

This spec is the first, narrow slice of a two-spec split. It bakes **only**
map-placed `billboard_emitter` `_NN` collections and leaves the runtime PNG
decode path in place as the fallback for everything else. The downstream
`sprite-png-retirement` spec owns the rest: baking every remaining sprite source
(descriptor-spawned collections, the hardcoded `"impact"` effect, and direct
single-frame `.png` references) and only then retiring the runtime decode path
(`load_collection_frames`, `load_sprite_frames`, `SpriteFrame`). Coordination
points if both are in flight:

- **The decode path is a shared asset, not dead code, until `sprite-png-retirement`
  lands.** Nothing here removes `load_collection_frames` / `load_sprite_frames` /
  `SpriteFrame`; `sprite-png-retirement` removes them once no source needs the
  fallback. A change here that broke the fallback would regress every unbaked
  source.
- **Addressing contract.** `sprite_collection_filename_key` (Task 1) is the shared
  key function; `sprite-png-retirement` extends the same addressing to the sources
  it bakes rather than inventing a second scheme, and inherits the sprite-domain
  discriminator so its keys also stay clear of world/model keys.
- **`SpriteSheet` shape.** This spec adds `specular_view` / `normal_view` /
  `slot_mask` to `SpriteSheet` for the billboard-specular-shimmer consumer;
  `sprite-png-retirement`, when it routes the remaining sources through the baked
  path, populates the same fields rather than adding parallel ones.

## Rough sketch

- **Shared scan.** `load_collection_frames` already sorts `<collection>_NN.png`
  by parsed numeric suffix. Lift exactly that ordering into Task 1's
  `collection_frame_paths` so the compiler's column order and the runtime's
  `frame_count` agree by construction. For a baked hit the runtime only reads raw
  PNG bytes to compute the lookup key (no decode); on a miss it still decodes the
  frames to pixels for the retained decode-and-upload fallback, exactly as today.
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
  sprite_collection_filename_key(texture_root, collection))`, where the key mirrors
  `bundle_hash_for`'s mask+tag scheme and prepends a sprite-domain discriminator
  byte, then folds per slot (diffuse, spec, normal) the slot tag, that slot's frame
  count, and its frame bytes. The discriminator is load-bearing, not defensive:
  `filename_key_for` hashes a diffuse-only bundle as bare `blake3(diffuse_bytes)`
  with no tag, so a single-frame diffuse-only sprite key without the discriminator
  would equal a world/model diffuse-only key for the same PNG and the
  `matching_diffuse_only` richer-bundle guard (`texture_mips.rs`) would treat it as
  a real hit and cross-contaminate chains. With the discriminator a sprite key is
  unreachable from any world/model key at every frame count; the per-slot count +
  tag additionally keeps two collections that differ only in spec/normal (or by a
  frame moved diffuse→spec) on distinct keys.
- **Sampler.** Today `register_collection` builds one shared `Nearest`-mip
  sampler in `SmokePass::new`. Either rebuild a per-collection sampler keyed on
  `level_count` (cheap, few collections) or reuse the world path's
  `mip_count_aniso_samplers` pool idea. Per-collection is simplest at sprite
  scale.

## Boundary inventory

| Name | Rust | Wire / serde | FGD KVP |
|---|---|---|---|
| Sprite collection name | `BillboardEmitterComponent.sprite` / MapEntity `sprite` KVP | n/a (not in PRL) | `billboard_emitter.sprite` (default `"smoke"`) |
| Sprite sidecar key | `sprite_collection_filename_key(texture_root, collection) -> [u8;32]` (scans diffuse+spec+normal internally; discriminator + mask + per-slot tag/count/bytes) | `.prm` filename stem (hex) via `cache_filename_for_key` | n/a |
| Sprite `.prm` slot mask | `PrmSlots::DIFFUSE` always; `SPECULAR` / `NORMAL` when companion frames present | PRM header `slot_mask` bits 0 (diffuse), 1 (specular), 2 (normal) | n/a |
| Spec companion frames | `<collection>_NN_spec.png` → `PrmFormat::R8Unorm` slot | PRM SPECULAR slot | n/a |
| Normal companion frames | `<collection>_NN_normal.png` → `PrmFormat::Bc5RgUnorm` slot | PRM NORMAL slot | n/a |
| Frame count | runtime PNG-count → `SpriteDrawParams.params.x` | bitcast `f32` in draw-params UBO | implied by `<collection>_NN.png` count |

## Wire format

No new binary surface. Sprite collections reuse the existing `.prm` wire format
(`postretro-level-format::prm`) unchanged. The sidecar always carries the diffuse
slot and may also carry the SPECULAR and NORMAL slots the format already
defines (`PrmSlots` bits 1 and 2) — byte-identical in shape to a world/model
bundle with those slots. Adding the companion slots is not a format or version
change: `prm.rs` already defines all four slots and the `slot_mask` bits. No PRL
section is added, so `pack.rs` and the PRL header version are untouched. The PRM
`STAGE_VERSION` does **not** bump: the format is unchanged; only a new
producer/consumer of the existing format is added.

## Open questions

- **Descriptor-spawned, weapon-impact, and direct-`.png` sprites.** The compiler
  sees only `billboard_emitter` map placements. Collections introduced by
  data-script descriptors or the hardcoded `"impact"` effect
  (`weapon::impact_sprite_collection()`), and direct single-frame `.png`
  references, bake no sidecar and keep rendering on the runtime decode fallback
  this spec preserves. Baking them (e.g. a compiler allowlist of engine-built-in
  collections like `"smoke"` / `"impact"`, or a descriptor-manifest pass) and
  retiring the decode path once every source is baked is owned by
  `sprite-png-retirement` (see *Cross-spec coordination*), not this slice.
  Runtime-bake-on-first-load stays out — runtime baking is an explicit engine
  non-goal.
- **Truncation level vs. visible smallest mip.** Truncating at sub-4px-per-frame
  is a conservative anti-bleed bound. If profiling shows distant sprites still
  want one coarser level, the per-frame downsample can go to 1px with the edge
  clamp absorbing the bleed — revisit only if AC's shimmer check fails at the
  truncated depth.

## Sources

- [Kyle Halladay — Minimizing Mip Map Artifacts In Atlassed Textures](https://kylehalladay.com/blog/tutorial/2016/11/04/Texture-Atlassing-With-Mips.html)
- [0 FPS — Texture atlases, wrapping and mip mapping](https://0fps.net/2013/07/09/texture-atlases-wrapping-and-mip-mapping/)
