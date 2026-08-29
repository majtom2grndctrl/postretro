# Billboard Sprite PRM Baking

## Goal

Move billboard sprite-collection textures — specifically the multi-frame `_NN`
collections discovered from a map's `billboard_emitter` placements — off the
runtime single-mip decode-and-upload path and onto the `prl-build` → `.prm`
baking pipeline that already serves world and model textures. Each frame is
baked as an independent standard image into its own `texture_2d_array` layer
with a full Mitchell-Netravali mip chain, and the collection is sampled with
mipmaps at runtime, eliminating the distance shimmer that single-mip array
layers produce. These collections become aesthetically consistent with world
surfaces: same filter, same linear-space downsample, same `.prm` sidecar
addressing. A collection that ships companion specular or normal frames bakes
them into extra slots of the same sidecar, giving the downstream
billboard-specular-shimmer work the per-texel data it samples; a collection
without companions stays diffuse-only.

This spec builds directly on `prm-array-layers` (done), which extended the
`.prm` format with a file-header `layer_count`, migrated the billboard sprite
texture path to `texture_2d_array` (one frame per layer), and left the sprite
path on an **array-based, single-mip** decode fallback with no baked mips. This
spec is the payoff layer: it bakes the mipped array `.prm` sidecars, builds the
`texture_2d_array` mip-chain uploader (`upload_texture_array_data`) plus the
layer-major sibling of `slot_levels` (`slot_layer_levels`) that `prm-array-layers` explicitly
left to its first consumer, and flips the sprite sampler to mipmapped filtering.

This spec does **not** retire the runtime PNG decode path. Every sprite source
that is not a map-placed `billboard_emitter` `_NN` collection —
direct-`.png` single-frame references, data-script descriptor-spawned sprites,
and the hardcoded weapon-impact effect — stays on the existing runtime
decode-and-upload array path, untouched. The baked `.prm` path becomes the
*preferred* path for map-emitter collections; the runtime decode path *remains*
as the fallback for everything not baked. Full PNG-decode retirement and baking
of every sprite source is owned by the separate `sprite-png-retirement` spec
(see *Cross-spec coordination*).

## Scope

### In scope

- **Compile-time per-frame array-layer bake.** `prl-build` discovers sprite
  collections from the map's `billboard_emitter` placements, bakes each frame as
  an independent standard image with its own full mip chain into a distinct
  `texture_2d_array` layer (layer-major payload, `layer_count == frame_count`),
  and writes a `.prm` sidecar — content-addressed exactly like a world/model
  sidecar. The sidecar always carries the diffuse slot; it carries specular
  and/or normal slots when the collection ships companion frames for them.
- **Optional per-collection specular/normal slots.** A collection MAY provide
  companion `<collection>_NN_spec.png` and/or `<collection>_NN_normal.png` frames
  alongside its `<collection>_NN.png` diffuse frames. When present, they bake
  into the SPECULAR / NORMAL slots of the same `.prm` bundle (each slot layered
  the same `frame_count`); when absent, the sidecar stays diffuse-only. A
  collection with no companions bakes a diffuse-only bundle whose content key
  equals the key computed with the spec/normal sets empty. This spec only bakes
  and uploads the extra slots — the shader-side consumption is owned by the
  separate billboard-specular-shimmer spec. See *Design decision 4*.
- **Per-frame mip independence, structural.** Each animation frame is a distinct
  array layer, so its mip chain is downsampled in isolation and no mip level can
  bleed one frame's texels into the next — the cross-frame atlas-mip artifact
  (which the strip layout `prm-array-layers` deleted was prone to) is impossible
  by construction, not by a re-stitch discipline.
- **The layered `.prm` mip-chain uploader.** Build `upload_texture_array_data`
  (the `texture_2d_array` multi-mip GPU uploader, alongside the existing
  single-layer `upload_texture_data`) and add a layer-major sibling
  `slot_layer_levels` that walks every layer's chain (leaving single-layer
  `slot_levels` unchanged) — the split `slot_levels`'s own doc comment defers
  to a `texture_2d_array` consumer. This spec is that first consumer.
- **Runtime load.** For a map-emitter collection the renderer content-hashes the
  collection's frames through the same shared function the compiler uses and, when
  that key resolves a `<key>.prm`, opens it directly and uploads the baked layered
  mip chain via `upload_texture_array_data` (the new *preferred* path). When no
  sidecar resolves, the renderer falls back to the existing array-based
  single-mip decode-and-upload path exactly as today. No PRL section carries
  sprite keys — the prop_mesh "no-section, runtime re-hash" pattern.
