# RTX-lite GI — IP and Cost Check

**Date investigated:** 2026-10-22, repo at `473185e88`; claim-level re-chart 2026-10-23
**Status:** Engineering triage for counsel, not legal advice. Companion to [index.md](index.md). "Appears present", "appears absent" and "reads on" are engineering readings of claim text, not opinions on infringement, validity or freedom to operate.

> **Read this when:** designing probe selection, a runtime radiosity update, any probe that changes with lighting at runtime, door-driven lighting, or a bake-only artefact.
> **Key limit (read first):** every patent database (Google Patents, Justia, USPTO, Espacenet, Patentscope) was blocked by the session's network policy. Google's legacy PDF path (`patentimages.storage.googleapis.com/pdfs/USnnnnnnn.pdf`) still serves grants below ~US 8.8M and pre-2015 applications: **full claim text** was read for Disney US8558837, Microsoft US8451270, Bunnell US7408550 and Crytek US20140327673A1. **Every other filing is charted from search-snippet paraphrase** (column *Source*).
> **Update 2026-10-30:** `patents.google.com` and `patentscope.wipo.int` are now allowed. Full claims were read for Take-Two '688 / '404 and Tencent WO2026113651A1 (machine translation). Google Patents then rate-limited (HTTP 503) the US12293451B2 page and Patentscope refuses automated queries (HTTP 403), so '451 claim text is still unread; see §Prior art verified 2026-10-30. Statuses and expiry projections are unverified. To chart the rest, allow `patents.google.com`, `patents.justia.com`, `ppubs.uspto.gov`, `worldwide.espacenet.com` and `patentscope.wipo.int`. Re-chart against certified claims before any design decision rests on it. Tencent WO2026113651A1 is charted from [the reflection-probe IP check](../../plans/done/reflection-probes/ip-check.md), itself read from a machine translation.

---

## Pieces under review

1. **Bake-time probe classification and relocation** (Majercik et al., JCGT 2021). Offline only.
2. **One probe type, two readers** (McGuire et al., I3D 2017). Per-cell radiance + inverse-L1 distance tiles; bake-derived irradiance mip and moments; glossy reader is today's distance search, hard-bound by cell; live share ratio; tiles never rewritten.
3. **Patch radiosity re-solved at runtime.** ~0.5 m patches, baked top-K links with crossed portals, CPU Jacobi on door and light changes, sealed-portal links zeroed.
4. **Probe-to-patch links.** Probe diffuse SH = Σ weight × patch radiance; reflection tiles not re-shaded.
5. **Shipped animated paths.** Animated SH deltas (id 27): runtime compose = base + Σ curve(t) × per-light baked unit delta. Animated lightmap compose: per-light weight maps × curve, in compute.
6. **Bake-only artefacts.** Surface radiance cache and gather; summed static bounce layer; door influence graph; light → cell reach; distance-to-edge owner masks. Nothing derived from a light change at runtime.
7. **Door channel.** Per (door, baked light) closed and open visibility masks, blended by door state on door events.

## Filings found

