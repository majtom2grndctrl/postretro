# Shadowmask Atlas — BC5 Compress-at-Rest — Research

Grounding for `index.md`, read at 8ce91e682 (after the `sh-probe-streaming` merge).
Facts cited by symbol; the brief holds the decisions, this file holds the
investigation. No line numbers — they stale.

Capacity-side grounding — the layer budget, the binding ceiling, the assignment
rewrite, consolidation and streaming sequencing — lives in
`shadowmask-atlas-mask-capacity/research.md`.

## The id-42 surface today

`ShadowmaskAtlasSection` (`crates/level-format/src/shadowmask_atlas.rs`):

- 16-byte header (width, height, layer_count, selected_light_count), then `channels`
  (one byte per selected light, `0..3` = RGBA channel, `SHADOWMASK_CHANNEL_DROPPED` =
  `0xFF`, padded to 4), then `data`, layer-major `Rgba8Unorm`. 255 means fully visible.
  No format tag, no in-payload version.
- `from_bytes` validates `width × height × layer_count × 4` and gates channels at
  `> 3 && != 0xFF`.
- **Size is the lightmap atlas footprint, independent of light count.** Dims come from
  the shared atlas (`ShadowmaskFill::new` from `PreparedAtlas`); the loader rejects a
  mismatch. A dropped light frees zero bytes.

**No byte magnitude is recorded anywhere.** `lighting-scale--sh-base-atlas-at-rest-slimming`
measured ids 34, 35, 27, 41 and 45 on `campaign-test` and omitted 42.

## Where compression happens, and where id 42's encode lands

Precedents encode before the pack seam, never inside it:

- Lightmap (id 22): `bc6h::encode_bc6h_rgb_from_f32_rgba` via
  `lightmap_bake::encode_atlas_layer`, per layer, inside
  `pipeline/lightmap_stage.rs` `bake_fused_prepared`. The memo caches the compressed
  section.
- Direct SH (id 35): `direct_sh_bake::encode_direct_section_bc6h` at emit in
  `pipeline.rs` `run_after_parsing`; the raw section is what is cached.

`pack/section_plan.rs` `build_finalized_section_plan` (new since the merge) receives
finished sections and pairs an arithmetic `byte_len` with a one-shot writer.
`build_pipeline.md`'s planned-section footprint rule forbids reporting that re-encodes
a section or holds more than one payload at a time — so id 42's BC5 bytes must already
exist when planned.

Shadowmask production: `shadowmask_bake::prepare_fused_shadowmask` →
`FusedShadowmaskPlan::consume_partition` → `FusedShadowmaskPlan::finish` →
`ShadowmaskFill::finish` (`shadowmask_bake/fill.rs`), which moves the raw buffer into
the section. `empty_section_for_dimensions` builds sections too.

Memo: `CacheKey::new(SHADOWMASK_ATLAS_STAGE_ID, SHADOWMASK_ATLAS_STAGE_VERSION = 2,
shadowmask_atlas_input_hash(..))`. The write, `cache_shadowmask_section_then_complete`,
**streams its own header layout and bypasses `to_bytes`**; the read goes through
`from_bytes`. `validate_cached_shadowmask_section` checks dims and channel count only.
Encoding in `finish` puts compressed blocks in the memo, so warm and cold paths share
one representation. The streamed writer is the drift risk.

Rejected alternative: encode at emit like id 35. It would either re-encode on every
warm build or cache raw, and the footprint rule forbids an encode during reporting.

## Lightmap layer budget

Two BC5 groups need `2 × layer_count ≤ 256` (`REQUIRED_MAX_TEXTURE_ARRAY_LAYERS`,
`renderer_init_resources.rs`). The compiler allows `MAX_ATLAS_LAYERS = 256`
(`lightmap_bake.rs`), so as first drafted a 129–256-layer level would degrade to fully
lit — a silent regression on content that works today.

