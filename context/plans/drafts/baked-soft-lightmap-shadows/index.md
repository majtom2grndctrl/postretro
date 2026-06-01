# Baked Soft Shadows (Static Directional Lightmap)

## Goal

Static `static_light_map` lights bake **hard** occlusion folded into per-texel irradiance (`lightmap_bake.rs` per-texel loop), sampled nearest — so shadow edges are blocky 1-texel steps while geometry, normal maps, and SH indirect render at native resolution. The blocky edge is the lone aesthetic holdout. Replace the hard shadow gate with **bake-time area-light visibility** (stratified shadow-ray sampling) so static shadows gain smooth, contact-hardening penumbra edges that stay coherent with the retro look. Realizes the original "baked DF shadow" intent from the lighting-foundations milestone via area sampling rather than a runtime distance-field trace; coexists with the runtime SDF path.

## Design pivot (supersedes the earlier K-channel decision)

Drafting surfaced a simplification that overrides the earlier "K-channel, per-texel separable visibility" choice. Reasoning: K-channel only earns its cost if per-light static visibility must stay **separable at runtime**. Steady (non-animated) static lights have fixed bake-time intensity — nothing to recombine later — so their soft visibility can be multiplied per light and **summed at bake** into the existing single irradiance atlas. Summation handles **arbitrary overlap with no K cap** (the "rarely >3 overlap" worry dissolves), each light keeps its own penumbra (computed with its own size), storage and runtime cost are unchanged, and no new PRL section is needed. The animated path is already per-light sparse (`TexelLight.weight`), so it softens by scaling the weight — also no K-channel. K-channel is therefore dropped; see Non-goals.

## Scope

### In scope
- New per-light **size** input (FGD KVP + `MapLight` field): `_light_size` (world-unit radius, Point/Spot) and `_angular_diameter` (degrees, Directional). Parsed and validated in the compiler alongside existing light KVPs.
- **Soft area-visibility bake helper**: returns a `[0,1]` unoccluded fraction for `(surface_point, surface_normal, light)` by stratified-sampling the light's area and tracing each sample through `segment_clear`. Deterministic (no RNG), per `LightType`. Adaptive sample count to bound bake cost.
- **Static lightmap bake**: multiply each light's contribution by its soft visibility before summing into irradiance; weight the dominant-direction accumulation by soft visibility too. No format change; stays `LightmapMode::Shadowed`.
- **Animated weight-map bake**: replace the binary `shadow_visible` membership gate with a soft-visibility-scaled `TexelLight.weight`. No wire-format change.
- **Runtime bilinear irradiance**: split the group-4 sampler so irradiance and the animated atlas filter **linear** while the octahedral direction stays **nearest**. Smooths residual texel steps inside each penumbra ramp.
- **Bake diagnostics**: per-light warning when computed penumbra width falls below ~1 atlas texel (edge will still read hard/blocky at the chosen `_light_size` and atlas density); a global sample-count knob.

### Out of scope (Non-goals)
- **K-channel / runtime-separable static visibility.** Superseded (see Design pivot). Steady lights sum at bake.
- **Distance-field sharp-edge reconstruction** for sub-texel-narrow penumbras. This slice targets visibly-soft penumbras where area sampling suffices; near-hard edges remain atlas-resolution-limited.
- **Runtime SDF path changes.** Baked soft shadows own the cheap static default; runtime SDF stays for animated-intensity, specular shadowing, and cases needing live recomputation. Not mutually exclusive.
- **Moving-object shadows.** Dynamic tier (shadow-map pool) only.
- **New PRL sections or section-ID changes.** Sections 22 (Lightmap) and 25 (AnimatedLightWeightMaps) keep their byte layout; only the baked *values* change. (No collision with the `octahedral-irradiance-atlas` plan, which owns SH sections 20/27 only.)