- **Mip-aware sprite sampler.** For a baked collection `mipmap_filter` flips from
  `Nearest` to `Linear` and `lod_max_clamp` is set per collection from the baked
  chain depth. The decode fallback keeps its existing single-mip sampler.
- Frame count stays runtime-derived from the PNG file count (unchanged source of
  truth); it continues to reach the shader through `SpriteDrawParams`.
- Fallback parity: a collection whose sidecar fails to resolve, hash, or load
  degrades to the existing runtime decode-and-upload array path (which itself
  falls to the 1×1 white one-layer placeholder if even decoding fails); load
  continues.

### Out of scope

- **A PRL sprite section** (TextureNames/TextureCacheKeys analog for sprites).
  Sprite collection names are not fully known at compile time. See *Design
  decision 2*.
- **Frame count in the PRM header or a baked KVP.** See *Design decision 1*.
  (`layer_count` in the PRM header records the array depth for the reader; it is
  not a second source of truth for the shader's `frame_count`, which stays
  runtime-derived — see *Design decision 1*.)
- The billboard pass blend mode, depth state, lighting math, and the
  `SpriteInstance` storage-buffer layout. The shader already samples
  `layer = frame_idx` with within-layer UV (shipped by `prm-array-layers`); the
  baked path changes only the uploaded mip count and the sampler, not the WGSL.
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
  They all keep working on the runtime decode fallback (see *Cross-spec
  coordination*); their PRM migration is owned by `sprite-png-retirement`, not
  this spec.
- **A PRM format or `STAGE_VERSION` change.** The layered v3 format
  (`layer_count`, layer-major payload) already shipped in `prm-array-layers`;
  this spec is a new producer/consumer of it, not a format change. See *Wire
  format*.

## Design decisions

### Decision 1 — Frame count stays runtime-derived from the PNG count

**Chosen:** the runtime keeps counting `<collection>_NN.png` files to derive
`frame_count`, exactly as `load_collection_frames` does today. Baking moves to
compile time; counting does not.

**Why:** `frame_count` already flows runtime → `SpriteDrawParams.params.x` →
shader UV/layer math, and that path is untouched. The PRM header's `layer_count`
records the array depth for the reader's payload validation, but the shader never
reads it; persisting the count as a *second* shader-facing source of truth would
add a wire surface for a value the runtime recovers for free by listing the same
directory it already lists to compute the content hash. The shader's frame-count
sourcing stays byte-for-byte unchanged. The compiler and runtime must agree on
frame **order and count** so the baked layer order matches the runtime's
`layer = frame_idx` sampling — both derive it from the same sorted-by-numeric-
suffix file scan, so the order is already shared by construction (lift the scan
into a shared helper; see *Rough sketch*). At bake time the compiler writes
`layer_count == frame_count`; at load time the parsed `layer_count` and the
directory count must match, or the sidecar is treated as stale and the runtime
falls back. This check is a defensive guard, not a reachable AC obligation: the
content key already folds every frame's bytes and per-slot counts, so a changed
frame count yields a different key and a plain cache miss — the mismatch branch
is reachable only via a blake3 collision.

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

### Decision 3 — Per-frame array layers with full mip chains (inherited representation)

**Chosen:** each frame is baked as an independent standard image into its own
`texture_2d_array` layer, carrying a **full** mip chain down to 1×1 (BC5 normal
to its 4×4 floor). The slot payload is layer-major (`layer_count == frame_count`),
matching the format `prm-array-layers` shipped. There is no stitched strip, no
per-frame re-stitch, and no sub-4px chain truncation.

