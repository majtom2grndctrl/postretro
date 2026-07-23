# Emissive Surfaces + Bloom

## Goal

Texture-driven self-lit surfaces: a world material with an `_e.png` sibling adds
an **additive HDR** emissive contribution on top of its lit color, and bright
emissive texels bloom. This is the correct model an earlier `neon_`-based
lighting-*replacement* stub (cut 2026-05) got wrong — emissive **adds to** scene
lighting, it never replaces it. Realizes the reserved `.prm` bit-3 slot and the
cyberpunk-neon look; consumed later by in-game button/trigger activation feedback
(see Non-goals → the runtime-animated follow-up).

## Scope

The pipeline is **LDR/sRGB end-to-end today** — `scene_color` is the surface
format, the forward pass writes into it directly, and there is **no tonemap**
(research.md). So "additive HDR emissive + bloom" is three layers: an HDR
compositor foundation, the emissive material slot, and the bloom pass.

### In scope

- **HDR scene target + tonemap.** `scene_color` and every scene-pass color target
  → `Rgba16Float`; a tonemap operator maps HDR → sRGB at the resolve; the E20
  capture readback is reconciled to the new format.
- **Emissive material slot.** `{name}_e.png` sibling (sRGB color), baked into the
  `.prm` as a 4th slot (bit 3), uploaded into `LoadedTexture`, sampled in the world
  + kinematic-brush shaders, added additively (scaled by an emissive strength) to
  the HDR lit output.
- **Bloom pass.** Renderer-owned bright-pass → blur → additive composite into the
  HDR `scene_color`, between fog composite and the resolve.
- **Tooling + docs.** `tools/gen_emissive.py`; flip `resource_management.md §4.5`
  Reserved → implemented; update `rendering_pipeline.md §7.8`.

### Out of scope

- **Runtime-animated / trigger-driven emissive** (a button surface glowing on
  activation). No per-surface runtime emissive scalar seam exists — world surfaces
  are static material data, not entities, so the animated-light pattern does not
  transfer (research.md). The button-glow consumer lands in a **named follow-up**
  that adds per-surface (or per-mesh-entity) emissive-intensity state + its GPU
  feed. v1 emissive is static per-texel.
- **Emissive as a light source.** An emissive surface does not illuminate its
  neighbors — no GI, no light injection. To cast light, an author places a separate
  light entity (an authoring choice). This is the no-double-count boundary.
- **Model / mesh emissive.** Models consume the diffuse slot only; the skinned
  shader binds a group-1 subset. Character/prop emissive is a later follow-up.
- **CRT/scanline and other post effects.** Bloom + tonemap only; the wider
  post-processing set stays in Future/Speculative.
- **PBR / metallic-roughness.** Emissive is an additive term, not a PBR channel.

## Acceptance criteria

- [ ] A world surface with an `_e.png` sibling renders visibly self-lit — brighter
  than its lit-only appearance and readable in an unlit area — with **no change to
  neighboring surfaces' shaded (lit) contribution**. This is the no-light-injection
  bar (emissive adds nothing to the lightmap/SH), tested **with bloom disabled**; the
  post-bloom screen halo of AC 3 is a separate screen-space effect and is expressly
  not a violation of this criterion.
- [ ] A material with **no** `_e.png` renders identically to today's material path
  (absent emissive = 1×1 black placeholder = additive-of-zero no-op).
- [ ] Emissive texels authored bright enough exceed 1.0 in the HDR scene target and
  bloom into a soft halo around the surface.
- [ ] With no emissive present and no bloom-bright content, the final image matches
  today's within a stated visual tolerance (tonemap of in-range content ≈ current),
  and an E20 frame capture still produces a valid PNG at the same resolution.
- [ ] `.prm` files carrying an emissive slot parse and render; existing 3-slot
  `.prm` files still parse unchanged; a content rebuild regenerates the cache with
  emissive bundles.
