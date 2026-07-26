# Per-Surface Bloom Opt-Out

## Goal

Let authors declare "this surface emits light but must never bloom" cleanly and
reliably, without hand-tuning `_e` texel byte values. Dramatic neon still blooms;
an interactive button glows crisply with no halo, guaranteed regardless of the
light landing on it. This is the deferred per-material follow-up the just-shipped
`mod-bloom-render-profile` explicitly scoped out.

## Problem framing

Emissive-without-bloom already works today by keeping a surface's emissive
luminance under `BLOOM_THRESHOLD` (`1.0`, `crates/renderer/src/render/bloom.rs`).
`content/dev/maps/combat-demo.map` demonstrates it: `neon_glow_panel` (peak `_e`
term 4.0) blooms; `neon_dim_panel` (peak term 0.985) glows with zero bright-pass
extraction. The bright pass gates on Rec.709 luminance of the linear scene color
per texel (`crates/renderer/src/shaders/bloom_extract.wgsl`).

So this is **not** a missing rendering capability. It is ergonomics + robustness:

- **Ergonomics.** Today the only lever is the authored `_e` byte value. Staying
  no-bloom means tuning the peak byte to sRGB 136 by hand and hoping. There is no
  way to *declare* intent.
- **Robustness.** The bright pass tests the summed fragment
  (`base_color·total_light + emissive·emissive_strength`,
  `crates/renderer/src/shaders/forward.wgsl`). A sub-threshold emissive surface
  still blooms if bright direct/indirect light lands on it. The combat-demo README
  already flags this: a shipping idle wants margin "so indirect light on the
  surface can't nudge it over." Byte-tuning gives no hard guarantee.

## Chosen approach

Add one per-material chokepoint — `Material::participates_in_bloom() -> bool` —
alongside the existing `emissive_strength()` / `shininess()` prefix properties,
and add one new matte-emissive material identity that returns `false`. The forward
and kinematic-brush fragment shaders read a `blooms` bit off the existing material
uniform; when it is clear, they **clamp the final fragment's linear luminance
strictly below `BLOOM_THRESHOLD`** (hue-preserving) before writing scene color.

The surface therefore never produces an HDR value the bright pass extracts, for
any lighting — a hard, light-independent, whole-fragment guarantee — at a few ALU
ops, with **no new GPU pass, no persistent GPU state, no MRT, and no new
wire/FGD/script surface**. It reuses the existing texture-prefix → `Material` →
material-uniform path end to end.

`participates_in_bloom()` is the seam. A future data-driven, per-surface, per-texel,
or FGD-authored version resolves behind that one predicate; a future
supra-threshold-brightness-without-halo need is met by the heavier bloom-mask
mechanism (below) behind the same seam.

### Why not the alternatives

- **(b) Per-surface bloom-exclusion mask.** True per-surface and allows a surface
  to be arbitrarily bright yet halo-free. But scene-color alpha is already
  load-bearing (forward writes `base_color.a`; billboards write
  `sprite_sample.a * in.opacity`, `billboard.wgsl:538`), so it needs a dedicated
  mask attachment (MRT) across every scene-color writer plus resize handling — a
  real new-GPU-state feature. Its only advantage over the clamp is
  supra-threshold brightness with no halo, which the button use case does not
  need. **Deferred**, seamed behind `participates_in_bloom()`.
- **(c) Two emissive channels.** A bloom-eligible vs self-lit `_e` slot doubles
  authoring cost and still sums into the fragment the bright pass sees, so it
  needs the clamp or mask anyway. Rejected.
- **(d) Do nothing; document the sub-threshold pattern + `gen_emissive` helper.**
  Cheapest, already works, but leaves both the ergonomic and robustness gaps.
  The clamp is the same order of cost and gives a real guarantee. The helper stays
  a complementary authoring aid, not the answer.

## Scope

### In scope

- `Material::participates_in_bloom() -> bool` in `crates/render-data`, defaulting
  `true` for every existing variant.
- One new matte-emissive `Material` variant + texture prefix: `emissive_strength()
  > 0`, `participates_in_bloom() == false`.
- Packing the `blooms` bit into the free padding of the existing 32-byte material
  uniform and threading it through the single bind-group build site.
- A hue-preserving luminance clamp in the forward and kinematic-brush fragment
  shaders, gated on the `blooms` bit, pinned below `BLOOM_THRESHOLD`.
- Dev-content A/B (extend `combat-demo`) and README/doc updates.

### Out of scope

- Any bloom-exclusion mask, stencil, MRT target, or new render pass (the deferred
  (b) upgrade).
