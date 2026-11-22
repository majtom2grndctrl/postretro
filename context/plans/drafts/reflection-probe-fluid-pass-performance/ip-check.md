# reflection-probe-fluid-pass-performance — IP check

> **Engineering due diligence, not legal advice. 2026-11-21.** This compares published claims and dated prior art against the brief's levers, to brief the owner and counsel. "Appears present", "appears absent", "arguable" and "leans" are engineering readings of claim text. They are not opinions on infringement, validity or freedom to operate, which belong to counsel. A negative search result means "not found", never "clear".

Compared against the brief as it stood before its split (2026-11-22): Lever A is now `index.md` and `research.md` §Precompute here, and Levers C1–C3 are `../reflection-probe-cell-path/index.md`. Also against `rendering_pipeline.md` §7.3a (Live lighting, Style, Two pipelines). Builds on `ip-research.md` here (the landscape), `plans/done/reflection-probes/ip-check.md` (Tencent WO chart, §4 fences), `context/research/gi-rtx-lite/ip-check.md` §Tencent US12293451B2 (verbatim claim 1, design rules, prior art) and `plans/drafts/dynamic-reflection-layer/ip-delta-tiles.md` §2 (the shipped ratio's '451 shape). It does not redo those charts. It charts only what this brief changes.

**Cell-path pass (2026-11-22).** Extends §Lever C and §Path choice for `../reflection-probe-cell-path/index.md` (shape A, the drafted cell pool) and charts its rival, shape B, in §Shape B. Shape B is charted as the owner's request describes it: `research.md` §Screen-space rival was not in the checkout read (origin at `439683f7e`). Shape B's exposure through the shipped screen-space layer is `plans/done/fluid-ssr-reflections/ip-check.md`'s; this pass reuses that chart and does not redo it.

**Sources.** Claim text is verbatim unless a row says otherwise:
- US and EP filings come from Google Patents page text. Long claims were read in segments; "…" marks a cut.
- Tencent WO2026113651A1 comes from Google's machine translation of the Chinese text.
- Intel US10699475B1 comes from its grant PDF on `patentimages.storage.googleapis.com`, through two-column text extraction.
- Status, priority, prosecution and fee events are Google Patents' legal-event data, unconfirmed against USPTO.
- Papers were read in full text on the second pass (2026-11-21) unless a row says otherwise. Each prior-art row names its source.

**Gaps:**
- No CPC- or assignee-filtered database query ran. The fresh sweep was web search restricted to `patents.google.com`, plus citation lists on the pages read.
- File-wrapper documents (office actions, reasons for allowance) were not read. USPTO Patent Center and Global Dossier now require a USPTO.gov sign-in: anonymous document requests return HTTP 405 (checked 2026-11-22). Patentscope refused automated access. Prosecution dates and examiner-cited art come from Google's legal events and citation tables.
- The EPO Register and Espacenet sat behind a bot check this session did not pass. EP status comes from Google's INPADOC legal events.
- Applications filed in the last 18 months are invisible to any search.

## Levers under review

| Lever | What runs | Changes vs today |
|---|---|---|
| **A. Precompute** | On a frame with any reflecting draw, one compute dispatch writes, per resident probe, five RGB sums `S_k = Σ_l share_{l,k} · (current_l / ref_l − 1)`, one per §7.3a lobe. Fragments read `ratio(d) = max(1 + Σ_k w_k(d) · S_k, 0)`. The probe tile is never written | The same ratio, hoisted from per fragment to per probe per frame. It adds a stored per-probe, per-direction state, rewritten every frame and ordered before the fluid pass (acceptance P1) |
| **C1. Cell pool** | Per frame, the cell-path draws' faces, clipped to the screen, pack nearest first into a pool sized from the scene extent. A compute pass evaluates the probe term once per water texel cell. The pixel-style fragment reads its cell's record | A same-frame, history-free texture-space intermediate for the probe term |
| **C2. Hidden-cell skip** | Each cell invocation tests its screen footprint conservatively against the depth pre-pass and skips evaluation if the footprint is hidden everywhere. Every packed cell is still dispatched (acceptance: "dispatches its cells yet evaluates none") | A per-cell depth visibility test in texture space |
| **C3. Path choice** | Per draw, on the CPU: estimated screen pixels per cell against one cutoff, with hysteresis per volume | Chooses texture-space or per-pixel evaluation per draw |
| **Shape B. Screen-space probe target** (rival to C1–C2) | After the depth pre-pass, a separate pass rasterizes the same water draws into a target at ½ or ¼ of the scene extent. Every target pixel inside a water texel cell evaluates that cell's ray. The full-resolution fragment reads the target nearest at its cell centre's screen projection. Draws whose cells shrink toward one target pixel fall back per pixel, chosen per draw as in C3 | A reduced-resolution screen-space intermediate for the probe term, built like the shipped screen-space layer's reflection pass (§7.3a Reflection pass, Rays, Composite read) |

Lever B, the view-dependent early-out, is out of scope: the brief decided against it. `ip-research.md` §Lever B stands as the record.

## Summary

