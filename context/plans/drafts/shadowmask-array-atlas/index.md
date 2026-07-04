# Shadowmask Array Atlas — Investigation: No Action Needed

> **Status:** investigation complete. **Verdict: no overflow gap exists — no spec, no code change.**
> **Question asked:** does the newly-merged `ShadowmaskAtlas` (PRL id 42) overflow device texture
> limits on large maps the way the SH octahedral atlas does (`plans/drafts/sh-array-atlas/`), and does
> it need the same 2D→`texture_2d_array` treatment?
> **Answer:** no. The shadowmask atlas was **born layer-shaped** and rides the lightmap's
> already-array-atlased `SharedAtlas` verbatim. It cannot overflow any limit the lightmap doesn't
> overflow first, and every overflow axis is already handled — graceful at load, hard-fail at bake.
> **Related:** `context/lib/rendering_pipeline.md` §4 (Promoted static lights, lightmap array atlas) ·
> `context/lib/build_pipeline.md` §PRL section IDs (id 22 Lightmap, id 42 ShadowmaskAtlas) ·
> `context/plans/drafts/sh-array-atlas/index.md` (the sibling — a **real** gap, contrasted below) ·
> `context/plans/done/lightmap-array-atlas/` (the array-atlas machinery the shadowmask inherits) ·
> `context/plans/in-progress/static-light-shadowmask-world-receipt/index.md` (the plan that shipped id 42).

## Why this was investigated

The `sh-array-atlas` draft found a real overflow: the SH octahedral atlas is a single 2D texture whose
side grows with a **volumetric** probe count and blows past `max_texture_dimension_2d = 8192` on
warren-scale maps. By analogy, the concern was that `ShadowmaskAtlas` (id 42, merged with
`direct_sh_compose` / `promoted_depth_cache.rs` / `shadow_ranking.rs`) is likewise a single oversized
2D atlas needing the array-atlas fix.

The concern does not hold. The two atlases are structurally different. The evidence follows.

## What the GPU path actually does (verified)

The shadowmask atlas is created, viewed, bound, and sampled as a **`texture_2d_array` that honors
`layer_count`** — it does **not** flatten layers into an oversized `height × layer_count` 2D texture.

| Stage | Anchor | What it does |
|---|---|---|
| Texture create + upload | `crates/renderer/src/lighting/lightmap.rs:495-519` (`upload_shadowmask_texture`) | `Extent3d { width: sec.width, height: sec.height, depth_or_array_layers: sec.layer_count }`, `dimension: D2`, `TextureDataOrder::LayerMajor`. A real array texture; `layer_count` is honored. |
| View | `lightmap.rs:169-172` | `TextureViewDimension::D2Array`. |
| BGL entry | `lightmap.rs:292-301` (`BIND_SHADOWMASK_ATLAS = 6`, group 4) | `view_dimension: D2Array`. |
| Shader declaration | `crates/renderer/src/shaders/forward.wgsl:229` | `var shadowmask_atlas: texture_2d_array<f32>`. |
| Shader sample | `forward.wgsl:777` | `textureSample(shadowmask_atlas, lightmap_filtering_sampler, lightmap_uv, i32(lightmap_layer))` — array index is the **per-vertex `lightmap_layer`** (in `forward.wgsl:288/302/341`). |

This is the exact array-atlas shape `sh-array-atlas` proposes to *add* to SH. The shadowmask already
has it.

**The per-receiver layer index the SH spec had to work for is already free here.** `sh-array-atlas`'s
key difficulty is that SH is sampled by world position, so the layer must be *derived in-shader* from
the probe's linear index. The shadowmask has no such problem: it is sampled in **lightmap UV space**
with the same per-vertex `lightmap_layer` channel the irradiance and direction atlases already use
(`forward.wgsl:895` samples `lightmap_direction` with `i32(in.lightmap_layer)` identically). No
world-position layer derivation, no new vertex channel — the receiver carries its own layer.

## Where the atlas is sized (verified) — it rides the lightmap's SharedAtlas

The shadowmask bake does **not** size itself. It reads the dimensions straight off the lightmap's
`SharedAtlas`:

- `crates/level-compiler/src/shadowmask_bake.rs:64-67` (`bake_shadowmask_atlas`) passes
  `shared.atlas_width`, `shared.atlas_height`, and `layer_count_from_shared(shared)` into the section.
- `layer_count_from_shared` (`shadowmask_bake.rs:305-312`) = `max(placement.layer + 1)` over the
  **lightmap's own chart placements** — byte-for-byte the same spilled layer count the lightmap layer
  bake computes (`crates/level-compiler/src/lightmap_layer.rs:219-224`, identical expression).
- The `SharedAtlas` (`lightmap_layer.rs:179-183`) is the packing produced by the lightmap's
  multi-bin `pack_layers`, which caps **each layer** at `MAX_ATLAS_DIMENSION = 8192`
  (`crates/level-compiler/src/lightmap_bake.rs:34`) and spills overflow onto new layers up to
  `MAX_ATLAS_LAYERS = 256` (`lightmap_bake.rs:40`), erroring `LayerOverflow` past that
  (`lightmap_bake.rs:63,798`).

So `ShadowmaskAtlasSection.width = irr_width ≤ 8192`, `height = irr_height ≤ 8192`,
`layer_count = lightmap layer_count ≤ 256` — **by construction**. The CPU format
(`crates/level-format/src/shadowmask_atlas.rs`) has carried `width`/`height`/`layer_count` + a
layer-major payload since it shipped; the runtime honors all three.

## The overflow arithmetic

The question "on stress-warren, would any axis exceed the limit?" has a structural answer that needs no
per-map measurement: **the shadowmask's three size axes are identically the lightmap's three size
axes.** There is no independent growth driver.

