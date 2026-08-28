# PRM Array-Layer Textures — Billboard Sprite Migration

## Goal

Extend the `.prm` texture format with a per-file **layer count** and migrate the
billboard sprite texture path from a stitched horizontal strip in one 2D slot to
**per-frame `texture_2d_array` layers** — each animation frame an independent
standard image with its own normal mip chain. This is the foundation the
`billboard-sprite-prm-baking` (mip bake) and `billboard-specular-shimmer`
(per-texel spec/normal) specs depend on, and it lands first. End state: the
billboard sprite path is `texture_2d_array` end to end, filled by the
(array-based, single-mip) runtime decode fallback so the engine builds and
renders; world and model textures are unchanged (`layer_count == 1`, still 2D;
per-slot payloads byte-identical, the file header alone gaining the version bump
and `layer_count`); no baked sprite mips exist yet — those are the downstream bake's
payoff.

## Scope

### In scope

- **PRM format extension.** A `layer_count` field in the file header;
  `STAGE_VERSION` 2 → 3; per-slot payload sized `× layer_count` in **layer-major**
  order (layer 0's full chain, then layer 1's, …). World/model bundles carry
  `layer_count == 1`; sprite books carry `layer_count == frame_count`.
- **Reader/writer.** `parse_slot` takes `layer_count`; `expected_payload_bytes`
  and `slot_levels` multiply by it. Per-slot `level_count` validation is
  unchanged (non-BC5 full chain, BC5 `bc5_level_count`), applied per layer.
- **Upload path (world/model only).** `upload_texture_data` stays 2D at
  `layer_count == 1`; a golden pins the D2 view and per-slot payload encoding. The
  `D2Array` uploader is out of scope.
- **Billboard sprite path migration (atomic — all of it, or the engine does not
  build):** the g1b0 sprite binding becomes `texture_2d_array`; `billboard.wgsl`
  samples by layer (`layer = frame_idx`, within-layer UV `0..1`) instead of the
  strip's `u = (frame_idx + cd.z) / frame_count`; `frame_idx` is computed in
  `vs_main` and passed to `fs_main` flat-interpolated on `VertexOutput`;
  `register_collection`'s decode fallback uploads `frame_count` array layers
  (still single-mip; sampler unchanged — Linear mag/min, Nearest mipmap) instead
  of one strip.
- **Layer-count gates.** A bake-time cap at the portable wgpu baseline
  (`max_texture_array_layers`, 256) sourced from a named constant, and a runtime
  cap querying `device.limits().max_texture_array_layers` as a backstop.

### Out of scope

- **Baking mipped sprite `.prm` sidecars, the sprite content-hash, and sprite
  discovery** — owned by `billboard-sprite-prm-baking`. This spec makes those
  baked array sidecars *loadable* (the layer-aware reader) but bakes nothing and
  builds no `D2Array` GPU uploader for them: nothing in scope consumes one — the
  sprite fallback uploads its layers inline — so `upload_texture_array_data` and
  the `slot_levels` layer-major generalization land in that spec, with their
  baked-load first caller. The sprite path stays on the array-based single-mip
  decode fallback.
- **Per-texel specular/normal sampling and the shimmer lighting math** — owned by
  `billboard-specular-shimmer`. This spec only routes `frame_idx` to the fragment
  stage for that spec to sample by.
- **Any world/model behavior change.** World/model stay `layer_count == 1`, 2D;
  their payload encoding and GPU upload are unchanged (the golden proves it) —
  only the file header carries the version bump and `layer_count`. The sprite
  migration is the only
  consumer flipped to arrays here.
- **A per-slot layer count.** Layer count is a file-level (bundle) property; all
  slots in a `.prm` share it. Per-slot-independent layer counts are not needed by
  any planned consumer (the sprite bundle's diffuse/spec/normal all carry the same
  frame count).

## Direction

