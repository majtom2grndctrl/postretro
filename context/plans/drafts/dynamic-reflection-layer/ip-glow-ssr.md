# glossy-world-reflections — IP check: selective glow SSR

> **Engineering due diligence, not legal advice. 2026-11-21. Read at 2678c8a91.** A technical comparison of published claims and dated prior art against a proposed technique, to brief the owner and a patent attorney. "Appears present", "appears absent", "arguable" and "reads on" are engineering readings of claim text. They are not opinions on infringement, validity or freedom to operate, which are counsel's. Every negative search result means "not found", never "clear".

Compared against: the technique as the owner described it for the glossy brief (below; `research.md` §Glow SSR was being written in parallel and was not read), `index.md` here, `rendering_pipeline.md` §7.3a (Screen-space layer, its IP rules, Reflection quality tiers). Builds on `plans/done/fluid-ssr-reflections/ip-check.md` (the fluid SSR record: Tencent WO claim 15, EA, Microsoft, NetEase, Unity PPv2, Killzone), `plans/done/reflection-probes/ip-check.md` (Tencent WO chart and §4 fences), `ip-delta-tiles.md` here (the sibling rival), and `context/research/gi-rtx-lite/ip-check.md` §Tencent US12293451B2. It does not redo their charts; it reads only what this technique changes.

**Sources and confidence.** Google Patents pages were read through a summarising fetch, which returns paraphrase and short quoted fragments, not verbatim claims. Each row's *Source* column says which. Google Patents' search page renders by script and returned nothing; a direct query of its search endpoint was refused in this session, so there was no CPC- or assignee-filtered sweep. Discovery was by web search in English and Chinese. `docs.unity3d.com` is outside this session's network allowlist (DNS refusal), so Unity HDRP features rest on search excerpts. Filings from the last 18 months are invisible to any search.

## Technique under review

| # | Element |
|---|---|
| G1 | **Glow pass.** After forward, a pass redraws only selected faces (animated `light_area` glass; `emissive_volume` faces whose presented value differs from reference) with depth `Equal` into a small screen target. Each pixel holds that face's glow **change from reference** (current − reference), not its full glow. Forward is untouched. |
| G2 | **Probe term** unchanged: base tile with glow at its reference state; live cast light by the existing post-sample per-light share ratio. |
| G3 | **March.** In the glossy overlay pass, glossy fragments march the current frame's depth buffer along the reflected ray under a fixed step budget (the reflection-quality tiers). |
| G4 | **Hit read.** On a hit, sample the glow-change target at a mip chosen from the texel's Blinn-Phong exponent and the hit distance, and add it to the probe term. |
| G5 | **Miss.** Off screen, occluded, past the budget, or a hit on an unselected surface: the change term is 0, so the reflection shows the probe's reference glow. The change term fades to 0 at screen edges. |
| G6 | **Gate.** The pass is skipped on frames where no selected source is in view and off reference (known on the CPU). |
| G7 | **Capture shadow.** Never drawn where the probe's capture-shadow test withholds the term: the overlay draws nothing there. |
| G8 | No history or temporal accumulation; no baked per-source data. |

What differs from the shipped fluid SSR (`fluid-ssr-reflections/ip-check.md` D1–D8): the source is a **separate target holding only selected faces' glow change**, not the scene colour; the result **adds to** the probe term instead of blending with it by confidence; **blur by mip** from exponent and hit distance; **frame-level CPU gating**; and the term sits **inside the capture-shadow weight** (G7), where fluid SSR deliberately sits outside it.

## Summary