- [ ] An emissive sibling is validated as **sRGB color** (not rejected the way the
  linear-required `_s`/`_n` siblings are).
- [ ] `tools/gen_emissive.py` produces `_e.png` siblings that bake and render.
- [ ] `resource_management.md §4.5` reads as implemented and `rendering_pipeline.md
  §7.8` reflects the HDR `scene_color` + tonemap + bloom compositor.

## Tasks

### Task 1: HDR scene target + tonemap (foundation)
Convert `scene_color` and every scene-pass color-target format from the surface
sRGB format to `Rgba16Float` (the forward "Textured" pipeline, kinematic brush,
skinned mesh, smoke/billboard, fog composite, wireframe, debug lines, UI — each
pipeline's color-target `format` must match, plus the `scene_color` texture
itself). Insert a **tonemap** operator into the resolve: the resolve now reads the
HDR `scene_color`, applies a **near-neutral / soft-knee** tonemap (passes in-range
[0,1] values through near-untouched, compresses only the >1.0 overshoot — retro-punch
preserved, not a filmic ACES recolor; see Decisions), **then** the existing
flash/vignette/shake, writing the sRGB swapchain. **Color-encoding pin:** an
`Rgba16Float` `scene_color` stores **raw linear** (no hardware sRGB encode on store),
so (a) the scene passes write linear as they do today but the store no longer encodes;
(b) the resolve samples `scene_color` through a **linear (non-sRGB) view** and writes
the **sRGB-format swapchain**, where the hardware performs the single sRGB encode on
store; (c) the UI pass (now storing into linear float) is in-range content, so the
near-neutral tonemap leaves it within tolerance — no per-pass pre-encode is added.
(If a future filmic tonemap is ever adopted, UI must move **after** the resolve; not
now.) Reconcile E20 capture — the readback assumes 4 bytes/px from an Rgba8
`scene_color`; keep the deterministic Rgba8 PNG path by having capture read a
**tonemapped LDR** copy: add a **capture-path-only** tonemap-to-LDR step targeting an
`Rgba8UnormSrgb` texture (capture already returns before the resolve; this step is
gated to the capture path, not run every frame), which the existing `read_texture_rgba8`
consumes unchanged. Task 1 makes only the minimal note that the byte-identity resolve
contract is superseded; the full `rendering_pipeline.md §7.8` rewrite is owned by Task 4
(single owner, avoids a concurrent double-edit).
Because tonemap is deliberately not identity, the parity bar becomes **visual
tolerance**, not byte-identity: in-range (≤1.0) content must render within tolerance
of today, verified by tolerance-scoped capture goldens. Blocks Tasks 2 and 3.
**Note — this extends an already-HDR lighting path.** The engine already *composes*
lighting in `Rgba16Float` (the lightmap atlases, the SH irradiance/direct atlases,
and the in-flight animated-direct composed atlas are all 16-bit float); the forward
pass is the one place that range collapses to LDR sRGB. Task 1 carries the existing
HDR range through to the scene target instead of clamping at forward output — so its
beneficiaries are broader than emissive (bright dynamic and animated alarm lights
stop hard-clipping too), and it aligns with, rather than bolts onto, the lighting
architecture.