- Supra-threshold brightness that never haloes (only (b) delivers this).
- Runtime / reaction-driven emissive intensity — the button's "lights up when
  approached" is a separate concern (see Dynamic angle). This feature governs
  halo, not brightness-over-time.
- Per-texel or per-surface bloom control; FGD keys; PRL wire changes; script SDK
  surface. Material identity stays prefix-derived.
- Billboard/sprite emissive. Matte-emissive is a world/kinematic-brush surface
  material only.
- Changing `BLOOM_THRESHOLD`, `BLOOM_INTENSITY`, or the global bloom profile.

## Dynamic angle (explicitly scoped out, with synergy noted)

The user's button "lights up when approached/pressed" is a runtime emissive change
— today emissive is fully static (`resource_management.md` §4.5: "no per-surface
runtime scalar in v1"). Delivering runtime emissive control is a **separate future
spec**, not absorbed here. This feature is a safe foundation for it: because the
clamp acts on the final fragment independent of brightness, a future runtime
feature that drives a matte-emissive surface brighter will still never bloom it.

## Boundary inventory

n/a — deliberately. The feature adds no cross-boundary name.
`participates_in_bloom` is a runtime-derived `Material` property (like
`emissive_strength`), never serialized. Material identity still flows through the
existing texture-name prefix → `Material` derivation. No PRL section, FGD KVP, or
JS/Luau surface changes. The only new byte is an internal material-uniform field.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Material uniform stays a 32-byte layout; `shininess`/`emissive_strength` byte positions unchanged. | Task 2 | Adding the `blooms` field to `build_material_uniform` + both WGSL `MaterialUniform` decls | AC 2 |
| Matte-emissive clamp ceiling equals `BLOOM_THRESHOLD` (strictly-below). | Task 3 | Threshold drift between `bloom.rs` and the shaders | AC 4 |
| Blooming materials (bit set) are byte-for-byte unaffected — no clamp applied; existing neon bloom preserved. | Task 1, 3 | Clamp gating on the bit | AC 3, 6 |
| Forward and kinematic-brush shaders apply the identical clamp. | Task 3 | Only one of the two world-surface shaders edited | AC 4, 5 |

## Acceptance criteria

Automated (a test proves it):

- [ ] **AC1.** `Material::participates_in_bloom()` returns `false` for the new
  matte-emissive variant and `true` for every other variant, and that variant's
  `emissive_strength()` is `> 0`. A texture with the matte-emissive prefix derives
  to that variant. (render-data unit tests.)
- [ ] **AC2.** The material uniform still packs `shininess` then
  `emissive_strength` in the first eight bytes, stays 32 bytes, and now carries the
  `blooms` bit; existing byte positions are unchanged. (render-cpu packing test.)
- [ ] **AC3.** With the `blooms` bit set, the forward and kinematic-brush shaders
  apply no clamp — the emissive/bloom code path is unchanged. (Shader-source
  contract test asserting the clamp is gated on the bit.)
- [ ] **AC4.** Both shaders clamp final linear luminance strictly below a ceiling
  equal to `BLOOM_THRESHOLD` when the bit is clear, hue-preserving; a test reads
  `BLOOM_THRESHOLD` and asserts the shader ceiling matches. Both shaders parse and
  pass naga validation. (Shader-contract + naga tests, mirroring
  `bloom_wgsl_sources_parse_and_validate`.)
- [ ] **AC5.** A CPU reference of the clamp maps a supra-threshold input to a
  sub-threshold, hue-preserved output, and leaves sub-threshold inputs untouched.
  (render-cpu or renderer unit test.)

Manual-visual (author-confirmed A/B — no image-diff exists in this project; a human
observes):

- [ ] **AC6.** On the dev map, a matte-emissive surface authored bright (emissive
  term well above `1.0`) glows crisply with **no halo**, beside a `neon_` surface
  at the same authored brightness that blooms. Existing neon bloom in the demo is
  visually unchanged.
- [ ] **AC7.** A bright `light_spot` aimed at the matte-emissive surface does not
  make it halo (the light-independent guarantee), while the same light on a
  neighboring bloom-eligible surface produces the expected bloom.

## Tasks

### Task 1: Add the material bloom predicate and matte-emissive variant

In `crates/render-data/src/material.rs`, add `participates_in_bloom(self) -> bool`
parallel to `emissive_strength()`, returning `true` for all current variants. Add
one new matte-emissive `Material` variant with `emissive_strength() > 0` (match the
neon 4.0 term so the A/B is at equal brightness) and `participates_in_bloom() ==
false`, wired into `lookup_material` under a new prefix. Non-emissive variants keep
`true` (moot while `emissive_strength` is 0, but correct for the clamp gate). Add
unit tests for the predicate per variant and prefix derivation. Confirm the exact
variant name and prefix against the Open-questions decision before landing.

### Task 2: Thread the bloom bit through the material uniform

In `crates/render-cpu/src/material_plan.rs`, extend `build_material_uniform` with a
`participates_in_bloom: bool` (or `blooms: f32`/`u32`) argument packed into the
currently-zero padding after `emissive_strength`, keeping `MATERIAL_UNIFORM_SIZE ==
32` and the first-eight-byte layout intact. Update the sole runtime caller,
`build_material_bind_group` in `crates/renderer/src/render/material_plan.rs`
(currently `build_material_uniform(material.shininess(), material.emissive_strength())`),
to pass `material.participates_in_bloom()`. Add the matching field to the WGSL
`MaterialUniform` struct in **both** `forward.wgsl` and `kinematic_brush.wgsl`
(reuse a padding slot; do not grow the struct stride). Update the render-cpu
packing test.

### Task 3: Clamp non-blooming surfaces below threshold in both world shaders

In `forward.wgsl` and `kinematic_brush.wgsl`, after the existing
`base_color·total_light + emissive·emissive_strength` composite and before writing
scene color, when the `blooms` bit is clear, clamp the fragment's Rec.709 linear
luminance to strictly below the bloom ceiling by scaling RGB by
`min(1, ceiling/luminance)` (hue-preserving; guard `luminance ≈ 0`). The ceiling
must equal `BLOOM_THRESHOLD` (`crates/renderer/src/render/bloom.rs`) minus a small
epsilon so quantization/interpolation cannot cross it — mirror the demo's finding
that sRGB 137 (`1.0006`) is the first byte over. Gate strictly on the bit so
bloom-eligible fragments are bit-identical to today. Add shader-source contract
tests (both shaders gate on the bit and use the shared ceiling), a test reading
`BLOOM_THRESHOLD` to pin the ceiling, and a CPU clamp reference (AC5). Keep the
naga validation coverage.

### Task 4: Dev content A/B and documentation

Extend `content/dev/maps/combat-demo.map` with a matte-emissive panel authored
bright (above threshold) beside the existing `neon_glow_panel`, plus a `light_spot`
aimed at it for the AC7 light-independence check. Update
`combat-demo.README.md`'s "Emissive panels" section to describe the declared
opt-out as the robust path and reframe the sub-threshold byte-tuning as the legacy
manual pattern. Note the deferred bloom-mask upgrade and the runtime-emissive
separation. Perform the AC6/AC7 visual A/B by eye.

## Sequencing

**Phase 1 (sequential):** Task 1 — defines the predicate and variant every later task consumes.
**Phase 2 (sequential):** Task 2 — consumes the Task 1 predicate; establishes the uniform bit both shaders read.
**Phase 3 (sequential):** Task 3 — consumes the Task 2 uniform field.
**Phase 4 (sequential):** Task 4 — visual verification needs the working Task 3 clamp; map/README edits can be prepared earlier but the A/B check gates on Phase 3.

## Rough sketch

`Material::participates_in_bloom()` is a `match` returning a compile-time constant
per variant, exactly like `emissive_strength()`. The bit rides the existing
material uniform's zeroed padding; both world-surface shaders already share that
uniform layout, so both read the new field by lexical name. The clamp is a
post-composite tone scale gated on the bit — no branching cost concern at retro
fragment counts. The ceiling is pinned to `BLOOM_THRESHOLD` by a test rather than a
shared constant, since the value lives in the renderer and the clamp lives in the
shaders; drift is the one real hazard, caught by AC4.

## Open questions

- **Variant + prefix naming.** Proposed `neonmatte_` → a matte-emissive variant
  reading as "neon that doesn't halo," consistent with `neon_`. But matte-emissive
  is a behavior, not a physical material, which strains the physical-name prefix
  convention (metal/concrete/glass/…). Alternative: a neutral `emissive_` /
  `Material::Emissive`. Needs the user's aesthetic call. Does the matte variant
  need its own `shininess`/`ricochet`, or should it mirror neon's?
- **Bit encoding.** `f32` (0.0/1.0) reads uniformly with the existing float fields;
  a `u32` is truer to a flag. Either fits the padding. Implementer's call unless
  the user has a preference.
- **Clamp ceiling margin.** Clamp to exactly `BLOOM_THRESHOLD·(1-ε)` — pick ε so a
  matte surface can idle as bright as possible while staying provably under the
  bright pass (the demo's 0.985 headroom is the reference).
- **Is the clamp's brightness cap acceptable for all intended matte-emissive
  content?** It caps on-screen brightness at threshold. If any use case needs a
  bright-yet-halo-free surface, that is the deferred (b) mask, not this. Confirm no
  v1 content needs supra-threshold-no-halo.
