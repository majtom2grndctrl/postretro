# glossy-world-reflections — IP check: blurred floor SSR

> **Engineering due diligence, not legal advice. 2026-11-22. Read at a1d143518.** A technical comparison of published claims and dated prior art against a proposed technique, to brief the owner and a patent attorney. "Appears present", "appears absent", "arguable" and "leans" are engineering readings of claim text. They are not opinions on infringement, validity or freedom to operate, which are counsel's. Every negative search result means "not found", never "clear".

Compared against: the owner's ruling of 2026-11-22 (glossy up-facing static floors gain a screen-space layer over their probe term, blurred, so a monument excluded from the floor probe's capture still reflects), `rendering_pipeline.md` §7.3a (Weight and composite, Screen-space layer, March, Confidence, IP rules), `resource_management.md` §5.2 (Exclusion), and `index.md` here. Builds on, and does not redo: `../dynamic-reflection-layer/ip-glow-ssr.md` (SSR on glossy world faces blurred by mip from the texel's exponent; Nintendo, Perfect World, '451 charts), `plans/done/fluid-ssr-reflections/ip-check.md` (the shipped layer: Tencent WO 15, EA, Microsoft, NetEase), `plans/done/reflection-probe-fluid-pass-performance/ip-check.md` §Shape B (reduced-resolution targets, the separation rule), and `plans/done/reflection-probes/ip-check.md` §1, §3, §4.

**Sources and confidence.** Claim text marked *verbatim* was extracted from Google Patents page HTML (machine translation for CN). Rows marked *summarised* went through a summarising fetch. Google Patents' query endpoint answered this session, so keyword sweeps ran on it in English, Chinese, Japanese and Korean, and with assignee filters; it rate-limited (HTTP 503) near the end. Hosts outside this session's network allowlist: `guerrilla-games.com`, `bartwronski.com`, `jcgt.org`, `advances.realtimerendering.com`, SlideShare, `ea.com`, Epic's docs, GDC Vault, `web.archive.org`, Espacenet. Papers and talks therefore rest on search excerpts or prior records unless a row names source code read. GitHub source was read for Unity's Post-processing Stack v1 and v2 and for Godot 3.0, with commit dates from git history. Filings from the last 18 months are invisible to any search.

## Technique under review

| # | Element |
|---|---|
| F1 | **Receivers.** Probe-bound, up-facing, static glossy world faces only; the layer reuses the probe term's gate, so a face with no probe, and any mover face, gets none |
| F2 | **March.** Water's shipped `screen_march` unchanged: fixed stride, fixed step budget, interval hit test, in-front latch, no direction-dependent step (§7.3a March, IP rules). One ray per target pixel or texel cell |
| F3 | **Target.** A separate reflection pass writes premultiplied (c·X, c) to a target at a reduced fraction of the scene extent, cleared every frame |
| F4 | **Blur.** The hit colour is blurred to a width picked per texel from the floor texel's Blinn-Phong exponent, never below an engine-owned minimum. Variants: **(i)** a mip pyramid of the SSR target, read at a level from the exponent; **(ii)** a mip chain of scene colour, the hit sampled at a level from the exponent and hit distance (cone footprint); **(iii)** a fixed separable blur of the SSR target, no per-texel selection |
| F5 | **Confidence.** c = reach fade × screen-edge fade, as shipped. No direction term |
| F6 | **Composite.** One overlay pass: reflected `F·[c·X + (1−c)·s·R]`, weight `W = F·[c + (1−c)·s]`; s the probe's capture-shadow weight, R the probe colour |
| F7 | **Exclusion.** A monument small relative to its floor is excluded from the floor probe's primary rays at compile time (§5.2 rule), so the probe shows what stands behind it and SSR shows the monument |
| F8 | No history, no ray tracing, nothing stored written, probes placed and captured at compile time |