| Filing | Owner | Status | Lever | Closest element | Reading | Confidence |
|---|---|---|---|---|---|---|
| US12293451B2 ('451) | Tencent | Granted 2025-05-06, active, ≈2043-01-19. One non-final action (2024-11-12), allowed 2025-01-28 | A | 1(c) transfer × coefficient; 6 per-direction probe update | Claim 1: no new element; the shipped ratio's exposure is unchanged. Claim 6: moves toward it, but needs claim 5's dynamic-object pixel. **The description defines a dynamic object by a changeable position or direction**, so claim 5 leans absent more firmly | Medium |
| WO2026113651A1 / CN122115679A | Tencent | Pending. No national or regional phase yet; EPO notified of EP designation 2026-07-15. US phase ≈2027-05-28, EP ≈2027-06-28 | A, C1 | 10 / 19 "update the illumination information collected by the probe" before collection | Appears absent: no tile is written. Nearer than per-fragment, because a per-probe state is rewritten before collection | Medium-low |
| US12136166B2 ('166) | Qualcomm | Granted 2024-11-05, active; priority 2021-09-30; one non-final action (2024-02-29), allowed 2024-08-14. No continuation found | C1, C2 | 1: meshlet visibility, size from texture coordinates and perspective projection, atlas layout, shade atlas | Most elements read on the cell path. The crux is unchanged: "select a size … based on … a perspective projection". **Shading Atlas Streaming already sizes patches by projected screen area**; it groups 1–3 triangles, not meshlets | Medium |
| EP4409531B1 | Qualcomm | Granted 2026-07-22 (EP22792976.7). All 38 EPC states designated; national entries recorded for CH, GB, IE, DE. No opposition event seen; **opposition window to ≈2027-04-22** | C1 | Same as '166 claim 1 **without the visibility element** | Same crux as '166. Broader by one element, so the cell path's face-level clip no longer matters | Medium |
| US12664715B2 (A1 US20250095266A1) | Qualcomm | Granted 2026-06-23 (publication date); priority 2023-09-14. Three notices of allowance before issue. EP4778033A1 pending | C1, C2 | 1: identify visible shading elements, offsets from counts of visible elements, allocate texture memory to visible elements only | **New.** Arguable, leaning absent: rects are allocated per face from the screen clip, and cells keep texel-grid offsets. **A visible-cell compaction would read on it** | Medium |
| US10699475B1 | Intel | Granted 2020-06-30, active, ≈2038-12-28; 4th-year fee paid 2023-09-20 | C2 | 1: texel visibility tests, a stored texel mask, an any-visible bit, dispatch only for visible texels | Appears absent on three elements while the skip stays inline per cell. An indirect-dispatch compaction would read on it | Medium-high |
| US9728002B2 | AMD | Granted 2017-08-08, active, fees paid through year 8 | C1 | 1: select texels to shade from texels cached for another frame | Appears absent (no history) | High |
| US10535186B2 | Intel | Granted 2020-01-14, active | C1, C3 | 1: first pass computes a per-pixel shading rate; second pass texel-shades a procedural texture after querying whether texels are already shaded | Appears absent | High |
| US9754407B2 | NVIDIA | Granted 2017-09-05, active | C1 | 1: shading-grid elements shaped like the triangle | Appears absent (square texel cells) | High |
| US12602856B2 (A1 US20250095265A1) | Qualcomm | Granted 2026-04-14, ≈2044-05-22 | C1, C3 | 1: render geometry for screen UV derivatives; mip-region resolution from derivatives and target pixels per region | Appears absent (CPU estimate, no render, fixed resolution). **Granted claims now read** | Medium-high |
| US20260203991A1 | Qualcomm | Pending; continuation of '856, filed 2026-03-12 | C1, C3 | 1: mip-region resolution "based on a set of derivatives for a set of visible meshlets" | Appears absent. **Broader than '856**: no render step, no target pixels | Medium |
| US10152819B2 | Microsoft | Granted 2018-12-11, ≈2036-10-14 | C3 | 1: SRP values per primitive region; fewer frame-buffer samples covered per colour | Appears absent | High |
| US10147227B2 | Microsoft | Granted 2018-12-04, ≈2037-06-22 | C3 | 1: SRP vertex values, coarse scan conversion, SRP-map lookup, per-fragment rate | Appears absent | High |
| US10109039B1 | Intel | Granted 2018-10-23, ≈2037-04-24 | C3 | 1: gaze monitor, focus area, display-engine blend and offload. Texel-to-pixel threshold only in dependents | Appears absent (no gaze, no display-engine offload) | High |
| US20200364926A1 | Qualcomm | **Abandoned** 2020-12-19 | — | Shade a primitive when its frame-to-frame shading difference passes a threshold | Prior art only | High |
| US11721064B1 | Meta | Granted 2023-08-08, active | C1 | Server atlas, send to client, reuse view-independent shading across viewpoints | Appears absent | High |
| US11605198B2 | Baobab | Granted 2023-03-14. No fee-payment or expiry event on Google | A | Two precomputed GI sets (SH probes and lightmaps), up-sample the lower-resolution one, blend by weight | Appears absent | High |
| EP1652151A1 / US7532220B2 | Philips | Refused (EP); US expired, fee related | C3 | Per primitive, pick a resampling algorithm "in dependence on a size of the primitive" | Prior art only | High |
| US10198856B2 | Oxide Interactive | Granted 2019-02-05; 4th-year fee paid 2022-07-20 (small entity); ≈2034-03-05 | A, C3 | 1: per object, a shading parameter image; sort objects "to minimize state changes of a shader program"; a MIP chain of the shaded mesh, then rasterize that MIP chain. Cl. 4: reduced-size images "based on an estimated pixel coverage" | **New.** Appears absent: no MIP chain of the shaded result, no state-change sort. Cl. 4 is what the sizing fence already forbids | High |
| US11436783B2 | Oxide Interactive | Granted 2022-09-06; ≈2040-11-19; priority 2019-10-16 | A | 1: a raster loop and a shade loop "at a second frequency"; a shadel storage buffer; on a miss the raster loop generates the shaded portion and stores it "in a shadel miss buffer", then reads it back | **New.** Appears absent: overflowed draws evaluate per fragment and store nothing; no miss buffer | Medium-high |
| US12154208B2 (division of '783) | Oxide Interactive | Granted (OG 2024-11-26); priority 2019-10-16 | A | 1: allocate "space in a shadel storage buffer … by subdividing", "atomic exchange increments … using hash functions"; "a work queue buffer" of commands per shadel | **New.** Appears absent: the pool is shelf-packed on the CPU; no hashed atomic allocation, no GPU work queue | High |
| US10198788B2 | Oxide Interactive | Granted 2019-02-05 (CIP); 4th-year fee paid 2022-07-20; ≈2033-11-11. Continuation US20190295214A1 abandoned | A | '856's steps plus shading "at a first frequency" and rasterizing at a second | **New.** Appears absent (shades every frame; no MIP chain) | High |
| US9342926B2 / EP2797054B1 | Square Enix | US granted 2016-05-17; EP granted 2018-11-21, ≈2034-04-02 (fee status unread) | B | 1: a low-resolution "indirect illumination buffer for a specular reflection", raised in resolution with each pixel weighted by "a level of similarity of specular reflection lobes" | **New.** Appears absent: B reads one target texel, nearest, with no lobe weighting | Medium-high |
| US10510185B2 | AMD | Granted 2019-12-17, ≈2037-08-25 | B | 1: rasterizer quads into a tile buffer, downsampled to coarse quads by a shading rate, shaded, modulated with fine rasterization data | **New.** Appears absent | High |
| US9710881B2 | Sony | Granted 2017-07-18, ≈2034-10-26 | B | 1: per screen subsection, metadata fixing "a number of active pixels per unit area" used in rasterization | **New.** Appears absent | High |
| US10235799B2 | Microsoft | Granted, ≈2037-06-30 | B | 1: variable-rate shading, then a coordinate buffer and "an execution control buffer" driving deferred passes on shaded subsets | **New.** Appears absent | High |
| US11869115B1 | Qualcomm | Granted 2024-01-09, ≈2042-08-13 | B, C3 | Per screen bin, a VRS rate from primitive density (abstract; claims not read) | **New.** Appears absent on the abstract: per bin, primitive density, no per-draw choice | Medium |

Read in `ip-research.md`, not re-read here: ARM US10636213B2 (stored intersections), Autodesk US9355475B1 (reprojection), EA's SSR-to-ray-trace family and Microsoft US10586375B2 / US10360720B1 (Lever B). The brief keeps no intersections or history, so those readings hold. NVIDIA US10424074B1 (priority 2018-07-03; continuation US10699427): claim 1 reports "a texture footprint" in answer to "a texture footprint query request" from a shader. The brief issues no footprint query, so this appears absent.

**Changes from the first pass:**
- '451 claims 5–6: the description's definition firms the "leans absent" reading for static fluid faces. It opens a new trigger: fluid on a mover (§Lever A fences).
- Qualcomm '166: verified prior art discloses the sizing element for triangle patches. The crux row stands; the counsel question gains a reference. EP's claim 1 drops the visibility element.
- Qualcomm '715 is new. With Intel '475 it fences visible-cell compaction from two sides.
- Far Cry 4: the slide text read does not show the cubemap G-buffer relight. The row rests on FFXV's description, now verified.
- Citation corrections: AMFS is SIGGRAPH 2014 (ACM TOG 33(4)), not HPG; '856 granted 2026-04-14, not 2026-04-01; Far Cry 3 DRTV was presented at GDC 2012 by Stefanov (Gilabert presented it at CEDEC 2012).

No reading moved toward present. No crux row changed. No existing fence weakened.

**Changes in the cell-path pass (2026-11-22):**
- Shape B charted (§Shape B). No filing found reads on it as '166 reads on shape A.
- New for shape A: the Oxide families. 2013 priority: appears absent on its MIP-chain element, and is art on sizing by pixel coverage. 2019 priority ('783, '208): appears absent on the miss buffer and the hashed GPU allocation. '715's EP twin is pending.
- EP4409531B1: designations and national entries recorded; no opposition event seen; the opposition window closes about 2027-04-22.
- No '166 or '715 continuation found. No SAS filing found. No filing found claims the inline hidden-cell test or a size-keyed path switch with hysteresis.
- No source contradicted an existing row. No reading moved toward present. Nothing found clears counsel questions 3 or 5.

## Lever A: precompute

### Tencent US12293451B2

Claim 1, verbatim (also 9 and 17): "determining light source change information when a light source changes in a virtual scene; determining a current light source projection coefficient corresponding to the changed light source according to the light source change information; determining an indirect illumination value of a target pixel point in the virtual scene according to a radiance transfer parameter corresponding to the target pixel point in the virtual scene and the current light source projection coefficient; determining a direct illumination value corresponding to the target pixel point under the changed light source; and performing illumination rendering on the target pixel point according to the direct illumination value and the indirect illumination value."

| Element | Per-fragment ratio (shipped) | Precompute (Lever A) | Reading |
|---|---|---|---|
| (a) light source change information | current_l compared with ref_l, its capture-time value. Col. 5 says change information is found by comparing with the pre-change light | Same comparison, made per probe | Leans present for both, as for id 27. Evaluating every frame is no lever: the prior record says "per-frame vs event-triggered evaluation" is not one |
| (b) current projection coefficient | `current_l / ref_l − 1`, RGB, per light | Same | Arguable for both: the id-27 crux (a scalar or RGB as "coefficient"; paras. 32, 35, FIG. 4) |
| (c) indirect value from the pixel's radiance transfer parameter × coefficient | share_{l,k}: the baked per-light, per-lobe fraction of the capture's radiance. It stays fixed while the light changes (col. 17's premise) and is read per fragment through d | Same shares, same coefficients; the multiply moves to the probe | Arguable, nearest risk. Reflecting the lit static world is light off a surface, so "indirect" fits better here than for glow (`ip-delta-tiles.md` §2). The precompute adds nothing to this element |
| (d), (e) direct under the changed light; render both | The water surface's own runtime lighting, then the composite | Unchanged | Present |

**"Dynamic object" in the description.** Every passage found, verbatim:
- "The dynamic object in the virtual scene refers to a non-fixed object in the virtual scene, that is, may be a mobile object that moves in the virtual scene, and at least one of a position, a direction, or the like of the dynamic object is changeable."
- "The static object in the virtual scene refers to a fixed object in the virtual scene, and its position and direction do not change."
- Objects "may include at least one of terrain, a house, a tree, a character". Static pixels take transfer from a lightmap ("one lightmap may correspond to an object surface map corresponding to one static object"); dynamic pixels from probes ("Baking data of a dynamic object in the virtual scene is stored in a light probe").
- No passage names water, animation, texture scrolling or surface deformation.

The test is the object's position or direction. A fluid face on static brush geometry keeps both fixed. Its scrolling texture coordinates and per-pixel tilt are not the object's position or direction. "Or the like" leaves room for counsel. The prosecution record is unread, so whether the term was argued is unknown.

Dependent claims touched by the move:

| Claim | Text (verbatim, abridged with …) | Precompute | Reading |
|---|---|---|---|
| 5 / 13 | "obtaining, for the target pixel point belonging to a dynamic object in the virtual scene, a light probe matching the target pixel point of the dynamic object; and obtaining the radiance transfer parameter … from the light probe" | Fluid faces are static brush geometry: fixed position and orientation. Texture coordinates and pixel tilt animate | **Appears absent** for static fluid faces, under the description's definition. **Fluid carried by a mover would change position**, which reads on "dynamic object" |
| 6 / 14 (on 5) | "updating light brightness corresponding to directions on the light probe according to the current light source projection coefficient and the radiance transfer parameter; performing interpolation on the updated light brightness …; and determining the indirect illumination value … according to the light brightness after the interpolation and a normal direction of the target pixel point" | S_k is a per-probe, per-direction value from coefficient × share, rewritten each frame. w_k(d) interpolates the lobes. d is the reflected lookup direction, built from the normal and the view. S_k is a relative change multiplied onto captured colour, not a brightness | Moves toward claim 6's first two elements. The third is arguable (reflected direction vs normal; relative change vs brightness). Falls with claim 5 for static faces |
| 3–4 / 11–12 | Static pixel: transfer "from the lightmap" | Shares live in probe records, not a lightmap | Appears absent |
| 2, 7, 8 | Illumination transform matrix; basis projection with virtual lights; a distant light's direction change | None | Appears absent |

**Reading.** Lever A adds no claim-1 element. It leaves the shipped ratio's exposure where it is, and that exposure has no counsel answer yet. It adds claim 6's per-direction probe update in substance. Claim 6 needs claim 5, and claim 5 needs a pixel of an object whose position or direction changes. Static fluid faces lack that.

### Tencent WO2026113651A1, update chain

Claim 10 (machine translation): "When the update condition of the reflection probe is met, the illumination information collected by the reflection probe is updated to obtain updated illumination information; wherein the update condition includes at least one of … The light at virtual objects within the target area in the virtual scene changes … From the updated lighting information, obtain the lighting information of the reflected light." Claims 11 and 13 define that information as "color information obtained by coloring the virtual objects … based on the texture information".

| Claim | Precompute | Reading |
|---|---|---|
| 10 / CN 9 | A per-probe state is rewritten every frame before the fluid pass collects the reflection. The ordering is pinned (P1). The tile, the colour the probe collected, is never written. The sums are a per-direction relative modifier, not colour information | Appears absent: what the probe collected is not updated. Nearer than per-fragment: a stored per-probe state now changes with the lights ahead of collection. Counsel |
| 13 / CN 11 | No texture or G-buffer information, no re-shading | Appears absent |
| 14 / CN 12 | No faces updated in turn; every resident probe every frame | Appears absent. Moves toward 14 if the precompute is ever amortised over frames by probe or face |
| 19 | Same as 10 | Same as 10 |

**Search record.** The international search cited four documents, all marked examiner-cited on Google: Fujifilm CN104221370A (EP2833638B1), Tencent CN112870707A, Tencent CN114299220A and 粒界 CN116758208A. Their X/Y/A categories and the written opinion were not seen. None is the Far Cry 4, FFXV or Division art below. CN122115679A shows only publication events (2026-05-29); no examination request is listed.

### Prior art for Lever A

| Publication | Date | What it discloses | Elements | Evidence |
|---|---|---|---|---|
| id Software, Quake `R_AnimateLight` (WinQuake `gl_rlight.c`; called once from `R_SetupFrame`, `gl_rmain.c`) | 1996; GPL 1999 | Each light style's value is evaluated once per frame into `d_lightstylevalue[]`. Every consumer reads that array. Surfaces whose cached style value differs rebuild `blocklights[i] += lightmap[i] * scale` | Per-light, per-frame evaluation shared by all consumers (the brief's per-light rival). A stored per-light basis × current value, recomposed on change | Verified, source read 2026-11-21 (`id-Software/Quake`) |
| Jedi Academy `R_SetupEntityLightingGrid` with q3map2 `-bouncegrid` | 2003; GPL 2007 / 2013 | Σ style value × baked per-style ambient and directed grid values, trilinear, lighting dynamic entities | '451 claims 1, 5, 6 with a scalar coefficient | Verified in the prior record (`gi-rtx-lite/ip-check.md`) |
| Dorsey, Arvo & Greenberg, IEEE CG&A 15(2) | 1995 | Per-light basis images superposed by current intensity | The ratio's algebra | Prior record |
| Stefanov, "Deferred Radiance Transfer Volumes: Global Illumination in Far Cry 3", GDC 2012 (Gilabert gave it at CEDEC 2012) | 2012-03 | Abstract: probes store "spherical harmonics coefficient matrices". "We can relight the probes on the fly, thus supporting illumination under time of day changes, as well as local lighting such as muzzle flashes and explosions." "Shading using the relit probes is done on the GPU in screen space" | '451 claims 1, 6 (stored transfer × lighting, per probe, local lights). The abstract does not say every frame, or where the relight runs | GDC Vault abstract read verbatim; slides need a login |
| Stefanov, "Global Illumination in Tom Clancy's The Division", GDC 2016 | 2016-03 | Slides: "Relight probes every frame on the GPU", async compute on consoles. Local lights: "Evaluate them at each surfel"; point, spot and area lights "Completely dynamic and editable, no rebake required". Each probe stores "an explicit list of surfels that each probe 'sees'". Performance: "Relight two probe sectors every frame" | '451 claims 1, 6; WO 10 (per-frame GPU probe relight with local lights). The sector rotation is art for amortised updates (WO 14) | Slide PDF read (`mrakobes.com/Nikolay.Stefanov.GDC.2016.pdf`) |
| McAuley, "Rendering the World of Far Cry 4", GDC 2015 | 2015-03 | Slide text: environment maps filtered at run time ("filtering environment maps at run time"; "Batching cube map faces … is key for performance") under time-of-day change. **The extracted slide text has no cubemap G-buffer and no relight step.** Speaker notes and video not read | Tencent WO 11–13, only through FFXV's description | Slides read as text (GDC Vault PDF). **Contradicts the first pass**, which charted the relight from the slides' content |
| Elcott, Chang, Miyamoto & Metaaphanon, "Rendering Techniques of Final Fantasy XV", SIGGRAPH 2016 Talks, DOI 10.1145/2897839.2927398 | 2016-07 | On prior work: techniques "which store a mini g-buffer for each cube map and relight them at run time [McAuley 2015]". Its own probes split into "a high-frequency component, represented in cube maps" and "a time-varying low-frequency component represented in spherical harmonics". How the two combine is not stated | WO 11–13 (re-shade stored texture information), dated 2016. FFXV's own split is art for a stored cubemap modulated by a time-varying per-probe term, which is Lever A's shape | Talk abstract read in full (SIGGRAPH history archive) |
| Sloan, Kautz & Snyder, PRT, SIGGRAPH | 2002 | Transfer × lighting coefficients | '451 claim 1 | Prior record |

All five game sources predate Tencent WO's 2024 priority. The Division and Far Cry 3 predate '451's 2021 priority. None was cited in '451's record or WO's search report as far as the pages show.

**Trigger for the Far Cry 4 relight.** Neither the slide text nor FFXV states one. FFXV ties its own variation to "time of day, weather, and other factors".

### Fences for Lever A

| Rule | Avoids | Cost |
|---|---|---|
| The sums live in their own buffer. They never write the probe tile, a "live tile" or any record the capture owns | WO 10, 13, 19 | None (as briefed) |
| Every resident probe, every frame that has a reflecting draw: no faces, octants or probe subsets amortised over frames | WO 14 | None (as briefed) |
| No SH basis, transform matrix or virtual-light projection in the coefficient | '451 2, 7, 8 | None |
| **New.** The precompute serves only fluid faces whose position and orientation are fixed. A fluid volume on a mover, or any moving reflecting surface, takes the per-fragment form, or waits for counsel | '451 5–6 ("dynamic object": "at least one of a position, a direction … is changeable") | None today if no fluid rides a mover; otherwise a per-fragment path for moving fluid |
| If counsel reads claim 6 onto a static animated surface: take the brief's rival instead. Evaluate `current_l / ref_l` once per light per frame (Quake's shape) and keep the share × value sum per fragment, so no per-probe, per-direction state exists | '451 6, element 1; WO 10's "probe information updated" framing | Unmeasured per-fragment cost (`index.md` Path, rival 2). The claim-1 reading does not change either way |

## Lever C: cell path

### Qualcomm US12136166B2, "Meshlet shading atlas"

Inventors Neff, Müller, Steinberger, Schmalstieg. Application 17/934,159. Priority 2021-09-30; filed 2022-09-21; granted 2024-11-05. Assigned from TU Graz to Qualcomm Technologies (recorded 2022-11-04). Family: EP4409531B1, CN118043850A, KR20240093463A, TW202322043A.

**Record.** One non-final action (2024-02-29), a response (2024-06-15), allowance (2024-08-14). Examiner-cited patents: Microsoft's index-buffer block compression family (US20180232912A1, US10417787B2, US20200051286A1, US10991127B2). Non-patent citations, none marked examiner-cited: the PCT search report, Kubisch's Turing mesh shader blog (2018), Mueller et al. 2018 and Neff et al. 2022 (published after priority). Hillesland & Yang 2016, Baker 2016 and AMD '002 are not of record. Reasons for allowance not read.

Claim 1 (16, 25 and 26 are the same steps), verbatim:

| Element | Cell path | Reading |
|---|---|---|
| "divide at least one scene into a plurality of meshlets, each … including a plurality of primitives" | Fluid faces are compile-time brush polygons, each triangulated. The runtime divides nothing | Arguable. Counsel: whether a compiler's face set is the claimed division |
| "identify a visibility of each of the plurality of meshlets …, wherein each … is identified as visible or invisible, wherein each … identified as visible corresponds to at least one of a plurality of pixels in a meshlet atlas" | Faces are clipped to the screen, and a face with no on-screen part packs no rect. The room cull picks draws. The hidden-cell skip acts per cell, after allocation | Leans present (frustum visibility per face decides allocation). **Absent from EP claim 1** |
| "calculate a pair of texture coordinates for each of the plurality of vertices …" | The water's texel grid maps world position to texel cells. Claim 11 covers precomputed coordinates | Leans present. Whether coordinates are per vertex or a per-face affine fit (the spike's form) is a detail counsel may weigh |
| "select a size of each of the plurality of meshlets … based on the pair of the texture coordinates … and based on a perspective projection of each of the plurality of meshlets" | A rect holds the face's texel cells inside the screen clip. Cell resolution is fixed at one per water texel and never scaled by distance or projected size. The projection decides only which cells survive the clip | **Crux, unchanged.** Leans absent if "size" means shading resolution, which the description and claims 4/19 tie to the subtended angle. Arguable-present if it covers a rect's extent after a projection clip |
| "calculate layout information in the meshlet atlas for each … based on the selected size" | Shelf pack, nearest first | Present |
| "shade each of the plurality of pixels in the meshlet atlas based on the calculated layout information" | The cell pass writes reflection colour and weight per cell | Present |
| Claim 8: "render each … identified as visible, wherein the meshlet atlas is sampled to determine a color of each … pixels in an output image" | The fragment reads its cell's record | Present |

**EP4409531B1** (granted 2026-07-22, ≈2042-09-22). Claim 1 has divide, texture coordinates, "select a size … based on … a perspective projection", layout and shade. It has no visibility element. Claims 2–4 and 9–11 add visibility as dependents.

**EP status (Google INPADOC events, read 2026-11-22).** Application EP22792976.7. Designated at A1 and again at B1: all 38 EPC states. Validation and extension requests were deleted (2025-01-08). Intention to grant 2026-02-19; grant fee paid 2026-06-13; B1 published 2026-07-22. National events so far: CH (F10) and GB (FG4D) 2026-07-22, IE (FG4D) 2026-08-12, DE (R096, 602022040282) 2026-08-13. No opposition event and no unitary-effect event appear. The opposition period is nine months from the mention of grant, so it ends about **2027-04-22**. The EPO search report was not read (Register behind a bot check).

**Continuations and SAS filings.** The Official Gazette entry for '166 shows provisional 63/261,909 (2021-09-30) and prior publication US 2023/0101978 A1, with no parent and no child. No continuation or divisional of '166 turned up in Google Patents or web search. No filing claiming Shading Atlas Streaming itself was found. Related Qualcomm filings by the same group, screened by abstract only: US20200364926A1 (adaptive object-space shading; abandoned, charted above), US10504280B2 (accelerated occlusion for streaming PVS), US11132831B1 (multi-view rasterization into a shading atlas), US11875452B2 (billboard layers in object-space rendering). None claims per-frame atlas sizing by projection.

**Shading Atlas Streaming against claim 1.** Mueller, Voglreiter, Dokter, Neff, Makar, Steinberger & Schmalstieg, ACM TOG 37(6), article 199, SIGGRAPH Asia 2018 (cited on '166's face as published 2018-12-04). Read in the authors' preprint (`arbook.icg.tugraz.at`, Schmalstieg_362):

| '166 element | SAS discloses | Reading |
|---|---|---|
| Divide into meshlets of many primitives | "We pack one, two or three adjacent triangles into a patch" (§4.1); grouping runs in preprocessing, and "solo triangles are used only when unavoidable" (§3.1) | Patches of 1–3 triangles, not meshlets. The likeliest point of allowance (reasons unread) |
| Visibility | "The visibility stage renders a patch id buffer, with depth buffer enabled" (§3.1). Shading covers a potentially visible set: "the union of all patches contained in at least one EVS sample" (§3.1) | Disclosed for patches |
| Size from texture coordinates and perspective projection | "Patch sizes are determined based on the projected area of a patch on the screen" (§3.1); "Choosing the level based on screen-space edge lengths and area fulfills this requirement" (§4.3) | Size from projected area is disclosed for patches |
| Layout per frame | "Atlas memory management … is directly derived from the PVS" (§3.1). "Already allocated patches, which remain visible in subsequent frames, should remain unchanged" (§4.1). Lock-free request, provisioning and assignment; reset at I-frames (§4.2) | Disclosed, with allocations persisting across frames |
| Shade the atlas; sample it in the output | Object-space shading into the atlas, then rendering from it | Disclosed |

The cell path differs from SAS in two ways no claim names: it repacks from scratch every frame, and its rect extent comes from a screen clip, not a projected-area level. Neither is a design-around of '166.

**Reading.** The crux stands. SAS shows that sizing by projection was public in 2018 for triangle patches. That bears on what "select a size" could mean and on validity, which is counsel's. EP's claim 1 has one element fewer, so the face-level clip no longer separates the cell path from it.

**Fences:**
- Cell resolution stays fixed by the texel grid. No per-face LOD, mip level or cell size chosen from distance, projected area or the pixels-per-cell estimate. That estimate picks a path only.
- Pack whole texel rows and columns of a face, clipped only to bound the pool. Never resample a face to fit a budget.
- **Counsel question:** does screen-clipping a face's texel rect make its "size" "based on a perspective projection"? The alternative is to allocate whole faces and fall back per fragment for faces that do not fit, which loses large lake faces running behind the camera. Under EP4409531B1 that alternative still leaves the sizing element as the only question.

### Qualcomm US12664715B2, "Sampler feedback for texture space shading"

Garvey and Vrcelj. Application 18/467,666. Priority and filing 2023-09-14; A1 US20250095266A1 published 2025-03-20; B2 published 2026-06-23; ≈2043-10-18. Same inventors and day as the dicing oracle. **New this pass.**

Claim 1 (15 and 28 are the same steps), verbatim:

| Element | Cell path | Reading |
|---|---|---|
| "assign each shading element in a set of shading elements associated with a geometry unit to a corresponding shading element group" | Cells belong to a face; the face is the unit packed | Arguable (cell as shading element, face as group). The description calls shading elements "shadels" (shading elements or mip regions) |
| "identify whether each shading element in the set of shading elements is a visible shading element" | The screen clip decides which of a face's cells enter its rect. The depth test runs later, inside the evaluating invocation, and allocates nothing | Arguable for the clip. Absent for the depth skip |
| "calculate a group offset value for each shading element group … based on a number of visible shading elements" | Shelf pack places each face's clipped rect by its extent and a fixed priority | Leans absent: a rect's extent covers hidden and visible cells alike, and no count of visible cells is formed |
| "calculate an element offset value for each shading element within each of the shading element groups based on a number of visible shading elements in the shading element group" | A cell's place in its rect is its texel-grid coordinate, not a running count | Appears absent |
| "allocate … texture memory to the visible shading elements in each of the shading element groups" | Every cell in the rect gets a slot, seen or not | Leans absent |

Dependents: claim 4 "the geometry unit comprises at least one of a meshlet or a triangle"; claim 6 visibility bits per element; claim 7 a "wave operation" (the description's wave prefix sum); claim 8 mip-region levels; claim 12 a depth-equals test before shading texels; claim 13 an address "within a shading atlas". The description says allocation may bump-allocate per frame.

**Reading.** Appears absent while the pool allocates whole clipped rects and cells keep their grid offsets. It is the allocation-side twin of Intel '475: compacting visible cells by count, by prefix sum or otherwise, would add the missing offset elements.

**Record.** Three notices of allowance (2025-05-19, 2025-12-12, 2026-03-31), each followed by the case returning to the examiner; issue fee verified 2026-06-01. Non-patent art of record includes Hillesland & Yang 2016, Burns 2010, AMFS 2014, Mueller 2018, Baker & Jarzynski 2022 and an Intel sampler-feedback talk (2021); one examiner-cited item is Molnar et al. 1994. Family: WO2025058867A1, EP4778033A1 (EP24776069.7, pending, published 2026-07-22), CN121866591A, KR20260070408A, TW202512105A. EP claims not read; the description's Aspect 1 carries the same count-based offsets. A pending AMD application, 18/477,375 ("Hybrid deferred decoupled rendering"), stands rejected under §103 over Garvey ('715's A1): an examiner reads '715's publication as art for shade-space visibility and budgeted shading. Reasons for allowance not read.

**Fences** (with Intel '475's): no visible-count or prefix-sum compaction of cells into the pool; no per-cell visibility bit stored for allocation.

### Intel US10699475B1, "Multi-pass apparatus and method for early termination of graphics shading"

Barczak, Xiao, Apodaca, Raoux, Brownlee, Liktor. Application 16/235,517. Priority and filing 2018-12-28; granted 2020-06-30; active; ≈2038-12-28 (Google). 4th-year fee paid 2023-09-20.

**Record.** Examiner-cited: Apple US20070165035A1 (deferred shading pipeline) and Qualcomm US20180189923A1 (draw call visibility stream). No texture-space paper is of record: not Hillesland & Yang, Burns, Liktor or Mueller. Office actions not shown on Google; reasons for allowance not read.

Claim 1, verbatim from the grant PDF. Claim 10 (method) and claim 19 (medium) have the same steps:

| Element | Hidden-cell skip | Reading |
|---|---|---|
| "graphics processing circuitry to execute a sequence of visibility testing operations related to texels within a texture domain to generate visibility results" | Each cell's screen footprint is tested against the depth pre-pass. Claim 6 adds "compare the depth data with stored depth data" | Present |
| "a register or memory to store a texel mask" and "texel mask update circuitry/logic to update the texel mask based on the visibility results, the texel mask comprising a plurality of bits to indicate visibility of the texels …" | No mask. The test result lives in the invocation that evaluates the cell | Appears absent |
| "set a first bit to indicate whether any bits in the texel mask indicate a visible texel" | No any-visible flag | Appears absent |
| "a shader dispatcher to initiate conditional dispatch operations only if the first bit is set …, the shader dispatcher is to dispatch texel shaders for only those texels that the texel mask indicates may be visible" | Every packed cell is dispatched. A hidden cell runs and returns early | Appears absent |
| "a plurality of execution units (EUs) to execute the shaders dispatched" | GPU compute | Present |

Dependent claims 3–4 and 12–13 cover a bit per n×n tile; claims 6 and 15 a depth compare; claims 7 and 16 adaptive multi-frequency shading.

**Prior art on the mask and dispatch** (verified this pass; none of record):

| Publication | Gate on which samples get shaded | Nearest '475 element |
|---|---|---|
| Liktor & Dachsbacher, I3D 2012 | Global cache: a full-screen pass will "set a flag for the referenced shading samples" so later passes "ignore invisible samples" (§5.1). Shading kernels "only execute for samples that are marked as visible" (§5.3). Per-tile variant: the z-buffer from the first pass ensures "only the visible fragments will execute this step" and "can be combined with the standard z pre-pass" (§5.2) | Per-sample visibility flags gating shading execution after a depth pass. Nearest art found to the mask-and-conditional-dispatch combination |
| Hillesland & Yang, EG 2016 | Tiles of 8×8 texels not recently cached are appended to a work list (`ShadeWork.Append`, Algorithm 1); "A thread group shades an 8x8 tile of texels" | A compacted list of needed tiles, i.e. selective dispatch. No mask; depth use not stated |
| Burns, Fatahalian & Mark, HPG 2010 | Fragments passing early Z go to a buffer; grid samples they need "are tagged as 'requested' if they are not already shaded"; "if any sample in a quad is requested, the entire quad is shaded" (§5) | Per-sample request tags gating shading, at quad granularity |
| Ragan-Kelley et al., ACM TOG 2011 | Shading "triggered by visible samples"; "shading values are computed only when requested by a visible visibility sample" (§3.4); a memoization cache, no mask | Lazy shading on visibility, no mask |
| Haar & Aaltonen, SIGGRAPH 2015 Advances | Cull clusters "using last frame's depth pyramid", then a second phase against a refreshed pyramid; "Depth pre-pass with best occluders"; "GPU culling outputs cluster list & drawcall args"; per-cluster "Triangle Mask" | Visibility masks and compaction into indirect arguments, for geometry rather than texels |

**Reading.** As briefed, the skip appears to lack the mask, the any-visible bit and the selective dispatch. The brief's own acceptance row ("dispatches its cells yet evaluates none") documents that. Compacting visible cells or tiles into an indirect dispatch, or skipping a face's dispatch when nothing is visible, would add the missing elements. Liktor 2012 and Hillesland 2016 bear on validity of that combination; that is counsel's question.

**Fences:**
- The visibility test runs inline, in the invocation that evaluates the cell.
- No pass writes a per-cell or per-tile visibility bitfield that a later dispatch or allocation reads.
- No any-visible flag gates a dispatch.
- No visible-cell compaction into indirect dispatch arguments or into the pool (Qualcomm '715).
- Owner IP review before any of these is built.

### Hidden-cell test, inline (cell-path pass)

Search: a conservative depth test of a texel, patch or cell footprint, run inline in the shading invocation, with no stored mask and no compaction, for texture- or object-space shading. Web search on `patents.google.com` and the citation tables of '166, '715 and '475.

**No filing found claims it.** The nearest claim language sits under independent claims the skip lacks:

| Filing | Depth element | Depends on | Reading |
|---|---|---|---|
| Intel '475 claims 6, 15 | "compare the depth data with stored depth data" | Claims 1 and 10: stored texel mask, any-visible bit, conditional dispatch | Falls with claim 1 (above) |
| Qualcomm '715 claim 12 | A depth-equals test before shading texels | Claim 1: offsets from counts of visible elements, allocation to visible elements | Falls with claim 1 (above) |
| Imagination US20180197325A1 (tile-based tessellation) | A primitive's minimum depth against a tile's stored depth range, before hidden-surface removal | Screen-space binning of primitives | Screened by description; not object-space shading |

**Prior art for the inline test:**

| Publication | Date | Discloses | Evidence |
|---|---|---|---|
| Greene, Kass & Miller, "Hierarchical Z-Buffer Visibility", SIGGRAPH | 1993 | Conservative occlusion of a bound against a depth pyramid | Bibliographic |
| US6480205B1, "Method and apparatus for occlusion culling in graphics systems" | Not checked | Hierarchical z-buffering that "performs v-query tests on portals and bounding boxes" | Google Patents description; dates, owner and status not checked |
| Liktor & Dachsbacher, I3D 2012 §5.2 | 2012-03 | A depth pass first, so "only the visible fragments will execute this step"; combinable "with the standard z pre-pass" | Read (above) |
| Haar & Aaltonen, SIGGRAPH 2015 Advances | 2015-08 | Bound-versus-depth-pyramid tests from a "Depth pre-pass with best occluders" | Read (above). Their output is a compacted list, which the skip must not copy |

**Reading.** The inline, maskless test appears in no claim found. Its prior art is old and public. The fences above keep it that way.

### Other texture-space filings

| Filing | Independent claim (verbatim, abridged with …) | Cell path | Reading |
|---|---|---|---|
| AMD US9728002B2, "Texel shading in texture space" (Hillesland) | "selecting a subset of the texels for shading based on previously cached texels that were shaded for a second frame; … caching the shaded subset of the texels with the previously cached texels" | Written and read in one frame; every read record is written that frame. The no-history gate covers the pool | Appears absent. Keep the gate |
| Intel US10535186B2 (Clarberg) | "pixel shading computation logic to facilitate computation of shading rate in a first pass …; and shading pass logic to facilitate texel shading operations in a second pass using the shading rate, … on a procedural texture, … to query the procedural texture to determine whether relevant texels … have already been shaded" | Path chosen per draw on the CPU from geometry. Cells are evaluated unconditionally, with no shaded-yet query | Appears absent. Never add a per-cell "already shaded" check or GPU-computed rate |
| NVIDIA US9754407B2 (Patney et al.) | "defining an object-space shading grid for at least one of the … triangle primitives …, wherein a shape of an object-space shading element corresponds to a shape of the at least one triangle primitive" | Square cells on the water texel grid | Appears absent |
| Qualcomm US12602856B2, "Dicing oracle for texture space shading" (granted claims 1, 15, 28) | "obtain an indication of a UV parameterization for each of a set of geometry units and a target number of pixels per region …; render the set of geometry units in order to obtain a second set of UV coordinates for a screen space and a set of derivatives …; calculate a resolution for a mip region map based on the obtained set of derivatives … and the obtained target number of pixels per region …; and output an indication of the calculated resolution". Claim 10: "dice the set of geometry units into a set of shadels" | The pixels-per-cell estimate comes from camera, extent and face geometry, with no render and no derivatives. It picks a path, not a resolution. No mip-region map | Appears absent |
| Qualcomm US20260203991A1 (continuation of '856; pending) | "calculate a resolution for a multum in parvo (mip) region map based on a set of derivatives for a set of visible meshlets associated with a screen space; and output an indication of the calculated resolution" (1, 17, 20) | No mip-region map; resolution fixed by the texel grid; the estimate yields no derivatives | Appears absent. Broad if granted: any screen-derivative resolution for visible geometry units would read on it |
| Qualcomm US20200364926A1, "Adaptive object space shading" (Mueller, Neff, Schmalstieg) | Shade a primitive when the difference between its first- and second-frame shading values exceeds a threshold | No history | Abandoned. Prior art only |
| Meta US11721064B1 | A server builds an atlas with view-dependent and view-independent parts, sends it to a client, and regenerates only the view-dependent subset for a second viewpoint | Local, one frame, one view | Appears absent |
| Meta US12322029B2 (continuation of US11676324) | A texture atlas with a region per primitive, "the mapping is encoded based on a hierarchical organization of the texture atlas"; render visible portions from it (Official Gazette excerpt) | No hierarchical mapping; a flat shelf pack | Appears absent |

### Oxide Interactive decoupled-shading family

Baker and Kipp, Oxide Interactive (the Ashes of the Singularity engine). Priority 2013-11-11. **New this pass.** It is the patent side of Baker's GDC 2016 talk, which the chart already uses as prior art.

US10198856B2, "Method and system of anti-aliasing shading decoupled from rasterization". Application 14/076,604; granted 2019-02-05; 4th-year fee paid 2022-07-20 (small entity); ≈2034-03-05. Claim 1 (12 and 23 the same), verbatim:

| Element | Cell path (A) | Reading |
|---|---|---|
| "for each graphic object …: selecting a set of points … for shading via a graphic processor unit (GPU), the corresponding one of the plurality of graphic objects being represented as a mesh in object space" | Cells of a face's texel grid, on the GPU | Arguable |
| "determining at least one shading parameter …; precalculating a shading parameter image …; storing the shading parameter image in a memory of the GPU" | The cell pass reads probe records, sums and the face record. No per-object parameter image is precalculated | Leans absent |
| "sorting the plurality of graphic objects to minimize state changes of a shader program and allocating an image space in the memory of the GPU" | Faces pack nearest first for priority. Nothing sorts to save shader state | Appears absent on the sort's purpose. Allocation present |
| "shading … according to a sorting order … to produce a shaded mesh having corresponding shaded object space values" | The cell pass shades in pack order | Arguable |
| "anti-aliasing the shading of the shaded mesh by calculating a MIP chain of the shaded mesh; and rasterizing … the MIP chain of the shaded mesh" | No MIP chain. The fragment loads its own cell's record | **Appears absent** |

Claim 4 (15): "a plurality of reduced sized images is assigned to the corresponding one of the plurality of graphics object … based on an estimated pixel coverage". That is the sizing the cell path's fence forbids. It falls with claim 1.

US10198788B2 (CIP, application 14/709,064; granted 2019-02-05; 4th-year fee paid 2022-07-20; ≈2033-11-11): claim 1 adds shading "at a first frequency" and rasterizing at a second, beside the same sort and MIP-chain steps. Appears absent: the cell pool is shaded and read in one frame, with no MIP chain. Its continuation US20190295214A1, whose claim 1 had only the frequency split and no sort or MIP chain, is **abandoned**.

**Reading.** Appears absent on the MIP-chain element. The family dates the "size an object's shading image by its estimated pixel coverage" idea to a 2013 priority, beside SAS (2018) and Baker's talk (2016), for counsel's '166 question.

**Later family: decoupled object-space shading (priority 2019-10-16).** Baker, Kipp, Heazlett and Osefo; the engine generation in Baker & Jarzynski, EGSR 2022 (already of record in '715). **New this pass.** Claims read from Google Patents ('783) and the Official Gazette ('208).

US11436783B2 (application 17/073,193; granted 2022-09-06; ≈2040-11-19). Claim 1, verbatim:

| Element | Cell path (A) | Reading |
|---|---|---|
| "continuously executing a raster loop at a first frequency, the raster loop collecting meshes … and rasterizing the meshes into the frame images" | The fluid pass draws the water each frame | Present |
| "continuously executing a shade loop at a second frequency, the shade loop generating shaded portions of the meshes and storing the shaded portions … in a shadel storage buffer" | The cell pass runs once per frame, before the fluid pass, into the pool | Arguable: one pass per frame, but the claim does not say the frequencies differ |
| "responsive to the raster loop determining that a shaded portion of a respective mesh is stored in the shadel storage buffer, … read the shaded portion … and … rasterize" | A cell-path draw reads its cells | Present |
| "responsive to the raster loop determining that the shaded portion … is not stored …, (i) generate the shaded portion …, (ii) store the shaded portion … in a shadel miss buffer, (iii) read the shaded portion … from the shadel miss buffer, and (iv) rasterize" | A draw that does not fit is chosen per fragment on the CPU before the pass. Its fragments evaluate the probe term themselves and store nothing | **Appears absent**: no per-draw miss detection by the raster loop, no miss buffer |

Claims 3 and 6 are system forms (6 read only in part).

US12154208B2 (division, application 17/726,693; prior publication US 2022/0245883 A1). Claim 1, verbatim from the Official Gazette:

| Element | Cell path (A) | Reading |
|---|---|---|
| "determining, via the GPU in a raster loop at a first frequency, one or more shadels to be computed for the frame image based at least on the one or more input attributes …, each shadel being a shaded portion of the mesh" | Cells chosen on the CPU from the pack | Leans absent (CPU, not a GPU raster loop) |
| "allocating, via the GPU in a shade loop at a second frequency, space in a shadel storage buffer … by subdividing the shadel storage buffer into a plurality of subdivisions" | CPU shelf pack | Appears absent |
| "performing atomic exchange increments on respective ones of the plurality of subdivisions, … using GPU hardware atomic operations, … using hash functions to map the one or more shadels to the respective ones of the plurality of subdivisions" | None | **Appears absent** |
| "populating, via the GPU, a work queue buffer, the work queue buffer containing a list of commands to be executed to compute each of the one or more shadels" | The dispatch size comes from the pack alone; no command list | **Appears absent** |
| "computing … the determined one or more shadels …; and rasterizing … the shaded mesh into the frame image" | Cell pass, then the fluid pass | Present |

**Reading.** Both appear absent. Shape B appears absent from both: it stores no shaded portion of a mesh in object space.

**Fences:** no MIP chain built over the cell pool; no frequency split between cell shading and the fluid pass; an overflowed draw evaluates per fragment and writes no fallback buffer; allocation stays a CPU pack, with no GPU hashed or atomic allocation and no GPU work queue.

### Tencent WO2026113651A1 against the pool

| Claim | Pool | Reading |
|---|---|---|
| 1 / 20: "Obtain the texture image of the first virtual object, and determine the color value of the image pixel based on the texture image and the lighting information" | The pool is a texture-space image of the probe term, the lighting side. The water's albedo still comes from its material per fragment. The description defines "texture image" as the object's unshaded screen-space image | No new element. Claim 1 is already arguably present (`plans/done/reflection-probes/ip-check.md` §1). The pool sits on the "lighting information" side, not the "texture image" side |
| 10 / 19: probe-collected information updated before collection | The pool holds per-cell lookups from the water, not the probe's record of its range | Appears absent (low concern) |

### Path choice (C3)

No filing found claims a per-draw choice between texture-space and per-pixel evaluation keyed to on-screen pixels per texel, with or without hysteresis.

| Filing | Independent claims | Path choice | Reading |
|---|---|---|---|
| Microsoft US10152819B2, "Variable rate shading" (1, 13, 20) | "determining, by the GPU, respective sampling rate parameter (SRP) values for one or more regions of one or more primitives …"; shading fragments by sample sets and SRP values, "wherein the shading comprises specifying a different set of frame buffer pixels or multi-samples that are to be covered by a color value". Claims 5/17: a per-object SRP | The choice is between two evaluation paths, not a count of frame-buffer samples a colour covers. Every pixel still shades once on either path | Appears absent |
| Microsoft US10147227B2, "Variable rate shading" (1, 9, 16) | SRP vertex values "based at least on an application-based source of initial SRP values or one or more tessellation factors"; "determining, by the GPU at a rasterization stage, map coordinates based on coarse scan converting the primitive"; an SRP-map lookup; "shading, by the GPU at a pixel shader stage, the fragment … based on the respective fragment variable SRP value". Claims 3/11/18: map coordinates include texture coordinates | No SRP, no SRP map, no rasterizer-stage rate | Appears absent |
| Intel US10109039B1, "Display engine surface blending and adaptive texel to pixel ratio sample rate" (1, 8, 14, 20) | Each needs "a gaze monitor to: monitor a gaze of a user … and identify a focus area" and a display engine that blends surfaces, with offload "when an offload threshold is satisfied". Dependents 6–7, 11–12, 17–18, 23–24: a second sample rate "when the texel size satisfies a texel to pixel ratio threshold", the threshold indicating "a number of texels mapping to a pixel in the scene" | No gaze tracking, no display-engine offload. The texel-to-pixel element sits only under those claims | Appears absent |

**Prior art for C3:**

| Publication | Date | Discloses | Evidence |
|---|---|---|---|
| Philips EP1652151A1 / US7532220B2, "System for adaptive resampling in texture mapping" | Priority 2003-07-30 | Per primitive, "select a resampling algorithm", "the selection being in dependence on a size of the primitive"; a texture-space and a screen-space resampler | Google Patents claim 1. EP refused; US expired, fee related |
| Hillesland, "Texel Shading", GPUOpen article | 2016-07-21 | "Note that texel shading can be selectively applied on a per-object, or even per-fragment basis." "The fragment-level choice happens at the time of the first geometry pass" | Read in full (`gpuopen.com/learn/texel-shading/`) |
| Hillesland & Yang, EG 2016 | 2016-04-26 | Texel-shaded models beside conventionally rendered ground and skybox: per object. Mip level chosen "on a per-fragment basis" | Read in full |
| Baker, "Object Space Lighting", GDC 2016 | 2016-03 | "Any object can bypass the object space lighting system"; "Trees, particles which have very simple shaders get no benefit out of it, thus default back to forward". The criterion is shader cost, not pixel density | Slide PDF read |
| Funkhouser & Séquin, SIGGRAPH | 1993 | Hysteresis in level-of-detail switching | Bibliographic |
| Intel US10535186B2, background | Granted 2020-01-14 | On coarse pixel shading as conventional: the rate "remains constant per draw call or determined based on a radial falloff" | Google Patents description. A patent's own statement that a per-draw shading rate was known |
| Oxide US10198856B2 claim 4 | Priority 2013-11-11 | Per-object image size "based on an estimated pixel coverage" | Granted claim (above) |
| bgolus, Unity forum, "The Quest for Efficient Per-Texel Lighting" | 2018-05-05 | Per-texel lighting by snapping to the texel centre: from the snapped UV offset "you now have the texel's center in world space!" | Thread read |

**Hysteresis sweep (cell-path pass).** Search: switching a shading technique or rate per object or draw, keyed to on-screen size or texel density, with a hysteresis margin. **No filing found claims the combination.** Screened:

| Filing | What it switches, on what | Reading |
|---|---|---|
| Qualcomm US11869115B1, "Density driven variable rate shading" | A VRS rate per screen bin from primitive density, through a lookup table; no hysteresis described | Appears absent (abstract and description; claims not read) |
| Qualcomm US20130135329A1, "Switching between direct rendering and binning" | Render mode by a score over a threshold | Different subject (binning mode). Not charted |
| Magic Leap US11468640B2 | Hysteresis between display depth planes by vergence depth | Different subject. Shows hysteresis margins are routine in rendering switches |
| Pixar US8281281B1 | Level-of-detail transition points set by users | Different subject |

Hysteresis around a threshold is old art for level-of-detail switching (Funkhouser & Séquin 1993). The Philips row covers the size-keyed per-primitive choice; Hillesland 2016 and Baker 2016 cover the per-object texel-versus-pixel choice.

### Prior art for Lever C

| Publication | Date | What it discloses | Bears on | Evidence |
|---|---|---|---|---|
| Cook, Carpenter & Catmull, REYES | 1987 | Shading on object-space micropolygon grids before visibility | All texture-space filings | Bibliographic |
| id Software, Quake surface cache | 1996 | Per-surface texel images composed from lightmaps and styles, then sampled by the rasteriser | Texture-space shading in general | Source verified (`gl_rsurf.c`) for the light composition; the software cache is from memory |
| Greene, Kass & Miller, "Hierarchical Z-Buffer Visibility", SIGGRAPH | 1993 | Conservative occlusion of a bound against a depth pyramid | C2's footprint test | Bibliographic |
| Burns, Fatahalian & Mark, "A Lazy Object-Space Shading Architecture with Decoupled Sampling", HPG 2010, pp. 19–28 | 2010-06 | Visibility first; grid samples needed by early-Z survivors tagged "requested"; quads with any request shaded | Intel '475, Qualcomm '166 | Read (Eurographics digital library PDF) |
| Ragan-Kelley, Lehtinen, Chen, Doggett & Durand, "Decoupled Sampling for Graphics Pipelines", ACM TOG 30(3), 2011, DOI 10.1145/1966394.1966396 | 2011 | "decoupled sampling lazily evaluates shading only as required by visibility" (§3.3) through a memoization cache | Intel '475 | Read (author PDF) |
| Liktor & Dachsbacher, "Decoupled Deferred Shading for Hardware Rasterization", I3D 2012 | 2012-03 | Compact geometry buffer; per-shading-sample visible flags; kernels run only for flagged samples; optional z pre-pass | Intel '475 (mask, conditional dispatch) | Read (author preprint) |
| Clarberg, Toth, Hasselgren, Nilsson & Akenine-Möller, "AMFS: Adaptive Multi-Frequency Shading for Future Graphics Processors", ACM TOG 33(4), 141:1–141:12, SIGGRAPH 2014 | 2014-08 | Shading on small local grids in parametric patch space, reused across nearby samples; locally adaptive rates | Intel '475 cl. 7 (named in the claim) | Abstract only; the PDF host refused. **Venue corrected** from "HPG 2014" |
| Haar & Aaltonen, "GPU-Driven Rendering Pipelines", SIGGRAPH 2015 Advances | 2015-08 | Two-phase occlusion of clusters against a depth pyramid from a best-occluder depth pre-pass and last frame's reprojected depth; compacted cluster lists and indirect draw arguments; per-cluster triangle masks. The slides do not say "compute shader" or "Hi-Z" | C2; the compaction fence's alternative | Slide PDF read |
| Baker, "Object Space Lighting", GDC 2016 (Ashes of the Singularity) | 2016-03 | "During submission step, the estimated projected area of the object is calculated"; objects scaled down if requests exceed space; "Objects which are not visible are culled early in pipe". Compute only as future work ("Using async compute"). Shading frequency may differ from raster ("30 fps for shading") | Qualcomm '166 (size from projection, per object); C3 | Slide PDF read (`oxidegames.com`) |
| Hillesland & Yang, "Texel Shading", EG 2016 Short Papers, pp. 73–76, DOI 10.2312/egsh.20161018 | 2016-04-26 | A camera-view raster "records texel accesses as shading work"; 8×8 tiles appended to a work list; compute shades them; tiles reused if "no more than N frames old"; mip level per fragment, rounded up | Intel '475 (selective dispatch), Qualcomm '166, AMD '002 (history), C3 | Read in full |
| Mueller et al., "Shading Atlas Streaming", ACM TOG 37(6):199 | 2018-12-04 | Patch-id visibility with depth; patch size from projected screen area; persistent atlas allocation; 1–3-triangle patches (detail above) | Qualcomm '166 (on its face) | Read (authors' preprint) |
| NVIDIA Turing texture-space shading | 2018 | Footprint query finds texels; shade in texture space; sample in screen space | Intel '475, '186 | NVIDIA developer blog, search excerpt |

## Shape B: screen-space probe target

The rival to the cell pool (C1–C2). Shape B's pass is the shipped screen-space layer's reflection pass with the probe term added: the same water draws, a target at a fraction of the scene extent, every target pixel of a cell marching the cell's ray, and a nearest read at the cell centre's screen projection (`rendering_pipeline.md` §7.3a Reflection pass, Rays, Composite read). `plans/done/fluid-ssr-reflections/ip-check.md` charted that layer. This section charts only what the probe term in such a target adds, and the families the owner named.

| # | Element of shape B |
|---|---|
| B1 | A separate pass after the depth pre-pass rasterizes the water draws into a target at ½ or ¼ of the scene extent |
| B2 | Each target pixel inside a water texel cell evaluates that cell's ray, i.e. the probe term at the cell point |
| B3 | The full-resolution fragment reads the target nearest at its cell centre's screen projection |
| B4 | A draw whose cells shrink toward one target pixel falls back to per-pixel evaluation, per draw, on the CPU, with hysteresis (as C3) |

### Reduced-resolution reflection or lighting buffers

| Filing | Independent claim (verbatim, abridged with …) | Shape B | Reading |
|---|---|---|---|
| Square Enix US9342926B2 / EP2797054B1 (Tokuyoshi; JP priority 2013-04-23) | EP claim 1: "An information processing apparatus that generates an indirect illumination buffer for a specular reflection used for rendering a 3D scene"; "determination means for determining a viewpoint parameter for which the indirect illumination buffer is to be rendered"; acquire a buffer at lower resolution; "said generation means generates a pixel of the indirect illumination buffer generated by the increase in resolution based on at least a level of similarity of specular reflection lobes". US claim 1, from a search excerpt of Justia's text (not read in full): a pixel generated "based on at least a level of similarity of specular reflection lobes, between an object corresponding to the pixel and an object corresponding to a pixel of the low resolution" buffer | B1–B2: a reduced-resolution buffer of reflected light (arguable as "indirect illumination for a specular reflection"). B3: one nearest texel at the cell centre; no full-resolution buffer is generated, and nothing is weighted by lobe similarity | **Appears absent** on the lobe-similarity upsampling element. Arguable on the buffer element alone |
| Microsoft US10360720B1 dependents 11–13 (from `fluid-ssr-reflections/ip-check.md`) | A reduced-resolution image for one reflection technique ("one-quarter") | B1 | Present in substance, but they depend from claim 10, which needs a reflected-toward-camera determination shape B never makes |

The Square Enix description also re-samples at higher resolution where the summed weights are low. Shape B's fallback (B4) is per draw from a CPU estimate, never per pixel from weights.

### Coarse pixel and variable-rate shading families

All claim a change inside the rasterizer or its rate controls. Shape B renders an ordinary separate pass into a smaller target and reads it.

| Filing | Independent claim (verbatim, abridged with …) | Shape B | Reading |
|---|---|---|---|
| AMD US10510185B2 (granted 2019-12-17; ≈2037-08-25) | "rasterizing, via a rasterizer, a triangle to produce fine rasterization data and a set of one or more quads; accumulating at least one of the set of one or more quads … into a tile buffer …; downsampling quads in the tile buffer to generate coarse quads, the downsampling based on a shading rate …; shading the coarse quads …; and modulating the shaded coarse quads with the fine rasterization data" (1, 9, 17) | No tile buffer of fine quads; no modulation by fine coverage. The full-resolution fragment runs normally and loads one texel | Appears absent |
| Sony US9710881B2 (Cerny; granted 2017-07-18; ≈2034-10-26) | Pixels "subdivided into a plurality of subsections"; per subsection, "metadata determines a pixel resolution for the subsection, wherein the pixel resolution is a number of active pixels per unit area of the screen for the subsection"; parts "in two different subsections have different pixel resolution" (1, 22) | One fraction for the whole target, set by the tier; no per-subsection metadata | Appears absent |
| Microsoft US10235799B2 (granted; ≈2037-06-30) | Shading "at one or more variable shading rates"; "generating … a coordinate buffer"; "generating an execution control buffer"; "applying … one or more deferred passes to the one or more of the subsets of pixels" (1, 8, 15) | No variable rate, no coordinate or execution-control buffer | Appears absent |
| Microsoft US10152819B2 / US10147227B2 | Charted in §Path choice (C3) | Same readings for B | Appears absent |
| Intel US12243125B2, "Controlling coarse pixel size from a stencil buffer" (last of a continuation chain: US10109078 → US10706591 → US11244479 → US11869119 → US12243125; filed 2023-11-22) | "a two-dimensional (2D) texture memory; and logic to: identify a stencil value that is associated with the 2D texture memory, and determine a number of pixels that are to share an invocation of a pixel shader based on the stencil value, wherein the number of pixels is to be controlled via a stencil buffer" (Official Gazette) | No stencil value sets how many pixels share a pixel-shader invocation. B's fluid fragment runs once per pixel and loads one target texel | Appears absent. Earlier links in the chain not read |
| Intel "Decoupled shading pipeline" family (US9569883B2, priority 2013-12-12 → US10553019B2 → US10970917B2 → US11875453B2) | Abstract: "a given frame or picture may have different shading rates", changed in a region "by changing the size of the region". Latest continuation US20240169664A1 (claim 1: render a peripheral region "at a first rate" and another region "at a second rate higher") is **abandoned** | One target fraction for every water pixel; no peripheral or per-region rate | Appears absent on the abandoned continuation's claim; granted claims not read |
| Other Intel coarse-pixel filings: WO2016043909A1 / EP3195269A1 (deferred CPS), US9905046B2 (multi-rate shading to monolithic programs), US10242493 (filtered CPS), US10262455B2 (merging fragments), US11871142B2 (CPS with TAA), US11663774B2, US9916682B2 (variable precision by region) | Not read at claim level. Titles and abstracts place each inside the raster pipeline's coarse-pixel stage | No coarse-pixel raster stage | Not charted |
| Qualcomm US11869115B1 | Per-bin VRS from primitive density (§Path choice) | — | Appears absent on the abstract |

No NVIDIA, Apple, Nintendo or checkerboard filing turned up claiming a reduced-resolution reflection or shading target read back at full resolution. NVIDIA CN107392988B (variable sampling through projected geometric distortion, for lenses) was screened and not charted. The SSR chart's NetEase screen (CN114748874B, CN117496039A, CN115120973B, CN120022596A, CN116637366A) records no reduced-resolution element in its claim summaries.

### Snapped sampling at the cell centre (B3)

No filing found claims sampling a reduced-resolution target at a texel or cell centre projected to the screen. The shipped screen-space layer already does it for pixel style (§7.3a Composite read). Prior art for evaluating light at the snapped texel centre: bgolus, Unity forum, 2018-05-05 (§Path choice prior art). Prior art for per-texel lighting in object space: §Prior art for Lever C.

### Shipped-layer claims that a probe term in the target touches

| Filing | Rule from §7.3a IP rules | What shape B must keep |
|---|---|---|
| Tencent WO2026113651A1 claim 15 | "SSR never reads s, the distance tile or probe colour … F·c·SSR is the same at every s" | If the probe term and SSR share a pass or a target, store them as separate quantities. SSR's coefficient never gains s, and SSR's march never reads the probe's colour or distance tile. A target premultiplied as `c·SSR + (1−c)·s·R` would put s into the stored SSR share |
| NetEase CN114748874B | "The probe lookup is keyed to its own search from the fragment or cell point, never to the SSR hit, its depth or its screen position" | The probe term in shape B's pass runs its own search from the cell point, even where the same invocation has just marched the SSR ray |
| Microsoft US10586375B2 / US10360720B1 | "The march and c read no ray- or view-direction quantity" | Unchanged: the probe term has no technique choice by direction |

### Prior art for shape B

| Publication | Date | Discloses | Evidence |
|---|---|---|---|
| Valient, "Taking Killzone Shadow Fall Image Quality into the Next Generation", GDC 2014 | 2014-03 | Reflections: screen-space ray-trace at "Half resolution", output "hit color / hit mask / glossiness", "Compose with cubemaps", "Ray-trace mask used for blending", "Each stage falls back to next one on miss". Volumetrics "Rendered at half resolution" with "Bilateral upscale to full resolution" | Slide PDF read (`guerrilla-games.com`) |
| Wronski, "Assassin's Creed 4: Road to Next-gen Graphics", GDC 2014 | 2014-03 | Screen-space reflections in "Half resolution buffers" | Search excerpt (Scribd copy); not read in full |
| Stachowiak, "Stochastic Screen-Space Reflections", SIGGRAPH 2015 Advances | 2015-08 | "we ray-trace at half-resolution, and reuse the rays shot from adjacent pixels"; "We can resolve to full resolution" | EA Frostbite page |
| Tokuyoshi, "Specular Lobe Aware Upsampling Based on Spherical Gaussians", SIGGRAPH 2013 poster | 2013-07 | Bilateral upsampling of expensive specular shading from low resolution | SIGGRAPH archive PDF (abstract); the Square Enix patent's own technique |
| bgolus, Unity forum | 2018-05-05 | Lighting at the snapped texel centre in world space | Thread read |
| PostRetro's shipped screen-space layer | Landed before this brief | B1–B3 for the SSR term, charted with no reading present | `fluid-ssr-reflections/ip-check.md` |

None of these evaluates a **probe** term into a reduced target as its own pass. Killzone composes a half-resolution trace with cubemaps sampled in the lighting pass; where its cubemap lookup ran is not stated.

### Reading, A against B

- **Shape B adds no element found in any claim that shape A's '166 chart does not already face.** Every reduced-resolution family found claims something B lacks: lobe-similarity upsampling (Square Enix), coarse quads in the rasterizer (AMD), per-subsection resolution metadata (Sony), execution-control buffers (Microsoft '799), or a direction test (Microsoft '720).
- **Nothing found reads on B as '166 reads on A.** No filing found claims a texture-space-sized allocation for B to meet, because B allocates nothing per face: its target is fixed by the scene extent and tier.
- **B's live constraints are the shipped layer's,** plus the separation rule above if the probe term and SSR share a target.
- **This flags the brief's decision rule.** On the IP axis alone, B carries no counsel question of '166's kind; A's questions 3 and 5 stay open. Nothing found clears A's counsel questions.
- Search depth is uneven: Intel's coarse-pixel claims were not read (Google refused the pages), and the sweep was keyword web search, not a classification query.

**Fences for shape B:**
- The target's fraction comes from the tier (or one fixed value), never from a per-draw, per-region or per-pixel estimate (Sony '881, VRS families).
- The full-resolution read is one nearest load at the cell centre's projection. No weighted upsampling by lobe, normal or depth similarity (Square Enix '926).
- Fallback is per draw from the CPU estimate. No per-pixel re-evaluation chosen by low weights or by a mask (Square Enix description; Intel '475).
- The probe term and SSR stay separate quantities; s never enters SSR's share (Tencent WO 15). The probe lookup keeps its own search (NetEase).
- No coarse-quad or variable-rate raster state on the main fluid draw (AMD '185, Microsoft VRS).

## Fresh sweep

Topics: texture- or object-space shading with depth visibility per texel tile; per-frame atlas or pool allocation; texture-space vs screen-space choice per object; runtime relighting of baked reflection probes by per-light scalars; per-probe lighting sums. Method: web search on `patents.google.com`, then citation tables of every page read. Not a CPC- or assignee-filtered query (§Gaps).

| Hit | Owner | Lever | Outcome |
|---|---|---|---|
| US12664715B2 | Qualcomm | C1, C2 | Charted above |
| US20260203991A1 | Qualcomm | C1, C3 | Charted above |
| US12602856B2 (granted claims) | Qualcomm | C1, C3 | Charted above |
| EP1652151A1 / US7532220B2 | Philips | C3 | Prior art (above) |
| WO2017123321A1 "Texture space shading and reconstruction for ray tracing" | Intel | — | Claim 1 is texel shading inside a ray tracer. No lever ray traces. Not charted |
| US10679405B2 "Object illumination in hybrid rasterization and ray traced 3-D rendering" | Imagination | — | Maintained texture with a per-frame sampling budget. Not charted (claims not read; no lever keeps a texture across frames) |
| US12079897B2 "Visibility generation improvements in tile based GPU architectures" | Qualcomm | — | Binning-pass LRZ depth for draws. Not charted |
| GB2571656B "directional representations of lighting at probe positions" | Imagination | — | Probes refined progressively by traced rays over frames. Not charted (no rays, no history) |
| US12002165B1 "Light probe placement" | Apple | — | Placement. Not charted |

**Cell-path sweep (2026-11-22).** Topics: reduced-resolution screen targets read at full resolution; snapped or cell-centre sampling; per-draw reduced- versus full-resolution choice; probe reflection from a reduced-resolution pass; coarse pixel and variable-rate shading (Intel, AMD, NVIDIA, Qualcomm, Sony, Apple); checkerboard; Nintendo and NetEase reflection filings; inline footprint depth tests; per-object switching with hysteresis; SAS filings and '166 / '715 continuations. Same method.

| Hit | Owner | Shape | Outcome |
|---|---|---|---|
| US10198856B2, US10198788B2, US20190295214A1 | Oxide Interactive | A, C3 | Charted (§Oxide) |
| US11436783B2, US12154208B2 | Oxide Interactive | A | Charted (§Oxide) |
| US12444121B2 "Pre-filtering with anisotropic filter in decoupled shading" | AMD / ATI | A | Claim 1 (OG excerpt): gradients relating "shade space texel spacing and screen space pixel spacing" and material texel spacing, for filtering. The cell path computes no such gradient and filters nothing. Appears absent; not charted further |
| US9342926B2 / EP2797054B1 | Square Enix | B | Charted (§Shape B) |
| US10510185B2 | AMD | B | Charted (§Shape B) |
| US9710881B2 | Sony | B | Charted (§Shape B) |
| US10235799B2 | Microsoft | B | Charted (§Shape B) |
| US11869115B1 | Qualcomm | B, C3 | Screened (abstract) |
| Intel CPS family (WO2016043909A1, EP3195269A1, US20150287240A1, US20190087983A1, US9916682B2) | Intel | B | Claims not read (pages refused) |
| US12322029B2 | Meta | A | Charted (§Other texture-space filings) |
| 18/477,375 "Hybrid deferred decoupled rendering" | AMD | A | Pending; rejected over '715's A1. Claims not published in what was read |
| US10504280B2, US11132831B1, US11875452B2 | Qualcomm (Schmalstieg group) | A | Screened by abstract; none claims per-frame sizing by projection |
| US20180197325A1 | Imagination | C2 | Screened (§Hidden-cell test) |
| CN107392988B | NVIDIA | B | Lens-distortion sampling rate. Not charted |
| US20130135329A1, US11468640B2, US8281281B1 | Qualcomm, Magic Leap, Pixar | C3 | Screened (§Path choice) |

Nothing turned up from Apple, Nintendo or a checkerboard-rendering filing that claims a reduced-resolution shading or reflection target read back at full resolution.

No hit was found from NVIDIA, AMD, Sony, Samsung, Epic, Unity, NetEase or miHoYo on these topics beyond filings already charted. Tencent's own '451 is cited by 78 families, mostly Tencent CN filings on GI; none was read at claim level.

## For counsel

The `ip-research.md` §For counsel questions stand, narrowed by this chart:
1. **'451 claims 5–6 against Lever A.** The description defines a dynamic object as one whose "position, a direction, or the like … is changeable". Does "or the like" reach an animated fluid surface on fixed geometry? Does a fluid volume on a mover fall inside it? If either does: does a per-probe, per-lobe sum of share × `(current/ref − 1)`, interpolated by the reflected direction, meet claim 6's "updating light brightness corresponding to directions on the light probe … and a normal direction of the target pixel point"? The prosecution record (one non-final action, 2024-11-12) is unread. The claim-1 question on the shipped ratio (`ip-delta-tiles.md` §2) is still open and comes first. Art to weigh: Far Cry 3 DRTV (2012), The Division (2016).
2. **Tencent WO 10 / 19 against Lever A.** Is a per-probe modifier, rewritten each frame before collection but never written into the probe's collected colour, "updating the illumination information collected by the reflection probe"? The search report cited none of the Far Cry 4, FFXV or Division art. Watch the US national phase (≈2027-05-28) and EP regional phase (≈2027-06-28).
3. **Qualcomm '166 claim 1 and EP4409531B1 claim 1 against the cell pool.** Does clipping a face's fixed-resolution texel rect to the screen "select a size … based on … a perspective projection"? Is a compiler's face set "divide at least one scene into a plurality of meshlets"? Weigh Mueller 2018 (on the face; sizes patches by projected area), Baker 2016 (sizes per object by "estimated projected area"; not of record) and Hillesland & Yang 2016 (not of record). The examiner's own art was index-buffer compression. EP claim 1 has no visibility element.
4. **Intel '475.** Confirm that an inline per-invocation depth test with unconditional dispatch falls outside claims 1, 10 and 19. If the build ever wants compaction or indirect dispatch: weigh Liktor & Dachsbacher 2012 (per-sample visible flags gating shading kernels after a depth pass) and Hillesland & Yang 2016 (tile work list); neither is of record.
5. **Qualcomm '715 against the pool.** Confirm that whole-rect allocation from a screen clip, with cells at texel-grid offsets, is not "a group offset … based on a number of visible shading elements" or an element offset from a count.
6. **Qualcomm '856 continuation (US20260203991A1).** Its pending claim 1 needs only a mip-region resolution from derivatives of visible meshlets. The cell path computes none. Watch for allowance.
7. **Shape B (cell-path pass).** Confirm that no reduced-resolution family charted in §Shape B reaches a separate probe pass into a fixed-fraction target read by one nearest load at the cell centre. If Intel's coarse-pixel claims matter, read them; they were not reachable here.
8. **Oxide US10198856B2, US11436783B2, US12154208B2.** Confirm the MIP-chain and shader-state sort ('856), the miss buffer ('783) and hashed atomic allocation with a GPU work queue ('208) keep the cell pool outside each claim 1. The family is also 2013-priority art on sizing an object's shading image by pixel coverage, for question 3.
9. **Watch: EP4409531B1 opposition window closes about 2027-04-22** (nine months from the 2026-07-22 mention of grant). Opposition is the low-cost route against an EP patent once granted. Decide whether to file, or note that the window passed, before that date. National entries so far: CH, GB, IE, DE.
10. **Watch: EP4778033A1** ('715's EP twin, pending). Read its claims when examination produces them.
11. Fee status of Baobab US11605198B2. Google shows no payment or expiry event. The 3.5-year window closed about 2026-09-14; the surcharge period runs to about 2027-03-14.

## Not accessed

- File wrappers: office actions, responses and reasons for allowance for Qualcomm '166 (17/934,159), Qualcomm '715 (18/467,666), Intel '475 (16/235,517) and Tencent '451 (17/985,107). Patent Center and Global Dossier need a USPTO.gov sign-in (anonymous document requests return HTTP 405, 2026-11-22); ppubs anonymous access ended 2026-11-07. Only Google's legal events and citation tables were read.
- Tencent WO: X/Y/A categories of the four search-report citations and the written opinion (Patentscope refused). Certified English claims.
- EP4409531B1: the EPO search report and the Register's opposition file (EPO Register and Espacenet behind a bot check). Designations, national entries and the absence of an opposition event come from Google's INPADOC events, which can lag.
- Claims of EP4778033A1, Square Enix US9342926B2 in full, most of Intel's coarse-pixel family and Qualcomm US11869115B1 (Google Patents refused or truncated those pages; the Official Gazette covers granted US claim 1 where its page can be found).
- USPTO maintenance-fee records for Baobab '198.
- Litigation, PTAB and licensing: no PACER, Docket Navigator or PTAB database access. CourtListener's RECAP search (2026-11-22) for '166, '475, '451, '715, Oxide '856, '788, '783, '208, AMD '185 and Square Enix '926, with and without commas, returned only unrelated dockets whose text happens to contain the same digit strings. RECAP holds only dockets someone uploaded, so this is weak evidence. A web search for PTAB proceedings on '166, '475 and '451 found nothing.
- USPTO sources behind sign-in (checked 2026-11-22): Patent Center, Global Dossier, the Open Data Portal (sign-in required since 2026-06-18, file wrappers and office actions included) and the maintenance-fee lookup. Patentscope returned HTTP 403 in the browser too.
- Papers: the AMFS PDF (host refused; abstract read); McAuley's Far Cry 4 speaker notes and video; the Far Cry 3 DRTV slides (login); Wronski's AC4 deck in full (excerpt only).
- `reflection-probe-cell-path/research.md` §Screen-space rival: not in the checkout read; shape B is charted from the owner's description.
- A CPC- or assignee-filtered patent database query.