| Document | Owner | Priority | Status (unverified) | Source | Pieces |
|---|---|---|---|---|---|
| US20210012562A1 "Probe-based dynamic GI" | NVIDIA | 2019-07-11 | Reported abandoned 2023-08-23 (not re-confirmed); published 2021-01-14, citable art | Summary | 1, 2 |
| WO2026113651A1 / CN122115679A | Tencent | 2024-11-28 | Pending (published 2026-06-04; PCT/CN2025/126544); US national phase ≈2027-05, EP ≈2027-06 | Machine translation, all 23 claims, Google Patents 2026-10-30 | 2, 4 |
| US10137372B2 "Light probe culling based upon visibility" | Square Enix (Eidos Montréal) | filed 2016-12-14 | Granted 2018-11-27; ≈2036-12 + PTA | Abstract verbatim; claims snippet | 2, 4, 7 |
| US12293451B2 "Illumination rendering method…" | Tencent | CN 2021-04-02 | Granted 2025-05-06; ≈2042 + PTA | Snippet, claims 1–3; USPTO-record snippet 2026-10-30 (claim 1 says "radiance transfer parameter"); same title as CN114419240B, family link unconfirmed | 3, 4, 5, 7 |
| US8558837B2 "Modular radiance transfer" | Disney | 2010-01-18 | PTA 835 days → ≈2032-06-30 if fees paid | Full PDF | 3 |
| US10290148B2 / US10796479B2 | Suntracker (Ashdown) | CIP chain to 2012-04-13 | ≈2032-04, counsel to confirm CIP term | Snippet, claim 1 and dependents | 3, 5 |
| US10672177 (from US20170116773A1) | Geomerics → ARM | 2015-10-23 | Granted; ≈2036 | Title and abstract | 3, 6 |
| US10008032 / 10410408 / 10672183 / 11127198 / 11734879 / 12243156 | Imagination Technologies | 2015-03-03 | Active | Description snippets | 2 |
| US11854143B2 "Light probe generation" | Tencent | CN 2019-12-05 | Granted 2023-12-26 | Snippet, claims 1, 2, 4 | 1 |
| US20250037360A1 | Sony Interactive Entertainment | ≈2023 | Pending | Nothing found | 2 |
| US8451270B2 "Real-time radiosity system in a video game" | Microsoft | 2008-06-25 | PTA 905 days → ≈2030-12 unless fees lapsed; fee status not checked | Full PDF | 3, 4, 6 |
| US7408550 | Bunnell (no assignee on face) | filed 2007-07-23 | ≤ 2027-07-23; fee status not checked | Full PDF | 3 |
| US20140327673A1 | Crytek | published 2014 | Unknown; if granted, live concern | Full PDF claims | 3, 4, 6 |
| US11534688B2 / US12263404B2 | Take-Two | 2018-04-02 | '404 adjusted expiry 2039-09-04 (Google) | Full claims, Google Patents, read 2026-10-30 | 7 |

**NVIDIA as prior art.** US20210012562A1 published 2021-01-14, before Tencent US12293451B2's 2021-04-02 priority. Its probes update by ray tracing, not transfer × coefficient, so its overlap with '451 is weak; it is better art for piece 1.

Not found: Geomerics/ARM filings on the Enlighten runtime solve or radiosity-derived probes; Epic (Lumen surface or radiance cache); EA, Activision or Unity probe placement; portal-gated light transport. Door-driven baked lighting: only Take-Two's description ("a new baked ambient… placed over the area of the door as it opens"). Filings published in the last 18 months are invisible to search.

**Prior art relied on:** Goral et al. 1984; Cohen & Greenberg 1985; Ward 1988, 1992; Heckbert 1990; George, Sillion & Greenberg 1990; Dorsey, Sillion & Greenberg 1991; Hanrahan et al. 1991; Quake lightstyles 1996 (per-style lightmaps × runtime curve); Drettakis & Sillion 1997; qrad 1997; Sloan et al. 2002 and Microsoft's PRT patents US7262770 / US7061489; Source/vrad 2004 (per-leaf ambient samples); Doom 3 2004 (area portals gate light); Bunnell, GPU Gems 2 2005; Green (Valve) 2007 distance-field magnification; q3map2 surface lights; Martin & Einarsson SIGGRAPH 2010 (Enlighten in Frostbite 2, probes from the solution); Crytek US20140327673A1 (2014); McGuire et al. 2017; Suntracker US20170345208A1 (2017); Wang et al. I3D 2019; Majercik et al. 2019, 2021; Unity Adaptive Probe Volumes "Virtual Offset".

## Piece 1: probe classification and relocation