- **Per-layer width/height.** Sourced from `shared.atlas_width/height`, each `≤ MAX_ATLAS_DIMENSION =
  8192` by the lightmap packer's per-layer cap. Cannot exceed `max_texture_dimension_2d = 8192`.
- **Layer count.** Sourced from the lightmap's spilled placement count, `≤ MAX_ATLAS_LAYERS = 256` or
  the **lightmap bake already hard-failed** with `LayerOverflow` before the shadowmask bake runs
  (shadowmask bakes *after* the lightmap, on the same `SharedAtlas`). Cannot exceed
  `max_texture_array_layers = 256`.
- **RGBA channel packing is a compaction, not a growth axis.** Up to 4 selected lights share one
  texel's RGBA channels (`shadowmask_atlas.rs:11-18`, `channels` table); >4-way overlap **drops** masks
  (`0xFF`), never adds texels or layers. So selected-light count does not drive atlas size at all — it
  drives channel assignment within the fixed lightmap-shaped atlas.

Contrast the SH arithmetic that **does** overflow (`sh-array-atlas` §Problem): SH atlas side =
`ceil(sqrt(total_probe_count)) × tile_dimension`, with `total_probe_count = grid_x·grid_y·grid_z`
volumetric — on warren-scale that side exceeds 8192, and (before that spec) the SH GPU path was a
single `texture_2d`. The shadowmask has **neither** property: no volumetric size driver (it is
surface-area-bound like the lightmap, already tiled into ≤8192² layers) and no single-2D GPU path (it
is already `D2Array`).

## Device limits and graceful behavior (verified)

Both relevant limits are in `required_limits` **and** adapter-pre-checked with named `[Renderer]` bail
errors — no wgpu-default panic risk (`crates/renderer/src/render/renderer_init_resources.rs`):

| Limit | Value | Requested | Pre-checked |
|---|---|---|---|
| `max_texture_dimension_2d` | 8192 (`REQUIRED_MAX_TEXTURE_DIMENSION_2D`, line 130) | yes (line 145) | yes (lines 233-242) |
| `max_texture_array_layers` | 256 (`REQUIRED_MAX_TEXTURE_ARRAY_LAYERS`, line 14) | yes (line 146) | yes (lines 249-258) |
| `max_texture_dimension_3d` | not used by the shadowmask (it is 2D-array, not 3D — unlike SdfAtlas/SH depth moments) | — | — |

**Load does not panic if the atlas somehow exceeds limits.** `filter_usable_shadowmask_section`
(`lightmap.rs:360-393`) checks per-layer `width/height ≤ max_texture_dimension_2d` **and**
`layer_count ≤ max_texture_array_layers`; on failure it logs a `[Renderer]` error and returns `None`,
so the caller uploads a 1×1×1 fully-visible placeholder (`upload_placeholder_shadowmask`,
`lightmap.rs:574-595`) and sets `shadowmask_present = false`. The CPU-side promoted metadata also gates
the union term off when `shadowmask_present == false` (`renderer_resources.rs:198,281,423`;
`renderer_light_slots.rs:277-284`), so the placeholder is a hard disable, never a wrong-looking
fallback. This mirrors the lightmap's own guard (`filter_usable_section`, `lightmap.rs:324-358`) and the
SH volume's atlas-fits-device filter. Because the bake caps guarantee conformant output, this guard is
belt-and-suspenders — on a spec adapter it can only fire on a corrupt section.

## Relationship to the merged siblings (no collision to worry about)

The merge that added id 42 also added `direct_sh_compose`, `promoted_depth_cache.rs`, and
`shadow_ranking.rs`. Since the conclusion is **no change**, there is nothing to collide. For the record:
none of those touch atlas *sizing* — `direct_sh_compose` composes the `DirectShVolume` (id 35) octahedral
atlas (the SH family, `sh-array-atlas`'s concern), `promoted_depth_cache.rs` caches promoted-slot world
depth, and `shadow_ranking.rs` ranks pool lights. The shadowmask's size is owned solely by the lightmap
`SharedAtlas`.

## If the lightmap ever gets a genuinely bigger atlas

The one way the shadowmask's dimensions could grow is if the **lightmap** itself grows — e.g. a future
finer `_lightmap_density`, or a lightmap array-consolidation refactor. In every such case the shadowmask
inherits the change for free, because it reads `shared.atlas_width/height/layer_count` at bake time and
the runtime honors whatever `width/height/layer_count` the section carries. There is no shadowmask-side
constant to bump, no id-42 format field to add, no PRL version to advance. The correct maintenance
posture is: **keep the shadowmask sized off the lightmap `SharedAtlas`** (it already is) so the two stay
locked. No standing work item.

## Conclusion

No overflow gap. No spec. `ShadowmaskAtlasSection` needs no version bump, no new fields, and the section
id does not change. The device-limit checks the `sh-array-atlas` spec proposes to add for SH already
exist for the shadowmask (`filter_usable_shadowmask_section`). The graceful-refusal behavior already
exists. The `texture_2d_array` GPU path the `sh-array-atlas` spec proposes to build for SH already
exists for the shadowmask. The shadowmask atlas got the array treatment at birth by sharing the
lightmap's shipped array atlas — which is exactly why the merged CPU format already carried
`width`/`height`/`layer_count` and a layer-major payload.

Recommendation: close this thread. Do not promote. If a reviewer wants a guard against future drift,
the highest-value (still optional) addition is a compiler test asserting `ShadowmaskAtlas.width/height/
layer_count` equal the emitted `Lightmap` irradiance atlas's — pinning the invariant this whole
investigation rests on. That is a test, not a feature, and only if the existing `layer_count > 1`
round-trip fixtures (`static-light-shadowmask-world-receipt` T2) are judged insufficient.