What differs from the two prior charts. From water (`fluid-ssr-reflections`): the receiver is an **opaque floor**, not water; the hit colour is **blurred** (F4); **exclusion** now serves floor probes (F7). From glow SSR (`ip-glow-ssr.md`): no glow-only group target — the layer marches and reads the **ordinary scene**; its term sits **beside** the probe by confidence, outside s (water's W0 shape), not inside W; there is no CPU frame gate. Glow SSR's readings that turned on "colour from a glow target, not the frame buffer" therefore do not carry; this chart re-reads them.

## Summary

| Reference | Status as shown | Closest claim and element | Reading | Confidence |
|---|---|---|---|---|
| **Perfect World CN112233216B** (new) | CN granted 2021-03-02, active ≈2040-12-18; WO2022127242A1 ceased, no national entry seen | 1: per-plane reflection with authored "reflection threshold and noise intensity"; Gaussian blur; **Mipmap of the reflection map, level per pixel from material roughness**; screen UV offset by normal and noise intensity | **Closest filing to variant (i).** Mip-by-roughness of the reflection result appears present for (i). Appears absent on the authored per-plane structural data and the normal/noise UV perturbation. Arguable on "reflection calculation for each reflection plane" | Medium (verbatim machine translation; CN only) |
| **NVIDIA US20240257439A1** (18/612,293) (new) | **Notice of allowance mailed 2026-07-07 and 2026-08-03**; issue fee not yet shown paid; priority 2018-03-17 | Published 1: "selecting a specular model from a plurality of specular models" by a surface's reflective-property values; 3: threshold; 6: lobe sampling versus mirror | **Allowed claims unread.** Published claim 1 is very broad. A sharpness floor deciding which faces reflect is arguable as "selecting a specular model"; a continuous exponent→width function selects nothing. Leans absent on the published text; counsel to read the allowed claims | Low (allowed text unknown) |
| NVIDIA US10776985B2 / US11941745B2 (same family) | Granted, active | '985 1: anisotropic kernel sized and oriented from the BRDF lobe's reflection projected below the surface along the view vector. '745 1: roughness thresholds select biased BRDF-lobe sampling | Appear absent: isotropic width from the exponent (and hit length); no lobe sampling, no oriented kernel | Medium-high |
| Tencent WO2026113651A1 claim 15 | Pending; US ≈2027-05-28, EP ≈2027-06-28 | "If there is an obstruction … supplement" the probe's information | Unchanged from water **provided the blur reads no s**. Variant (ii) spreads nothing; (i) and (iii) spread c into neighbouring pixels, which stays s-independent only under fence B1 below. Exclusion (F7) removes an occluder at compile time; nothing is conditioned on a detected obstruction | Medium (pending; machine translation) |
| NetEase CN114748874B | Granted, CN only | 1: **water** pixel's reflection position; opaque texture plus preset environment; Fresnel | **Further away than water.** The claim recites "each pixel on the water surface"; a floor is not water. Scene colour as the first information is present; W1 still holds | Low-medium (unverified translation) |
| Nintendo US12694606B2 (granted) / US20250245909A1 | Granted 2026-07-28 / pending | '606 1: correct the reflection direction toward the screen interior; colour from the frame buffer. '909 1: second depth buffer for designated objects | Frame-buffer colour now **present** (glow SSR's distinction is gone). '606 stays absent on the direction correction; '909 on the second depth buffer | Medium |
| Perfect World CN112973121B | CN granted, active | 1: one step of preset distance; blend screen colour with a preset static reflection | Blend element now present (scene colour over a baked probe). Single-step element appears absent | Medium |
| Microsoft US10586375B2 / US10360720B1 | Granted | Reflected-toward-camera determination | Appear absent while F2 and F5 stay direction-blind. Hit length may pick a mip; no direction may. Reduced-resolution dependents fall with claim 10 | High |
| EA US10621774B2 / '474 / '079 | Granted | Ray count from roughness; neighbour reuse gated on roughness similarity; temporal reuse | Appear absent: the exponent picks a blur width, never a ray count; no history. A blur weighted by neighbours' exponent similarity moves toward '474 | High |
| Square Enix US9342926B2 | Granted | Low-resolution specular buffer upsampled by lobe-similarity weights | Appears absent while the read stays bilinear or one nearest load (Shape B fence) | Medium-high |

Bottom line: the nearest new item is a granted, CN-only Perfect World patent whose claim 1 includes variant (i)'s core step (a mip chain of a screen-space reflection result, level per pixel from roughness), kept apart by two elements the floor design lacks. The nearest open unknown is NVIDIA's allowed, unpublished claim set descended from an extremely broad published claim 1. The shipped layer's Tencent, Microsoft, EA and NetEase readings carry over; the blur needs one new fence (B1) to keep the WO 15 reading, and variant (ii) needs none.

## Documents found

| Document | Applicant | Priority | Published / granted | Status as shown (Google Patents, assumed) | Source |
|---|---|---|---|---|---|
| **CN112233216B** (A: CN112233216A 2021-01-15; PCT WO2022127242A1) | Chengdu Perfect World; inventor Jiang Boyao | CN 2020-12-18 | B 2021-03-02 | CN active ≈2040-12-18; WO ceased, no national entry listed. Cited by CN112973121B | Claims 1–3 verbatim (machine translation); 4–7 summarised |
| **US20240257439A1** (US 18/612,293), continuation of US11941745B2 | NVIDIA (Liu, Wyman, Hasselgren …) | US provisionals 2018-03-17 | A1 2024-08-01; NOA 2026-07-07, re-mailed 2026-08-03 | Allowed, not yet issued; a sibling 18/746,536 pending | Published claims 1–9 verbatim; legal events read |
| US10776985B2; US11941745B2 | NVIDIA | 2018-03-17 | Granted | Active | Claim 1 verbatim each |
| CN114327387A | China National Machinery Industrial Internet Research Institute (Henan) | CN 2021-12-21 | 2022-04-12 | Shown granted | Claim 1 verbatim: CesiumJS shader extraction, SSR by roughness from a rendered `roughTexture` |
| CN117036577A | Lijie (Shanghai) | CN 2023-07-05 | 2023-11-10 | Shown granted | Claim 1 verbatim: coarse step, then finer step around the candidate |
| US20250371799A1 (CN116672706B) | Tencent | CN 2023-08-03 | A1 2025-12-04 | Pending | Claim 1 verbatim: probe rays; on-screen hits from screen space, off-screen from a surfel/voxel library |
| WO2019195551A1 / EP3743893A1 | Google | 2018-04-06 | 2019-10-10 | WO ceased; EP not read | Claim 1 verbatim: environment map plus **camera feed** at the reflection intersection; claim 5 blurs when scattering |
| CN111738902A | AMD (Shanghai) | 2020-03-12 | 2020-10-02 | Deemed withdrawn 2024-05-24 | Summarised |
| Carried, not re-read | Tencent WO2026113651A1, '451; NetEase CN114748874B; Nintendo US12694606B2, US20250245909A1; Perfect World CN112973121B; EA, Microsoft, Square Enix families | — | — | As in the prior records | Prior records |

URLs: `https://patents.google.com/patent/<number>/en`.

Not found: any filing claiming SSR blurred by a mip pyramid **composited over a reflection probe by confidence**, a **minimum** blur width, a cone-footprint read of a scene-colour mip at the SSR hit, or SSR restricted to **floors** or up-facing surfaces. Assignee-filtered sweeps (reflection + roughness/gloss + screen space/frame buffer + blur/mip/cone) returned nothing on point for Sony, Ubisoft, Activision, Epic, Unity, Square Enix, Nintendo, AMD, Intel, Qualcomm, Arm, Apple, Samsung or Bungie. Keyword sweep only; not a clearance search.

## 1. Perfect World CN112233216B (new)

Claim 1 (verbatim, Google machine translation), abridged with …: "acquiring a plurality of reflection planes defined in a game scene, wherein the reflection planes comprise planes with roughness; acquiring structural data of the reflection plane, wherein the structural data comprises: a reflection threshold and a noise intensity …; performing reflection calculation according to the structure data, the projection data of the current camera, the pixel information of the current screen and the depth map information of the current screen space … executing reflection calculation aiming at each reflection plane …; acquiring a target chartlet containing roughness and normal information …; firstly, noise reduction processing … through a Gaussian fuzzy algorithm; then carrying out Mipmap processing on the map information to obtain a plurality of Mipmap maps with different definitions; … determining a Mipmap of corresponding definition of each pixel point … according to the roughness …; calculating texture mapping coordinates according to the current screen coordinates …; performing superposition calculation on the texture mapping coordinates according to the normal direction and the noise intensity …; sampling … after superposition calculation to realize disturbance processing; and finally, rendering". Claim 2 adds a per-plane specular transformation matrix (screen-space planar reflection, not a march).

| Element | Floor SSR | Reading |
|---|---|---|
| Plural reflection planes defined in the scene, with roughness | Glossy up-facing faces, grouped per coplanar probe group; per-texel exponent | Arguably present |
| **Structural data: a reflection threshold and a noise intensity per plane** | No per-plane threshold or noise value. The sharpness floor is per material texel, and nothing carries a noise intensity | **Appears absent** |
| Reflection from camera projection, screen pixels and screen depth, per plane | One march per fragment from the depth buffer, not a per-plane pass | Present in substance; "each plane" arguable |
| Gaussian blur, then a Mipmap chain of the reflection map | (i): a pyramid of the SSR target, Gaussian or box. (ii): a chain of scene colour, not of a reflection map. (iii): one fixed blur, no chain | (i) **present**; (ii) arguable (a scene-colour chain is not a "map containing a reflection result"); (iii) absent |
| Mip per pixel from material roughness | Level from the texel exponent (a gloss analogue) | Present for (i) and (ii); absent for (iii) |
| UV from screen coordinates, **offset by normal direction and noise intensity**, then sampled | Smooth style reads the target at its own screen position; pixel style at its texel centre's projection. No offset | **Appears absent** while the read carries no normal- or noise-driven offset |

CN only; China matters only for distribution there. Its WO is ceased with no national entry listed. Medium confidence. As art, it dates variant (i) to 2020-12 at the latest, behind Unity PPv1 (2016) and PPv2 (2017).

## 2. NVIDIA "Reflection denoising in ray-tracing applications" (new)

US18/612,293 published as US20240257439A1. Claim 1 (verbatim, as published): "determining one or more values representative of a reflective property for a surface based at least on a point …; selecting a specular model from a plurality of specular models based at least on the one or more values; based at least on the selecting, using the specular model to determine lighting condition data for the point; and rendering one or more images". Dependents: 3 selection by thresholds; 4 glossy → "a specular reflection model"; 5 roughness; 6 "performing sampling of a … BRDF lobe … or evaluating the point as a mirrored surface"; 7 a reflected ray. One non-final action (2026-02-12), a response (2026-06-12), then allowance. **The allowed claims were not seen**; an amendment is likely.

| Element (published) | Floor SSR | Reading |
|---|---|---|
| Reflective-property value at a surface point | The texel exponent; the material's sharpness | Present |
| **Selecting a specular model from plural models by that value** | Every texel keeps the Blinn-Phong highlight. A sharpness floor decides whether a face binds a probe and draws the overlay at all; within the overlay, one continuous function maps exponent to width, mip 0 included | **Arguable on the floor**, leaning absent: an added term gated by material is not obviously a choice between "specular models". Absent within the overlay |
| Threshold (3); mirror versus lobe sampling (6) | Floor threshold present; no lobe sampling; the mirror cap is the same function's end | 3 arguable; 6 absent |

Granted parents: '985 claim 1 needs "an anisotropic filter kernel of a size and orientation" from "a cross-section, at the virtual screen, of a projection of a reflection of the BRDF lobe below the surface along the view vector"; '745 claim 1 needs roughness thresholds selecting "sampling a portion of a … BRDF lobe … according to a sampling bias". Floor SSR has an isotropic width from the exponent (and, in (ii), hit length), no oriented kernel, no lobe sampling: **appear absent**. Art against a broad '439 claim: PPv1's 2016 composite skips `smoothness == 0` and switches roughness filters; UE4's SSR "Max Roughness" fade (2014, docs excerpt).

## 3. Shipped-layer filings: what the blur and the floor change

**Tencent WO2026113651A1 claim 15.** The water reading (`fluid-ssr-reflections` §1) rests on one fact: SSR's coefficient contains no s, so nothing is added because obstruction fired. The blur touches that fact in two ways. (a) In (i) and (iii) the blur spreads c·X and c from pixels whose rays hit into pixels whose rays missed. Done by a fixed kernel or a level read, that is s-independent and the reading holds. Done by a kernel that widens, renormalises or fills where the probe term is withheld, it becomes "if obstruction, supplement" in effect and arguably in structure. (b) If (i) or (iii) blurred a target that also held the probe term, s would enter the stored SSR share: the Shape B separation rule. Variant (ii) blurs scene colour at the hit only; c stays the hit's own, and a missed pixel stays c = 0. **Appears absent for all three variants under B1–B2; (ii) is the most robust.**

**Exclusion (F7) against claim 15.** Exclusion removes the monument from the capture by a compile-time size rule, so the probe sees no obstruction there and the capture-shadow test does not fire on it. Nothing is detected at runtime and nothing is supplemented upon detection; SSR runs on every gated fragment whatever was excluded. **Appears absent.** One drafting risk: §5.2 says excluded brushes "reflect only through the screen-space layer". Read alone, that sounds like screen space completing what the probe lacks. Counsel may want the floor docs to state the rule as size-keyed and runtime-blind, as §5.2's "Runtime-blind" bullet already does, and never as a fill.

**Glossy brief row conflict (not IP, noted).** `index.md` Acceptance requires "no fill from SH, lightmap, screen space or another probe" behind a pillar, and its Decisions say "Probe-only on world faces". The ruling changes both. The row should become water's: the probe term draws nothing behind the pillar; SSR's contribution there is the same at every s.

**NetEase CN114748874B.** Variant reads as water's, except the claim's "each pixel on the water surface" (PatSnap translation). A floor has no water surface. Appears absent on that element on a plain reading; W1 still applies.

**Nintendo, Perfect World CN112973121B.** Unlike glow SSR, the colour on a hit now comes from the frame buffer (scene colour), and the screen term blends with a baked static value. Both filings still need an element the design lacks: '606 the ray bent toward the screen interior; '909 a second depth buffer; CN112973121B one step of preset distance.

**Microsoft.** A cone footprint in (ii) reads hit length and the hit's view depth: a position, not a reflected-toward-camera determination. Neither may gate, skip or re-weight a ray. PPv1 and PPv2 both multiply in a ray-direction term; F5 omits it.

**EA, Square Enix.** The exponent picks a width, never a ray count ('774). A blur whose weights compare neighbours' exponent or normal (PPv1's bilateral `roughnessWeight`, 2016) moves toward '474's roughness-gated reuse and Square Enix '926's lobe-similarity upsampling; both are avoidable at no visual cost the owner asked for.

**'451 (c), WO 10–14, 19.** No precomputed transfer; the blur is a per-frame screen operation on a cleared target; no probe tile is read by the march or written by anything. Unchanged from `ip-glow-ssr.md` §4 and the probe record.

## 4. Others screened

| Item | Why it does not appear to read |
|---|---|
| CN114327387A (CesiumJS SSR) | Claim 1 needs CesiumJS shader extraction and an off-screen `roughTexture` pass |
| CN117036577A | Two-step search: coarse step, then a finer step around the candidate. F2 has one fixed stride and no refinement |
| Tencent US20250371799A1 | Probe rays split by screen hit; off-screen hits read a surfel or voxel library. No probe ray is traced here |
| Google WO2019195551A1 / EP3743893A1 | Second contribution from a live **camera feed** (AR); WO ceased; EP text not read |
| AMD CN111738902A | Convolution-kernel fitting; deemed withdrawn |
| NVIDIA CN112868047B, US12182927B2 | Spatiotemporal or recurrent denoising of ray-traced signals; history (F8 has none) |
| Arm US10089776B2, CN107430784B, US10636213B2 | Bounding-volume cube textures for transparency, refraction and local reflection; no screen march |

## 5. Prior art (dated, public)

Priorities to beat: Microsoft 2018-02-22; NVIDIA 2018-03-17; EA 2018-08-10; Perfect World 2020-12-18; NetEase 2022-04-02; Tencent WO 2024-11-28.

| Publication | Date | What it discloses, as we read it | Variant | Evidence |
|---|---|---|---|---|
| Sousa, Kasyan & Schulz, CryENGINE 3 (Crysis 2 DX11), SIGGRAPH | 2011 | Glossy real-time local reflections in screen space over cubemaps | Glossy SSR | Secondary sources only |
| Killzone Shadow Fall (shipped) / Valient, GDC | 2013-11 / 2014-03 | Half-resolution screen march; output "hit color / hit mask / glossiness"; "generate mip chain from ray-trace results"; compose with cubemaps, mask blending; each stage falls back to the next on miss | **(i)**, F3, F6 | Prior record read the slide PDF; the mip-chain line is a search excerpt |
| Schulz, Ryse, GDC | 2014-03 | Mirror trace, then convolved versions "by repeated downsampling and gaussian filtering", selected by roughness | (i) | Secondary summaries only |
| Wronski, "The future of screenspace reflections" / "GDC follow-up: … filtering and up-sampling" | 2014-01-25 / 2014-03-23 | "blur with varying radius (according to surface roughness)"; weighted separable blur; blur fills holes from missed rays; fade to cubemaps | (iii) and per-texel width | Search excerpts (host unreachable) |
| Uludag, "Hi-Z Screen-Space Cone-Traced Reflections", GPU Pro 5 | 2014-05 | Cone from material roughness widening with travel; mip-filtered colour buffer | **(ii)** | Search excerpts |
| Hermanns & Franke, "Screen Space Cone Tracing for Glossy Reflections", SIGGRAPH Posters 102 | 2014-08 (VAMR 2016 paper) | Cone tracing over prefiltered screen buffers | (ii) | Bibliographic records; content excerpt |
| McGuire & Mara, JCGT 3(4) | 2014 | Depth-buffer march | F2 | Abstract |
| Stachowiak & Uludag, "Stochastic Screen-Space Reflections", SIGGRAPH Advances | 2015-08 | Per-pixel roughness; pre-filtered image pyramid; cone footprint; half-resolution trace; probe fallback | (ii), F3 | Search excerpts |
| **Unity Post-processing Stack v1**, `ScreenSpaceReflection.shader` (GitHub `v1`, commit d5d5488) | **2016-10-10** | Half-resolution option; reflection buffer downsampled and blurred into mips (bilateral, roughness- and normal-weighted); composite mip from a footprint = roughness^(4/3) × ray distance ÷ hit depth; `lerp(specEmission, ssrResult, confidence)` over Unity's reflection probes; skips `smoothness == 0` | **(i) with cone footprint**, F3, F6 | **Source read** |
| **Unity Post-processing Stack v2**, `ScreenSpaceReflections.hlsl` (commit 34068b3) | **2017-08-30** | Gaussian pyramid of the resolve target; `SampleLevel(…, SmoothnessToRoughness(...) * (_BlurPyramidLODCount - 1.0) + 1.0)`; `lerp(reflectionProbes.rgb, resolve.rgb, confidence * fade)`; also temporal reprojection and a direction term | **(i)**, F6 | **Source read** |
| **Godot 3.0** `screen_space_reflection.glsl` (tag 3.0-stable) | **2018-01-29** | "use a blurred version (in consecutive mipmaps) of the screen to simulate roughness"; cone angle from roughness; `textureLod(source_diffuse, sample_pos, mipmap)` with mip = log2(cone radius) | **(ii)** | **Source read** |
| PostRetro's shipped water SSR | Landed | F2, F3, F5, F6 | — | `fluid-ssr-reflections/ip-check.md` |

**Strongest dated art.** For (i), exponent-picked level of a blurred SSR pyramid, over probes by confidence: Unity PPv2 (2017-08-30) element for element, and PPv1 (2016-10-10) with the cone footprint; both predate NVIDIA, EA and Perfect World. For (ii): Godot 3.0 (2018, source), Uludag 2014 and Stachowiak 2015 (excerpts). For (iii): Wronski 2014 (excerpts). Shipped before 2014: Killzone Shadow Fall (2013-11) and Crysis 2 DX11 (2011), both on secondary evidence for the blur.

## 6. Design choices that move the reading

| Choice | Moves | Why |
|---|---|---|
| (ii) scene-colour mip at the hit | **Safest for WO 15**; neutral elsewhere | Nothing spreads into missed pixels; c stays the hit's own. Arguable against CN112233216B's "Mipmap of the map information" |
| (i) pyramid of the SSR target, level from exponent | Toward CN112233216B (one element short twice) | Its mip-by-roughness step. Art: PPv2, PPv1 |
| (iii) one fixed blur | Away from CN112233216B and NVIDIA '439 | No per-pixel selection; drops the owner's width-from-exponent |
| Read offset by normal map or noise | **Toward CN112233216B** | Its "disturbance" element |
| Per-plane authored threshold or noise value | Toward CN112233216B | Its structural data |
| Blur width or weights reading s, the distance tile, a capture-shadow flag or probe colour | **Toward WO 15** | Fill keyed to obstruction |
| One target holding probe and SSR, then blurred | Toward WO 15 | s enters SSR's stored share (Shape B) |
| Kernel weights from neighbours' exponent or normal similarity | Toward EA '474, Square Enix '926 | Roughness- or lobe-gated reuse |
| Anisotropic kernel oriented by view and normal | Toward NVIDIA '985 | Its claim 1 |
| Exponent driving ray count, stride or step budget | Toward EA '774 | Keep F2 fixed |
| Direction term in c or in the level (PPv1, PPv2 style) | Toward Microsoft | Keep F5 |
| Hit length in the level | Neutral | A length; Microsoft needs a direction |
| Ray bent toward the screen interior near edges | Toward Nintendo '606 | Fade the term instead |
| Temporal accumulation to hide a reduced target's shimmer | Toward EA '079 | F8 |
| Engine-owned minimum blur | Neutral | No claim found recites a floor on blur |

## 7. Fences

| | Rule | Avoids | Note |
|---|---|---|---|
| **Carry** | §7.3a IP rules unchanged: direction-blind march and c; SSR never reads s, the distance tile or probe colour; the probe lookup keyed to its own search | Microsoft; WO 15; NetEase | The pass binds no probe colour or distance array |
| **New B1** | The blur's width, level and weights read the texel's exponent, the engine minimum and, in (ii), the hit length only. Never s, the distance tile, a capture-shadow flag, probe colour, or c's absence; the blur never widens, renormalises or fills where the probe term is withheld | WO 15 | Pin as a test: the blurred (c·X, c) is bit-identical at s = 0 and s = 1 |
| **New B2** | Blur only a target that holds the screen-space term alone; the probe term is never stored in, or blurred with, the SSR target | WO 15 (Shape B rule) | — |
| **New B3** | The blurred target is read at the fragment's own screen position (smooth) or its texel centre's projection (pixel), with no normal- or noise-driven offset; no per-plane authored threshold or noise parameter | CN112233216B | China only |
| **New B4** | Isotropic blur: no kernel oriented by view vector, normal or lobe projection; no weights from neighbours' exponent or normal similarity; upsampling bilinear or one nearest load | NVIDIA '985; EA '474; Square Enix '926 | — |
| **Keep** | One ray per target pixel or cell; the exponent sets blur only, never ray count, stride or budget | EA '774 | — |
| **Keep** | Exclusion stays a compile-time size rule, runtime-blind; docs never describe SSR as completing or filling what a probe lacks | WO 15 drafting | §5.2 "Runtime-blind" |
| **Keep** | Floors only where a probe binds a static face; no mover face, no sums on movers | Sums fence; '451 cl. 5–6 | F1 |
| **Avoid** | Variant (i) with a normal-perturbed read | CN112233216B | — |
| **Watch** | NVIDIA 18/612,293 issue and allowed claims; sibling 18/746,536 | '439 | — |
| **Watch** | Tencent WO national phase; Nintendo '909 prosecution | Carried | — |

**Water's SSR over a glossy floor (owner, 2026-11-22).** The overlay writes glossy floors' composited result into scene colour before water's screen-space pass reads it, so water reflecting an on-screen glossy floor shows that floor's reflection. Engineering reading: this is scene content, the lit result of another surface, not a read of water's probe tiles; water's pass still binds no probe colour or distance array, and water's coefficient never gains water's s. The owner accepted this reading.

## 8. For counsel

1. **NVIDIA US20240257439A1 (18/612,293), allowed.** Read the allowed claims and the 2026-06-12 response. If claim 1 still reads "selecting a specular model … based at least on" a reflective-property value: is a material sharpness floor that decides whether a face draws an added image-based term such a selection? Art: Unity PPv1 (2016-10-10, `smoothness == 0` skip, source), UE4 SSR Max Roughness (2014, docs).
2. **Perfect World CN112233216B.** Confirm that claim 1 needs per-plane "reflection threshold and noise intensity" data and the normal/noise UV "superposition", and that "each reflection plane" does not read on a per-fragment march. China only. Validity art: Unity PPv1 and PPv2 (source, 2016–2017), Killzone 2014.
3. **Tencent WO 15.** Confirm that a blur of the screen-space term whose width reads only the exponent and hit length (B1), spreading c into missed pixels irrespective of s, is not "supplementing" upon obstruction; and that compile-time size exclusion (F7) is outside 15. Whether to keep variant (ii) for robustness.
4. **NetEase CN114748874B.** Whether "each pixel on the water surface" (certified text) limits claim 1 to water, which would put floors outside it.
5. **Nintendo US12694606B2.** Unchanged question: confirm an edge fade is not the claimed direction correction, now that frame-buffer colour is present.

## Not accessed

Talk and paper sources: the Killzone, Ryse and Crytek slide decks, Wronski's posts, GPU Pro 5, the JCGT paper, the 2015 Advances slides and UE4 4.0 SSR source (hosts outside the allowlist, or access-controlled); rows rest on search excerpts or prior records. Unity's `cinematic-image-effects` history (GitHub refused). Espacenet, EP Register, Patentscope and USPTO file wrappers (sign-in or refusal). NVIDIA's allowed claim text and sibling 18/746,536; Google EP3743893A1 claims; CN112233216B dependents 4–10 verbatim. Google Patents began returning HTTP 503 during the final sweep (blur radius, kernel size, specular exponent queries in English), so those ran only in Chinese.