**Why:** the representation is inherited, not chosen here. `prm-array-layers`
migrated the sprite path from a single stitched strip in one 2D slot to
`texture_2d_array` layers precisely because the strip could not join the baked
`.prm` pipeline: the reader's per-slot `level_count`/payload validation rejects a
per-frame-independently-mipped strip (its level-*n* width `N·(W>>n)` diverges from
the reader's `(N·W)>>n` for non-pow2 `W`), and a whole-strip BC5 normal chain
bleeds neighboring frames' normals into flickering highlights. Per-frame array
layers make each frame a standard single image the reader validates as-is, and
each layer's chain is structurally independent — so the diffuse chain simply runs
to 1×1 like any world/model diffuse, with no truncation heuristic to tune.

**Frame-count ceiling.** The ceiling is the `texture_2d_array` layer cap, not a
strip-width limit: `PORTABLE_MAX_TEXTURE_ARRAY_LAYERS` (256), already defined in
`crates/level-format/src/prm.rs` and enforced by the writer (`PrmFile::to_bytes`
rejects with `PrmWriteError::LayerCountExceedsPortableLimit`); the parser is
deliberately permissive, so every array-`.prm` producer owns the cap. At runtime
upload `plan_sprite_array` falls back when `frame_count` exceeds the device cap.
This
spec owns the **bake-time** enforcement `prm-array-layers` left to the writer: a
collection whose `frame_count` exceeds `PORTABLE_MAX_TEXTURE_ARRAY_LAYERS` emits
no sidecar (warns), and the runtime renders it on the decode fallback (itself
capped to the device limit). No `MAX_DIMENSION` change and no PRM version bump:
the per-axis 4096 cap now bounds one frame's `W×H`, well within reach, and the
layered v3 format already exists.

### Decision 4 — Optional specular/normal slots, discovered per collection, complete-or-absent

**Chosen:** the bake discovers companion frames by the same numeric-suffix scan
that finds diffuse frames — `<collection>_NN_spec.png` for the SPECULAR slot and
`<collection>_NN_normal.png` for the NORMAL slot — and bakes each companion set
into its own layered slot of the same `.prm` bundle (each layer a per-frame full
chain, `layer_count == frame_count`). Each slot is **complete-or-absent**: for a
given slot the companion set must cover every `<collection>_NN` diffuse frame or
none. A collection missing even one frame's companion for a slot warns and omits
that slot; the diffuse slot (and any complete companion slot) still bakes. The two
slots are independent — a collection may ship spec-only, normal-only, both, or
neither.

**Why complete-or-absent, not per-frame:** the runtime samples every slot by
`frame_idx` as the array layer, so a slot must have exactly `frame_count` layers.
A partial companion set would leave undefined layers and give the reader a
`layer_count` that disagrees with the diffuse slot. Requiring the set complete
keeps every slot's layer geometry identical to diffuse, which is what makes AC
#2's "specular level_count == diffuse level_count" hold and keeps one shared
per-collection sampler valid across slots. This is stricter than the world path,
which builds specular and normal at their own independent resolutions with no
diffuse-match check at all (only emissive gets a hard dimension-match `bail!`
against diffuse).

**Per-frame geometry, not just count.** Complete-or-absent is a frame-*count*
check; a `texture_2d_array` additionally requires every layer share one `W×H`. A
ragged diffuse set (one frame a different `W×H`), or a same-count companion set
with one differently-sized frame, cannot assemble into a uniform array. The bake
therefore also enforces geometry per slot: all frames *within* a slot must share
one `W×H`, or the slot is malformed; and each companion slot's per-frame geometry
must equal the diffuse frame geometry, or that companion is a mismatch. Ragged
**diffuse** frames reject the whole collection (no sidecar → runtime decode
fallback). A companion slot whose frames are same-count but a different resolution
warns and is **omitted** (the diffuse slot, and any geometry-matching companion,
still bakes) — one warning per omitted slot, not one per bad frame.

**Colorspaces (mirroring the world/model bake):** SPECULAR bakes to
`PrmFormat::R8Unorm` — linear, single-channel, the R channel of the decoded PNG,
through `build_specular_chain`, exactly as the world specular map bakes. NORMAL
bakes to `PrmFormat::Bc5RgUnorm` — linear, never sRGB, through
`build_normal_bc5_chain`, exactly as the world/model tangent-space normal map
bakes. BC5 needs both axes ≥ 4 px, so the normal slot's `level_count` is
`bc5_level_count(frame_w, frame_h)` and a collection whose frames are below 4×4
omits the normal slot (the runtime substitutes its neutral-normal placeholder,
matching the world path).

**Per-slot natural chain depth (no truncation).** Because each frame is a full
standard image with no strip truncation, the diffuse and specular slots both run
their chains to 1×1 and share one `level_count = expected_level_count(frame_w,
frame_h)`; the BC5 normal slot's `bc5_level_count` is naturally shallower (stops
at 4×4) — exactly the world/model bundle shape. A single per-collection sampler
serves all slots: `lod_max_clamp` keys on the bundle's deepest slot
(`header_mip_count`, as the world path does), and wgpu clamps each texture to its
own available mip count, so the shallower normal slot needs no separate truncation
to stay sampleable.

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
of the same PNG — both carry `slot_mask == DIFFUSE`, so the richer-bundle guard in
`texture_mips.rs` (the `matching_diffuse_only` path) would treat it as a valid hit
and cross-contaminate chains. The leading sprite-domain tag byte makes a sprite key
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
      under `<workspace>/baked/materials/` (the shared `prm_cache_root`/prm-root the
      model bake writes and the runtime reads) whose filename equals the runtime-computed
      key for the same frames. A no-companion collection's key equals the key
      computed with the spec/normal sets empty; its `slot_mask` is diffuse-only;
      its diffuse slot has `layer_count == frame_count` and per-layer
      `level_count > 1` (the fixture frames must be ≥2px for this to hold).
