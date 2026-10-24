# planar-water-reflections — IP check: pixel-style tilt in the planar resolve

> **Engineering triage, not legal advice. 2026-10-23. Read at 986423d1f; spike at `claude/planar-water-spike` 8201dfb8e.** A technical comparison of published claim text against candidate implementations, to brief the owner and a patent attorney. "Appears present", "appears absent" and "reads on" are engineering readings, not opinions on infringement or validity. Claim construction, validity and freedom to operate are counsel's.

Scope: the open question in `index.md` (IP triage of the pixel tilt). Carries forward `plans/done/fluid-ssr-reflections/ip-check.md` (cited below as *SSR check*), the spike contract's IP rows and §7.3a IP rules; patents charted there are re-read here only where the tilt changes the analysis.

## The tilt as built today

Pixel style tilts both existing layers from one definition in `fluid_reflection_ray.wgsl`, mirrored by `render-cpu/src/fluid_surface/reflection_style.rs` and pinned in `fluid_surface_tests.rs`.

| Step | Symbol | Inputs |
|---|---|---|
| Cell offset | `fluid_displayed_offset` (`fluid_motion.wgsl`) | integer texel cell, renderer clock, texels per scene pixel. Shimmer (hashed four-step cycle) plus rings (hashed block drops); 0 for a cell past its fade threshold. Drift and wave never enter |
| Texel axes | `fluid_reflection_texel_axes` | UV and world-position screen derivatives, face texture size |
| Cell point | `fluid_reflection_cell_centre`, then `fluid_reflection_cell_within_box` | cell centre, pulled toward the **probe's box and capture point** at a box rim |
| Tilted normal | `fluid_reflection_cell_normal` | flat outward normal + unit axes × offset × `FLUID_REFLECTION_PIXEL_TILT` (0.05 per texel) |
| Ray | `fluid_reflection_mirror`, `fluid_reflection_above_horizon` | view from the cell point (`camera − point`), tilted normal; kept `FLUID_REFLECTION_RAY_MIN_Y` above the horizon |

- **What it perturbs.** The ray direction, about the cell's tilted normal, from the cell point. Not a lookup UV, not the march's step logic.
- **SSR.** `fs_reflection` (`fluid_reflection_pass.wgsl`) marches that ray from the cell point; every target texel of a cell writes the same value. The composite (`fluid_reflection_screen_layer`, `fluid_surface.wgsl`) loads the target nearest at the **unpulled** cell centre projected to the screen, times the group's `ssr_scale`.
- **Probe term.** Tilts too: `fs_main_reflecting` passes the same `point.normal` to `fluid_reflection_sample`. Fresnel uses the flat normal (`dot(view, outward)`), so tilt changes what reflects, not how strongly.
- **Smooth style.** `fluid_reflection_point` returns the fragment and the flat normal: no tilt, no distortion in either layer.
- **The spike resolve** (`planar_resolve.wgsl`, `fs_resolve`) is a full-screen pass. It reads the mirrored texel, mirror depth, water coverage and D/R fades. It has no UV, texture size or derivatives, so it **cannot compute a cell's offset**. Any tilt needs the selected plane's fluid draws rasterized with their vertex streams and world material (as the SSR pass does), or a mask pass that writes per-cell data. This holds for P1 and P2 alike.
- **The spike's IP scan** (`planar_shaders_preserve_ip_boundaries`) bans the substring `dot(` in planar shader text. `fluid_motion.wgsl` contains `dot(diff, diff)` (ring radius) and `fluid_reflection_ray.wgsl` contains the gate's and mirror's dots. Composing either, which parity requires, fails the scan as written.

## Paths

