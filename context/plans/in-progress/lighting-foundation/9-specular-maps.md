# Sub-plan 9 — Specular Maps

> **Parent plan:** [Lighting Foundation](./index.md) — read first for goals.
> **Scope:** Per-texel specular intensity sourced from a grayscale specular map texture, evaluated in the fragment shader direct lighting loop using Normalized Blinn-Phong. Adds a specular highlight term on top of the existing Lambert diffuse term (sub-plan 3) and respects shadow factors from CSM (sub-plan 5) and SDF sphere-tracing (sub-plan 8).
> **Crates touched:** `postretro` (shader + texture binding), `postretro-level-compiler` (specular-map association via texture naming convention).
> **Depends on:** sub-plan 8 (SDF shadows — the full direct lighting term, including shadowing for point/spot lights, must be in place before layering specular on top so that shadowed fragments receive no specular).
> **Blocks:** nothing.

---

## Shading model decision

**Decision: Normalized Blinn-Phong** with a per-texel specular intensity map and a per-material shininess exponent.

Rationale, for the record so the choice is not revisited without cause:

- **Retro aesthetic fit.** Blinn-Phong's tight analytical highlights look crisp on chunky pixelated texture art. GGX's micro-detail tail is imperceptible at the texture resolutions this engine targets and would cost shader complexity for no visible benefit.
- **SH-L2 indirect is diffuse-only.** The baked irradiance volume (sub-plan 2/6) cannot carry specular directional information. A PBR direct term without matching IBL specular indirect would produce inconsistent materials (physically-plausible direct highlights over non-physical indirect). Reflection probes and specular IBL prefilter are explicit non-goals for Milestone 5.
- **Normalization is cheap.** Multiplying by `(n + 8) / (8π)` is one `fma` — it keeps highlight intensity stable as shininess varies so an artist can retune `shininess` without also retuning `spec_intensity`. No runtime cost beyond what non-normalized Blinn-Phong already pays.
- **No metalness/roughness texture budget.** One R8 specular intensity channel fits the lean texture budget; PBR would at minimum double material texture memory per surface.

Specular color is implicitly white-times-light-color (dielectric assumption). No metalness. If a future art direction demands tinted metallic highlights, the spec map can be upgraded from R8 to RGBA8 (RGB = specular tint, A = intensity) without a pipeline rework; this is explicitly out of scope here.

---

## Description

A per-texel specular intensity (`s ∈ [0, 1]`, sampled from the specular map) modulates a Normalized Blinn-Phong highlight term in the per-fragment direct light loop. The final direct contribution for one light becomes:

```
// pseudocode — actual WGSL in implementation
let L = <already computed in sub-plan 3>;
let N = <geometric normal — no normal-map perturbation in Milestone 5>;
let V = normalize(camera_position - frag_world_pos);
let H = normalize(L + V);

let NdotL = max(dot(N, L), 0.0);
let NdotH = max(dot(N, H), 0.0);

let diffuse  = NdotL;                                     // Lambert (sub-plan 3)
let spec_norm = (shininess + 8.0) / (8.0 * 3.14159265);   // energy-normalization factor
let specular = spec_intensity * spec_norm * pow(NdotH, shininess) * NdotL;
//                                                                     ^^^^^^
//                            gated by NdotL so backfacing fragments get no highlight

total_light += light.color * attenuation * shadow_factor * (diffuse + specular);
```

Where:
- `spec_intensity` is the R channel of the specular map sampled at the fragment's UV.
- `shininess` is a per-material scalar sourced from the material enum variant (see below).
- `shadow_factor` is the product of CSM (directional) or SDF (point/spot) shadow terms already computed in the outer light loop — see sub-plans 5 and 8. Specular must never bypass shadowing.

The camera_position uniform already exists in group 0 as of sub-plan 3, so no new group-0 plumbing is needed. `N` is the geometric normal; normal-map perturbation is out of scope for Milestone 5 (see `context/lib/resource_management.md` §4 — Phase 5+).

### Energy behavior — deliberately non-conserving

The specular term is **added** to the Lambert diffuse term, not traded against it. There is no `(1 - spec_intensity)` attenuation of the diffuse and no Fresnel. At glancing angles and close light distances, `diffuse + specular` can exceed 1.0 before light color multiply — this is intentional.

**Why:** on chunky pixel-art textures, an energy-conserving highlight reads as a muted gloss smear. The retro aesthetic this engine targets is built on readable, punchy highlights that pop against the diffuse texture color the way they do in Quake-era rendering. Physical accuracy would blunt the silhouette of the highlight against the underlying texel grid — the exact thing that reads as "retro."

Do not "fix" this by multiplying diffuse by `(1 - ks)`, adding a Fresnel term, or otherwise conserving energy. If the scene exposes visible over-bright regions, the lever is the artist-authored specular map intensity or the per-material shininess, not the shading equation.

### Per-material shininess