- [ ] A collection that ships complete `<collection>_NN_spec.png` and/or
      `<collection>_NN_normal.png` companion sets bakes a `.prm` whose `slot_mask`
      has the SPECULAR and/or NORMAL bit set, every slot `layer_count ==
      frame_count`. The specular slot is `R8Unorm` with `specular level_count ==
      diffuse level_count`; the normal slot is `Bc5RgUnorm` with `level_count ==
      bc5_level_count(frame_w, frame_h)`.
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
      companion-bearing sidecar. (A structural regression guard — both sides call
      the same key function — not a divergence detector.)
- [ ] For the baked path, a CPU-computed plan (analogous to the existing
      `plan_sprite_array`, unit-testable headless) exposes `array_layer_count ==
      frame_count`, `mip_level_count == baked level_count`, and `lod_max_clamp ==
      level_count - 1` for the diffuse sprite texture and sampler. Creating the
      actual `wgpu::Texture` (`texture_2d_array`) and `wgpu::Sampler`
      (`mipmap_filter: Linear`) from that plan is a GPU/review gate — `wgpu`
      exposes no getters to assert these values back off a live texture or
      sampler.
- [ ] The new `slot_layer_levels` sibling splits a multi-layer slot so that layer
      *k*'s mip chain is the *k*-th full chain in the layer-major payload
      (CPU-testable, no device); `slot_levels` stays single-layer, unchanged. And
      `upload_texture_array_data` writes layer *k*'s chain into array layer *k* at
      each level (GPU/review gate).
- [ ] When the loaded sidecar carries a SPECULAR or NORMAL slot, the runtime
      creates and uploads its layered mip chain and **retains** the texture view
      (plus the parsed `slot_mask`) on the sprite sheet, such that the billboard
      pass can retrieve the specular/normal views and the slot mask — not merely
      that the textures were uploaded. Wiring those views into the bind-group
      layout and sampling them is owned by the downstream billboard-specular-
      shimmer spec, not this one.
- [ ] A sprite viewed at distance no longer shimmers: the coarse mips are present
      and selected. Verify visually on `content/dev/maps/campaign-test.prl` (which
      has smoke emitters) — distant smoke is stable frame-to-frame under camera
      motion where it previously crawled.
- [ ] Each frame's baked chain is an independent per-frame downsample landing in
      its own layer: a fixture collection of solid-color frames (frame 0 red,
      frame 1 blue) bakes a bundle where, at every mip level, layer 0 stays pure
      red and layer 1 stays pure blue (within filter tolerance at the interior;
      edges may soften inward only) — so `layer = frame_idx` sampling always shows
      the authored frame, not a neighbor.
- [ ] A collection that fails to resolve (missing directory), or whose
      `frame_count` exceeds `PORTABLE_MAX_TEXTURE_ARRAY_LAYERS` (256), emits **no**
      sidecar and logs one warning; the runtime, finding no sidecar for the key,
      falls back to the existing decode path (which itself reaches the 1×1 white
      one-layer placeholder when there are no frames to decode, and is itself
      capped to the device layer limit) without panicking.
- [ ] The WGSL `SpriteInstance` struct stride still equals `SPRITE_INSTANCE_SIZE`
      (existing test `billboard_wgsl_sprite_instance_stride_matches_cpu` passes
      unchanged) and naga validation still passes. `frame_count` still reaches the
      shader via `SpriteDrawParams` (`draw_params_layout` passes unchanged). (An
      existing-test/gate check — "still green" — not a new metric.)

## Tasks

### Task 1: Shared frame-scan + content-key helpers in level-format