| Path | What the planar layer does | Reads view dir | Fresnel | Probe / s | Notes |
|---|---|---|---|---|---|
| **P0** | No tilt (spike as built) | No | No | No | Owner rejected as a look; risk baseline |
| **P1** | Offset the mirror lookup UV by the cell's offset vector projected to the screen, times a constant (DuDv / normal-map water) | No | No | No | Shift is independent of reflected distance, so it cannot match SSR's or the probe's tilt |
| **P2** | Build the cell's tilted ray with the shared helpers; reproject `X = P + r′·d` through the mirror view-projection, `d` from the mirror depth at the untilted texel; sample there | For the reflection law only | No | No (unpulled centre) | Same ray as SSR by construction; one extra depth and colour tap per cell |
| **P2b** | As P2, but march `r′` through the mirror depth | Reflection law | No | No | A screen-space march: `index.md` §1.1 sign-off, cost, and miss handling. Not charted further |
| **P3** | Perturb the mirror camera, or one oblique view per cell or facet | Per view | No | No | One view per facet is unaffordable; see Snap below |
| **P4** | Tilt the planar read inside the fluid composite | Already there | Beside it | Beside it | Edits pinned shaders and pins; a non-starter by the brief |

## Charted references

### Carried fences, re-read for the tilt

| Reference | Fence | P1 | P2 | P3 | P4 |
|---|---|---|---|---|---|
| Microsoft US10586375B2 / US10360720B1; EP3756162 (grant intended 2026-07-16, text unread) | Every US/CN independent claim needs "determining whether the reflection ray … is reflected back to a virtual camera position" plus a technique chosen by it (SSR check §3, verbatim) | Absent: no direction read | Absent if no branch, select or weight on direction. A "behind the mirror eye" test (`w ≤ 0`) that drops to c = 0 would be a direction determination with a hand-over to the probe; clamp instead (D2) | Absent | Same as P2, inside the shader holding F |
| Tencent WO2026113651A1 cl. 15 | Supplementing where the capture shadow fires (SSR check §1) | Absent | Absent: reads no s, tile or probe colour | Absent | **Risk rises**: the offset would sit beside s; any coupling reads on 15b |
| NetEase CN114748874B cl. 1 (PatSnap translation, unverified) | Per water pixel a "reflection position"; first information from an opaque texture at its clip-space UV; second from environment "according to the reflection position"; Fresnel; combine (SSR check §4) | 1a absent: no reflection position is computed | 1a and 1b-first **appear present**, as for SSR on every other plane. 1b-second absent while the probe stays keyed to its own search (W1 carried). Claim 2 (position at a preset distance) absent while `d` comes from depth | Absent | Risk rises if the probe lookup ever reuses the planar position |
| NetEase CN117496039A (abstract, SSR check §5) | Static from probe, dynamic from planar | Unchanged: mirror draws the static world | Same | Same | Same |
| Tencent CN116778049B (title level, SSR check §5) | Multi-threaded mirror pipeline | Unchanged | Same | Multiplies views; keep one encoder | — |

### New in this search

**ByteDance US12131408B2**, "Image rendering method and apparatus, device, and medium". Priority 2021-10-13; granted 2024-10-29; active, anticipated expiry 2042-09-29 (Google Patents, not verified). Claim 1 read verbatim in segments from Google Patents' page text:

> "determining an initial rendering video stream for a background image captured in advance, … each frame of initial rendering image is generated based on a reflection texture and a map texture; obtaining an initial rendering coordinate of each rendering pixel … and obtaining a rendering moment …; determining an amount of noise of each rendering pixel at the rendering moment based on the initial rendering coordinate, a preset noise texture, and the rendering moment, and performing computing on the amount of noise according to a preset algorithm to obtain a target rendering coordinate …; and updating the initial rendering coordinate … based on the target rendering coordinate …"

| Element | P1 | P2 |
|---|---|---|
| Rendering video stream for "a background image captured in advance" | Mirror view rendered live each frame | Same |
| Per-pixel noise from "a preset noise texture" at coordinate and moment | Integer hash of cell and clock; no texture | Same |
| Target coordinate from the noise; coordinate updated | Present in substance (lookup offset by a time-varying per-cell value) | Coordinate from a reprojected ray, not from noise |

Reading: appears not to read on either; P1 is closer. Keep the offset a hash, never a sampled noise texture (D8). Confidence medium-high.