## Acceptance criteria
- [ ] A single light casting onto a flat receiver bakes a **multi-texel penumbra gradient**, not a 1-texel hard step, at the default `_light_size`.
- [ ] Shadow from a box onto a floor is **sharp near the contact and softer with distance** (contact hardening), with no authored distance-to-occluder input.
- [ ] A room with **>4 overlapping static lights** renders all shadows correctly softened and summed — no hard cap, no banding, no per-light artifacts.
- [ ] A flickering (animated) baked light shows a soft shadow whose **shape does not change** as intensity animates.
- [ ] Increasing `_light_size` (or `_angular_diameter`) visibly **softens** that light's shadow; the documented default produces a subtle penumbra on recompiled existing maps with no authoring change.
- [ ] Bake is **deterministic**: the same `.map` compiles to a byte-identical `.prl` across separate processes.
- [ ] Pre-existing `.prl` files still load and render (legacy hard-baked atlases sample fine under the new linear irradiance filter; no version break).
- [ ] Under camera magnification inside a penumbra, the gradient is smooth (no atlas-texel stair-steps); the direction channel and bumped-Lambert response are unchanged.
- [ ] Forward-pass cost is unchanged aside from one added sampler binding (no new per-fragment loop); holds an acceptable framerate on the 2020 16" MBP (Radeon Pro 5500M) floor. Lightmap-bake wall-time increase is measured and documented.

## Tasks

### Task 1: Light size authoring
Add `_light_size` (Point/Spot, world-unit radius) and `_angular_diameter` (Directional, degrees) to the `light` / `light_spot` / `light_sun` FGD entity definitions, and corresponding fields on `MapLight` (`map_data.rs`). Parse and validate where `MapLight` is built from FGD KVPs (clamp to non-negative; documented defaults applied when the KVP is absent). Existing plumbing for other light entities (e.g. a future rotating cone) reuses the same fields.