| Reference | Status as shown | Closest claim and element | Reading | Confidence |
|---|---|---|---|---|
| Tencent WO2026113651A1 / CN122115679A | Pending; US ≈2027-05-28, EP ≈2027-06-28 | WO 10 / 19: "update the illumination information" the probe collected when light changes | **Nearest-risk element.** Nothing stored is written; a live correction is added after sampling, as the shipped ratio is. Appears absent, arguable. Claim 15 moves further away than fluid SSR: the term is withheld under obstruction, never added | Medium (pending; machine translation) |
| Tencent US12293451B2 ('451) | Granted, active | 1(c): indirect from a precomputed "radiance transfer parameter" × coefficient | **Appears absent.** The reflected path is found at runtime by marching; nothing precomputed is scaled by the glow's state | Medium-high |
| NetEase CN114748874B | Granted, CN only | 1: first information from the "opaque texture" at the reflection position; second from preset environment reflection; Fresnel; combine | Further from it than fluid SSR: the screen information comes from a glow-change target, not the opaque scene texture. W1 still applies | Low-medium (unverified translation) |
| Nintendo US12694606B2 | **Granted 2026-07-28**, active, ≈2045-01-11 | 1: correct the reflected ray toward the screen interior; colour from the frame buffer at the hit | Appears absent on both elements | Medium (claims summarised) |
| Nintendo US20250245909A1 | Pending | 1: second depth buffer of designated objects for rays past a first distance; colour from frame buffer | Appears absent: one depth buffer; selection picks the **colour** source, not the depth tested | Medium |
| Nintendo US20250242255A1 | Pending | 1: a character drawn to a textured flat proxy; reflections hit the proxy | Appears absent | Medium |
| Perfect World CN112973121B (WO2022227553A1 ceased) | CN granted 2021-07-20, active ≈2041-04-30 | 1: **one step of preset distance**, depth compare, blend dynamic (screen) with static (preset) reflection | Appears absent on the single-step element; the blend element is arguable | Medium (claims summarised) |
| EA US10621774B2 / US10796474B2 / US10818079B2 | Granted, active | Roughness-driven ray count; ray tracing on viewport exit; neighbour or temporal reuse | Appear absent; exponent picks a mip, never a ray count or reuse | High |
| Microsoft US10586375B2 / US10360720B1; EP3756162A1 | Granted; EP intention to grant 2026-07-16 | Reflected-toward-camera determination; ray tracing among techniques | Appear absent while the march stays direction-blind | High (US); low (EP text unread) |

Bottom line: the nearest risk is the same pending Tencent WO update chain (claims 10 and 19) the probe term and delta tiles already sit beside, on the same footing as the shipped ratio. The technique appears to stay inside the existing fences, and appears to stay outside '451 element (c). The design's own choices push away from the fluid record's sharpest item (WO 15) and from NetEase. The new filings found (a granted Nintendo SSR family, a granted Perfect World CN patent) each need an element the design lacks.

## Documents found

| Document | Applicant | Priority | Published / granted | Status as shown (Google Patents, assumed) | Source |
|---|---|---|---|---|---|
| WO2026113651A1 / CN122115679A | Tencent | CN 2024-11-28 | 2026-06-04 / 2026-05-29 | Pending | Prior records (claims 1, 10–19 charted); not re-fetched |
| US12293451B2 | Tencent | CN 2021-04-02 | Granted 2025-05-06 | Active; ≈2043-01-19 | Prior record (claim 1 verbatim, USPTO text) |
| CN114748874B | NetEase | 2022-04-02 | B 2025-07-22 | Active, CN only | Prior record (PatSnap translation, unverified) |
| US10621774B2, US10796474B2, US10818079B2; KR102811807B1; CN110874858B | Electronic Arts (Uludag) | 2018-08-10 | Granted 2020 | Active | Prior record (claim 1 verbatim for '774) |
| US10586375B2, US10360720B1; EP3756162A1; CN111788608B | Microsoft | 2018-02-22 | Granted; EP Rule 71(3) 2026-07-16 | Active; EP grant intended | Prior record |
| **US12694606B2** (A1 US20250245907A1; US 18/883,392) | **Nintendo**; inventor Fujisawa | JP 2024-011579/-580/-581, 2024-01-30 | A1 2025-07-31; **granted 2026-07-28** | Active; adjusted expiry 2045-01-11. JP members JP7698754B1, JP7698755B1, JP7851339B2 | Independent claims 1, 9, 17, 25 and dependents 2–8 summarised with short quotes |
| **US20250245909A1** (US 18/883,274) | Nintendo | Same JP priorities | 2025-07-31 | Pending | Claim 1 summarised with a quoted fragment; 6, 11, 16 parallel |
| **US20250242255A1** (US 18/883,406) | Nintendo | Same JP priorities | 2025-07-31 | Pending | Claims 1, 4, 7, 10 and dependents summarised |
| **CN112973121B** (from CN112973121A; PCT WO2022227553A1) | Chengdu Perfect World | CN 2021-04-30 | B 2021-07-20 | CN active, ≈2041-04-30; **WO ceased** (no national entry seen) | Claims 1, 7–9 summarised with short quotes |
| US12469201B2 | NetEase | CN 2021-03-09 | Granted 2025-11-11 | Active | Summary only: SDF-guided marching for indirect light. Different subject |
| US12354210B2 | Electronic Arts | 2022-03-31 | Granted 2025-07-08 | Active | Prior record: needs fixed-distance rays and ray tracing |
| US11443475B2 | NVIDIA (Akenine-Möller) | — | Granted | Not checked | Search excerpt: mip level from ray-cone spread at a traced hit; adjacent, not screen space |

URLs: `https://patents.google.com/patent/<number>/en` for each.

Not found: any filing claiming screen-space reflection of an **emissive-only or glow-only** buffer, reflection of a glow **change** added over a baked probe, SSR whose colour source is restricted to a selected object group, or SSR mip selection from roughness and hit distance. Searches: web search in English and Chinese on each of those, and citation lists on the pages above. Weak evidence of absence, given the gaps.

## 1. Filings already charted for fluid SSR: what this technique changes

### Tencent WO2026113651A1 / CN122115679A

| Claim (WO / CN) | Glow SSR | Reading |
|---|---|---|
| 1 / 1 | The probe term on glossy world faces, unchanged (G2) | As before: the brief's build gate. Glow SSR neither adds nor removes an element |
| 10 / 9, 19 / — | Update the illumination information the probe collected when light at objects in range changes, then read from the updated data. Glow SSR writes nothing stored; it adds a per-frame screen-space correction to the sampled probe value where a glow differs from reference | **Appears absent as built; nearest-risk element.** Same footing as the shipped ratio and delta-tile S-A: a post-sample correction, nothing stored rewritten. Nearer the claim's purpose than a scalar ratio, since the sum is direction-resolved radiance a refreshed probe would hold. Further from it than delta tiles in one respect: no per-probe data exists for the correction to "be" the probe's information. Counsel |
| 11–14 / 10–12 | Re-shade stored texture information; polyhedral faces updated in turn | Appears absent: no stored G-buffer, nothing updated |
| 15 / 13 | If an obstruction lies in the acquisition direction, **supplement** the collected illumination | **Appears absent, and further away than fluid SSR.** G7 puts the change term inside the reflection weight W = Fresnel × s: where the capture-shadow test fires, the term is withheld with the probe, never added. This is the fluid record's W3 shape, which "removes 15b-ii entirely". The fluid layer read s nowhere; this one is multiplied by it. Reading s to withhold appears safe for 15; reading it to add would not (§7) |
| 16–18 / 14–16 | Distance test; completion by moving the probe or a lighting library | Unchanged from the probe record. Claim 18's library would need 15's completion step, which appears absent |

Note on §7.3a's IP rule "SSR never reads s": it was written for a layer composited beside the probe. Here the term is composited *inside* the probe's weight, so the rule's wording does not fit, though its purpose (nothing added where obstruction fires) is met more strongly. The brief should state this layer's own rule (§8, Keep).

### Microsoft US10586375B2 / US10360720B1 / EP3756162A1

Every US and CN independent claim needs a determination of whether the reflected ray heads toward the camera. G3 reuses the bounded march; if it composes `screen_march.wgsl` unchanged, the direction-blind rules of §7.3a carry over. Mip selection from hit distance (G4) reads a length, not a direction. CPU gating (G6) is per frame, not "adaptively selecting a technique per pixel". EP3756162A1 as published needs "a raytracing technique" among those selected; there is none. **Appears absent; high confidence for US and CN, low for EP until the granted text is read** (carried from the fluid record).

### Electronic Arts family

'774: ray count from roughness, ray tracing on viewport exit. G4 uses the exponent (a roughness analogue) to pick a **mip**, never a ray count; one ray per fragment; nothing is cast on a miss (G5). '474: reuse of neighbours' hits gated on roughness similarity. A prefiltered mip of the glow target averages neighbouring source pixels; it reuses no neighbour's ray or hit. Arguable only on a very broad reading; counsel may note it. '079: temporal reuse; G8 has none. **Appear absent; high confidence.**

### NetEase CN114748874B

Claim 1 (PatSnap translation): reflection position per water pixel; first information sampled from the "opaque texture" at its UV; second from "preset environmental reflection information"; Fresnel; combine.

| Element | Glow SSR | Reading |
|---|---|---|
| Reflection position on the reflected object | The march's hit | Appears present |
| First information "sampled from an opaque texture declaration map" | The glow-change target: selected faces' current − reference glow, zero elsewhere. Not the opaque scene colour | **Arguably absent**, more clearly than for fluid SSR. Turns on whether any screen texture is an "opaque texture" |
| Both informations "according to the reflection position" | Probe term from its own search, never from the SSR hit (W1) | Arguably absent, as for fluid |
| Fresnel; combine | W·(probe + change) | Present on a plain reading; claim 4's Fresnel lerp between the two is absent (they add) |

CN only. Low-medium confidence, unverified translation.

## 2. Nintendo "Non-transitory computer-readable medium, image processing system…" family (new)

Three US applications filed 2024-09-12 from three JP priorities of 2024-01-30; inventor Yasuhito Fujisawa. One is granted. These are the only filings found that select objects for SSR by kind.

### US12694606B2 (granted 2026-07-28)

Independent claims 1 (media), 9 (system), 17 (method), 25 (apparatus), summarised: first depth test into a first depth buffer; draw to a frame buffer; per pixel of interest, an incidence direction from the camera and a reflection direction; **a ray-tracing direction obtained by correcting the reflection direction toward the inside of the screen**; trace, find the collision through the depth buffer; if within a first distance, add a reflected colour **from the frame buffer** at the collision. Dependents: correction moves the terminal point's screen coordinates toward the reflection position's (2–4), more strongly near a screen edge (5), only near an edge (6), only when the reflection points off screen (7), horizontal component only (8).

| Element | Glow SSR | Reading |
|---|---|---|
| Depth test, frame buffer | Forward and depth prepass | Present (generic) |
| Reflection direction per pixel | Mirror direction about the face or texel normal | Present |
| **Correct the direction toward the screen interior** | No correction. A ray heading off screen marches as reflected and its term fades to 0 at the edge (G5) | **Appears absent** |
| Collision via the depth buffer; within a first distance | Fixed step budget; reach fade | Present in substance (a step budget is a distance bound) |
| Colour **from the frame buffer** at the collision | From the glow-change target, not the frame buffer | **Appears absent**, unless "frame buffer" is read as any screen target. Counsel |

Medium confidence. The edge fade is the opposite engineering answer to the same edge problem; keep it a fade of the term, never a bend of the ray (§7).

### US20250245909A1 (pending)

Claim 1, summarised: first depth test and frame buffer for scene objects; for a **first type of object**, a second depth test into a **second depth buffer**; per pixel trace a reflected ray, checking collisions against the first depth buffer within a first distance and against the second beyond it; colour "based on a color of a pixel in the frame buffer relating to the collision position". The description's first type: designated objects (a tree, a cylinder) that stay visible in reflections at range.

| Element | Glow SSR | Reading |
|---|---|---|
| Objects of a first type selected | Selected glow faces (G1) | Present in kind |
| A **second depth buffer** for them; depth switched by ray distance | One depth buffer for every ray at every distance. The selected faces write **colour** (glow change) at `Equal`, never depth | **Appears absent** |
| Colour from the frame buffer | From the glow-change target | Appears absent |

Pending: claims may change. Medium confidence.

### US20250242255A1 (pending)

Claim 1, summarised: draw objects other than a first type (a character); place a flat surface at the character textured with its rendering; reflected rays add the frame-buffer colour on a hit within a first distance, or the texture colour on the flat surface; then draw the character. Dependents: the first type is flat (2), a player character (3). Glow SSR has no proxy and draws selected faces into the forward result as usual. **Appears absent.**

## 3. Perfect World CN112973121B (new)

Claim 1 (summarised with quoted fragments): from a reflection point's normal and the camera direction, the reflection direction; **one step of a preset distance** to a reflected point; its screen UV and depth; "if the first depth value is larger than the second depth value", both static and dynamic reflected objects exist and the reflected point's screen colour (dynamic value) is **blended** with a preset static reflection value; otherwise the static value alone. Claims 7–9: apparatus, medium, device. WO2022227553A1 shows as ceased; CN granted 2021-07-20, active to ≈2041-04-30.

| Element | Glow SSR | Reading |
|---|---|---|
| Reflection direction from normal and camera | Same | Present |
| **One step of preset distance** | Bounded multi-step march (G3) | **Appears absent** |
| Depth comparison selecting both-or-static | Per-step interval hit test | Arguably present in substance |
| Dynamic = scene colour at the reflected point, blended with a preset static value | Change term from the glow target **added** to the baked probe term | Arguable: a baked static value plus a screen term is the same pairing; add versus blend, and glow change versus scene colour, differ |

Medium confidence; CN only. Its pairing of a precomputed static reflection with a screen-space dynamic correction is the closest structural match found to G2 + G4, which is why it is listed; the single-step element keeps it apart.

## 4. Tencent US12293451B2 element (c)

(c): "determining an indirect illumination value of a target pixel point … according to a radiance transfer parameter corresponding to the target pixel point … and the current light source projection coefficient". The prior record's high-confidence rule: no baked quantity that stays fixed while a light changes is multiplied or weighted at runtime by a value tied to that light's state.

| Element | Glow SSR | Reading |
|---|---|---|
| (a) light source change | Area glass follows its light; volumes cast no light | As for delta tiles: leans present for area glass, arguable for volumes |
| (b) coefficient from the change | current − reference per source, used to shade the selected face in G1 | Arguable for area glass |
| **(c) transfer parameter of the target pixel × coefficient** | The glossy pixel's reflection of the glow is found **at runtime** by marching this frame's depth and reading this frame's render of the source. No precomputed per-pixel, per-probe or per-source transfer exists (G8). The probe's baked reference tile is not scaled by any glow value; the change term is added beside it | **Appears absent.** Matches the record's "runtime indirect with no precomputed transfer (SSGI-style)" row, rated high for '451 |
| (d), (e) | Direct under the changed light; render both | Present, as for any area light |

One caveat to keep in view: G1 shades each selected face as its emissive texture × (current − reference). A fixed texture times a live value is the rule's literal wording, but it is the emitter's own appearance, which forward already draws the same way (`area-lights`, `emissive_volume`); it is not a transfer to the target pixel. Counsel may want that stated. **Medium-high confidence that glow SSR stays outside (c)**, against delta tiles' "arguable, nearest risk". The probe term's cast-light ratio keeps its own uncharted '451 shape (`ip-delta-tiles.md` §2); glow SSR neither adds to nor reduces it.

## 5. Others screened

| Item | Why it does not appear to read |
|---|---|
| EA US12354210B2 | Fixed-distance ray set; ray tracing for occluded directions (prior record) |
| NetEase US12469201B2 | SDF-guided marching for indirect light; no SSR target, no probe |
| NVIDIA US11443475B2 | Ray-cone mip selection at traced hits; no screen-space march. Not read past a search excerpt |
| Booming CN118096983A; NetEase CN117496039A, CN115120973B, CN120022596A, CN116637366A; Tencent CN116778049B | Prior record: ray tracing after SSR, planar maps, model textures, mirror cameras |

Nothing found from Sony, Unity, Epic, AMD, Activision, Apple, Qualcomm or Samsung claiming SSR restricted to an object group, an emissive-only reflection buffer, or SSR mip selection. Keyword search only; not a clearance search.

## 6. Prior art (dated, public)

Priorities: Microsoft 2018-02-22; EA 2018-08-10; Perfect World 2021-04-30; NetEase 2022-04-02; Nintendo 2024-01-30; Tencent WO 2024-11-28.

| Publication | Date | What it discloses, as we read it | Elements | Evidence |
|---|---|---|---|---|
| James & O'Rorke, "Real-Time Glow", GPU Gems ch. 21 (Addison-Wesley) | 2004 | Glow sources isolated from the scene and written to an off-screen render target; per-object values multiplied in to animate glow | **G1**: a separate screen target of selected glowing surfaces, animated per object | Search excerpt of the NVIDIA-hosted chapter |
| Sousa, Kasyan & Schulz, CryENGINE 3 (Crysis 2), SIGGRAPH Advances | 2011 | Real-time local reflections (screen space) over cubemaps | G3, SSR over probes | Secondary sources only; the 2011 course description does not name reflections |
| Valient, Killzone Shadow Fall, GDC | 2014-04 | Half-resolution screen march; hit mask; miss falls back to local then global cubemaps | G3, G5, half-res | Prior record (slide text) |
| McGuire & Mara, "Efficient GPU Screen-Space Ray Tracing", JCGT 3(4) | 2014 | Depth-buffer march, thickness test, near-plane clipping | G3 | Prior record (abstract) |
| Uludag, "Hi-Z Screen-Space Cone-Traced Reflections", GPU Pro 5 | 2014-05 | Hi-Z march; cone from material roughness widening with travel; blur from a mip-filtered colour buffer | **G4**: blur by roughness and hit distance through mips | Search excerpts (preview, forum); chapter text not read |
| Unreal Engine 4 SSR over reflection captures, edge fade | 2014 | SSR composited over static captures; screen-edge fade | G2–G5 | Prior record; not re-read |
| Stachowiak, "Stochastic Screen-Space Reflections", SIGGRAPH Advances | 2015 | Resolve reads a mip-mapped colour buffer, **mip from roughness and distance to the hit**; probe fallback | **G4** | Search excerpt of the slides |
| Herrmanns & Franke, "Screen Space Cone Tracing for Glossy Reflections" | Not dated here | Cone tracing over hierarchical prefiltered buffers | G4 | Search excerpt |
| Unity PPv2 `ScreenSpaceReflections.hlsl` | 2017 | Confidence = hit × edge fade × vignette; lerp over probes | G5 | Prior record (source read) |
| Unity HDRP: per-material "Receive SSR"; SSR fallback hierarchy SSR → probes → sky | ≈2018 onward | A material flag selects which receivers run SSR; misses fall back to probes | Receiver selection (glossy-only march, G3) | Search excerpts; release dates not verified. A layer-mask field on HDRP's SSR component appears in excerpts, but which objects it selects was not confirmed |
| Godot 3.0 SSR | 2018-01 | Edge fade; SSR blended with probes | G5 | Prior record (source read) |
| AMD FidelityFX SSSR | ≈2020 | Hierarchical traversal; cubemap fallback on miss | G3, G5 | Search excerpt |

**Strongest dated art.** For the separate selected-glow target (G1): James & O'Rorke 2004, which also animates glow per object into that target. For mip blur by roughness and hit distance (G4): Stachowiak 2015 and Uludag 2014. For SSR over a baked probe with miss-to-probe and edge fade (G2, G3, G5): Killzone 2014, UE4 2014, Unity PPv2 2017. For a preset static reflection plus a screen-space dynamic term: Perfect World's own CN112973121 (2021), now prior art against anything later. All of these predate Nintendo and Tencent WO; the 2004–2015 items predate every priority above.

**What is new in detail.** Writing the **change from reference** rather than the glow, and adding it to a probe that holds glow at reference, so the screen term corrects the probe instead of replacing or blending with it. No publication found does that. This bears on novelty, not on exposure.

## 7. Design choices that move the reading

| Choice | Moves | Why |
|---|---|---|
| Delta (current − reference) vs full glow | Neutral for '451 and Nintendo; slightly toward WO 10's "update" wording; away from NetEase | Full glow added over a probe holding reference glow would double-count, so it would have to replace the probe's glow, a per-pixel technique choice (Microsoft's "selecting" language) or a confidence blend (NetEase, Unity PPv2). A delta is a correction, which reads like "updating" in WO 10's terms but writes nothing stored |
| Selection by object group into a colour target | Away from Nintendo '909 | '909's selection feeds a second depth buffer; this feeds colour only |
| Selected faces also writing depth, or a second depth pass for them | **Toward Nintendo '909** | That is its claim 1 |
| Glow target instead of the frame buffer | Away from Nintendo '606, '909, '255 and NetEase CN114748874B | Each recites the frame buffer or opaque texture as the colour source |
| Edge fade of the term | Neutral | No claim found recites a fade |
| **Bending the ray toward the screen interior near edges** | **Toward Nintendo US12694606B2 (granted)** | That is its claim 1 element |
| Change term inside W (withheld with the probe in capture shadow, G7) | **Away from Tencent WO 15** | Obstruction only ever removes |
| Change term weighted by (1 − s), or the march run only where s < 1 | **Toward WO 15** | Adding where obstruction fires is "supplementing" |
| Skipping the march where s = 0 (cost) | Neutral, leaning safe | It withholds; it never triggers an addition. Counsel may confirm |
| Mip from exponent and hit distance | Neutral | Art: Stachowiak 2015, Uludag 2014. Not a ray count (EA '774) or neighbour reuse (EA '474) |
| Exponent driving ray count or a second technique | Toward EA '774 | Keep one ray per fragment |
| Half-resolution march or target | Neutral | Microsoft '375 dependents 11–13 fall with claim 10; Killzone 2014 is art |
| CPU frame gating (G6) | Neutral | Per frame, not per pixel; reads no ray direction |
| Any early-out, weight or step change by ray direction | Toward Microsoft (all independents) | Reuse the direction-blind march |
| Probe lookup seeded from the SSR hit | Toward NetEase 1b | Keep the probe on its own search |
| No history (G8) | Away from EA '079 | — |
| A baked per-source quantity scaled by live glow (aggregate share, delta tiles) | Toward '451 (c) | Glow SSR has none; keep it so |
| Ray tracing or probe re-capture on a miss | Toward EA '774, Microsoft EP, WO 10–13 | A miss shows the probe's reference glow, nothing more |

## 8. Design fences

| | Rule | Avoids | Note |
|---|---|---|---|
| **Keep** | The change term sits inside the reflection weight W = Fresnel × s: zero where the capture-shadow test withholds the probe, scaled by s in its fade, never by (1 − s) | WO 15 | Restate as this layer's own IP rule; §7.3a's "SSR never reads s" was written for a layer beside the probe |
| **Keep** | Nothing stored is written: probe tiles fixed after compile; the glow target is a per-frame screen target cleared each frame | WO 10–14, 19 | Extends the brief's source gate |
| **Keep** | No baked per-source, per-probe or per-pixel glow data; the glow's reflection is found only by the runtime march | '451 (c) | The property that separates this from delta tiles |
| **Keep** | One depth buffer for every ray; selected faces write colour only, at `Equal`, no depth | Nintendo '909 | — |
| **Keep** | Colour on a hit comes from the glow-change target; a hit on an unselected surface reads 0 | Nintendo '606, '909; NetEase CN114748874B | — |
| **Keep** | Reuse the direction-blind bounded march (`screen_march.wgsl`) with its §7.3a rules | Microsoft family | Hit distance may pick a mip; no direction quantity may |
| **Keep** | Probe lookup keyed to its own search, never to the SSR hit, depth or UV | NetEase CN114748874B 1b | W1 from the fluid record |
| **Keep** | One ray per glossy fragment; the exponent selects a mip, never a ray count; a miss casts nothing | EA '774 | — |
| **Keep** | Multi-step march | Perfect World CN112973121B | A single preset-distance step is its claim 1 |
| **Avoid** | Bending or clamping the reflected ray toward the screen interior near edges | **Nintendo US12694606B2**, granted | Fade the term instead |
| **Avoid** | A second depth buffer or depth pass for selected faces; distance-switched depth sources | Nintendo '909 | — |
| **Avoid** | Writing full glow and replacing or blending with the probe's glow by confidence | Microsoft "selecting", NetEase, Unity-style blend | Delta adds; nothing chooses |
| **Avoid** | Running or weighting the term where s < 1 to fill capture shadow | WO 15 | — |
| **Avoid** | Temporal accumulation of the glow target | EA '079 | G8 |
| **Watch** | Nintendo '909 and '255 prosecution; any continuation of '606 claiming selection of objects for SSR without the depth-buffer element | — | Re-read claims as granted |
| **Watch** | Tencent WO national phase (US ≈2027-05-28, EP ≈2027-06-28); amendments to 10/19 or a new screen-space claim | WO 10, 15 | Carried |
| **Watch** | Microsoft EP3756162 granted text | EP 6, 11 | Carried |
| **Watch** | Glossy world SSR is a second screen-space exception (`index.md` §1.1); the brief's Decision "probe-only on world faces" | Not IP | Owner decision, noted only |

## 9. For counsel

1. Tencent WO2026113651A1 claims 10 and 19: is adding a per-frame, screen-space glow correction to a sampled probe value "updating the illumination information the probe has collected"? Same question as the shipped ratio and delta-tile S-A; here no per-probe data carries the correction.
2. WO claim 15: confirm that a screen-space term multiplied by the capture-shadow weight (withheld where obstruction is detected) has no counterpart in "if there is an obstruction … supplement".
3. Nintendo US12694606B2 (granted 2026-07-28): whether "frame buffer" could read on a separate glow target; confirm the direction-correction element is absent from an edge fade.
4. Nintendo US20250245909A1 and US20250242255A1: monitor; whether a later claim could select objects for SSR **colour** rather than depth.
5. Perfect World CN112973121B: certified claim 1; whether "one step of preset distance" admits a multi-step march; relevant only for China.
6. '451 claim 1(c): confirm that runtime screen-space reflection of an emitter, with no precomputed transfer, is outside (c), and that the emitter's own shading (texture × live value) is not a "radiance transfer parameter".
7. Validity art for any SSR-selection claim: James & O'Rorke 2004 (selected, animated glow target), Stachowiak 2015 and Uludag 2014 (mip by roughness and hit distance), Killzone 2014 and UE4 2014 (SSR over probes).

**Not verified.** No verbatim or certified claim text for any filing; the Nintendo and Perfect World claims were read through a summarising fetch with short quotes, and the Nintendo JP members' status was not seen. Google Patents search could not be queried (script-rendered page; the endpoint was refused), so there was no CPC or assignee sweep. `docs.unity3d.com` was unreachable, so the HDRP Receive SSR flag, its layer mask and their dates rest on search excerpts. Uludag 2014's roughness-to-mip mapping and the 2011 Crytek slides were not read. Herrmanns & Franke's date was not seen. `research.md` §Glow SSR was not read; if its design differs from the elements above, re-check §7. Filings from the last 18 months are invisible to any search.