### Task 2: Emissive material slot
Widen the material path from 3 slots to 4, end to end. `.prm`: add
`PrmSlots::EMISSIVE` (bit 3), widen `PrmFile.slots` and `from_bytes_partial` to 4,
widen the wire-order iteration arrays, set the emissive slot format to
`Rgba8UnormSrgb`, and **narrow the reserved-bit guard to permit bit 3 while still
rejecting bits 4-7** (invert only the bit-3 case of `reserved_slot_bits_are_rejected`,
not the whole `from_bits` guard); existing bit-3-unset files must still parse. Baker (`level-compiler`): discover `{base}_e` siblings, bake
them as sRGB color (decode→filter linear→re-encode, like diffuse), set the emissive
mask bit, and extend the bundle-hash / filename-key / cache-validation helpers.
Color-space validation: add an emissive arm that **accepts sRGB** (the opposite of
the linear `_s`/`_n` requirement). Renderer: widen `TextureSlotPlan.consume` to 4
(WorldBundle consumes emissive, ModelDiffuseOnly does not); add
`emissive_texture`/`emissive_view` to `LoadedTexture` and its constructors; add a
`make_emissive_placeholder` (1×1 `Rgba8UnormSrgb` black) used on absence; add the
`Emissive` slot upload; add the emissive texture to the group-1 BGL at the **vacated
binding 1** and to `build_material_bind_group` (all 4 call sites) — and update the
sampled-texture budget test. Emissive joins the **shared** group-1 BGL, so the
fragment sampled-texture count rises for **every** pipeline that binds it, not just
forward: verify and adjust the budget assertions for the kinematic-brush pipeline as
well, and confirm whether the skinned-mesh pipeline binds this same widened BGL or a
distinct subset BGL (it samples only bindings 0/5) — if shared, its budget rises too
even though it never reads emissive. Forward reaches **exactly 16/16 fragment
textures** (the closed material vocabulary complete by design; see Decisions). World +
kinematic-brush shaders: declare and sample the emissive texture, decode sRGB→linear,
and add `emissive * strength` additively to the final HDR color after `total_light`.
**Emissive strength is prefix-driven**, following the existing `shininess()`
mechanism — **not** the gameplay-only `MaterialProperties` struct
(`crates/render-data/src/material.rs`, which carries `ricochet` and is never GPU-fed).
Add an `emissive_strength()` method on the `Material` enum beside `shininess()`,
resolved from the texture prefix and packed into the group-1 material uniform
alongside shininess via the same `material_plan.rs` `build_material_bind_group` path —
the `Neon` variant gets a high strength so `neon_` textures self-light with no new
authoring surface. Widening that uniform ripples to the material-uniform struct
declared in the **forward** and **kinematic-brush** shaders (skinned declares only
bindings 0/5 and is unaffected); pin the new uniform layout when implementing. Emissive is **never** written into any
light buffer (invariant: no light-loop feed). Depends on Task 1. Concurrent with
Task 3.

### Task 3: Bloom pass
A renderer-owned bloom pass: a bright-pass extracting HDR luminance above a
threshold, a separable Gaussian down/up-sample chain, and an additive composite back
into the HDR `scene_color`. **Insertion point (pinned):** bloom must composite into
`scene_color` **immediately after the fog composite (`renderer_render_frame.rs:722`)
and BEFORE the E20 capture return (`:728`)** — not the looser "before resolve" window,
which spans the capture return and the overlay passes. This placement makes the bloom
part of the captured scene (so AC 3's halo and AC 4's capture agree) and leaves the
overlay passes (wireframe, debug lines, the post-merge viewmodel pass at `:774`, UI)
**un-bloomed**. New `render/bloom.rs` + bloom shaders; pass state on the full
renderer. Register a `bloom` entry in the `POSTRETRO_GPU_TIMING` pass list (§12), plus
a `tonemap` entry if that resolve step is bracketed separately. Threshold and
intensity are tunable constants; the reference `Neon` emissive strength (Task 4) is
set so `emissive * strength` clears this threshold (see Invariants). Testable against
any HDR input (a bright dynamic light or a debug constant) — independent of the
emissive authoring path. Depends on Task 1 (needs the HDR `scene_color`). Concurrent
with Task 2 (disjoint files: bloom owns the frame graph + its shaders; emissive owns
the material/format/world-shader path).