Shininess is a property of the **material enum variant**, alongside the other behavior hooks described in `context/lib/resource_management.md` §3 (footsteps, impacts, etc.). There is no sidecar file, no FGD property, and no per-face override in this sub-plan — the same texture-prefix → material-variant lookup that already drives material behavior also supplies `shininess`.

- **Default variant:** `shininess = 32.0` (moderately tight highlight).
- **Matte variants** (e.g. `concrete`, `plaster`): `shininess = 4.0` — broad, soft highlight.
- **Glossy variants** (e.g. `metal`): `shininess = 64.0` — tight highlight.
- **Range:** `[1.0, 256.0]`. Values are compile-time constants in the material-variant table, not runtime-authored data, so no clamp or validation is needed at load time. If that assumption changes later (per-texture overrides, sidecar files), enforce the clamp at whatever boundary introduces the user-authored value.

The existing material-enum extension point is the authoritative knob for Milestone 5. Per-texture override via sidecar is an explicit non-goal here and would be layered on without pipeline rework when it's needed.

---

## Specular map convention

**Texture-naming convention.** For every diffuse texture `foo.png`, an optional sibling `foo_s.png` in the same collection directory provides the specular map. Mirrors the existing `_n` normal-map naming documented in `context/lib/resource_management.md` §4. No FGD surface-property plumbing and no map-file changes.

- **Format:** R8Unorm. Single channel — red = specular intensity in `[0, 1]`.
- **Sampler:** shares `base_sampler` (group 1 binding 1). No new sampler needed.
- **Missing map:** if `foo_s.png` is absent, the resource loader substitutes a shared 1×1 R8 black texture as the specular view for that material. The fragment shader unconditionally samples and multiplies; a black sample zeros the specular term. No shader branching, no missing-texture warning — absence means "no specular response," which is the correct default for matte surfaces.
- **Color space:** linear (not sRGB). Specular intensity is a physical-ish coefficient, not a perceptual color.
- **Dimensions:** must match the diffuse. Mismatched dimensions log a warning at load time and fall back to the 1×1 black texture.

Because PRL stores only texture names (see `resource_management.md` §1.2), **specular discovery happens at runtime, not at compile time.** When the resource loader loads `textures/<collection>/foo.png`, it also probes for `foo_s.png` in the same directory; found or not, the corresponding specular view is attached to the material bind group.

---

## Bind group changes

Extend **group 1 (per-material)** with one texture binding and one small uniform:

```wgsl
// Proposed design — remove after implementation; source of truth is the code
@group(1) @binding(0) var base_texture: texture_2d<f32>;         // unchanged
@group(1) @binding(1) var base_sampler: sampler;                 // unchanged
@group(1) @binding(2) var specular_texture: texture_2d<f32>;     // new — R8, sampled as .r
@group(1) @binding(3) var<uniform> material: MaterialUniform;    // new

struct MaterialUniform {
    shininess: f32,
    _pad: vec3<f32>,   // align to 16 bytes
}
```