**Wargaming US10002456B2 / US10290142B2**, "Water surface rendering in virtual environment". Priority 2016-07-08; active, '142 anticipated expiry 2037-06-23 (Google Patents). Independent claims read via Google Patents through a summarizing fetch: '456 claims 1/11/16 recite a displaced mesh from height-map data and an iterated view-ray intersection; '142 claims 1/11/16 recite surface and volumetric shadow components from a shadow map (segments verbatim). Neither recites a reflection camera or reflection texture coordinates. **Appear not to read.** Their shared description discloses P2's lookup: the reflection ray about the surface normal, intersected with the reflection camera's far clip plane, gives the reflection image's texture coordinates. That is prior art for P2 dated 2016, published 2018. Family continuations were not checked (Google rate-limited).

**Adshir / Snap hybrid-ray-tracing family**: US10699468, US10950030, US11302058, US12051145, US12573127; earliest provisional 2018-11-04 (inferred). **Search-snippet summaries only; claims not read.** Claim 1 as summarized: reflections in a **non-planar** specular surface, by projecting primary rays onto it and shooting secondary rays from **each surface triangle** toward object triangles, shading those, and matching secondary to primary rays. P1 and P2 use one plane-wide mirror view and no per-facet projection: appears absent on the summary. **P3 is the path that approaches it**: one view per cell is a non-planar surface approximated by small planar facets, the family's stated idea. Counsel item; one plane-wide camera is a design rule (D7).

**NVIDIA US11010963B2** (2018-04-16; active to ≈2039-04-02). Claims 1/13/14 read verbatim in segments: per-vertex reflected or refracted rays, lighting meshes, rays projected from the water mesh to light scene geometry (caustics). Not a reflection lookup. **Does not read.**

**Abstract level only, not charted:**
- Perfect World CN112233216B (2020-12-18): reflection planes in the scene (water, roads, glass, mirrors) resolved from screen pixels and screen depth, several planes at once; a screen-space planar method, no mirror render.
- Guangdong Q-Dazzle CN116206039A (2023-01-30, application): screen-space planar reflection completed with a skybox rendered from a mirrored camera.
- NetEase CN113617024A (2021-08-03, application): blends a reflection map and a refraction map by a preset interpolation parameter.
- Nintendo US8972230B2 (indirect texturing, per-texel UV offsets for droplets): title and description snippet; status not checked.

Not found: any claim on pixelated or per-texel-quantized reflections, oblique near-plane mirror clipping, or planar combined with probes or SSR beyond the carried fences. Google Patents rate-limited this session after about fifteen reads; USPTO PPUBS PDFs are image-only here; Justia, FreePatentsOnline and the EP Register returned 403. This is not a clearance search; filings from the last 18 months are invisible.

## Per-path risk

| Path | Risk | Why |
|---|---|---|
| P0 | Low (baseline) | Adds nothing to the spike's posture |
| P1 | **Low** | No view direction, Fresnel, probe or s; computes no reflection position. Practises expired and old art directly (Prior art). Closest live claim, ByteDance '408, needs a pre-captured background and a noise texture |
| P2 | **Low** (China: low-medium, unchanged) | Direction enters only the reflection law; no determination, branch or weight (D1–D2). Adds NetEase CN114748874B 1a/1b-first on the selected plane, which SSR already carries on every other reflecting plane; the W1 distinction holds. Lookup practised by Wargaming's 2016 description and UE 4.12 |
| P2b | Medium | Adds a screen-space march with misses; reopens Microsoft W0 and EA viewport-exit questions (SSR check §2) in a new pass |
| P3 | Medium, unverified | Approaches the Snap family's per-facet projection; multiplies views (Tencent CN116778049B if parallelized); unaffordable |
| P4 | Medium (process) | Puts the tilt beside F, s and the probe sample in the pinned shader; reopens the SSR check's W0/W1 pins and two Tencent/NetEase fences |

## Prior art