### Task 4: Tooling, generalization, verification, docs
`tools/gen_emissive.py` mirroring `gen_specular.py`/`gen_normal.py`, emitting sRGB
`_e.png` siblings (masking bright/neon diffuse regions) that pass the Task 2
validation arm. Populate the `emissive_strength()` values on the `Material` variants Task 2
added (`neon_` high; others as content needs), with the reference `Neon` strength
set so `emissive * strength` clears the Task 3 bloom threshold (and exceeds 1.0) —
otherwise the capture below cannot show bloom. Add a dev-map emissive
surface (`_e.png` on a dev texture, placed) and a tolerance-scoped E20 scripted
capture proving self-lit + bloom (the capture asserts the neon surface both self-lights
and produces the bloom halo). Docs: flip `resource_management.md §4.5` to
implemented, update the §3 material-table emissive row and the §4 sibling
convention with `_e`, and update `rendering_pipeline.md §7.8` for the HDR
`scene_color` + tonemap + bloom compositor. Depends on Tasks 1-3.

### Task 5: Split `texture_mips.rs` (behavior-preserving; precedes Task 2)
`crates/level-compiler/src/texture_mips.rs` is 1553 lines and Task 2 threads an
emissive arm through ~6 of its functions (sibling discovery, the slot-build cascade,
`bundle_hash_for`, `filename_key_for`, `cache_entry_has_valid_declared_slots`).
Split it first along the seams already present — the per-slot Mitchell-Netravali
chain builders (`build_diffuse_chain`/`build_specular_chain`/`build_normal_bc5_chain`),
the name→path collection + sibling resolution, and the content-hash/cache-key helpers
each move to a sibling module under a `texture_mips/` directory (or `_bake`/`_cache`
siblings), with `bake_texture_mips` staying as the orchestrator. Pure code motion:
no behavior change, the same functions with the same signatures, callers in
`pipeline.rs`/`main.rs` unchanged. This is the one oversized file whose emissive edit
genuinely tangles across functions (Decisions → oversized files); doing the split as
its own task keeps Task 2's baker diff readable instead of burying a refactor.
Depends on nothing; concurrent with Task 1.

## Sequencing

**Phase 1 (concurrent):** Task 1 (HDR + tonemap foundation — blocks the additive
emissive term and bloom; the capture path changes with the format), Task 5 (split
`texture_mips.rs` — behavior-preserving, precedes Task 2's baker arm). Disjoint files
(renderer vs. level-compiler).
**Phase 2 (concurrent):** Task 2 (emissive slot), Task 3 (bloom) — both consume Task
1's HDR `scene_color`; disjoint files (material/format/world-shader vs. frame-graph/
bloom-shader). Task 2 builds on the split `texture_mips.rs`. Isolated worktrees.
**Phase 3 (sequential):** Task 4 — consumes Tasks 1-3.

## Rough sketch

- Tonemap + bloom mirror the `FogPass` fullscreen-triangle precedent (`draw(0..3)`,
  no vertex buffer). Bloom threshold/blur is standard down/up-sample; keep it small.
- Emissive slot mirrors the `_s`/`_n` sibling machinery exactly, minus the linear
  color-space requirement (emissive is sRGB). Binding 1 (vacated) is the emissive
  texture slot; `emissive_strength` rides the existing group-1 material uniform.
- `additive HDR`: `final_rgb = base.rgb * total_light + emissive_linear * strength`,
  into the `Rgba16Float` target; tonemap maps it to sRGB at the resolve; bloom reads
  the same HDR target before tonemap.
- File:line inventory and the LDR-pipeline finding: `research.md`.

## Boundary inventory

| Name | Authoring | `.prm` / wire | Notes |
|---|---|---|---|
| emissive sibling | PNG `{name}_e.png` (sRGB) | slot_mask bit 3 (`PrmSlots::EMISSIVE`), 4th slot in wire order, `format_tag` = `Rgba8UnormSrgb` | no scripting/wire/FGD surface in v1; texture-convention only, like `_s`/`_n` |

## Wire format