**Problem.** The billboard sprite path cannot move onto the baked `.prm`
pipeline as a strip. `parse_slot` (`crates/level-format/src/prm.rs`) hard-rejects
any **non-BC5** slot whose `level_count != expected_level_count(width, height)`
(the full chain to 1×1), and `expected_payload_bytes` sizes level *n* as
`(width>>n)×(height>>n)`. A stitched strip mipped per-frame-independently records
a *truncated* `level_count` and produces level-*n* width `N·(W>>n)`, which
diverges from the reader's `(N·W)>>n` for non-power-of-two `W` — so the baked
strip fails `LevelCountMismatch`/`PayloadBytesMismatch` and never loads. Per-frame
`texture_2d_array` layers make each frame a standard single image the reader
validates as-is, dissolving the truncation rejection, the non-pow2 payload
divergence, the sub-4px zero-level case, and the `N·W` 4096 strip-width ceiling
(the per-axis cap now bounds `W×H`).

The BC5 normal slot the downstream shimmer feature needs is where the strip mips
*worst*: `bc5_level_count` is computed on the *strip* dimensions, so a strip's
coarse BC5 levels are whole-strip normal downsamples that bleed neighboring
frames' normals into a wrong-vector, flickering highlight. Array layers make that
a structural non-issue — each layer's normal chain is independent.

**Prior commitments.** The billboard sprite move onto `.prm` is the
`billboard-sprite-prm-baking` goal; this spec is the format/representation
foundation it was missing. "Baked over computed" and "runtime baking is a
non-goal" (index principles) keep baking at build time. The renderer already
binds and samples `texture_2d_array` in this very shader (`sh_total_atlas` g3b1,
`sh_direct_atlas` g3b15 in `billboard.wgsl`) and already uploads array layers
safely, so "renderer owns GPU / no `unsafe`" is satisfied by existing precedent,
not a new capability.

*Divergence.* `billboard-sprite-prm-baking` originally scoped `D2Array` **out**
(Decision 3 kept the strip). That is reversed here on the strength of the
`parse_slot` incompatibility above and the BC5-bleed argument: arrays are the
chosen representation, and that spec's Decision 3 (strip + per-frame-independent
re-stitch + sub-4px truncation) is superseded.

**Alternatives rejected.**
- *`texture_3d`.* Mips on all three axes — the depth (frame) axis halves per
  level, blending frame *n* with *n+1* at mip 1. Cross-frame bleed built into the
  resource type.
- *Per-frame separate `.prm` files (zero format change).* The honest runner-up,
  but strictly more total work: it still needs the same renderer D2Array + WGSL
  change, multiplies sidecar files and content keys by N (×3 with spec/normal),
  and pushes array assembly to a runtime `copy_texture_to_texture` path that does
  not exist. Keep only if the team vetoes touching the shared reader.
- *Sprite-specific mini-format.* Guarantees zero world/model risk but permanently
  forks baking, content-addressing, cache, and a second reader from the pipeline
  the sprite path is trying to *join*. The format change here is additive with a
  `layer_count == 1` golden proving world/model bytes are unchanged — lower total
  cost than a fork.
- *Keep the strip: full chain + power-of-two frames + `lod_max_clamp` anti-bleed.*
  Reader-compatible without a format change, but perpetuates the atlas-mip hack
  (coarse levels are stored-but-clamped junk), imposes a permanent pow2 frame
  authoring constraint, and leaves the per-collection "which coarse level is safe"
  tuning the bake spec's own Open Question flags. Arrays replace all of that with
  a structural guarantee; the one-time format bump is cheaper on a pre-stable
  engine with disposable caches than shipping the strip and redoing it when
  shimmer's distance-crawl AC fails on BC5 bleed.

## Acceptance criteria

- [ ] A multi-layer `PrmFile` round-trips: writing a slot with `layer_count = N`
      and layer-major payload, then parsing it back, yields the same
      `layer_count`, the same single per-slot `level_count` (non-BC5 full, BC5
      truncated — one header value all layers share, plus the aggregate
      `× layer_count` payload size the reader checks; there are no per-layer
      headers), and the same bytes.