| Publication | Date | What it discloses | Read |
|---|---|---|---|
| Microsoft US6985148B2, "Interactive water effects using texture coordinate shifting" | Filed 2001-12-13; **expired** (fee-related) | Flipped scene image as the reflection; a height map shifts texture coordinates per frame for waves and ripples | Description via Google Patents |
| Valve, Half-Life 2 / Source water | 2004 (Source SDK 2013 public source, 2013) | `water_ps2x_helper.h`: reflection coordinate = projective coordinate + normal.xy × `$reflectamount` scale; `Water_ps14.psh` "perturb coords by constant displacement" | Read (public source, 2013 snapshot) |
| Johanson, "Real-time water rendering: the projected grid concept" (MSc, Lund) | 2004 | Normal-based offset of the reflection lookup, per Wargaming's background | Not re-read |
| Sousa, "Generic Refraction Simulation", GPU Gems 2 ch. 19 | 2005 | Bump-perturbed projective texture lookups for refraction and reflection | Not re-read |
| Lengyel, "Oblique View Frustum Depth Projection and Clipping", J. Game Development 1(2) | 2005 | The spike's oblique near plane | Not re-read; no patent found |
| Sousa, "Crysis Next-Gen Effects", GDC | 2008 | Planar water reflection with bump distortion | Not re-read |
| Wargaming US10002456 description | 2016 priority, 2018 published | Reflection ray about the surface normal → far-clip-plane intersection → reflection-texture UV (P2's reprojection, at infinite distance) | Description via Google Patents |
| Unreal Engine 4.12 planar reflections | 2016-06 | Planar reflection distorted by surface normals ("Normal Distortion Strength"); distance and angle-from-plane fades | Epic docs, release coverage |
| Blender EEVEE reflection planes | 2019 | Planar captures as SSR support buffers | Docs |

Every P1 element appears in Source (2004/2013) and in the expired Microsoft '148. P2's reprojection appears in Wargaming's 2016 description and is the reflection law plus a projective lookup. What remains Postretro's own is pixel style: a hashed whole-texel offset per cell driving both layers' tilt. No claim found reaches it.

## Workarounds

| # | Change | Removes | Visual cost | New risk |
|---|---|---|---|---|
| W1 | P1 instead of P2 | NetEase 1a/1b-first on the selected plane | Medium: near reflections over-shift, far ones under-shift; the planar/probe crossfade at D and R shows two shimmer amplitudes; the parity row cannot hold | None |
| W2 | P0 (no tilt) | Everything the tilt adds | High for the look: the selected plane reads as glass beside shimmering SSR planes, and its own probe term still tilts beneath | None |
| W3 | P2 with a fixed or far-plane distance instead of mirror depth | One depth tap | Medium: nearby posts and walls over-shift | Adds NetEase claim 2's element; not recommended |
| W4 | P2 with c taken from the untilted texel; colour from the tilted one | A per-pixel c change driven by the tilted lookup | Low-medium: a tilted lookup onto an empty mirror texel shows black at full weight | None; only if counsel reads Microsoft broadly |
| W5 | China only: invalidity route on NetEase CN114748874B | — | — | As the SSR check's W2 |

## Design rules

| # | Rule | Fence |
|---|---|---|
| D1 | Tilt moves only where the planar layer samples. c stays a function of coverage, D, R and the sampled texel's own content, never of the offset, the ray or the view | Microsoft |
| D2 | No branch, select, threshold or weight on any direction quantity. A reprojection outside the mirror frustum or behind the mirror eye clamps (w to a floor, UV to the target): a placement, not a decision | Microsoft |
| D3 | Tilt inputs: the cell's displayed offset, texel axes, flat plane normal, eye position for the reflection law, mirror depth. No Fresnel, s, probe colour, distance tile or probe table: the origin is the unpulled cell centre the composite reads, so `fluid_reflection_cell_within_box` is never called | Tencent cl. 15; spike row 4 |
| D4 | Compose the shared helpers (`fluid_displayed_offset`, `fluid_reflection_texel_axes`, `fluid_reflection_cell_centre`, `fluid_reflection_cell_normal`, `fluid_reflection_mirror`, `fluid_reflection_above_horizon`) for parity; never call `fluid_reflection_gate` or `fluid_reflection_seen_from_above` from planar code | Microsoft; parity |
| D5 | The probe lookup stays keyed to its own search; no planar position, depth or UV feeds it, and no probe value feeds the planar lookup | NetEase CN114748874B |
| D6 | Reprojection distance comes from the mirror depth, not a preset constant | NetEase cl. 2 |
| D7 | One mirror view per selected plane; never a view per cell, face or facet | Snap family; Tencent CN116778049B |
| D8 | The offset comes from the existing integer hash, never a sampled noise texture | ByteDance '408 |
| D9 | Smooth style applies no tilt, as SSR | Parity |

## Recommended path

**P2**, under D1–D9. It gives the selected plane the same ray SSR and the probe term use for each cell, so the shimmer matches across layers and across the D/R crossfade, which is what the brief's style decision asks for. Its risk read is low: the only direction it reads is the reflection law, which no charted claim reaches without a toward-camera determination. Its one added exposure, NetEase CN114748874B elements on the selected plane, is the posture SSR already holds on every other plane, and the same distinction (W1 of the SSR check) covers it. If the owner wants the smallest possible footprint and accepts a look that only approximates SSR's tilt, P1 is the fallback (W1 here).

## For counsel

- **NetEase CN114748874B** (carried, China only): whether a mirror render texture sampled at a reprojected position's UV is "an opaque texture declaration map", and whether "according to the reflection position" binds the second information. Certified text needed.
- **Snap / Adshir family** (US10699468, US10950030, US11302058, US12051145, US12573127): read the claims; confirm that a single plane-wide mirror view with a per-pixel tilted lookup lacks per-triangle secondary-ray projection; check whether any member claims planar surfaces. New to this brief.
- **ByteDance US12131408B2**: confirm "a background image captured in advance" cannot be read onto a per-frame mirror render.
- **Wargaming US10002456 / US10290142**: confirm no continuation claims the reflection-camera texture-coordinate method its description discloses.
- **Microsoft EP3756162** (carried): the text intended for grant, for any claim without the toward-camera element.
- **Perfect World CN112233216B, Q-Dazzle CN116206039A**: abstract only; read claims if China matters.

## Brief wording

Proposed for `index.md`; not applied.

**Decisions**, replacing the sentence "Whatever style-dependent shimmer tilt SSR applies in its own pass, the planar resolve applies to its lookup" and the IP row's last sentence:

> In pixel style the planar layer reflects each water texel cell about the same tilted normal SSR and the probe term use, built from the shared ray helpers, and samples the mirror view where that ray reaches at the mirror depth of the untilted texel. Tilt moves only where it samples: c stays a function of coverage, D, R and the sampled texel. Smooth style tilts nothing. The planar pass rasterizes the selected plane's fluid draws for per-cell inputs; a full-screen resolve cannot compute a cell's offset.

> The pixel tilt follows `ip-check.md` D1–D9: direction enters only the reflection law, with no branch, select or weight on it, and an off-frustum reprojection clamps; the tilt reads no Fresnel, s, probe colour, distance tile or probe table, and its origin is the unpulled cell centre; the offset is the existing cell hash; one mirror view per plane.

**Acceptance**, replacing the source-scan row and the pixel-style tilt row:

> - [ ] Shader source-scan IP tests cover the planar-authored shader text (not composed shared modules): no Fresnel term, no `fluid_probe` binding, no call to `fluid_reflection_gate`, `fluid_reflection_seen_from_above` or `fluid_reflection_cell_within_box`, and no direction quantity in a comparison or `select`. `dot(` is allowed only inside the composed, pinned helpers.
> - [ ] Pixel style: for the same cell, clock and texels-per-pixel, the planar pass's tilted ray equals the SSR pass's ray (CPU reference). On one GPU frame, an asymmetric reflected edge on the selected plane and on an SSR plane at the same height shifts in the same direction and by the same texel count, away from depth edges. Smooth style: the planar layer applies no tilt.
> - [ ] Tilt never changes c: at a fixed pose, c over the selected plane matches the same frame with the offset forced to zero, except where the tilted lookup lands on an empty mirror texel.