### Task 2: Soft area-visibility bake helper
Add a function returning the `[0,1]` unoccluded fraction for a texel/light pair. Sample the light area by `LightType`: Point/Spot → points on a sphere/disk of radius `_light_size` at `light.origin`; Directional → directions within a cone of half-angle `_angular_diameter/2` about `-cone_direction`, traced to a far point. Trace each sample with `segment_clear` (reuse `RAY_EPSILON` origin offset, max-distance clamp). Sampling is deterministic: a fixed low-discrepancy pattern (mirror `sh_bake.rs`'s Fibonacci-lattice convention) with a per-texel rotation derived from integer texel coordinates via a deterministic hash — no `rand`, no `std` `RandomState`-ordered iteration feeding output. Use adaptive sampling: a small probe set, escalating to the full count only when probe rays disagree (penumbra), to bound bake cost. `_light_size == 0` (and `_angular_diameter == 0`) short-circuits to the existing single hard ray.

### Task 3: Static lightmap bake — soft sum
In the per-texel loop, replace the hard `if !shadow_visible(...) { continue; }` gate with `let v = soft_visibility(...); if v <= 0 { continue; } irr += contribution * v;` and weight the dominant-direction accumulation by `v`. Output stays a single Rgba16 irradiance atlas + Rgba8 direction atlas, `LightmapMode::Shadowed`. The dilation/chart-seam handling (`dilate_edges`, `CHART_PADDING_TEXELS`) is unchanged — soft visibility is a scalar that dilates identically to today's irradiance.

### Task 4: Animated weight-map bake — soft weight
In the animated weight-map bake, replace the binary `shadow_visible` membership gate with the soft-visibility fraction multiplied into the emitted `TexelLight.weight`. Per-light sparsity already handles overlap; the compose pre-pass and `AnimatedLightWeightMapsSection` layout are untouched.

### Task 5: Runtime bilinear irradiance
Add a second, **filtering** sampler binding to group 4; mark the irradiance and animated-atlas texture bindings `filterable: true` (Rgba16Float is filterable without extra features) and sample them through the linear sampler. The direction texture keeps the existing nearest/non-filtering sampler (octahedral lerp ≠ slerp). Update `lighting/lightmap.rs` (sampler + BGL), the group-4 BGL/visibility in `render/mod.rs`, and the `forward.wgsl` group-4 bindings + static-direct term. Bumped-Lambert and the lighting isolation modes are otherwise unchanged.

### Task 6: Diagnostics + perf gate
Emit a per-light bake warning when estimated penumbra width at its receivers is < ~1 atlas texel. Expose the area-sample count as a bake knob (default tuned for quality vs. wall-time). Add or extend a test map with a single key light, a box-on-floor contact case, and a many-overlapping-light room. Measure forward-pass cost on the MBP floor and lightmap-bake wall-time delta; record both.

## Sequencing

**Phase 1 (sequential):** Task 1 — light size feeds the helper.
**Phase 2 (sequential):** Task 2 — the soft-visibility helper blocks both bakes.
**Phase 3 (concurrent):** Task 3, Task 4, Task 5 — static bake, animated bake, and runtime filtering are independent (Tasks 3/4 consume Task 2; Task 5 is bake-independent, touches only runtime).
**Phase 4 (sequential):** Task 6 — diagnostics, test maps, and perf/determinism validation consume all prior tasks.

## Rough sketch

- Reuse primitive: `segment_clear(bvh, primitives, geometry, origin, target) -> bool` (`lightmap_bake.rs`). Soft visibility = fraction of `M` area samples whose segment is clear.
- Contact hardening is emergent: near a contact all samples occlude together (sharp); with receiver distance the sample cone widens (soft). No distance-to-occluder term needed.
- Determinism: follow `sh_bake.rs` (`SAMPLING_LATTICE_OFFSET`, Fibonacci directions, order-preserving `into_par_iter` ranges). Per-texel rotation from an integer hash of texel `(x, y)`, not `RandomState`.
- Sampler split (Task 5): group 4 gains binding 4 = filtering sampler; bindings 0 (irradiance) and 3 (animated atlas) become `filterable: true` and sample through it; binding 1 (direction) stays on the non-filtering sampler at binding 2.

## Boundary inventory

`_light_size` / `_angular_diameter` are **bake-only** — consumed by the compiler, never serialized to a runtime PRL section (dynamic/SDF lights ignore them this slice).

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| light size (point/spot) | `MapLight.light_size: f32` | n/a (bake-only) | n/a | n/a | `_light_size` |
| angular diameter (sun) | `MapLight.angular_diameter: f32` | n/a (bake-only) | n/a | n/a | `_angular_diameter` |

## Wire format

No new binary surface. **Lightmap section (id 22)** and **AnimatedLightWeightMaps section (id 25)** keep their exact existing layouts, formats, and version tags; only baked sample *values* change (soft visibility vs. a hard 0/1 gate). `LightmapMode` stays `Shadowed` (shadows remain folded into irradiance — now soft; `Unshadowed` is not used). Legacy `.prl` decode is unaffected.

## Open questions
- **Default `_light_size` / `_angular_diameter`.** Need tuning against real maps and world scale (near 0.1, far 4096). Starting points: `_light_size ≈ 0.25` world units, `_angular_diameter ≈ 0.5°` (physical sun). The default is nonzero by the "default upgrade" decision — existing maps soften on recompile; confirm that's acceptable for all shipped test maps.
- **Bake cost ceiling.** Adaptive sampling bounds the common case, but worst-case many-light penumbra-dense rooms multiply ray count. Confirm the measured wall-time is acceptable or gate the full sample count behind a quality flag.
- **Spot penumbra vs. cone falloff.** The spot's *cone* soft edge (`spot_cone`) and its *shadow* penumbra (area sampling) are independent; confirm they read coherently together rather than double-softening the rim.
- **Bilinear vs. baked penumbra sufficiency.** If baked multi-texel ramps already de-block adequately, Task 5 could ship separately; keep it in-scope unless it adds risk.