| Filing | Element | Proposal | Reading |
|---|---|---|---|
| NVIDIA US20210012562A1 | Probe active only near geometry; probes moved off grid; backface-fraction test | Same idea, offline | Abandoned; prior art |
| Tencent US11854143B2 cl. 1 | Voxelize a target object's shadow points, reduce vertices, scale the reduced object to fit the shadow surface, place probes at its vertices (cl. 2: lattice shadow points by light-ray hits; cl. 4: polyhedron around the shadow region) | Keep probes seeing a receiver in their cell; move out of walls | Appears absent |
| Sony US20250037360A1 | Higher resolution in a region of one probe | Uniform tile | Appears absent |

**Verdict: appears clear.**

**Costs (est.).** Bake: counts backface hits among the SH bake's 256 radiance rays per probe (validity itself is BSP-based, no rays); relocation ≈64 rays per invalid probe, ~15–40 s on the hallway (962,636 valid probes), noise against the SH stage. VRAM: stored tiles perhaps −20–40%; fog-reachable and entity-reachable air must stay "receiver", which caps the saving. Runtime: classification free (validity 0 exists); relocation needs a 3×i8 offset in the per-probe moment record, +0–2% forward, and relocated probes pin their nodes to L0.

**Levers.** Backface threshold, relocation step limit, receiver definition (surfaces, fog, navmesh air), classification aggressiveness as a bake setting.

**Cache keys.** Geometry, probe spacing, threshold constants. Not lights or materials.

## Piece 2: one probe type, two readers

| Filing | Element | Proposal | Reading |
|---|---|---|---|
| Tencent WO cl. 1–3 | Parallax-corrected probe reflection on a textured surface | Glossy reader = today's search | Present, as for water; exposure widens to every glossy surface |
| Tencent cl. 4–9 | Camera-placed or camera-moved probes | Bake-placed | Appears absent |
| Tencent cl. 10, 19 | Update stored probe lighting on a light change | Post-sample share ratio; tiles never rewritten | As landed: closest element, counsel |
| Tencent cl. 15–18 | Complete occluded lookups (move probe / lighting library) | No continuation; hard cell binding | Appears absent, provided a glossy fragment in capture shadow never falls back to the irradiance mip |
| Square Enix US10137372B2 | Cl. 1 (abstract verbatim): "identifying a set of probes associated with the selected point… determining which of a plurality of zones for the selected probe contains the selected point and determining visibility of said determined zone from the selected probe"; combine visible probes' irradiance. Second independent claim: zones from per-probe occlusion planes. Dependents: probe lookup by grid region; cl. 20–21 distance-weighted "sighted" probes. Description: one bit per zone; occluders include walls and doors | Entities and fog blend nearby per-cell probes | **Near a live claim** on the zone-visibility element; continuous Chebyshev weighting arguably differs. Counsel |
| Imagination '879 family | Probes updated by runtime ray tracing; local probes per screen block | Bake only | Appears absent |