`.prm` gains a 4th slot, appended after normal in wire order (diffuse, specular,
normal, **emissive**). `slot_mask` bit 3 flags presence; the per-slot 12-byte header
layout is unchanged (`format_tag` = 0 `Rgba8UnormSrgb` for emissive). Existing
3-slot files (bit 3 unset) parse unchanged under the widened reader. The bundle
content-hash already includes the mask byte + per-slot PNG bytes, so a bundle that
gains an `_e.png` produces a **new** content-addressed `.prm` filename — no stale
collision, and the baked cache is regenerable, so no migration path is needed.
Whether to bump `STAGE_VERSION` (2→3) is an implementation choice — the content hash
already disambiguates; a bump only adds an explicit signal.

## Invariants

| Invariant | Established by | Threatened at | Verified by |
|---|---|---|---|
| Additive only, never lighting-replacement | Task 2 (`base*light + emissive`, never a replace) | any future strength/prefix change to the world-shader composite | AC 1, AC 3 |
| Reference `Neon` strength clears the bloom threshold (`emissive*strength` > threshold > 1.0) | Task 4 (strength values) against Task 3 (threshold constant) | either value edited in isolation → non-blooming neon, AC 3 silently unmet | AC 3, Task 4 E20 capture |
| Emissive never feeds the light loop (no double-count) | Task 2 (screen-space additive term on the emitting fragment; never written to any light buffer) | a future attempt to make emissive illuminate neighbors (out of scope) | AC 1 (neighbors unchanged) |
| Absent `_e` → true no-op | Task 2 (1×1 black placeholder + additive-of-zero) | placeholder must be black sRGB; strength must not lift a black sample | AC 2 |
| Existing 3-slot `.prm` files stay valid | Task 2 (reader accepts bit 3 but never requires it; hash regenerates) | the slot-widening must keep bit-3-unset files parsing | AC 5 |
| All HDR/tonemap/bloom stays in `postretro-renderer` | Task 1, Task 3 | Renderer-owns-GPU (`index.md §2`) | structural (`cargo tree`) |

## Coordination with the (now-landed) lighting work

