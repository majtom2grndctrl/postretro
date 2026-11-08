# RTX-lite GI — IP and Cost Check

**Date investigated:** 2026-10-22, repo at `473185e88`; claim-level re-chart 2026-10-23; Take-Two and Tencent WO claims 2026-10-30; Tencent US12293451B2 claims 2026-11-03; Square Enix US10137372B2 full claims 2026-11-07 ([companion](ip-372-direction-aware-visibility.md))
**Status:** Engineering triage for counsel, not legal advice. Companion to [index.md](index.md). "Appears present", "appears absent", "reads on", "in", "out" and "arguable" are engineering readings of claim text, not opinions on infringement, validity or freedom to operate.

> **Read this when:** designing probe selection, a runtime radiosity update, any probe that changes with lighting at runtime, door-driven lighting, or a bake-only artefact.
> **Sources (read first):** full claim text was read for Disney US8558837, Microsoft US8451270, Bunnell US7408550 and Crytek US20140327673A1 (Google's legacy PDF path, `patentimages.storage.googleapis.com/pdfs/USnnnnnnn.pdf`); Take-Two '688 / '404 and Tencent WO2026113651A1 (Google Patents; WO from machine translation); Tencent US12293451B2 (granted B2 PDF and USPTO ppubs full text); and Square Enix US10137372B2 with its A1 publication, EP3306569B1 and CA2944435C (Google Patents, 2026-11-07; charted in [ip-372-direction-aware-visibility.md](ip-372-direction-aware-visibility.md)). Not read: '372's US file wrapper (15/377,913). **Every other filing is charted from search-snippet paraphrase** (column *Source*); re-chart those against full claims before a design decision rests on them. Statuses and expiry projections are unverified except '451's. Not read: '451's US file wrapper; ppubs anonymous access ends 2026-11-07. Google Patents rate-limits (HTTP 503); Patentscope refuses automated queries (HTTP 403); Justia and Espacenet refused. Tencent WO2026113651A1 is also charted in [the reflection-probe IP check](../../plans/done/reflection-probes/ip-check.md).

---

## Pieces under review

1. **Bake-time probe classification and relocation** (Majercik et al., JCGT 2021). Offline only.
2. **One probe type, two readers** (McGuire et al., I3D 2017). Per-cell radiance + inverse-L1 distance tiles; bake-derived irradiance mip and moments; glossy reader is today's distance search, hard-bound by cell; live share ratio; tiles never rewritten.
3. **Patch radiosity re-solved at runtime.** ~0.5 m patches, baked top-K links with crossed portals, CPU Jacobi on door and light changes, sealed-portal links zeroed.
4. **Probe-to-patch links.** Probe diffuse SH = Σ weight × patch radiance; reflection tiles not re-shaded.
5. **Shipped animated paths.** Animated SH deltas (id 27): runtime compose = base + Σ curve(t) × per-light baked unit delta. Animated lightmap compose: per-light weight maps × curve, in compute.
6. **Bake-only artefacts.** Surface radiance cache and gather; summed static bounce layer; door influence graph; light → cell reach; distance-to-edge owner masks. Nothing derived from a light change at runtime.
7. **Door and mover channels.** Per (door, baked light) closed and open visibility masks, blended by door state on door events. The probe channel (`plans/drafts/mover-light-flow--probe-channel/`) adds per-probe direct visibility and static lights' bounce per mover state.

## Filings found

| Document | Owner | Priority | Status | Source | Pieces |
|---|---|---|---|---|---|
| US20210012562A1 "Probe-based dynamic GI" | NVIDIA | 2019-07-11 | Reported abandoned 2023-08-23 (not re-confirmed); published 2021-01-14, citable art | Summary | 1, 2 |
| WO2026113651A1 / CN122115679A | Tencent | 2024-11-28 | Pending (published 2026-06-04; PCT/CN2025/126544); US national phase ≈2027-05, EP ≈2027-06 | Machine translation, all 23 claims, Google Patents 2026-10-30 | 2, 4 |
| US10137372B2 "Light probe culling based upon visibility" | Square Enix Limited (UK), from Eidos Interactive | CA2944435 2016-10-04; US filed 2016-12-13 (EP filed 2016-12-14) | Granted 2018-11-27; ≈2037-02-01 (Google's PTA figure, unconfirmed); 4th- and 8th-year fees paid (2026-05-20); no US continuation; family CA, EP (granted 2024, every independent claim has planes), JP | Full claims, A1, EP and CA text, Google Patents 2026-11-07 | 2, 4, 7 |
| US12293451B2 "Illumination rendering method…" | Tencent | CN 2021-04-02 (CN112927341A/B → WO2022206380A1 → US20230076326A1) | Granted 2025-05-06; ≈2043-01-19 (PTA 309 days, USPTO record); no maintenance fee due until 2028-05-06; no EP/DE members | Granted B2 PDF and USPTO ppubs full text, all 20 claims, 2026-11-03 | 3, 4, 5, 7 |
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

**Prior art relied on:** Goral et al. 1984; Cohen & Greenberg 1985; Ward 1988, 1992; Heckbert 1990; George, Sillion & Greenberg 1990; Dorsey, Sillion & Greenberg 1991; Hanrahan et al. 1991; Quake lightstyles 1996 (per-style lightmaps × runtime curve); Drettakis & Sillion 1997; qrad 1997; UE1 movers 1998; Sloan et al. 2002 and Microsoft's PRT patents US7262770 / US7061489; q3map2 `-bouncegrid` with the Jedi Academy grid runtime 2003; Source/vrad 2004 (per-leaf ambient samples); Doom 3 2004 (area portals gate light); Bunnell, GPU Gems 2 2005; Green (Valve) 2007 distance-field magnification; q3map2 surface lights; D3DX9 PRT volume samples; Martin & Einarsson SIGGRAPH 2010 (Enlighten in Frostbite 2, probes from the solution); Far Cry 3 DRTV 2012; Crytek US20140327673A1 (2014); McGuire et al. 2017; ericw-tools `_bouncestyled` 2017; Suntracker US20170345208A1 (2017); Wang et al. I3D 2019; Majercik et al. 2019, 2021; Unity Adaptive Probe Volumes "Virtual Offset" and lighting-scenario blending. Detail in §Prior art.

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
| Square Enix US10137372B2 | Nine independent claims. Cl. 1, 34–36, 42, 43 add "wherein the zones for each selected probe are defined by intersections of pre-defined occlusion planes for the selected probe" (in A1 that was dependent cl. 6, so likely a narrowing amendment; wrapper unread); cl. 38 operates on the planes; cl. 44 needs a visibility mask, a zone code and "a bit-wise AND". **Cl. 37** is the one plane-free claim: a region from "a grid of regions", "determining which of a plurality of zones for the selected probe contains the selected point and determining visibility of said determined zone", combine visible probes. Cl. 11: "the number of probes in the set is greater than the number of sighted probes". Description: one visibility bit per zone, sighted/blind, a blind probe "should NOT influence" the result | Entities and fog blend nearby per-cell probes; direction-aware visibility samples a per-probe octahedral tile of depth moments, bilinearly, toward the point | Plane and mask claims **appear absent** (no per-probe planes, zones or masks stored). Cl. 37 **arguable, leaning absent**: no zone index is produced, and the Chebyshev bound depends on the point's distance, so visibility is never a property of a zone. The 0.03 floor is evidence, not the escape: cl. 11 implies the claims cover every probe sighted. Counsel |
| Imagination '879 family | Probes updated by runtime ray tracing; local probes per screen block | Bake only | Appears absent |

**Verdict: needs counsel** (Square Enix '372 on the diffuse reader).

**Free design-arounds.** Select diffuse probes by cell adjacency (own cell, then portal-adjacent) with continuous moment weights; store no per-probe zone or cell visibility mask and make no binary visible decision. Glossy capture shadow draws no probe term and is never filled.

**'372 rules for any probe-weight change** (from [ip-372-direction-aware-visibility.md](ip-372-direction-aware-visibility.md) §Design levers):
- Sample per-direction depth bilinearly through a linear sampler; never `textureLoad` or nearest, which picks one cell per point.
- Keep the point's distance inside the visibility function. Store moments, never verdicts: no per-probe or per-texel blocked flag, visible bit or per-direction mask, even as an optimisation.
- Keep the Chebyshev floor at 0.03 until counsel answers; never 0, where the L1 whole-cell path would drop the probe outright.
- No per-probe planes, half-space tests or stored region-to-probe tables.
- Tile selection is a representation choice: never let tile presence gate a probe's inclusion, never ship the score. Mover tiles switch or blend moment sets, never set an occluded flag or force a weight to zero.
- The backface factor `max(dot, 0)` is the blend's one point-dependent hard zero; a smooth or floored form needs a capture A/B and the owner's call.

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
| Tencent US12293451B2 cl. 1 | (a) on a light change, determine "light source change information"; (b) a "current light source projection coefficient" for the changed light; (c) indirect at a "target pixel point" from its "radiance transfer parameter" × the coefficient; (d) direct under the changed light; (e) render from both. Basis wording first appears in cl. 7 and 15 | E = Σ scalar intensity × baked unit per-light patch irradiance; Jacobi over links | **Near a live claim** on (b) and (c). A scalar is a weak distinction: the description supports reading it as the coefficient. Counsel |
| '451 cl. 2, 3 | Cl. 2: coefficient = "illumination transform matrix" × initial coefficient. Cl. 3: transfer for static-object pixels from a matching lightmap | Baked-tier lights never move; per-light bounce not stored per texel | Cl. 2 absent. Cl. 3's transfer is per basis function (cl. 7), not per light; absent while no coefficient multiplies stored bounce |
| Disney US8558837B2 (full text) | Cl. 1, 9: a "palette of shapes", each with sampled direct lighting states, a "lighting prior" and an SVD-"factorized transport operator"; "extension operators" between adjacent shapes, "interfaces" between non-adjacent ones. Cl. 20: scene chunks mapped to palette shapes. Dependents: each shape "a cube with zero or more faces removed" | Explicit top-K links; no shape palette | Appears absent; keep links uncompressed |
| Suntracker '148/'479 | Per-channel "canonical radiosity solutions" in GPU texture memory, read per vertex in a vertex shader × channel intensity; dependents: channel texel-array index, global dimmer or sky multiplier, inverse gamma | CPU solve | Appears absent; keep any per-light layer sum out of a vertex shader |
| ARM US10672177 | Final-gather rays distributed by directional irradiance | Cosine-distributed link rays | Appears absent if rays never follow a prior solution |
| Microsoft US8451270 (full text) | Cl. 1: transfer-patch partition with curvature limits and UV packing. Cl. 7: per texel, a Z-buffered cube map of visible patch ids sorted by flux, top contributors stored in a texture + indirection texture. Cl. 11: real-time GPU radiosity in UV space, one pass per bounce | CPU links, no UV bundles | Appears absent; never ship per-texel flux-sorted lists |
| Crytek US20140327673A1 (full text) | Cl. 1: samples each holding "an indication of intersections of sample rays with other samples", a lighting contribution per sample, GI from those. Cl. 6: index array of hit samples. Cl. 9: per-sample light lists | Top-K links between patches | **Concern if granted**; status unknown |
| Bunnell US7408550 (full text) | Deformable geometry, control mesh, iteration seeded from the prior frame, a specific form-factor formula | Static geometry | Appears absent beyond warm-starting |
| Portal- or door-gated transport | None found for portal gating; Take-Two describes door-driven baked ambient (piece 7) | Sealed-portal links zeroed | Prior art: Drettakis & Sillion 1997; Doom 3 2004 |

**Verdict: needs counsel** (Tencent '451; Crytek '673 if granted). The '451 risk is not redesign-only: see piece 5.

**Free design-arounds.** Never project lights onto SH or any basis; no low-rank link compression; cosine link rays; no per-texel flux-sorted lists. Unit per-light emission × runtime intensity is the risky shape under §Tencent US12293451B2 design rules.

**Costs (est.).**
- Patches: hallway ≈1.0–2.0M at 0.25 m² (inferred from pool layers, not measured); campaign 0.1–0.4M.
- Links: 144–512 MB on the hallway, CPU RAM and disk; must stream per cell.
- Bake: ≈0.6–1.3G rays → 105–430 s; per-light emission from cached lightmap layers; +0.5–1 GiB peak.
- VRAM: radiosity map 6–12 MB.
- Runtime GPU: 1–2 taps per world fragment, replacing the 8-probe stencil.
- Runtime CPU per door event: ≈0.7M link-ops per sweep, ≈1 ms on 4 threads for 4 sweeps.
- **Animated worst case** (50 lights changing every frame): ≈3.6 ms per frame on 4 threads, over budget. Cost fix: B = B_door + Σ c_l(t)·B_l with per-light baked bounce; re-solve only on door events. That fix is the risky '451 shape (cl. 3–4).

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
| '451 (a) | Light source change information | Compose re-evaluates curves every frame; the light bridge dirty-tracks light components and uploads descriptors | Leans present for curve-driven lights: col 5 lists intensity change |
| '451 (b), (c) | Projection coefficient × target pixel's transfer | Scalar curve × baked per-light, per-probe unit delta; no basis projection | Arguable; the crux. Per-probe transfer does not distinguish: cl. 5–6 read the pixel's transfer from a probe |
| '451 cl. 5–6 | Probe-lit dynamic pixel; per-direction probe brightness updated from coefficient × transfer, interpolated, evaluated with the normal | id 27/34 octahedral per-direction tiles, trilinear | Close match |
| '451 (d), (e) | Direct under the same light; render both | `lm_anim` carries direct under the same curve | Present |
| '451 cl. 2 | Transform matrix | Baked-tier lights never move | Absent |
| Suntracker cl. 1 | Per-channel solutions × intensity, summed per vertex in a vertex shader | Per-light layers × curve, summed in compute, sampled per fragment | Shares the per-channel idea; vertex-shader element absent. Suntracker's 2017 publication is also art against '451 |
| Square Enix '372 | Binary zone visibility (cl. 37); plane-defined zones (cl. 1 and most others) | 8-probe blend with continuous Chebyshev weights; no probe ever excluded; no planes or masks stored | Plane claims appear absent; cl. 37 arguable, leaning absent |

**Verdict: top counsel question.** id 27 is the risky '451 shape; id 25 and id 45 carry direct only and stay out while id 25 carries no bounce. Headline art: q3map2 `-bouncegrid` with the Jedi Academy grid runtime (2003). Also Quake lightstyles (1996), ericw-tools `_bouncestyled` (2017), Sloan PRT (2002) and Microsoft's PRT patents, D3DX neighbourhood transfer, Far Cry 3 DRTV (2012), Enlighten (2010), Suntracker (2017), NVIDIA's application. None is cited on '451's face.

**Free design-arounds.** Keep baked lights static. Follow §Tencent US12293451B2 design rules; a scalar or RGB per-light term is not a design-around. Sum layers in compute or fragment shaders, never per vertex.

## Piece 6: bake-only artefacts

| Filing | Element | Proposal | Reading |
|---|---|---|---|
| ARM '177 | Final-gather rays distributed by directional irradiance | Uniform or cosine gather from the radiance cache | Appears absent while rays never follow a prior result |
| MS '270 cl. 7 | Per-texel flux-sorted patch lists in a texture + indirection | Cache dropped before packing | Absent |
| Crytek '673 | Samples storing ray hits on other samples → GI | Bake-only gather | Distinguishable on "real-time" if the claims require it |
| '451 cl. 3 | Transfer per lightmap texel, per basis function (cl. 7) | One summed static bounce layer | Absent: no coefficient multiplies it. Per-light unit bounce per texel × a runtime scalar would be the risky shape (cl. 3–4) |
| Square Enix '372 | Binary zone visibility at read time | Door influence graph and light → cell reach scope work only | Absent while no reader drops probes on a binary test |
| Valve 2007 (prior art) | Distance-field magnification | Distance-to-edge owner masks | Art; the combination with owner lights is untriaged |

**Verdict: appears clear,** the lowest-risk family here. Bake-time radiosity has qrad (1997) and vrad (2004) as art; whether vrad bakes bounce per lightstyle is worth checking.

**Free design-arounds.** Drop intermediates before packing. Keep shipped bounce summed. Keep gather rays unguided.

## Piece 7: door and mover channels

| Filing | Element | Proposal | Reading |
|---|---|---|---|
| Take-Two '688 / '404 | Claims: '688 cl. 1–7, a per-location dynamic terrain type selecting a surface shader plus a trail map of character marks, stored while off-camera and applied near it (snow, mud). '404 cl. 1–4, a top-down rendered reflected-light texture map plus a same-resolution height map, sampled for ground bounce per pixel (box blur; bent normal to the ground plane). Doors appear only in the shared description: "a new baked ambient is placed over the area of the door as it opens" | Direct-light visibility masks per (door or mover rest pose, light), blended by mover state | **Granted claims absent.** No claim element touches doors, movers or state-swapped baked layers |
| '451 | Light change → coefficient × transfer | Direct visibility blended by mover state; static lights' bounce stored absolute per mover state | Direct half absent: no light changes, no indirect. Bounce half out-leaning (medium) |
| Square Enix '372 | Doors as zone occluders; visibility mask "pre-computed for each probe" (cl. 33) | Door factor on direct-SH entries; closed-pose depth tiles switch a probe's moment set through the tile reference; probes never dropped | Absent while no probe is excluded and no mask is stored; whether switching a tile by door state moves toward cl. 33 is counsel question 8 |

**Verdict: appears clear on the granted claims.** The Take-Two family is application 16/373,514 ('688) and its continuation 18/056,862 ('404, issued 2025-04-01); Google Patents lists no further member. Residual risk: a later continuation could claim the description's door-ambient sentence, and filings from the last 18 months are invisible to search. Keep the design-around anyway, since it costs nothing: visibility masks per light, not a swapped ambient or AO layer. Contact AO swapped on mover state stays closest to the description. For '451, mover bounce follows the mover rules in §Tencent US12293451B2: absolute states, never base + delta, never a light that carries a curve.

## Tencent US12293451B2

Read 2026-11-03 from the granted B2 PDF (scanned page images) and USPTO ppubs full text; claim 1 confirmed verbatim against the USPTO text. Not read: the US file wrapper (application 17/985,107; examiner Werner G. Garner, art unit 3715; office action 2024-11-12, response 2025-01-14, allowance 2025-01-28). ppubs anonymous access ends 2026-11-07.

**Family.** CN202110359533.9 → CN112927341A/B → PCT/CN2022/081063 (WO2022206380A1) → US20230076326A1 → US12293451B2 (continuation). No EP or DE members (EP non-entry 2024-10-16). CN114419240B is a different Tencent family, not charted: CN202210337007.7, priority 2022-04-01, WO2023185262A1.

**Prosecution.** Claim 1 is identical in A1 and B2: no narrowing amendment. Every face citation is applicant-cited: US 2013/0335434 (Microsoft), US 2017/0263043 (Imagination), five CN references, Wang 2007 (spectral-mesh PRT), Zhu, Humphreys EGSR 2007. The examiner added no art. Never before the examiner: Sloan 2002, Quake, ericw-tools, Enlighten, Suntracker. CNIPA granted a narrower claim 1 after citing Wang 2007; it adds the transform matrix and the position/intensity/direction list.

**Claim 1** (9 device, 17 media recite the same steps), verbatim: "(a) determining light source change information when a light source changes in a virtual scene; (b) determining a current light source projection coefficient corresponding to the changed light source according to the light source change information; (c) determining an indirect illumination value of a target pixel point in the virtual scene according to a radiance transfer parameter corresponding to the target pixel point in the virtual scene and the current light source projection coefficient; (d) determining a direct illumination value corresponding to the target pixel point under the changed light source; and (e) performing illumination rendering on the target pixel point according to the direct illumination value and the indirect illumination value."

| Claims | Adds |
|---|---|
| 2 / 10 / 18 | Coefficient from an "illumination transform matrix" and the pre-change coefficient |
| 3 / 11 / 19 | Static-object pixel: transfer from "a lightmap matching the target pixel point" |
| 4 / 12 | That texel via the texture mapping relationship |
| 5 / 13 | Dynamic-object pixel: transfer from "a light probe matching the target pixel point" |
| 6 / 14 | Update per-direction probe brightness from coefficient × transfer, interpolate, evaluate with the pixel normal |
| 7 / 15 | Bake projects the light onto basis functions, each a virtual light; transfer ray-traced into lightmaps and probes |
| 8 / 16 / 20 | Distant light; the change is its direction |

**Description** (col = B2 column, para = paragraph). "Light source change information" "may include at least one of light source position change information, light source intensity change information, or light source direction change information" (col 5), found by comparing against the pre-change light. The projection coefficient is the light projected onto a basis (SH, spherical Gaussians, piecewise; col 5). Paras 32 and 35 and FIG. 4 (half brightness; linear per-light superposition) also support reading a per-light intensity scalar as the coefficient. The radiance transfer parameter includes self-occlusion and interreflection, stored per lightmap texel per basis function or per probe face (cols 7, 20, 24), and "when a light source changes, the radiance transfer parameter does not change" (col 17). Doors, occluders and object movement never appear as a trigger; dynamic objects appear only as probe-lit receivers (col 14).

**What "light intensity" covers.** The light source's own output, not baked lighting: baked lightmaps and probes are the transfer side of (c). The claim targets a light that changes at runtime *and* whose indirect comes from precomputed transfer.

**Bakes.** Claim 1 (and 9, 17) is a runtime method; every element hangs off a light change. A bake runs before any light changes, so it performs none of (a)–(c). The bake appears only in dependent claim 7, which cannot be met without claim 1's runtime steps. A bake infringes only through what the runtime does with its output.

| Engine path | Runtime use | (a) light changes | (c) indirect from baked transfer × coefficient | Reading |
|---|---|---|---|---|
| Static lightmaps, shadowmask, id 34 summed bounce, id 35 direct SH | Read as-is | No | No coefficient | Absent |
| Surface radiance cache (bake intermediate) | None; dropped before packing | — | — | Absent |
| id 25 animated weights, id 45 animated direct deltas | × curve; direct only | Leans present (col 5) | No indirect | Absent while id 25 carries no bounce |
| id 27 animated indirect deltas | Baked per-light unit bounce × curve, per probe | Leans present (col 5) | Arguable: cl. 5–6 read transfer from probes | **Top counsel question.** The bake is fine; its runtime rescale is '451's shape |
| Dynamic runtime lights | No baked transfer, no bounce | Yes | No | Absent |
| Mover probe channel, direct half | Blended by mover position | No: an occluder moves, the light never changes | Direct only | Absent |
| Mover probe channel, bounce half (static lights, absolute per mover state) | Blended by mover position | No while gated lights are static | Arguable, though transfer varying with occluder pose inverts col 17's premise | Out-leaning (medium); '451-shaped if a gated light also carries a curve |

**Where the position stops.** Any feature that rescales baked *bounce* by a runtime light change: today's id 27, the parked `surface-radiance-cache-animated`, and mover bounce on a light that also carries a curve. Runtime bounce with no precomputed transfer lacks (c)'s "radiance transfer parameter", so it falls outside '451, though other filings may read on it. The engine does not pursue ray-traced bounce (no RTX dependency).

### Design rules

| Rule | Avoids | Confidence |
|---|---|---|
| No baked quantity that stays fixed while a light changes is multiplied or weighted at runtime by a value tied to that light's state | (b), (c) | High |
| Indirect stays frozen at its baked value while direct animates | (c) | Medium-high |
| Select between absolute baked states; never base + 1 × delta | (b), (c) | Medium |
| Recompute only on occluder or mover state; lights unchanged | (a) | Medium-high |
| Runtime indirect with no precomputed transfer (DDGI-style, SSGI, voxel) | (c) | High, for '451 only |

- **Risky:** per-light unit response × runtime scalar or RGB, per probe (cl. 5–6) or per texel (cl. 3–4); today's id 27. A crossfade weight that represents a light's intensity.
- **In:** runtime basis projection of a light against baked transfer, including SH sun rotation (cl. 2, 7, 8). Do not build.
- **Not levers:** per-frame vs event-triggered evaluation; RGB vs scalar; probe vs pixel.
- **Mover work (owner decision):** drive recomputation by mover state, never light state. Mover bounce is stored absolute at each light's fixed intensity, sums static lights only, and never covers a light that also carries a curve.

| Technique | Reading |
|---|---|
| Quake lightstyles, direct only | Out |
| ericw-tools `_bouncestyled` (styled bounce × style value) | Arguable |
| Binary swap of absolute baked states | Out-leaning |
| Lerp of absolute states, t from a light curve | Arguable (doctrine of equivalents) |
| Enlighten-style runtime solve | Arguable |
| True PRT | In |
| Mover-state direct blend | Out |
| Mover-state bounce, static lights | Out-leaning (medium) |

## Prior art

Checked from primary sources where reachable; otherwise from search-result text, marked *(snippet)*. Verbatim means quoted from source code, docs or changelog.

### Against Square Enix US10137372B2

Priority CA 2016-10-04. Full table and quotes: [ip-372-direction-aware-visibility.md](ip-372-direction-aware-visibility.md) §Prior art.

- **On the face:** Sugden, "Mega Meshes" (GDC 2011) stores per probe "a distance to the nearest occluder in every direction" and ignores probes that cannot see the object; Cupisz (GDC 2012) calls baked per-probe visibility what most games do. The claims were allowed over both with "zones" (**verified**, Mega Meshes from a transcription).
- **Not on the face:** Greger et al., "The Irradiance Volume" (1998): grid lookup over the 8 corners. Donnelly & Lauritzen, variance shadow maps (I3D 2006): Chebyshev visibility from two depth moments. Far Cry 3 (2012): grid → probe lookup. Hooker, Treyarch volumetric GI (Advances 2016). Source engine per-leaf ambient sets (2006).
- **Context only (after priority):** McGuire et al., light field probes (I3D 2017; PDF built 2016-12-21, no earlier preprint found); Majercik et al., DDGI (2019).

### Against Tencent WO2026113651A1

Priority 2024-11-28. Claim map from the full machine translation, checked 2026-10-30:

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

### Against Tencent US12293451B2

Priority 2021-04-02; claims in §Tencent US12293451B2. All art below predates it, and none was before the examiner. It bears on validity of claims 1, 5–6 if "projection coefficient" covers a scalar, the same broad reading that puts id 27 at risk.

**Headline art for the id-27 shape:** the q3map2 `-bouncegrid` bake with the Jedi Academy grid runtime (2003). No shipped map was proven compiled with `-bouncegrid`; the combination is a toolchain configuration.

**Claim elements** (checked 2026-10-30):

| Element | Art | Level |
|---|---|---|
| Runtime scalar × baked per-light layer | Quake `gl_rsurf.c` (1996; GPL 1999): `blocklights[i] += lightmap[i] * scale;` with `scale = d_lightstylevalue[...]`, up to 4 styled lightmaps per surface | Verified, verbatim |
| Styled (per-light) baked layers carrying bounce | ericw-tools changelog 2017-06-10, "styled lights no longer bounce by default, set "_bouncestyled" "1" to enable"; docs: "allows styled lights to bounce (e.g. flickering or switchable lights)" | Verified, verbatim |
| Projection coefficient × precomputed transfer | Sloan, Kautz, Snyder, PRT, SIGGRAPH 2002 | Verified |
| Runtime indirect from precomputed data, direct rendered separately, updated on light change | Martin & Einarsson, SIGGRAPH 2010 Advances course (Enlighten in Frostbite 2); Unity 5 `DynamicGI.updateThreshold` | Verified (existence, architecture) |
| Per-light baked direct pages merged by name | Valve VRAD named lights (direct only; named lights do not bounce) | Partly verified |
| Dynamic probes without transfer | NVIDIA US20210012562A1, published 2021-01-14 | Verified; weak match |
| Suntracker US20170345208A1 / US10290148B2 | Not re-found; earlier snippet chart stands | — |

**Per-light bounce in probes × runtime scalar** (checked 2026-11-03). The id-27 shape: per animated light, a baked per-probe unit-brightness bounce response; at runtime base + Σ curve × response, trilinear, lighting dynamic objects. Against claims 1, 5–6.

| Art | Date | Evidence | What it covers | Closeness |
|---|---|---|---|---|
| q3map2 `-bounce N -bouncegrid` for Raven BSP games (JK2, JA, SOF2) | q3map2 2.5.11 changelog 2003-12-01; source public in GtkRadiant GPL import 2007-09-12 | Verbatim (`TTimo/GtkRadiant` `tools/quake3/q3map2/light_bounce.c`, `light.c`, `q3map2.h`): bounce lights inherit their source lightmap's style (`light->style = style;`); grid points store per style (`byte ambient[MAX_LIGHTMAPS][3]; byte directed[MAX_LIGHTMAPS][3]; byte styles[MAX_LIGHTMAPS]`); `if( bouncegrid ) … TraceGrid` | Bake side: bounce baked per light style into each grid probe | Same shape per style (group), not strictly per light; ambient + one directed term, not SH; ≤ 4 styles per point |
| Jedi Academy renderer `R_SetupEntityLightingGrid` | Game 2003; source GPL 2013 (OpenJK) | Verbatim (`JACoders/OpenJK` `code/rd-vanilla/tr_light.cpp`): `ent->ambientLight[0] += factor * data->ambientLight[j][0] * styleColors[style][0] / 255.0f;`, same for `directedLight`, trilinear over 8 grid points | Runtime side: Σ style value × baked per-style grid value, interpolated, lighting dynamic entities | Claim 1 (a)–(c) with a scalar as the coefficient; claim 5 (probe-lit dynamic objects); claim 6 in part |
| FTE `gl_rlight.c` RBSP grid | Code date unproven (RBSP commits 2005–2012) | Verbatim: "rbsp has up to 4 styles per grid element, which needs to be scaled by that style's current value" | Second runtime with the same lookup | Same; date to establish |
| D3DX9 PRT `ComputeVolumeSamples` / `ComputeVolumeSamplesDirectSH` | June 2010 SDK header seen; first SDK not established | Verbatim (Microsoft `d3dx9mesh.h`): "…a form of 'neighborhood transfer.' The result is a linear operator (matrix) at every sample point, if you multiply this matrix by the distant SH lighting coefficients you get… local incident radiance" | Transfer stored at points in space for other objects × SH lighting coefficients | Closest shipped API for claims 1, 5–6, 7; distant SH lighting, not a per-light scalar |
| Sloan, Kautz, Snyder, SIGGRAPH 2002 | 2002 | *(snippet)*: transfer "through a preprocessed object to neighboring points in space… cast onto arbitrary, dynamic receivers"; "a transfer matrix… within a simple 3D grid surrounding O" | Neighbourhood transfer on a 3D grid | Claims 1, 5, 7; body text and section not obtained |
| Stefanov (and Gilabert), "Deferred Radiance Transfer Volumes", Far Cry 3, GDC 2012 | 2012 | *(snippet)*, GDC Vault abstract: probes "storing spherical harmonics coefficient matrices… can be relit on the fly… time of day changes… local lighting such as muzzle flashes" | Probe transfer relit by sun/sky SH coefficients | Strong on claims 1, 7, 8; use on dynamic objects unconfirmed |
| Unity 5 Precomputed Realtime GI (Enlighten) | Blog 2014-09-18; Unity 5.0 2015 | *(snippet)*: "feeding the actual lights present into these previously computed light propagation paths"; probes updated at runtime for dynamic objects | Probes whose bounce follows runtime light values | Claims 1, 5; runtime solve, not a stored unit response |
| Kristensen, Akenine-Möller, Jensen, SIGGRAPH 2005 | 2005 | *(snippet)*: per-vertex indirect "due to a set of input lights", "storing only the indirect component" | Per-light precomputed indirect | Per vertex, not per probe; runtime scaling unconfirmed |

**Negative findings.** Source VRAD ambient cubes take style 0 only (`leaf_ambient_lighting.cpp`: "We only care about light style 0 here."); Quake 3 IBSP and DarkPlaces grids carry no styles; ericw-tools' lightgrid (2023) postdates the priority date.

**Unread (hosts blocked):** Far Cry 3 slides (gdcvault.com), Sloan 2002 body (dl.acm.org, jankautz.com), Kristensen PDF (lup.lub.lu.se), Unity 5 manual (docs.unity3d.com), the D3DX first SDK version (learn.microsoft.com).

### Mover state channel

Checked 2026-11-03. Design: per baked light a mover blocks, its direct visibility stored per probe (brief 1) and per texel (brief 2) at each end of travel, blended at runtime by the mover's path position.

| Element | Closest art | Date | Evidence | Closeness |
|---|---|---|---|---|
| Mover shadow baked at a chosen pose | UE1 Mover `WorldRaytraceKey` ("whatever you set this to is where the mover will block light"), `BrushRaytraceKey`; ericw-tools bmodel `_shadow` (TyrUtils v0.7) | 1998; 2013-04-10 | Verbatim (Unreal Wiki mirror, `DarklightGames/unrealwiki` `mover-ut.html`; `Mover.uc` not read); verbatim (`docs/light.rst`, changelog) | Same, one pose |
| Per-state baked layer switched at runtime | ericw-tools `_switchableshadow` (0.15.10): bmodel shadow on its own lightstyle, toggled by `lightstyle()`; `_switchableshadow_target` (2.0.0-alpha9) | 2017-07-30; 2025-02-06 | Verbatim (`light.rst`, changelog, `light/ltface.cc`) | Similar: per caster not per light, present vs absent not two poses, lightmaps only (lightgrid does not carry it) |
| Continuous blend of baked probe states | Unity APV `ProbeReferenceVolume.BlendLightingScenario(name, factor)`; "HDRP changes only the indirect lighting data" | SRP 14.0.2 (changelog dates inconsistent; about 2021–22) | Verbatim (Unity Graphics repo docs and CHANGELOG) | Similar: whole scene, indirect, script-keyed |
| Door gates which lights reach an area | Doom 3 `connectedAreas` ("avoid drawing interactions when the light is behind a closed door"), `idMover::SetPortalState` | 2004 | Verbatim (GPL source) | Same, runtime lights, binary |
| Door event switches a baked light layer | Quake 1 `doors.qc` `door_go_up` → `SUB_UseTargets()`; `misc.qc` `light_use` → `lightstyle(self.style, "m"/"a")` | 1996 | Verbatim (v1.01 QC) | Similar, binary or time-ramped |

Snippet only, sites blocked: UE4 Precomputed Lighting Scenarios (4.13 unconfirmed); a ModDB Doom 3 tutorial blending open and closed lightmaps by script; a gamedev.net post on open/closed probe sets near doors (≈2019).

**Not found:** a blend keyed to mover position; per-(occluder pose, light) probe data; the combination. These are where the design is new, not exposure: no live claim charted so far (Take-Two '688/'404, '451) reads on it. Blocked hosts: wiki.beyondunreal.com, docs.unity3d.com, dev.epicgames.com, moddb.com, gamedev.net, quakewiki.org.

## Summary

| Piece | IP verdict | Watch | Bake Δ (est.) | Memory Δ (est.) | Runtime (est.) |
|---|---|---|---|---|---|
| 1 | Appears clear | — | +15–40 s | SH −20–40% | ≈0 |
| 2 | Needs counsel | Square Enix '372; Tencent WO cl. 1, 18 | +94–279 s; −SH bake only with a new bounce source | −SH grid (49–125 MiB mandatory on the hallway); +4–38 MiB resident; 131–397 MiB disk | Entities ≈0; fog +0–30%; glossy 0.5–1.2 ms/MP (1660) |
| 3 | Needs counsel | Tencent '451 (unit per-light terms × intensity) | +105–430 s | +6–12 MB VRAM; 144–512 MB links | Door ≈1 ms (4 threads); animated needs hybrid |
| 4 | Needs counsel | Tencent WO cl. 10; '451; Crytek '673 | ≈0 | +9.5–17.5 MB | ≈0.2 ms per door event |
| 5 | Top counsel question | '451 cl. 1, 5–6 on id 27; direct-only id 25 / id 45 absent | Shipped | Shipped | Shipped |
| 6 | Appears clear | ARM '177 gather; '451 cl. 3–4 if bounce is rescaled at runtime | Faster SH bake (*est.*) | Bake RAM ~24–100 MB | None new |
| 7 | Appears clear (granted claims) | Take-Two continuations; '451: direct half absent, bounce half for static lights out-leaning | Masks per (door, light) | ≤ ~10% of lightmap mandatory set (gate) | Door events only |

**Top IP risks.** (1) Tencent US12293451B2 claims 1, 5–6 against id 27 (per-light unit bounce × curve, per probe) and any feature that rescales baked bounce by a light's state. A scalar or RGB term is no design-around; follow §Tencent US12293451B2 design rules. (2) Square Enix US10137372B2 claim 37 against any reader that turns a probe's visibility into a per-zone verdict; keep continuous, point-distance-dependent weights and follow the '372 rules in §Piece 2. Its plane and mask claims appear absent while no per-probe plane, zone or mask is stored. (3) Tencent WO2026113651A1: claim 1 widens to every glossy surface; claim 10 on runtime data beside a reflection probe; claims 11–13 keep re-shading rejected. (4) A future Take-Two continuation claiming door-driven baked layers; the granted '688 and '404 claims do not reach them. (5) Crytek US20140327673A1 against runtime links, if granted.

**Questions for counsel, ranked.**
1. Pull '451's file wrapper (17/985,107) before ppubs needs sign-in on 2026-11-07: action grounds, response arguments, reasons for allowance. Also any PTAB or court proceeding, and any CNIPA invalidation of CN112927341B.
2. Construction of "light source projection coefficient" (paras 32 vs 35; claim 7 differentiation). Is a per-light scalar or RGB curve one?
3. Is a per-light unit bounce response a "radiance transfer parameter" when the description defines transfer per basis function?
4. Validity of '451 claims 1, 5–6 against art the examiner never saw: q3map2 `-bouncegrid` + Jedi Academy 2003, Quake lightstyles, `_bouncestyled` 2017, Sloan 2002, D3DX neighbourhood transfer, Far Cry 3 DRTV 2012, Enlighten 2010.
5. What weight does the narrower CN claim 1 carry for the US claim's scope?
6. Does '451 (a) cover continuous curves?
7. Equivalents for a lerp of absolute states with t from a light curve; inducement exposure from SDK modders authoring animated lights.
8. '372 claim 37: does bilinearly sampling a per-probe octahedral tile of depth moments toward the point "determine which zone contains the point", and does a point-distance-dependent Chebyshev bound "determine visibility of said determined zone"? Given claim 11, does the floor matter beyond evidence? Pull the wrapper (15/377,913) for why the planes entered claim 1 and why "grid of regions" made claim 37 allowable. Sharpened in nine parts: [ip-372-direction-aware-visibility.md](ip-372-direction-aware-visibility.md) §Questions for counsel.
9. Status: Crytek '673 (granted?), Microsoft '270, Bunnell '550, Disney '837 and Suntracker fees; NVIDIA abandonment.
10. Take-Two: confirm no pending continuation of 18/056,862 claims the door-ambient description. The granted claims do not touch doors.
11. Tencent WO claim 1 against water and glossy world reflections; claim 10 against runtime probe gathers.
12. Is Suntracker's "per vertex in a vertex shader" limiting against compute compose?
13. ARM '177 against any future guided gather in the bake.
14. Chart Tencent CN114419240B / WO2023185262A1 (priority 2022-04-01, a separate family with the same title as '451) and any US counterpart.

**Top cost unknowns.** (1) Radiance-cache SH bake: grey single-bounce parity with id 34 and hallway-mini bake time. (2) Patch count, link bytes, link bake time and K-truncation energy loss on the hallway and campaign. (3) Glossy world-surface cost on the 5300M, plus the 16-texture fix. (4) CPU Jacobi under animated load, validating the hybrid and its error when doors cut a light's region.