- [ ] **Golden (CPU, always-run):** a `layer_count == 1` bundle (a world/model
      diffuse+spec+normal case) serialized under v3 has per-slot **payload** regions
      byte-identical to an explicit committed byte-literal baseline (one mip chain
      per slot, unchanged) — the world/model payload encoding is provably untouched;
      only the file header differs (v3, `layer_count == 1`). **GPU/review gate:**
      `upload_texture_data` still yields a `D2`, single-layer texture for it (checked
      by code review + the device-gated path, not a headless assertion —
      `wgpu::TextureView` exposes no view-dimension getter).
- [ ] `parse_header` rejects `layer_count == 0` as malformed; a stale pre-change
      sidecar is rejected and re-baked — a real v2 file trips the magic
      version-byte check (`magic[3] != 0x02` → `UnsupportedVersion`) before the
      `stage_version` check, which stays the secondary guard for a
      magic-consistent-but-stale file (the re-bake is the existing content cache
      reacting to the load failure — no new code); the header now reports
      `STAGE_VERSION == 3`.
- [ ] **(review gate)** The portable-baseline cap `u32` constant
      (`max_texture_array_layers` = 256) is defined on the dependency-free
      format/compiler side and documented as the contract every array-`.prm` writer
      must honor (the bake-time *rejection* using it — warn, no sidecar — lands in
      `billboard-sprite-prm-baking`, which owns the writer). **(unit test)** The
      runtime cap predicate rejects `frame_count > max_layers`;
      `register_collection` calls it before creating its D2Array texture and, on
      failure, `warn!`s and falls back rather than hitting device validation.
- [ ] **(review + naga gate)** `register_collection` uploads `frame_count`
      single-mip array layers and `billboard.wgsl` samples `texture_2d_array` at
      `layer = frame_idx` with the strip UV math
      (`u = (frame_idx + cd.z)/frame_count`) removed (naga-validated).
      **(manual visual check)** An animated collection plays its frames in order —
      verify by eye on `content/dev/maps/campaign-test.prl` smoke; there is no
      automated pass/fail for the on-screen result.
- [ ] `frame_idx` reaches `fs_main` flat-interpolated on `VertexOutput` (an
      `@interpolate(flat) u32`) — naga rejects an un-flat integer varying, so the
      existing naga-validation test guards it. **(review gate)** Its availability to
      the downstream shimmer spec has no functional consumer yet.
- [ ] WGSL naga validation passes and the existing
      `billboard_wgsl_sprite_instance_stride_matches_cpu` and `draw_params_layout`
      tests still pass; `SpriteInstance` stride and `SpriteDrawParams` are
      unchanged (the migration touches the sprite *texture* binding and
      `VertexOutput`, not the instance/draw-params layouts).

## Tasks

### Task 1: `layer_count` in the PRM file header + layered payload sizing