`plans/done/animated-direct-sh-dynamic-receivers` (how kinematic movers, skinned
meshes, and billboards receive a baked light's animated **direct** term) **has merged
into main** (this draft's branch is rebased onto it). It touched adjacent ground; the
verification below confirms its landing left this spec's load-bearing assumptions
intact. Four intersections, none blocking — three reassure the design, one was the
merge-ordering note now discharged:

- **No shader-consumer conflict.** That plan is *producer-side only* — movers already
  sample the composed direct atlas at binding 15 and its consumers (`kinematic_brush.wgsl`,
  `skinned_mesh.wgsl`, `billboard.wgsl`) are explicitly unchanged. Emissive's additive
  term sits **after** `total_light` and is agnostic to how `total_light` is assembled,
  so the two are orthogonal in the shader: emissive neither reads nor perturbs the
  direct-light term the other plan feeds.
- **The HDR reframe is shared upside.** That plan composes its animated-direct atlas
  in `Rgba16Float` (like every other lighting atlas). Task 1's HDR scene target is the
  natural completion of that HDR lighting path — a pulsing alarm light bright enough to
  bloom is exactly the theatrical payoff both plans point at.
- **Emissive lands on movers for free.** Movers bind the world-material bundle
  (`rendering_pipeline.md §7.3`), so the emissive slot reaches them with no mover-path
  work — a `neon_`-textured `kinematic_mover` self-lights. This directly serves the
  button-as-kinematic-mover motivation (co-op-triggers research §4.2). v1 emissive is
  **static** per-texel, so the mover glows constantly; activation-driven glow is the
  named runtime-emissive follow-up.
- **Merge ordering (now discharged).** Both plans edit `pipeline.rs`, `forward.wgsl`,
  and `renderer_render_frame.rs` in different regions. `animated-direct` landed first, so
  this draft's `research.md` file:line grounding is **rebaselined onto the merged
  result** — the merge grew `renderer_render_frame.rs` (919→1001), `pipeline.rs`
  (1340→1392), `forward.wgsl` (1328→1332) and added a new viewmodel pass, but left the
  two load-bearing premises intact: group-1 **binding 1 is still vacated** (the new
  animated-direct atlas bound at group-3 binding 15, not group-1) and the
  **sampled-texture budget is still 15** (so emissive's 16/16 math holds). The
  renderer-side split-deferral now rests on the localized-edits read alone (Decisions →
  oversized files), not on any live-rewrite collision.

Emissive is also **orthogonal to shadowing**: it is self-illumination, not a light —
it neither casts nor receives shadows, and an emissive surface in shadow still glows
(a neon sign is bright in a dark room). Deliberate, and it keeps emissive fully
decoupled from the shadow-receipt work.

## Decisions

Resolved against the project's north stars (cyberpunk-neon as a product-identity
pillar; the lean, closed material vocabulary; the retro-punchy — not filmic — look;
prefix-driven modder-friendly materials; ship a visible, testable slice):

- **One spec, not split.** A HDR/tonemap-only Phase 1 has no visible outcome
  (tonemapping in-range content is near-invisible), and splitting risks shipping the
  plumbing while the neon look — a product pillar — lags. The visible, testable ship
  is "neon surfaces glow," which needs all three layers. Phase 1 stays the clean pause
  point if capacity demands one.
- **Near-neutral tonemap, not filmic.** The retro-punchy palette and "retro filters
  used sparingly" rule out an ACES-style recolor; the base image stays punchy and only
  the emissive/bright overshoot is compressed so it blooms. This also sets a tight
  parity tolerance (in-range content ≈ today).
- **Accept 16/16.** The material vocabulary is closed by design (PBR is a non-goal);
  emissive *completes* it rather than crowding it. Env-map reflections, if ever built,
  bind as an atlas/probe, not a per-material forward slot, so they don't compete for
  this budget.
- **Prefix-driven strength.** Emissive strength lives on the `Material` enum keyed by
  texture prefix (like `shininess()`), giving the property-less `Neon` variant real
  meaning and adding zero authoring surface. A per-material KVP defers until content
  proves prefix granularity insufficient.
- **Oversized files: split one, defer the rest.** Of the six files >800 lines this
  plan touches, only `texture_mips.rs` (1553) has an emissive edit that genuinely
  tangles across functions — split it first (Task 5). The renderer-side files
  (`forward.wgsl` 1332, `renderer_render_frame.rs` 1001, `renderer_types.rs`) take
  **localized** edits (one binding + one additive term; one pass insertion before the
  capture return; a few fields), so per the dev guide's "soft smell, not a gate; a
  cohesive large file is fine" they do not warrant a split-before-extend — the split
  would bury a refactor in a feature diff for no blast-radius win. (The earlier draft
  also deferred these because the in-flight `animated-direct-sh` work was rewriting
  them; that work has since **merged into main**, so the rationale now rests on the
  localized-edits read alone — which is unchanged by the merge. Those files did grow in
  the merge; the file:line anchors are rebaselined in `research.md`.) `prm.rs` and
  `pipeline.rs` take coherent-localized edits (the 3→4 slot widening; two bake call
  sites), not buried refactors.

## Open questions

- **Follow-up — runtime-animated emissive (the theatrical bridge).** The button/trigger
  surface-glow consumer needs per-surface (or per-mesh-entity) emissive-intensity
  runtime state + a GPU feed — net-new, no existing seam (the light-brightness bridge
  is the closest template but does not transfer to static surfaces). This is **not**
  speculative polish: scripted reveals and reactive set-pieces are a stated product
  pillar, and this is the bridge from v1 emissive to E18 co-op set-pieces and the
  activation-feedback thread that motivated this spec. Sequence it with intent as that
  bridge once v1 lands, not as an open-ended someday.