The band is reachable. `pack_layers` sizes one shared square layer dimension via
`choose_layer_dim` to host the largest single BVH leaf (doubling from
`MIN_ATLAS_DIMENSION`, capped at `MAX_ATLAS_DIMENSION = 8192`), then packs leaves in
order and rolls a leaf that does not fit, whole, to a fresh layer. A level of many small
leaves gets a small dimension and many layers; `shadowmask-atlas-mask-capacity`'s
research estimates 65 layers at 128² as roughly 1,700 m² of lit surface.
`_lightmap_density` does not steer it. Overflow past 256 returns
`LightmapBakeError::LayerOverflow` today.

Fix: when a pack would exceed the budget, double the dimension and repack. Quartering
layers per doubling, 128 layers at 8192² is far past any real level, so the cap is
unreachable in practice. Levels within budget are untouched, byte for byte.

Alternatives weighed:
- Compile-time warning with runtime fallback — leaves the regression in place.
- Fail the bake — breaks maps that compile today.
- Build the capacity brief first — it grows groups inside the same budget, so it narrows
  the band rather than closing it, and it is gated on this brief's overlap report.

Seams: `prepare_atlas` calls `pack_layers(&charts, MAX_ATLAS_DIMENSION, texel_density)`
on both its static-lights and no-static-lights branches; tests pass a small `max_dim`,
so the budget wants to be a parameter the same way for fixtures. Downstream: the
`"lightmap_section"` memo keys on the prepared atlas layout (`build_pipeline.md`
§Build Cache), so a repacked level misses. Confirm the per-light layer, shadowmask and
animated weight-map keys fold the layout too, or bump their versions. Durable
revision at promotion: `build_pipeline.md` §Compiler pipeline step 10 (spill "up to a
fixed layer cap").

## Versioning

`PlannedSection::new(SectionId::ShadowmaskAtlas, 1, ..)` — id 42's container-entry
version is 1. The loader checks container-entry versions only for `ClusterDirectory`
and `ClusterShPayloads`. Sections that evolved carry the version in the payload and
check it in `from_bytes`: `DIRECT_SH_VOLUME_VERSION`, `LightmapSection::irradiance_format`
(`IRRADIANCE_FORMAT_BC6H`), `LightmapSection::direction_format`
(`DIRECTION_FORMAT_OCT_RG8`). The renderer branches upload format on those tags
(`direction_texture_format`, `upload_irradiance_texture`).

Loader (`prl_loader.rs` `load_prl_from_container`): `from_bytes` error →
`[PRL] ShadowmaskAtlas malformed; ignoring section` warning → `None` → fully lit. A
second gate drops the section when `entity_shadow_lights` is empty or disagrees with
the channel count. So a named reject needs only `from_bytes` to name the format mismatch.

`cluster-residency` asked the epic not to change existing section versions; that
constraint was scoped to the epic and id 42 is not a cluster section.

## Why BC5, and why the group count is fixed at two

Masks are independent per-light scalars, never a correlated colour tuple — BC4's home
case. `crates/level-compiler/src/bc5.rs` `encode_bc5_rg(rgba, w, h)` emits per 4×4
block a BC4 R block then a BC4 G block from RGBA8 input, ignoring B and A. Serial,
row-major, per-block min/max endpoints, fixed integer palette matching the hardware
ladder, no cluster fit, no parallelism. Block alignment is only `debug_assert`ed.
Its only caller today is `texture_mips` (normal-map mips).

Two BC5 groups carry `(g0.r, g0.g, g1.r, g1.g)` — exactly four masks, at 2 bytes per
lightmap texel against 4.

Fixing the group count at two keeps the brief small:

- **The slot table needs no change.** A grown group count would push slots past `0..3`
  and collide with the `0xFF` sentinel.
- **The samples stay hoisted.** Both layers (`lightmap_layer`,
  `lightmap_layer + layer_count`) follow from `lightmap_layer` alone.
- **No capacity floor**, so no raw fallback and no dependence on overlap frequency.

## Why BC5 only, not raw behind the tag

Keeping raw loadable would buy a live A/B toggle. Every baseline it serves is available
without it: bytes from a pre-change bake, fidelity from the compile-time encode against
the in-memory raw masks, and the visual A/B from a pre-change capture. The cost would
be a uniform-gated branch in the shader helper, a second upload format, and doubled
decode coverage — a permanent second path for a one-time comparison. The capacity
brief may add group-addressed raw later; tags are additive.

## Why not BC7

BC7 would keep RGBA packing and reach ≈4:1, but models per-block cross-channel
correlation four independent masks lack, so its ratio is bought with error concentrated
on this data. No BC7-unorm encoder exists in-tree; `bc7-color-textures` owns that codec.

## Renderer and shader

- `upload_shadowmask_texture` (`lighting/lightmap.rs`): `Rgba8Unorm`,
  `depth_or_array_layers = layer_count`, one mip, `TextureDataOrder::LayerMajor`,
  D2Array view (`Lightmap::new`). Group 4, `BIND_SHADOWMASK_ATLAS`, filterable float.
- `filter_usable_shadowmask_section`: non-zero dims, within
  `max_texture_dimension_2d`, `layer_count <= max_texture_array_layers`. The limit is
  pinned by `REQUIRED_MAX_TEXTURE_ARRAY_LAYERS = 256` (`renderer_init_resources.rs`).
  No BC alignment check.
- Placeholder: `upload_placeholder_shadowmask`, 1×1×1 `Rgba8Unorm` white.
- `TEXTURE_COMPRESSION_BC` is in `request_renderer_device`'s required features; the
  renderer bails without it, so a no-BC-adapter row is unreachable. Nothing checks BC5
  filterability separately (BC6H has `bc6h_irradiance_filterable`); BC5 unorm is
  filterable wherever BC is supported.
- `forward.wgsl`: every read goes through `sample_shadowmask_atlas(uv, layer)` —
  `textureSample` with `lightmap_filtering_sampler`, clamping the layer to
  `textureNumLayers - 1` for the placeholder. Two call sites, each hoisted:
  `shadowmask_union_subtraction` (promoted-union path, reads the channel from metadata)
  and `specular_shadowmask` in `fs_main` (world specular, reads `SpecLight.cone_cos.z`
  via `shadowmask_visibility_for_spec_light`). Per-light select is
  `shadowmask_channel_value(vec4, channel)`. Both sites sit under their own gating
  condition, so today a fragment issues up to two samples; after, up to four, or two if
  both paths share one pair.
- Channel values reach the shader as floats: `metadata_channel_value`
  (`render/shadowmask.rs`) and `pack_spec_lights` (`crates/lighting/src/spec_buffer.rs`)
  map `0..3` to `0.0..3.0` and the sentinel to `4.0` (`SPEC_LIGHT_SHADOWMASK_NONE`).
  Unchanged by this brief.
- Guards: `forward_pipeline_sampled_texture_request_matches_bgl_definitions` pins
  per-group sampled-texture counts and is untouched (same binding count).
  `forward_shader_shadowmask_fallback_clamps_multilayer_indices` (`shader_tests.rs`)
  pins the helper's call count, its single `textureSample`, and the clamp text — it
  changes with the helper.

## Runtime lifecycle — the double residency

Chosen shape: the upload owns the payload. Install moves it out of the world and the
upload drops it; the world keeps slot table and dims. A clear-in-place alternative would
leave a header whose length arithmetic its empty data contradicts, reachable through
`LevelWorld::lighting()` (`prl_lighting.rs`) — guardable with a filter length check,
but ownership makes the state unrepresentable instead. `Vec::clear` alone also keeps
the allocation.


- **Load.** `load_prl_from_container` → `LoadedLighting.shadowmask_atlas` →
  `LevelWorld.shadowmask_atlas` (`pub`, `level-loader/src/prl.rs`); accessor
  `LevelWorld::shadowmask_atlas()`.
- **Readers.** All through `level_world_to_geometry` → `LevelGeometry.shadowmask_atlas`
  inside one `Renderer::install_level_geometry`: `build_spec_light_shadowmask_channels`
  (keys on `Some`, reads `channels`), the `shadowmask_channels` clone, and
  `LightmapResources::new` → filter → upload (the only `data` reader). No reader in
  dev-tools, diagnostics or capture outside `LevelGeometry`.
- **Stash.** `install_level_payload` (`startup/lifecycle.rs`) installs geometry, then
  `self.level = Some(world)` into `App.level`. The world is owned outright, not behind
  an `Arc`, and the geometry borrow ends before the stash.
- **Reload.** Every load re-reads from disk: `drain_level_requests` → `unload_level` →
  `begin_level_load` → worker `load_prl` → `install_level_payload`. The dev level cycle
  uses the same path. Capture loads its own world and drops it. Nothing rebuilds
  geometry from a retained world.
- **Precedent.** The streaming epic set a "never materialize" pattern for SH
  (`ShStorage::Streaming`), not free-after-upload. This brief sets the first release.

## Compile-time lifetime (`development_guide.md` §1.4)

| Phase | Coexisting representations | Bound |
|---|---|---|
| Fill | raw RGBA buffer, `W × H × L × 4` | as today |
| Finish (encode) | raw buffer + BC5 output (`W × H × L × 2`) + one layer's RG scratch | ≈1.5× raw + one layer |
| Memo write | BC5 section only, streamed | 0.5× raw |
| Pack | BC5 section, one payload at a time | 0.5× raw |

Runtime: file bytes → `from_bytes` copy (as today) → GPU upload → CPU payload cleared.

## Proof support

- **Byte report.** `pack_output::report_section_footprint` logs
  `[Compiler] PRL section footprint: id N (Name), B payload bytes` at info — visible
  with `prl-build -v` or `RUST_LOG=info`.
- **Frame time without GPU timestamps.** The capture measurement harness: a
  `measurement` block (`report`, `warmup_frames`, `sample_frames`) in a scene JSON,
  run with `cargo run -p xtask -- capture <scene.json>`, writes
  `postretro.capture.measurement.v1` with `cpu_completion` median and p95
  (`crates/postretro/src/capture/report.rs`; `rendering_pipeline.md` §Capture
  measurement; example `measurements/sh-probe-streaming/premise/fine-1.0m.scene.json`).
  `measurements/sh-probe-streaming/premise.md` records one unattended run on this Mac
  that could not initialize an adapter — plan an attended run. Its resident-byte
  ledger covers SH only.
- **Fixtures.** Focused, with selected specular lights; the ratio is codec-intrinsic
  and the error texel-local. No stress map.

## Prior commitments preserved

- `rendering_pipeline.md` §4 World specular shadowmask: absent, rejected or dropped
  data is fully lit; the world-only signal stays independent of pool-shadow promotion
  and its crossfade. Extended to an over-budget or misaligned BC5 section.
- `rendering_pipeline.md` §4 Promoted static lights: static→static world shadowing is
  exactly zero via the union-subtraction dead-zone. Group addressing changes mask
  location and encoding, not the union term.
- `build_pipeline.md` §Build Cache lists BC6H irradiance as the only lossy exemption
  from byte-identity. Not extended to id 42.
- `shadowmask-bake-scaling`'s byte-identity AC is knowingly broken — the encoding
  changes. Fixtures re-bake.
- The `build_pipeline.md` id-42 row and §PRL ShadowmaskAtlas paragraph, and the
  `rendering_pipeline.md` §4 world-specular statement, are revised at promotion.

## Why this is its own brief

`lighting-scale--sh-base-atlas-at-rest-slimming` argued it for ids 34/35 and the
argument applies here: a certain at-rest win must not be bundled with a gated,
contract-rewriting sibling. `/build-brief` consumes one brief per run, and a mid-run
no-go on the gated half would leave a mixed diff. Pre-release, a second id-42 format
change is nearly free (`development_guide.md` §1.6).

## Prior-art and collision map

- **No plan owns id-42 size, RAM or VRAM.**
- `lighting-scale--shadowmask-cold-working-set` (done) — restructured assignment;
  compile-time RAM only; named "bounding the output below its on-disk size" a format
  question. This brief does not touch assignment.
- `lighting-scale--sh-base-atlas-at-rest-slimming` (done) — BC-at-rest precedent and
  the split argument.
- `static-light-shadowmask-cache-addendum` (done) — the byte-for-byte cache guarantee.
- `shadowmask-array-atlas` (done) — id 42 is already a `texture_2d_array`.
- `sh-probe-streaming` (merged) — never touches id 42; excludes lightmap residency; its
  forward texture inventory AC stays satisfied. Its cluster directory admits only
  ids 27/34/35/41/45/47/48.
- `large-map-spatial-residency` — lightmap streaming is a seed only ("keep lightmaps
  whole initially"). BC5 blocks are no harder to stream per layer than id 22's BC6H.
- `bc7-color-textures` (draft) — owns BC7. Not a dependency.
- `shadowmask-atlas-mask-capacity` (draft, gated) — gated on this brief's overlap report.

## Orderings and edges

| id | scenario | mechanism | expected outcome |
|---|---|---|---|
| format-edges | empty selection, single light, full four-slot table | tag in header; `from_bytes` recomputes length from the tag | byte-exact round-trip |
| stale-payload | pre-change raw id-42 payload | `from_bytes` rejects on tag; loader's malformed path | warning names the mismatch; fully lit; no panic |
| misaligned | BC5 dims not multiples of 4 | `from_bytes` rejects before any upload | fully lit; `encode_bc5_rg`'s debug_assert never the only guard |
| budget-boundary | `2 × layer_count` at the pinned maximum | filter evaluates the product before texture creation | equal kept; one greater → placeholder |
| second-group-light | slot 2 or 3 | helper samples both layers hoisted, returns the four-lane vec4 | the light's shadow renders; first-group output unchanged |
| placeholder-second-group | slot 2 or 3 against the 1-layer placeholder | group offset `textureNumLayers / 2 = 0`, layer clamped | fully lit, in range |
| warm-equals-cold | memo hit vs miss | streamed writer and `to_bytes` share the header layout | identical bytes |
| release-then-reload | reload after the payload was cleared | fresh disk read; nothing re-uploads from `App.level` | correct atlas |
| stale-payload-tag-collision | pre-change payload whose bytes at the tag position equal a valid tag (slot bytes are `0..3`/`0xFF`, padding and high header bytes zero) | tag value chosen so no pre-change byte pattern matches it | rejected by format, not by length; fully lit; no panic |
| stale-memo | warm cache holding a pre-change raw shadowmask entry | stage-version bump misses the key; a same-key raw entry fails `from_bytes` and re-bakes (`corrupt shadowmask atlas, re-baking`) | never served; rebuilt section equals uncached |
| all-sentinel | every selected slot is the sentinel (all selected lights invalid) | section still emitted — `empty_section_for_dimensions` builds it — tagged BC5 over `2 × layer_count` layers | half raw bytes; loads; fully lit |
| bake-misaligned | bake handed a non-4-aligned atlas (every `SharedAtlas` test fixture in `shadowmask_bake.rs` is 5×5) | alignment checked before encode in every profile; `encode_bc5_rg` only `debug_assert`s and drops remainder blocks in release | error naming the dims; never a truncated payload; existing fixtures move to 4-aligned dims |
| no-upload-install | install with no renderer | world stashed before any upload | payload retained; release never precedes upload |
| budget-permit | level packs within 128 layers | packer takes today's path; no repack | prepared atlas byte-identical to the pre-change packer |
| budget-boundary | pack needs exactly 128 vs 129 layers | overflow check at the budget, then double dimension and repack from scratch | 128 keeps dimension; 129 grows it and lands ≤ 128; leaf cohesion kept |
| layout-selection-independent | same geometry, with and without selected shadowmask lights | budget is unconditional | identical layout |
| repacked-end-to-end | level repacked to a larger dimension | shadowmask shares the repacked dims; `2 × layer_count ≤ 256` | loads and resolves masks; never the placeholder |
| overlap-memo-hit | warm rebuild with no edits | memo carries peak overlap beside the section; hit reports it without building the graph | same value as the cold bake; shipped section unchanged |