In `crates/level-format/src/prm.rs`, add a `layer_count: u16` to `PrmHeader` (and
its parse/serialize). Adding the field is compile-forced at every `PrmHeader`
construction site — update them all to `layer_count: 1`, notably the production
baker in `crates/level-compiler/src/texture_mips.rs` (AC2's "re-baked under v3"
bundle depends on it emitting `layer_count: 1`). Today the file header is 43 bytes
with the magic
`b"PRM\x01"` at bytes 0..4 and one free reserved byte at `data[6]`. Append
`layer_count` as a little-endian `u16` at bytes 43..45 (after `total_body_bytes`),
growing the header 43 → 45 and leaving the reserved `data[6]` byte reserved; a
`u16` (rather than a `u8` reused into `data[6]`) lands cleanly on the 256 cap with
headroom, at the cost of one header byte. Bump `STAGE_VERSION` 2 → 3 **and** the
magic's fourth byte in lockstep (`b"PRM\x01"` → `b"PRM\x02"`), per the format's
stated magic/version lockstep invariant for incompatible layout changes. Require
`layer_count >= 1` in `parse_header` (reject 0 as malformed). Thread `layer_count` from the parsed
header into `parse_slot` (currently `parse_slot(body, offset, slot_index)`) and
into `expected_payload_bytes`, multiplying the summed chain bytes by
`layer_count` (layer-major: the slot payload is `layer_count` back-to-back full
chains). Leave the per-slot `level_count != expected_levels` check byte-for-byte
(non-BC5 `expected_level_count`, BC5 `bc5_level_count`) — it validates one layer's
chain, and every layer shares dimensions and depth. Confirm the
`total_body_bytes`/body-length cross-checks in `from_bytes_partial` still hold:
they sum `SLOT_HEADER_SIZE + slot.payload.len()` over the actual (already-layered)
wire payload, so they need no per-layer awareness. Make the new `× layer_count`
multiply in `expected_payload_bytes` a `saturating_mul` (matching that function's
existing saturating arithmetic), so a pathological `layer_count × chain` overflow
saturates to a clean size mismatch/reject rather than panicking; the
`from_bytes_partial` `total_body_bytes` cross-check (`u32`, `saturating_add`) then
rejects the truncated body. Tests: multi-layer round-trip; `layer_count == 0`
reject; `STAGE_VERSION == 3`; a version-2 fixture rejected — build it with the
**old** magic 4th byte (`magic[3] == 0x01`, the pre-change value; the magic byte
and `STAGE_VERSION` are numbered independently), which now fails `magic[3] != 0x02`
→ `UnsupportedVersion` before the secondary `stage_version` check. Update the
in-file churn the field + magic bump force: flip the writer's `b"PRM\x01"` literal
to `b"PRM\x02"`; invert the existing `unsupported_magic_version_byte_is_rejected`
test (`0x02` is now the *valid* byte); and fix the hand-built
`vec![0u8; HEADER_SIZE]` fixtures so their new bytes 43..45 carry
`layer_count == 1`, not 0.

### Task 2: `layer_count == 1` golden (world/model upload unchanged)

Add the golden proving the world/model upload path is untouched. The runnable core
is CPU-only: a `layer_count == 1` bundle (diffuse+spec+normal) serialized through
`to_bytes` produces per-slot payload regions byte-identical to an **explicit
committed byte-literal baseline** (header stripped) — not to bytes recomputed by
the same `to_bytes`/`slot_levels` code under test, which would be tautological. Pin
the baseline as a fixture. `upload_texture_data` and `slot_levels`
(`crates/render-cpu/src/loaded_texture.rs`) stay as they are — at `layer_count == 1`
each slot is one chain, so the task asserts the degenerate path rather than
touching the uploader. The D2-view half (that `upload_texture_data` yields a `D2`,
single-layer texture) is a **GPU/review gate**, not a headless unit assertion:
`wgpu::TextureView` exposes no view-dimension getter, and `texture.dimension()` /
`depth_or_array_layers == 1` need a device (renderer GPU tests self-skip headless).
The `D2Array` uploader is out of scope.

### Task 3: Migrate the billboard sprite path to `texture_2d_array`

This is one atomic change — the engine does not build in any partial state.
In `crates/renderer/src/render/smoke.rs`: flip the g1b0 entry in
`sprite_sheet_bind_group_layout_entries` to `view_dimension: D2Array`; in
`register_collection`, create a `frame_count`-layer texture (`mip_level_count: 1`)
and a `D2Array` view, and upload each frame as a layer (the decode fallback path —
`stitch_frames_to_strip`'s role changes from producing one strip to producing
per-frame layer data, or is replaced by a per-frame upload loop; its existing
mismatched-frame drop becomes the shared-dimension precondition array layers
require). The sampler is unchanged (Linear mag/min, Nearest mipmap) — still
single-mip, no baked mips yet. In `billboard.wgsl`:
`sprite_texture` becomes `texture_2d_array<f32>`; `sample_post_retro` gains a
`layer: u32` argument (snapping now against the true per-frame
`textureDimensions`, which is more correct than the whole-strip dims it snaps
against today); replace `u = (frame_idx + cd.z)/frame_count` with within-layer UV
(`cd.z`, `cd.w` → `0..1`) sampled at `layer = frame_idx`; compute `frame_idx` in
`vs_main` (as today, from `age`) and add `@interpolate(flat) frame_idx: u32` to
`VertexOutput` so `fs_main` (and the downstream shimmer spec) can sample by it.
The `register_smoke_collection` wrapper (`renderer_resources.rs`) and its two call
sites (`crates/postretro/src/startup/lifecycle.rs`) are unchanged in this task —
they still pass decoded `SpriteFrame`s; only the upload representation changes.
If `stitch_frames_to_strip` is replaced by a per-frame upload loop, update or
retire its unit tests (`stitch_rejects_empty`, `stitch_single_frame`,
`stitch_two_frames` in `smoke.rs`) in the same change. Confirm naga validation +
the stride/draw-params tests pass.