Lift the frame-discovery logic into shared, runtime-and-compiler code so both
sides agree on frame order, count, and content hash. Add to
`postretro-level-format` (the crate both `prl-build` and the runtime already
depend on for `prm`): a `collection_frame_paths(texture_root, collection, slot)`
that returns frame PNG paths in numeric-suffix order for the given slot, where
`slot` is a small `SpriteSlot` enum (`Diffuse` / `Spec` / `Normal`) selecting the
`<collection>_NN.png` / `<collection>_NN_spec.png` / `<collection>_NN_normal.png`
set — one call per slot.

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

`postretro-level-format` gains a `blake3` dependency (`blake3.workspace = true` —
already a workspace dependency) for `sprite_collection_filename_key`'s hashing.

### Task 2: Per-frame array-layer mip bake in the compiler

Add `bake_sprite_collection(texture_root, collection, cache_root) -> Option<[u8;
32]>` in the `texture_mips` module (its `cache`/`bake` submodules —
`bundle_hash_for`/`filename_key_for` live in `cache`, `build_diffuse_chain`/
`build_specular_chain`/`build_normal_bc5_chain` in `bake`; `matching_diffuse_only`,
for module-home context, is a local `let` inside `bake_diffuse_texture` in
`texture_mips.rs` proper, not a symbol the sprite bake calls — its collision
guard handles a case the sprite key's discriminator already prevents), alongside
the world/model bakers. It resolves the diffuse frames via the Task 1 helper and rejects (warn +
`None`) an empty set, a `frame_count` exceeding
`PORTABLE_MAX_TEXTURE_ARRAY_LAYERS` (256 — the bake-time enforcement
`prm-array-layers` left to the writer), or a **ragged** diffuse set whose frames
do not all share one `W×H` (a `texture_2d_array` requires uniform layer
dimensions). It then bakes the diffuse slot by running the existing per-image
Mitchell-Netravali chain (`build_diffuse_chain(rgba, width, height, lut:
&[f32; 256])`) on **each frame independently** down to 1×1, and concatenating
the per-frame chains **layer-major** into one `PrmSlot` payload with
`layer_count == frame_count`. `build_diffuse_chain` requires an sRGB→linear LUT;
`bake_sprite_collection` feeds it the same sRGB decode LUT the world/model
diffuse bake builds. (The specular and normal chains take no LUT.)

Then discover the optional companion sets — `<collection>_NN_spec.png` and
`<collection>_NN_normal.png` — via the same Task 1 helper. Each companion slot is
**complete-or-absent** and **geometry-matched**: if present it must cover every
diffuse frame (else warn and omit that slot) and each of its frames' `W×H` must
equal the diffuse frame geometry (else warn and omit that slot); diffuse and any
valid companion still bake. For a present SPECULAR set, flatten each frame to its
R channel and run `build_specular_chain` per frame, emitting a layer-major
`R8Unorm` slot; for a present NORMAL set, run `build_normal_bc5_chain` per frame,
emitting a layer-major `Bc5RgUnorm` slot whose per-layer `level_count` is
`bc5_level_count(frame_w, frame_h)` (omit the normal slot when frames are below
BC5's 4×4 minimum). Each slot carries its natural chain depth — diffuse and spec
to 1×1, normal to BC5's floor — exactly the world/model bundle shape; no
per-slot truncation is applied. Only the filter primitive and slot format differ
per slot; the per-frame layer assembly is identical.

Emit one `PrmFile` carrying `PrmSlots::DIFFUSE` plus whichever companion slots
baked (`SPECULAR`, `NORMAL`), header `layer_count == frame_count`, addressed by
the Task 1 content hash (which folds every slot's bytes), written through the
existing `atomic_write` + cache-hit-check path so re-bakes are idempotent and
cross-collection dedupe is preserved. A collection with no companions emits a
diffuse-only bundle. Reuse the existing filter primitives (`build_diffuse_chain`,
`build_specular_chain`, `build_normal_bc5_chain`); the only new logic is the
per-frame iteration and layer-major payload assembly around them.

Build the per-layer-content fixture here (a two-frame collection, frame 0 solid
red / frame 1 solid blue) so the per-frame layer assembly has a test — the
per-layer-content AC — asserting layer 0 is pure red and layer 1 pure blue at
every level.

Note (pre-existing, not solved here): `atomic_write` writes sidecars straight into
the prm-cache and bypasses the `StageCache` LRU, so a companion edit that changes
the content key leaves the old content-addressed sidecar as an unpruned orphan.
This mirrors the world/model bake's existing behavior; this spec does not add
pruning.

### Task 3: Compiler discovery + bake pass

Add `billboard_sprite_collections(entities) -> Vec<&str>`: walk MapEntity records,
select `classname == "billboard_emitter"`, read the last-wins `sprite` KVP
(default `"smoke"` when absent, matching the FGD default and the runtime
component default), dedupe in map order — structurally the twin of
`prop_mesh_model_handles`. Add `bake_sprite_textures(entities, texture_root,
prm_cache_root)` that calls `bake_sprite_collection` per discovered collection and
warns-and-continues on failure — the twin of `bake_model_textures`. The optional
spec/normal companion frames are discovered inside `bake_sprite_collection`
(Task 2) from the collection name, so this pass needs no extra discovery — it
bakes whatever companion slots the collection ships. Define both twins in
`main.rs`, alongside `bake_model_textures`, but wire the call in
`crates/level-compiler/src/pipeline.rs`, next to the `TextureMips` stage's
existing `bake_model_textures` call (right after `bake_texture_mips`) — the
production call site, not `main.rs`. At that call site the in-scope root
variable is `prm_root` (not `prm_cache_root`) and the entity slice is
`map_data.map_entities`. Pass the same `texture_root` that stage passes to
`bake_texture_mips` (frames live under `textures/`), not the `content_root`
`bake_model_textures` uses.

### Task 4: Runtime baked-preferred load path + the layered uploader

Build the layered uploader this spec owns, then add a baked-preferred branch to
`register_collection` (`crates/renderer/src/render/smoke.rs`), keeping its existing
`plan_sprite_array` + per-layer `upload_layer` single-mip body as the decode
fallback rather than removing it.

**The layered uploader.** `slot_levels(slot: &PrmSlot)` takes no `layer_count`
(that field lives in the parsed PRM header, not `PrmSlot`) and its
`debug_assert` pins the single-layer payload length; keep it and its existing
single-layer callers (`upload_slot_or_placeholder` in
`renderer/loaded_texture.rs`, the CPU test) working unchanged. Add a sibling
function — e.g. `slot_layer_levels(slot, layer_count)`, with `layer_count` taken
from the parsed PRM header — that yields each layer's per-level list by walking
the layer-major payload `layer_count` times; `slot_levels`'s own doc comment
already defers this `texture_2d_array` split to this consumer. Add
`upload_texture_array_data` (`crates/renderer/src/render/loaded_texture.rs`,
alongside `upload_texture_data`) that creates a `texture_2d_array` with
`array_layer_count == layer_count` and `mip_level_count == level_count` and writes
every layer's chain. These are the `slot_levels` layer-major generalization and
the `D2Array` uploader `prm-array-layers` named as landing "with their baked-load
first caller" here.

**Baked-preferred branch.** Given a collection name and `texture_root`, compute the
key through the shared Task 1 `sprite_collection_filename_key(texture_root,
collection)` (which scans diffuse + spec + normal internally, so the runtime cannot
fold a different slot set than the compiler did). Thread `prm_cache_root` (the
`baked/materials` root, live at both `lifecycle.rs` call sites) from
`install_level_payload` → `register_smoke_collection` → `register_collection`; the
baked branch resolves the sidecar as
`prm_cache_root.join(format!("{}.prm", cache_filename_for_key(&key)))`. When that resolves a `<key>.prm`
**and** its header `layer_count` matches the directory frame count, parse it and
upload the diffuse slot's layered mip chain via
`upload_texture_array_data`, and build the sprite sampler with `mipmap_filter:
Linear` and `lod_max_clamp = header_mip_count(slots) - 1` (the deepest slot; wgpu
clamps each texture to its own mip count). When **no** sidecar resolves (or the
`.prm` fails to parse, or `layer_count` disagrees with the frame count — a
defensive guard with no AC obligation; see Decision 1), fall
through to the existing `plan_sprite_array` decode-and-upload single-mip array
path exactly as today — `load_collection_frames` / `SpriteFrame` stay for this
purpose — which itself uploads the 1×1 white one-layer placeholder when there are
no frames at all.

Factor a baked-path plan struct (analogous to the existing `plan_sprite_array`
in `crates/renderer/src/render/smoke.rs`) that exposes `array_layer_count`,
`mip_level_count`, and the `lod_max_clamp` value, unit-testable headless without
a device; `upload_texture_array_data` and the sampler build consume that plan
(the wgpu texture/sampler objects remaining the GPU/review gate).

When the parsed sidecar's `slot_mask` also carries the SPECULAR and/or NORMAL
bits, create and upload those slots' layered mip chains through the same uploader
(it handles `R8Unorm` and `Bc5RgUnorm`), and **retain** their texture views — add
optional `specular_view` / `normal_view` fields plus the parsed `slot_mask` to
`SpriteSheet` (`crates/renderer/src/render/smoke.rs`, today only `bind_group` and
`frame_count`) so the views survive past this function and the billboard pass can
retrieve them. A wgpu texture/view with no retained owner is dropped on return, so
without this the "resident for the billboard bind group" property is not actually
produced. Wiring those views into the billboard bind-group layout and sampling them
is a downstream consumer owned by the billboard-specular-shimmer spec — this task
uploads and retains the views, it does not change the shader or the existing
bind-group bindings.

Frame count is still derived from the PNG file count and reaches the shader via
`SpriteDrawParams` unchanged. The emitter/projectile call site
(`crates/postretro/src/startup/lifecycle.rs`) needs the frame count *before*
calling the wrapper, because it feeds `resolve_sprite_collection_draw_contract`,
which derives `required_lifetime` from the frame count: that site keeps a
`load_sprite_frames(texture_root, collection)` call for its `.len()` — the
decode-superset count (1 for a direct `.png`, N for an `_NN` collection) — and
uses that count for both `resolve_sprite_collection_draw_contract` and
`SpriteDrawParams` (`frame_count`); this is correct in every case. The wrapper
`register_smoke_collection`, once it stops taking `&[SpriteFrame]`, loads the
decode-fallback frames internally via the same `load_sprite_frames(texture_root,
collection)` on a baked-path miss — the superset loader that returns a single
frame for a direct-`.png` reference and delegates to `load_collection_frames`
for a non-`.png` collection name (e.g. `"smoke"`, `"impact"`) — so it covers
both the emitter/projectile call site and the weapon-impact call site. The
baked-sidecar key is computed inside `sprite_collection_filename_key` (Task 1,
which scans internally) — do not source the shader/contract `frame_count` from
`collection_frame_paths(...Diffuse).len()`, which is 0 for a direct `.png` and
would mistime single-frame collections.

Update the `register_smoke_collection` wrapper — a `Renderer` method in
`crates/renderer/src/render/renderer_resources.rs` — so it no longer takes
`&[SpriteFrame]`; it takes the collection name + texture root + prm-cache root +
the spec/lifetime params it already passes. Update the **two** call sites, both in
`crates/postretro/src/startup/lifecycle.rs` (the emitter/projectile-loop call and
the `weapon::impact_sprite_collection()` call — not `main.rs`), accordingly.

No WGSL logic changes: the shader already samples `texture_2d_array` at
`layer = frame_idx` with within-layer UV (shipped by `prm-array-layers`); baked
mips are selected automatically once the sampler filters mips. Confirm the
existing naga-validation and `billboard_wgsl_sprite_instance_stride_matches_cpu`
tests still pass; keep the GPU-layout pins `MAX_SPRITES` and `SPRITE_INSTANCE_SIZE`
untouched.

## Sequencing

**Phase 1 (sequential):** Task 1 — defines the frame-scan and content-hash
contract every other task consumes. Blocks 2, 3, 4.
**Phase 2 (concurrent):** Task 2 (compiler bake entry) and Task 4 (runtime load +
uploader) — they meet only at the Task 1 hash + the layered `.prm` wire format, so
they parallelize against a hand-built fixture sidecar. The fixture **must include
companion (spec and/or normal) slots** and assert the round-trip that the runtime's
`sprite_collection_filename_key` for that companion-bearing collection equals the
fixture's filename. Without a companion slot in the fixture, a Blocker-class
compiler/runtime divergence over which slots the key folds goes unsurfaced until
integration.
**Phase 3 (sequential):** Task 3 — wires the compiler discovery to the Task 2 bake
entry; consumes Task 2's `bake_sprite_collection`.

Task 4 keeps the (retained) decode fallback in place, so there is no separate
retirement phase — the decode path is kept, not retired, by this spec.

## Cross-spec coordination

**Builds on `prm-array-layers` (done).** That spec extended the `.prm` format with
a file-header `layer_count` (STAGE_VERSION 3, layer-major payload), migrated the
billboard sprite path to `texture_2d_array` end to end (g1b0 bind layout,
`billboard.wgsl` sampling `layer = frame_idx`, the decode fallback uploading array
layers), left the sprite path on an array-based single-mip decode fallback with no
baked mips, and defined `PORTABLE_MAX_TEXTURE_ARRAY_LAYERS`. It named this spec as
its first consumer for the layer-major `slot_layer_levels` split, the
`upload_texture_array_data` uploader, and the bake-time layer-count enforcement —
all of which this spec delivers.

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
  `collection_frame_paths(texture_root, collection, slot)` so the compiler's
  layer order and the runtime's `frame_count` agree by construction. For a baked
  hit the runtime only reads raw
  PNG bytes to compute the lookup key (no decode); on a miss it still decodes the
  frames to pixels for the retained decode-and-upload fallback, exactly as today.
- **Per-frame mip.** For each frame, run the existing separable
  Mitchell-Netravali downsample (`build_diffuse_chain`) on that frame's `W×H`
  pixels alone with edge clamp, producing that frame's full chain to 1×1. Lay the
  per-frame chains out layer-major into the slot payload (`layer_count ==
  frame_count`); the reader validates each layer's chain as a standard image, so
  there is no cross-frame divergence to reconcile.
- **Layer-count cap.** `frame_count > PORTABLE_MAX_TEXTURE_ARRAY_LAYERS` (256) →
  reject at bake (warn, no sidecar). This is the array analog of the strip-width
  ceiling the earlier draft carried; the per-axis 4096 cap already bounds one
  frame's `W×H`.
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
  sampler. For a baked collection build a `Linear`-mip sampler with `lod_max_clamp`
  from the bundle's mip depth — either a per-collection sampler keyed on
  `header_mip_count` (cheap, few collections) or the world path's
  `mip_count_aniso_samplers` pool. Per-collection is simplest at sprite scale.

## Boundary inventory

| Name | Rust | Wire / serde | FGD KVP |
|---|---|---|---|
| Sprite collection name | `BillboardEmitterComponent.sprite` / MapEntity `sprite` KVP | n/a (not in PRL) | `billboard_emitter.sprite` (default `"smoke"`) |
| Sprite sidecar key | `sprite_collection_filename_key(texture_root, collection) -> [u8;32]` (scans diffuse+spec+normal internally; discriminator + mask + per-slot tag/count/bytes) | `.prm` filename stem (hex) via `cache_filename_for_key` | n/a |
| Sprite `.prm` layer count | header `layer_count == frame_count` (each frame one array layer) | PRM header `layer_count` (v3, layer-major payload) | implied by `<collection>_NN.png` count |
| Sprite `.prm` slot mask | `PrmSlots::DIFFUSE` always; `SPECULAR` / `NORMAL` when companion frames present | PRM header `slot_mask` bits 0 (diffuse), 1 (specular), 2 (normal) | n/a |
| Spec companion frames | `<collection>_NN_spec.png` → `PrmFormat::R8Unorm` slot | PRM SPECULAR slot (layered) | n/a |
| Normal companion frames | `<collection>_NN_normal.png` → `PrmFormat::Bc5RgUnorm` slot | PRM NORMAL slot (layered) | n/a |
| Frame count | runtime PNG-count → `SpriteDrawParams.params.x` | bitcast `f32` in draw-params UBO | implied by `<collection>_NN.png` count |

## Wire format

No new binary surface. Sprite collections reuse the layered v3 `.prm` wire format
(`postretro-level-format::prm`) that `prm-array-layers` shipped: a sprite bundle
sets `layer_count == frame_count` with a layer-major payload, where world/model
bundles set `layer_count == 1`. The sidecar always carries the diffuse slot and
may also carry the SPECULAR and NORMAL slots the format already defines
(`PrmSlots` bits 1 and 2). Adding the companion slots is not a format change:
`prm.rs` already defines all four slots and the `slot_mask` bits. No PRL section is
added, so `pack.rs` and the PRL header version are untouched. The PRM
`STAGE_VERSION` does **not** bump: the layered format already exists at v3; this
spec only adds a new producer/consumer of it.

## Sources

- `context/plans/done/prm-array-layers/index.md` — the layered `.prm` format and
  the `texture_2d_array` sprite migration this spec bakes for.