Group 1 currently has no per-material uniform buffer (per sub-plan 3's bind-group description and the state in `resource_management.md` §2) — this sub-plan introduces the slot. Keep the struct minimal; future per-material scalars extend `MaterialUniform` in place rather than allocating new bindings.

The uniform is populated at level load from the material-variant lookup (per-variant `shininess` constant). One `MaterialUniform` value per unique material bind group.

No changes to group 0 or group 2.

---

## Compiler changes

**None.** PRL already stores texture names only (see `resource_management.md` §1.2), and specular maps follow the same sibling convention the engine already uses for normal maps. Sibling discovery, 1×1 black fallback, and `MaterialUniform` population are all runtime concerns in the resource loader.

No FGD changes, no map-file syntax changes, no new PRL section, no per-face material record extension. If a later optimization packs textures into an atlas or array (`resource_management.md` §2.2), specular siblings travel through that path identically to diffuses; that is a separate, unscheduled initiative.

---

## Acceptance criteria

- [ ] Shading model decision recorded in this document (done — Normalized Blinn-Phong).
- [ ] Texture-naming convention (`*_s.png`) documented here; update `context/lib/resource_management.md` §4 (or a new §4.1) when this sub-plan ships.
- [ ] Resource loader probes for `{name}_s.png` sibling for each diffuse and attaches either the loaded texture or a shared 1×1 R8 black fallback to the material bind group.
- [ ] Dimension mismatch between diffuse and specular sibling logs a warning and falls back to the 1×1 black texture.
- [ ] Engine binds specular texture at group 1 binding 2 and `MaterialUniform` at group 1 binding 3 for every material draw.
- [ ] `MaterialUniform.shininess` is sourced from the material-enum variant lookup described in `resource_management.md` §3. Default variant: `32.0`. At least one matte and one glossy variant carry distinct values.
- [ ] Fragment shader evaluates Normalized Blinn-Phong specular per light, gated by `NdotL` and multiplied by the existing `shadow_factor`. `V = normalize(camera_position - frag_world_pos)` is hoisted out of the per-light loop.
- [ ] Specular is **added** to diffuse with no `(1 - ks)` attenuation and no Fresnel — the non-conserving behavior is deliberate (see "Energy behavior" above).
- [ ] Specular contribution respects CSM shadows (directional) and SDF shadows (point/spot) — shadowed fragments receive zero specular.
- [ ] Specular contribution respects spot cone attenuation and light-type falloff (flows through `attenuation`).
- [ ] `assets/maps/occlusion-test.map` extended with at least three surfaces: matte wall (no `*_s.png`, matte material variant), glossy metal panel (full-white `*_s.png`, glossy material variant), and a masked panel (half-black / half-white `*_s.png`) demonstrating per-texel variation.
- [ ] Visual validation on `occlusion-test.map`: moving the player past the glossy panel under a point light produces a visible highlight that tracks view direction; matte wall shows no highlight; masked panel shows the highlight only on the white-specular half.
- [ ] Visual validation under each light type (point, spot, directional) and through a CSM-shadowed region — highlight vanishes wherever diffuse is shadowed.
- [ ] `cargo test -p postretro` passes.
- [ ] `cargo clippy -p postretro -- -D warnings` clean.

---

## Implementation tasks

1. **Material variant: shininess property.** Extend the material enum (or its property table — whichever form the mechanism in `resource_management.md` §3 currently takes) with a `shininess: f32` constant. Populate the existing variants with sensible defaults (Default=32, matte variants≈4, glossy variants≈64).
2. **Resource loader: specular sibling discovery.** When loading `textures/<collection>/{name}.png`, also probe for `{name}_s.png` in the same directory. On success, upload as R8Unorm (linear). On absence, reuse a shared 1×1 R8 black texture. On dimension mismatch vs the diffuse, log a warning and use the 1×1 fallback.
3. **Engine: bind-group layout.** Add binding 2 (specular `texture_2d<f32>`) and binding 3 (`MaterialUniform` uniform buffer, 16 bytes) to the group 1 layout. Update material bind-group creation to include the specular view and a per-material uniform buffer populated from the variant's `shininess`.
4. **Shader: specular term.** Extend the fragment-shader light loop (`forward.wgsl`) with the Normalized Blinn-Phong evaluation above. Hoist `V = normalize(camera_position - frag_world_pos)` out of the per-light loop. Keep the specular multiply gated by `NdotL` so backfacing fragments emit nothing. Do **not** add a `(1 - ks)` diffuse attenuation or Fresnel term (see "Energy behavior").
5. **Shader: shadow integration.** Multiply the full `(diffuse + specular)` by `shadow_factor` rather than diffuse alone. Verify by stepping into a CSM-shadowed region — the highlight must vanish with the diffuse.
6. **Test-map authoring.** Extend `assets/maps/occlusion-test.map` with three surfaces: a matte wall (matte material variant, no `*_s.png`), a glossy metal panel (glossy variant, full-white `*_s.png`), and a masked panel (half-black / half-white `*_s.png`) demonstrating per-texel variation. Author the sibling `*_s.png` textures under the appropriate `textures/<collection>/` directories.
7. **Visual validation.** Walk `occlusion-test.map` under each light type (point, spot, directional). Confirm: highlight tracks view direction, vanishes in CSM/SDF shadow, respects spot cone and distance falloff. Compare matte vs glossy side by side; confirm the masked panel only highlights on the white half.

---

## Migration on plan ship

When Milestone 5 ships, the following migrates out of this sub-plan:

**To `context/lib/rendering_pipeline.md`** (or a new `lighting.md` if the section outgrows §4):

- Shading model choice (Normalized Blinn-Phong) and the rationale bullets above.
- The deliberately non-conserving energy behavior (specular added, not traded) and why.

**To `context/lib/resource_management.md`** (adjacent to the `_n` normal-map convention in §4):

- Specular-map texture convention (`*_s.png` sibling, R8Unorm, linear color space, 1×1 black fallback, dimension-match requirement).
- Shininess as a property of material enum variants (the concept — not defaults or ranges, which live in code).

**Explicit future option, not planned:** upgrade path to RGBA8 tinted specular (RGB tint + A intensity) and to a full metalness/roughness PBR pipeline if future art direction demands it. Either would require matching IBL infrastructure (reflection probes, specular prefilter) — out of scope here.

## Notes

**Animated lights have static specular contribution.** Same limitation as sub-plan 3's direct term — the GPU light buffer uses each light's static base properties, so animation curves modulate indirect (SH) only. A flickering torch produces a steady highlight on a glossy panel; the animation reads through the bounce light, not the direct term. Matches the sub-plan 3 known-limitation note; layering runtime light-animation evaluation on top is additive in a later milestone and requires no rework here.