### Task 4: Layer-count gates (portable bake cap + runtime backstop)

Define the bake-time ceiling as a fresh named `u32` `const` (value 256 — the
portable wgpu baseline / WebGPU spec floor for `max_texture_array_layers`) in the
dependency-free format/compiler side (e.g. `postretro-level-format`). It cannot be
*sourced from* `wgpu` (that crate is deliberately dependency-free and pulls in no
`wgpu`), and it cannot reuse the renderer's existing
`REQUIRED_MAX_TEXTURE_ARRAY_LAYERS`, which is a private `const` in the renderer
crate unreachable from the compiler — so this is a new literal, documented as the
cap any array `.prm` writer must honor — because a `.prm` is content-addressed and shipped across machines and the
compiler has no adapter, the bake cap cannot query a device. (The array `.prm`
*writer* lives in `billboard-sprite-prm-baking`; this task establishes and
documents the contract + the constant, and the runtime side.) At runtime, guard
the sprite `frame_count` against the device cap through a **pure predicate**
(e.g. `fn array_layers_within_cap(frame_count: u32, max_layers: u32) -> bool`, or
an existing `array_layers_sufficient`-style helper) so the check is unit-testable
headless, without a device. `register_collection` calls it before `create_texture`
(the `frame_count` is the inline D2Array layer count it requests as
`depth_or_array_layers`, and `device` is already a parameter); on failure it
`warn!`s (log-capture-observable) and falls back rather than triggering device
validation. A *parsed* `.prm` `layer_count` cannot reach here: the baked-array load
that yields one is out of scope, so this backstop guards the decode fallback's
frame count.

### Task 5: Documentation

Document the array-layer **format** extension (`layer_count`, layer-major payload,
`STAGE_VERSION` 3, world/model = 1) where the PRM format lives — `build_pipeline.md`
(the baked-texture-mips section `prm.rs` points at) — and the billboard sprite
path's D2Array **representation** in `context/lib/resource_management.md` §6
(Billboard Sprites) and `context/lib/rendering_pipeline.md` §7.4; note the strip
layout is retired for sprites.

## Sequencing

**Phase 1 (sequential):** Task 1 — the format contract every other task and the
downstream bake consume. Blocks 2, 3.

**Phase 2 (concurrent):** Task 2 (`layer_count == 1` golden) and Task 3 (renderer
+ shader migration) — independent: Task 2 pins the degenerate world/model path,
Task 3 flips the sprite path to arrays. Neither builds the `D2Array` `.prm`
uploader; Task 3's array texture is created inline in `register_collection`. **Thin-slice note:** stand up the sprite
array path end-to-end first — an animated collection decoded into `frame_count`
inline array layers (Task 3) sampled at `layer = frame_idx` by the migrated shader
— so the decode→array-upload→sample boundary is falsified before the rest of the
ACs are written.

**Phase 3 (sequential):** Task 4 — the gates wrap the path Tasks 1–3 establish.

**Phase 4 (sequential):** Task 5 — documents the landed representation.