**Verdict: needs counsel** (Square Enix '372 on the diffuse reader).

**Free design-arounds.** Select diffuse probes by cell adjacency (own cell, then portal-adjacent) with continuous moment weights; store no per-probe zone or cell visibility mask and make no binary visible decision. Glossy capture shadow draws no probe term and is never filled.

**Costs (est.).**
- Probe count: hallway 2,082 cells → 2.1k–6.2k probes; campaign 198 cells → 200–600.
- Per probe: 128² BC6H radiance 16 KiB + R16F distance 32 KiB + mips ≈ 64 KiB; irradiance mip + moments ≈ 1.6 KiB.
- Disk: hallway 131–397 MiB at 128², 33–99 MiB at 64². Resident tiles 4–38 MiB; diffuse data ≈10 MiB whole-map. Replaces the SH grid (hallway: 193.8 MiB across 3,385 clusters, worst mandatory set 49–125 MiB).
- Bake: ≈45 ms per probe captured → 94–279 s at 128². Removes the SH bake only if world indirect gets another source (piece 3 or a baked bounce lightmap).
- Runtime GPU: entities ±20% of today (4 irradiance + 4 moment taps vs 8 + 8). Fog +0–30% (needs a coarse 3D probe index). Glossy world surfaces 0.5–1.2 ms per screen-megapixel on the 1660, 1.5–2.5× on the 5300M; needs a per-material pipeline variant. SH compose passes removed.
- **Binding:** the forward pass has a fixed 16-texture floor; dropping SH atlases (−2) and adding radiance, distance and radiosity (+3) is net +1, or net 0 once SDF shadows leave and free group 5 binding 3.
- **Scope gap:** tiles carry no emitters. Direct light from baked lights onto entities and billboards (ids 35/41/45/47/48) needs a carrier, e.g. per-pixel direct for entities.

**Levers.** Tile resolution, probes per large cell, moment filter width, irradiance mip size, index voxel size; player tiers for glossy on/off per material class, mip bias, search taps.

**Cache keys.** Split capture into a geometry pass (hit, distance, moments: geometry, placement, tile size) and a shading pass (lights, albedo, emissive, the bounce source). Capture animated lights at zero and carry them in the share table so curve edits do not re-key tiles.

## Piece 3: patch radiosity

| Filing | Element | Proposal | Reading |
|---|---|---|---|
| Tencent US12293451B2 cl. 1 | (a) on a light change, determine "light source change information"; (b) a "current light source projection coefficient" for the changed light; (c) indirect at a "target pixel point" from its "radiance transfer parameter" × the coefficient; (d) direct under the changed light; (e) render from both. "Basis functions" appears in the description, not confirmed in cl. 1 | E = Σ scalar intensity × baked unit per-light patch irradiance; Jacobi over links | **Near a live claim** on (b) and (c); a scalar is arguably not a projection coefficient. Counsel |
| '451 cl. 2, 3 | Cl. 2: coefficient = "illumination transform matrix" × initial coefficient. Cl. 3: transfer for static-object pixels from a matching lightmap | Baked-tier lights never move; per-light bounce not stored per texel | Cl. 2 absent; cl. 3 absent while bounce stays summed |
| Disney US8558837B2 (full text) | Cl. 1, 9: a "palette of shapes", each with sampled direct lighting states, a "lighting prior" and an SVD-"factorized transport operator"; "extension operators" between adjacent shapes, "interfaces" between non-adjacent ones. Cl. 20: scene chunks mapped to palette shapes. Dependents: each shape "a cube with zero or more faces removed" | Explicit top-K links; no shape palette | Appears absent; keep links uncompressed |
| Suntracker '148/'479 | Per-channel "canonical radiosity solutions" in GPU texture memory, read per vertex in a vertex shader × channel intensity; dependents: channel texel-array index, global dimmer or sky multiplier, inverse gamma | CPU solve | Appears absent; keep any per-light layer sum out of a vertex shader |
| ARM US10672177 | Final-gather rays distributed by directional irradiance | Cosine-distributed link rays | Appears absent if rays never follow a prior solution |
| Microsoft US8451270 (full text) | Cl. 1: transfer-patch partition with curvature limits and UV packing. Cl. 7: per texel, a Z-buffered cube map of visible patch ids sorted by flux, top contributors stored in a texture + indirection texture. Cl. 11: real-time GPU radiosity in UV space, one pass per bounce | CPU links, no UV bundles | Appears absent; never ship per-texel flux-sorted lists |
| Crytek US20140327673A1 (full text) | Cl. 1: samples each holding "an indication of intersections of sample rays with other samples", a lighting contribution per sample, GI from those. Cl. 6: index array of hit samples. Cl. 9: per-sample light lists | Top-K links between patches | **Concern if granted**; status unknown |
| Bunnell US7408550 (full text) | Deformable geometry, control mesh, iteration seeded from the prior frame, a specific form-factor formula | Static geometry | Appears absent beyond warm-starting |
| Portal- or door-gated transport | None found for portal gating; Take-Two describes door-driven baked ambient (piece 7) | Sealed-portal links zeroed | Prior art: Drettakis & Sillion 1997; Doom 3 2004 |

**Verdict: needs counsel** (Tencent '451; Crytek '673 if granted). The '451 risk is not redesign-only: see piece 5.

**Free design-arounds.** Never project lights onto SH or any basis; no low-rank link compression; cosine link rays; no per-texel flux-sorted lists.

**Costs (est.).**
- Patches: hallway ≈1.0–2.0M at 0.25 m² (inferred from pool layers, not measured); campaign 0.1–0.4M.
- Links: 144–512 MB on the hallway, CPU RAM and disk; must stream per cell.
- Bake: ≈0.6–1.3G rays → 105–430 s; per-light emission from cached lightmap layers; +0.5–1 GiB peak.
- VRAM: radiosity map 6–12 MB.
- Runtime GPU: 1–2 taps per world fragment, replacing the 8-probe stencil.
- Runtime CPU per door event: ≈0.7M link-ops per sweep, ≈1 ms on 4 threads for 4 sweeps.
- **Animated worst case** (50 lights changing every frame): ≈3.6 ms per frame on 4 threads, over budget. Fix: B = B_door + Σ c_l(t)·B_l with per-light baked bounce; re-solve only on door events.

**Levers.** Patch size (integer multiple of lightmap texels), K, rays per patch, sweeps, update cadence, CPU ms budget, per-region patch scale.

**Cache keys.** Links: geometry, portal polygons, mover sealed-portal ids, patch size, K, ray count, seed; stable patch ids (face + grid coordinate). Not lights, colours, curves, albedo (store ρ separately), lightmap density or block order.

## Piece 4: probe-to-patch links

| Filing | Element | Proposal | Reading |
|---|---|---|---|
| Tencent WO cl. 10 | Update the probe's stored lighting before collection | Diffuse SH recomputed; tile untouched | Arguably absent; counsel on whether co-located data is "the probe" |
| Tencent WO cl. 11–13 | Re-shade stored G-buffer information on a light change | Sum of weight × patch radiance | Appears absent |
| Rejected variant: per-texel hit, UV and material re-shaded at runtime | — | — | **Reads directly on cl. 11–13**; rejected |
| Tencent US12293451B2 | Probe transfer × light projection coefficient | SH links × patch radiance | Same question as piece 3 |
| Square Enix '372 | Zone-visibility culling | Same reader as piece 2 | Same as piece 2 |

**Verdict: needs counsel** (also Crytek '673 if granted; a lightmap-gather variant raises '451 element (a) if a per-light change triggers it).

**Free design-arounds.** Re-gather on door events and a fixed cadence over dirty patches, never from a per-light change record. Runtime SH in a separate resource from the reflection probe record; the glossy reader never reads it; no per-texel re-shading; no time-slicing of tile faces.

**Costs (est.).** ≈0 extra bake rays (from piece 2's capture); 9.5–17.5 MB links; ≈0.2 ms per door event; worst case all probes 10–20 ms, so dirty-track changed patches and resident cells only.

## Piece 5: shipped animated paths

| Filing | Element | Shipped | Reading |
|---|---|---|---|
| '451 (a) | Light source change information | Compose re-evaluates curves every frame; the light bridge dirty-tracks light components and uploads descriptors | Unclear |
| '451 (b), (c) | Projection coefficient × per-pixel transfer | Scalar curve × baked per-light, per-probe unit delta; no basis projection; transfer per probe, not per pixel | Unclear; the crux. The description's probes-for-dynamic-objects variant weakens the per-pixel distinction |
| '451 (d), (e) | Direct under the same light; render both | `lm_anim` carries direct under the same curve | Present |
| '451 cl. 2 | Transform matrix | Baked-tier lights never move | Absent |
| Suntracker cl. 1 | Per-channel solutions × intensity, summed per vertex in a vertex shader | Per-light layers × curve, summed in compute, sampled per fragment | Shares the per-channel idea; vertex-shader element absent. Suntracker's 2017 publication is also art against '451 |
| Square Enix '372 | Binary zone visibility | 8-probe blend with continuous Chebyshev weights; no probe ever excluded | Likely distinguishable |

**Verdict: top counsel question.** Prior art against the scalar reading: Quake lightstyles (1996), Sloan PRT (2002) and Microsoft's PRT patents, Suntracker (2017), NVIDIA's application.

**Free design-arounds.** Keep baked lights static. Keep per-light terms scalar or RGB × a baked result. Sum layers in compute or fragment shaders, never per vertex. Frame compose scoping as a residency or visibility gate, not a light-changed trigger.

## Piece 6: bake-only artefacts

| Filing | Element | Proposal | Reading |
|---|---|---|---|
| ARM '177 | Final-gather rays distributed by directional irradiance | Uniform or cosine gather from the radiance cache | Appears absent while rays never follow a prior result |
| MS '270 cl. 7 | Per-texel flux-sorted patch lists in a texture + indirection | Cache dropped before packing | Absent |
| Crytek '673 | Samples storing ray hits on other samples → GI | Bake-only gather | Distinguishable on "real-time" if the claims require it |
| '451 cl. 3 | Per-light transfer in lightmap texels | One summed static bounce layer | Absent while summed; per-light bounce per texel moves toward it |
| Square Enix '372 | Binary zone visibility at read time | Door influence graph and light → cell reach scope work only | Absent while no reader drops probes on a binary test |
| Valve 2007 (prior art) | Distance-field magnification | Distance-to-edge owner masks | Art; the combination with owner lights is untriaged |

**Verdict: appears clear,** the lowest-risk family here. Bake-time radiosity has qrad (1997) and vrad (2004) as art; whether vrad bakes bounce per lightstyle is worth checking.

**Free design-arounds.** Drop intermediates before packing. Keep shipped bounce summed. Keep gather rays unguided.

## Piece 7: door channel

| Filing | Element | Proposal | Reading |
|---|---|---|---|
| Take-Two '688 / '404 | Claims: '688 cl. 1–7, a per-location dynamic terrain type selecting a surface shader plus a trail map of character marks, stored while off-camera and applied near it (snow, mud). '404 cl. 1–4, a top-down rendered reflected-light texture map plus a same-resolution height map, sampled for ground bounce per pixel (box blur; bent normal to the ground plane). Doors appear only in the shared description: "a new baked ambient is placed over the area of the door as it opens" | Direct-light visibility masks per (door or mover rest pose, light), blended by mover state | **Granted claims absent.** No claim element touches doors, movers or state-swapped baked layers |
| '451 | Per-light transfer updated on change | Door bounce deltas (optional) | Same question as piece 5 |
| Square Enix '372 | Doors as zone occluders | Door factor on direct-SH entries; probes never dropped | Absent while no probe is excluded |

**Verdict: appears clear on the granted claims** (read 2026-10-30). The family is application 16/373,514 ('688) and its continuation 18/056,862 ('404, issued 2025-04-01); Google Patents lists no further member. Residual risk: a later continuation could claim the description's door-ambient sentence, and filings from the last 18 months are invisible to search. Keep the design-around anyway, since it costs nothing: visibility masks per light, not a swapped ambient or AO layer. Contact AO swapped on mover state stays closest to the description.

## Prior art verified 2026-10-30

Checked from primary sources where reachable; otherwise from search-result text, marked *(snippet)*. Engineering triage, not an opinion on validity.

**Against Tencent WO2026113651A1** (priority 2024-11-28). Claim map from the full machine translation:

| Claims | Element | Art | Level |
|---|---|---|---|
| 1–3 | Parallax-correct a probe from the reflecting point and view; sample from the probe toward the reflected ray's hit on a second object | Szirmay-Kalos, Aszódi, Lazányi, Premecz, "Approximate Ray-Tracing on the GPU with Distance Impostors", CGF 24(3) 2005: colour + distance cube map, hit point "based on the distance values stored in environment map texels" *(snippet)*. Umenhoffer, Patow, Szirmay-Kalos, GPU Gems 3 ch. 17 (2007): ray march + secant search against stored distance. Bjorke, GPU Gems ch. 19 (2004): proxy-sphere correction. Lagarde & Zanuttini, SIGGRAPH 2012 talk 36: parallax-corrected cubemaps. Unity 5.1 Box Projection (2015). UE4 parallax-corrected captures (Epic staff, 2014) *(snippet)* | Verified (Szirmay-Kalos, Bjorke, Lagarde bibliographic, Unity); the search detail partly verified via the 2007 follow-up |
| 4–8 | Probe generated at the camera; moved past a distance threshold, on entering a grid cell, or after a dwell time | Camera-following realtime probes in community practice (Unity forum, ≈2015, date unconfirmed; dynamic cubemaps in racing games) | Generic idea partly verified; the specific triggers not found |
| 10–13 | Re-capture when objects in range move or light changes; re-shade from stored texture info | None found | — |
| 14 | Cube faces updated in sequence with a per-face interval | Unity 5.0 `ReflectionProbeTimeSlicingMode.IndividualFaces` (2015) | Verified |
| 15 | Occlusion test: probe's stored depth along the direction vs probe-to-hit distance | Distance impostors (2005) and GPU Gems 3 ch. 17 compare stored distance with the candidate's distance | Verified via the 2007 follow-up |
| 16–17 | Move the probe to an unobstructed position | None found | — |
| 18 | Fall back to a stored lighting database at the hit | None found | — |

Shipped water reflection (compile-time capture, colour + distance tiles, distance search, no camera-following probe, no runtime re-capture, no time slicing, nothing added where the capture shadow fires) sits on the claim 1–3 / 15 elements, which the 2005 distance-impostor art and its 2007 follow-up anticipate closely. Elements without art found (4–8 triggers, 10–13, 16–18) are ones the engine does not use. Keep it that way: probes stay compile-time and hard-bound; never re-capture on change, follow the camera, or relocate a probe at runtime.

**Against Tencent US12293451B2** (priority 2021-04-02). Claim 1 per the USPTO-record snippet, unconfirmed wording: on a light source change, determine change information; derive the current light projection coefficient; compute a target pixel's indirect value from its precomputed *radiance transfer parameter* and that coefficient; combine with direct light under the changed source. Claim 2: the coefficient from an illumination transform matrix and the pre-change coefficient.

| Element | Art | Level |
|---|---|---|
| Runtime scalar × baked per-light layer | Quake `gl_rsurf.c` (1996; GPL 1999): `blocklights[i] += lightmap[i] * scale;` with `scale = d_lightstylevalue[...]`, up to 4 styled lightmaps per surface | Verified, verbatim |
| Styled (per-light) baked layers carrying bounce | ericw-tools changelog 2017-06-10, "styled lights no longer bounce by default, set "_bouncestyled" "1" to enable"; docs: "allows styled lights to bounce (e.g. flickering or switchable lights)" | Verified, verbatim |
| Projection coefficient × precomputed transfer | Sloan, Kautz, Snyder, PRT, SIGGRAPH 2002 | Verified |
| Runtime indirect from precomputed data, direct rendered separately, updated on light change | Martin & Einarsson, SIGGRAPH 2010 Advances course (Enlighten in Frostbite 2); Unity 5 `DynamicGI.updateThreshold` | Verified (existence, architecture) |
| Per-light baked direct pages merged by name | Valve VRAD named lights (direct only; named lights do not bounce) | Partly verified |
| Dynamic probes without transfer | NVIDIA US20210012562A1, published 2021-01-14 | Verified; weak match |
| Suntracker US20170345208A1 / US10290148B2 | Not re-found this pass; earlier snippet chart stands | — |

**For door and mover light flow.** The door channel and mover rest-pose channel blend direct-light visibility masks by mover state; no light source changes and no transfer × coefficient step exists, so '451's indirect elements stay absent for direct-only channels. Door bounce deltas (optional) keep the '451 question, now with Quake styles + ericw-tools `_bouncestyled` as the closest art for a scalar × baked per-light bounce layer. Compose on a mover dock event is an occluder event, not light source change information; still frame any compose gate as residency or visibility (piece 5 design-around).

## Summary

| Piece | IP verdict | Watch | Bake Δ (est.) | Memory Δ (est.) | Runtime (est.) |
|---|---|---|---|---|---|
| 1 | Appears clear | — | +15–40 s | SH −20–40% | ≈0 |
| 2 | Needs counsel | Square Enix '372; Tencent WO cl. 1, 18 | +94–279 s; −SH bake only with a new bounce source | −SH grid (49–125 MiB mandatory on the hallway); +4–38 MiB resident; 131–397 MiB disk | Entities ≈0; fog +0–30%; glossy 0.5–1.2 ms/MP (1660) |
| 3 | Needs counsel | Tencent '451 | +105–430 s | +6–12 MB VRAM; 144–512 MB links | Door ≈1 ms (4 threads); animated needs hybrid |
| 4 | Needs counsel | Tencent WO cl. 10; '451; Crytek '673 | ≈0 | +9.5–17.5 MB | ≈0.2 ms per door event |
| 5 | Top counsel question | '451 cl. 1 | Shipped | Shipped | Shipped |
| 6 | Appears clear | ARM '177 gather; '451 cl. 3 if per-light | Faster SH bake (*est.*) | Bake RAM ~24–100 MB | None new |
| 7 | Appears clear (granted claims) | Take-Two continuations | Masks per (door, light) | ≤ ~10% of lightmap mandatory set (gate) | Door events only |

**Top IP risks.** (1) Tencent US12293451B2 claim 1 against the shipped animated SH deltas and any light-change-triggered update; keep per-light terms scalar. (2) Square Enix US10137372B2 against any reader that excludes probes; keep continuous weights. (3) Tencent WO2026113651A1: claim 1 widens to every glossy surface; claim 10 on runtime data beside a reflection probe; claims 11–13 keep re-shading rejected. (4) A future Take-Two continuation claiming door-driven baked layers; the granted '688 and '404 claims do not reach them. (5) Crytek US20140327673A1 against runtime links, if granted.

**Questions for counsel, ranked.**
0. Obtain '451's verbatim claims (Google Patents rate-limited this session; USPTO and Patentscope automated access blocked).
1. Can '451 claim 1 read on a per-light scalar curve × a baked per-light, per-probe delta? Is the light bridge's dirty upload "light source change information"? Weigh Quake lightstyles, Suntracker 2017, Sloan PRT 2002, NVIDIA's application.
2. Does a continuous Chebyshev weight escape '372's "determining visibility of said determined zone"?
3. Status: Crytek '673 (granted?), Microsoft '270, Bunnell '550, Disney '837 and Suntracker fees; NVIDIA abandonment.
4. Take-Two: confirm no pending continuation of 18/056,862 claims the door-ambient description. The granted claims were read 2026-10-30 and do not touch doors.
5. Tencent WO claim 1 against water and glossy world reflections; claim 10 against runtime probe gathers.
6. Is Suntracker's "per vertex in a vertex shader" limiting against compute compose?
7. ARM '177 against any future guided gather in the bake.

**Top cost unknowns.** (1) Radiance-cache SH bake: grey single-bounce parity with id 34 and hallway-mini bake time. (2) Patch count, link bytes, link bake time and K-truncation energy loss on the hallway and campaign. (3) Glossy world-surface cost on the 5300M, plus the 16-texture fix. (4) CPU Jacobi under animated load, validating the hybrid and its error when doors cut a light's region.