**Cross-spec:** this whole spec sequences **before** `billboard-sprite-prm-baking`
(whose Decision 3 truncation/re-stitch deletes, which becomes the mip-bake payoff
on this format, and which additionally builds the `D2Array`
`upload_texture_array_data` uploader + `slot_levels` layer-major generalization to
load its baked array sidecars); `billboard-specular-shimmer`'s prerequisite #1
points here (it inherits the D2Array shader and consumes `frame_idx`);
`sprite-png-retirement` depends on this transitively. `billboard-volumetric-
direct-lighting` shares a `billboard.wgsl`/`VertexOutput` merge point (orthogonal;
rebase, not dependency).

## Boundary inventory

| Name | Rust | Wire / serde | WGSL |
|---|---|---|---|
| Layer count | `PrmHeader.layer_count: u16` | file header, little-endian, `>= 1` (world/model = 1) | implicit (array-layer count of the sprite texture) |
| Layered slot payload | `PrmSlot.payload` = `layer_count` back-to-back full chains | slot `payload_bytes` = `layer_count × per-layer chain`, layer-major | n/a |
| Upload | `upload_texture_data` = D2 at `layer_count==1` (world/model); sprite fallback builds its D2Array texture inline in `register_collection`; `.prm` array uploader → `billboard-sprite-prm-baking` | n/a | `texture_2d_array<f32>` at g1b0 |
| Frame index | computed in `vs_main` from `age` | n/a | `VertexOutput.frame_idx: u32` (`@interpolate(flat)`), `layer = frame_idx` |
| Layer cap | bake: fresh named `const` (256) in the format/compiler crate; runtime: `frame_count` vs `device.limits().max_texture_array_layers` in `register_collection` | n/a | n/a |

## Wire format

Extends the existing `.prm` format (`postretro-level-format::prm`) — a breaking
change, so `STAGE_VERSION` bumps 2 → 3 and the disposable prm-cache re-bakes.
Content-addressed filenames are unchanged (the sidecar key hashes source image
bytes, not header bytes). File header grows to carry `layer_count` (little-endian
`u16`; the current 43-byte header has only one free reserved byte, so the header
extends to 45 bytes — pin the exact `layer_count` offset against the current
header layout when implementing). Per-slot on-wire layout is unchanged except the
`payload_bytes` count now covers `layer_count` back-to-back full chains in
**layer-major** order (layer 0's complete mip chain, then layer 1's, …). Empty /
single-layer encoding: `layer_count == 1` is the world/model case and its payload
is exactly one chain per slot, byte-identical in shape to the pre-change payload
encoding under the golden; the file header itself changes (v3, +`layer_count`). `layer_count == 0` is rejected as malformed.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| World/model per-slot payloads and 2D upload are unchanged at `layer_count == 1` (only the file header gains version + `layer_count`) | Task 1 (layer-major degenerates to one chain), Task 2 (`upload_texture_data` keeps D2 at 1) | Task 2 — the sibling split must not change the `layer_count==1` path | Golden AC |
| Per-slot `level_count` stays independent and validated per layer | Task 1 (the `level_count` check is unchanged; only payload sizing gains `× layer_count`) | Task 1 — do not fold layer count into the per-slot chain-depth check | Round-trip AC |
| The bake layer cap is portable, not device-queried | Task 4 (named baseline constant) | Task 4 — a `.prm` is content-addressed and cross-machine; the compiler has no adapter | Cap-constant + runtime-backstop AC (bake-time enforcement verified in `billboard-sprite-prm-baking`) |
| The billboard sprite path is D2Array end to end (bind layout, shader, upload agree) | Task 3 (atomic migration) | Task 3 — a partial flip (shader without bind layout, or upload) fails pipeline creation and does not build | Render AC, naga/stride tests |

## Open questions

- **`stitch_frames_to_strip` fate.** Task 3 either repurposes it to emit per-frame
  layer data or replaces it with a per-frame upload loop; decide during the thin
  slice. Its mismatched-frame drop must survive as the shared-dimension gate array
  layers require.
