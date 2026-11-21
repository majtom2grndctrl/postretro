# reflection-probe-fluid-pass-performance — IP check

> **Engineering due diligence, not legal advice. 2026-11-21.** This compares published claims and dated prior art against the brief's levers, to brief the owner and counsel. "Appears present", "appears absent", "arguable" and "leans" are engineering readings of claim text. They are not opinions on infringement, validity or freedom to operate, which belong to counsel. A negative search result means "not found", never "clear".

Compared against: `index.md` (Decisions, Path) and `research.md` §Precompute here; `rendering_pipeline.md` §7.3a (Live lighting, Style, Two pipelines). Builds on `ip-research.md` here (the landscape), `plans/done/reflection-probes/ip-check.md` (Tencent WO chart, §4 fences), `context/research/gi-rtx-lite/ip-check.md` §Tencent US12293451B2 (verbatim claim 1, design rules, prior art) and `plans/drafts/dynamic-reflection-layer/ip-delta-tiles.md` §2 (the shipped ratio's '451 shape). It does not redo those charts. It charts only what this brief changes.

**Sources.** Claim text is verbatim unless a row says otherwise:
- US filings come from Google Patents page HTML, fetched raw.
- Tencent WO2026113651A1 comes from Google's machine translation of the Chinese text.
- Intel US10699475B1 comes from its grant PDF on `patentimages.storage.googleapis.com`, through two-column text extraction.
- Status, priority and fee events are Google Patents' legal-event data, unconfirmed against USPTO.

**Gaps:**
- Google Patents' search endpoint refused queries (HTTP 503), so there was no CPC- or assignee-filtered sweep. Discovery came from web search and the citation lists on the pages read.
- The session's network blocks these paper hosts: `gpuopen.com`, `diglib7.eg.org`, `arbook.icg.tugraz.at`, `advances.realtimerendering.com`, `image-ppubs.uspto.gov` and `patents.justia.com`. Prior-art rows marked *(not read)* rest on bibliographic knowledge or search excerpts.
- Applications filed in the last 18 months are invisible to any search.

## Levers under review

| Lever | What runs | Changes vs today |
|---|---|---|
| **A. Precompute** | On a frame with any reflecting draw, one compute dispatch writes, per resident probe, five RGB sums `S_k = Σ_l share_{l,k} · (current_l / ref_l − 1)`, one per §7.3a lobe. Fragments read `ratio(d) = max(1 + Σ_k w_k(d) · S_k, 0)`. The probe tile is never written | The same ratio, hoisted from per fragment to per probe per frame. It adds a stored per-probe, per-direction state, rewritten every frame and ordered before the fluid pass (acceptance P1) |
| **C1. Cell pool** | Per frame, the cell-path draws' faces, clipped to the screen, pack nearest first into a pool sized from the scene extent. A compute pass evaluates the probe term once per water texel cell. The pixel-style fragment reads its cell's record | A same-frame, history-free texture-space intermediate for the probe term |
| **C2. Hidden-cell skip** | Each cell invocation tests its screen footprint conservatively against the depth pre-pass and skips evaluation if the footprint is hidden everywhere. Every packed cell is still dispatched (acceptance: "dispatches its cells yet evaluates none") | A per-cell depth visibility test in texture space |
| **C3. Path choice** | Per draw, on the CPU: estimated screen pixels per cell against one cutoff, with hysteresis per volume | Chooses texture-space or per-pixel evaluation per draw |

Lever B, the view-dependent early-out, is out of scope: the brief decided against it. `ip-research.md` §Lever B stands as the record.

## Summary

| Filing | Owner | Status | Lever | Closest element | Reading | Confidence |
|---|---|---|---|---|---|---|
| US12293451B2 ('451) | Tencent | Granted 2025-05-06, active, ≈2043-01-19 | A | 1(c) transfer × coefficient; 6 per-direction probe update | Claim 1: no new element; the shipped ratio's exposure is unchanged. Claim 6: **moves toward it**, but claim 6 needs claim 5's dynamic-object pixel | Medium |
| WO2026113651A1 / CN122115679A | Tencent | Pending; US phase ≈2027-05-28, EP ≈2027-06-28 | A, C1 | 10 / 19 "update the illumination information collected by the probe" before collection | Appears absent: no tile is written. Nearer than per-fragment, because a per-probe state is rewritten before collection | Medium-low |
| US12136166B2 | Qualcomm | Granted 2024-11-05, active; priority 2021-09-30 | C1, C2 | 1: meshlet visibility, size from texture coordinates and perspective projection, atlas layout, shade atlas | **Most elements read on the cell path.** The crux is "select a size … based on … a perspective projection" | Medium |
| US10699475B1 | Intel | Granted 2020-06-30, active, ≈2038-12-28 | C2 | 1: texel visibility tests, a stored texel mask, an any-visible bit, dispatch only for visible texels | Appears absent on three elements while the skip stays inline per cell. **An indirect-dispatch compaction would read on it** | Medium-high |
| US9728002B2 | AMD | Granted 2017-08-08, active, fees paid through year 8 | C1 | 1: select texels to shade from texels cached for another frame | Appears absent (no history) | High |
| US10535186B2 | Intel | Granted 2020-01-14, active | C1, C3 | 1: first pass computes a per-pixel shading rate; second pass texel-shades a procedural texture after querying whether texels are already shaded | Appears absent | High |
| US9754407B2 | NVIDIA | Granted 2017-09-05, active | C1 | 1: shading-grid elements shaped like the triangle | Appears absent (square texel cells) | High |
| US12602856B2 (A1 US20250095265A1) | Qualcomm | Granted 2026-04-01; continuation US20260203991A1 pending | C1, C3 | A1 cl. 1: render geometry to get screen UV derivatives; set mip-region resolution from target pixels per region | Appears absent (CPU estimate; resolution fixed). Granted claims not read | Medium |
| US20200364926A1 | Qualcomm | **Abandoned** 2020-12-19 | — | Shade a primitive when its frame-to-frame shading difference passes a threshold | Prior art only | High |
| US11721064B1 | Meta | Granted 2023-08-08, active | C1 | Server atlas, send to client, reuse view-independent shading across viewpoints | Appears absent | High |
| US11605198B2 | Baobab | Granted 2023-03-14; fee payment not seen | A | Two precomputed GI sets (SH probes and lightmaps), up-sample the lower-resolution one, blend by weight | Appears absent | High |

Read in `ip-research.md`, not re-read here: ARM US10636213B2 (stored intersections), Autodesk US9355475B1 (reprojection), EA's SSR-to-ray-trace family and Microsoft US10586375B2 / US10360720B1 (Lever B). The brief keeps no intersections or history, so those readings hold. NVIDIA US10424074B1 (priority 2018-07-03; continuation US10699427): claim 1 reports "a texture footprint" in answer to "a texture footprint query request" from a shader. The brief issues no footprint query, so this appears absent. Screened by title only: Microsoft VRS US10152819B2 / US10147227B2, and Intel US10109039 (display-engine texel-to-pixel sample rate). Their claims were not read.

## Lever A: precompute

### Tencent US12293451B2

Claim 1, verbatim (also 9 and 17): "determining light source change information when a light source changes in a virtual scene; determining a current light source projection coefficient corresponding to the changed light source according to the light source change information; determining an indirect illumination value of a target pixel point in the virtual scene according to a radiance transfer parameter corresponding to the target pixel point in the virtual scene and the current light source projection coefficient; determining a direct illumination value corresponding to the target pixel point under the changed light source; and performing illumination rendering on the target pixel point according to the direct illumination value and the indirect illumination value."

| Element | Per-fragment ratio (shipped) | Precompute (Lever A) | Reading |
|---|---|---|---|
| (a) light source change information | current_l compared with ref_l, its capture-time value. Col. 5 says change information is found by comparing with the pre-change light | Same comparison, made per probe | Leans present for both, as for id 27. Evaluating every frame is no lever: the prior record says "per-frame vs event-triggered evaluation" is not one |
| (b) current projection coefficient | `current_l / ref_l − 1`, RGB, per light | Same | Arguable for both: the id-27 crux (a scalar or RGB as "coefficient"; paras. 32, 35, FIG. 4) |
| (c) indirect value from the pixel's radiance transfer parameter × coefficient | share_{l,k}: the baked per-light, per-lobe fraction of the capture's radiance. It stays fixed while the light changes (col. 17's premise) and is read per fragment through d | Same shares, same coefficients; the multiply moves to the probe | Arguable, nearest risk. Reflecting the lit static world is light off a surface, so "indirect" fits better here than for glow (`ip-delta-tiles.md` §2). The precompute adds nothing to this element |
| (d), (e) direct under the changed light; render both | The water surface's own runtime lighting, then the composite | Unchanged | Present |

Dependent claims touched by the move:

| Claim | Text (verbatim, abridged with …) | Precompute | Reading |
|---|---|---|---|
| 5 / 13 | "obtaining, for the target pixel point belonging to a dynamic object in the virtual scene, a light probe matching the target pixel point of the dynamic object; and obtaining the radiance transfer parameter … from the light probe" | Fluid faces are static brush geometry. Their texture coordinates and pixel tilt animate; their geometry does not move | Leans absent. **Counsel**: whether an animated fluid surface is a "dynamic object" |
| 6 / 14 (on 5) | "updating light brightness corresponding to directions on the light probe according to the current light source projection coefficient and the radiance transfer parameter; performing interpolation on the updated light brightness …; and determining the indirect illumination value … according to the light brightness after the interpolation and a normal direction of the target pixel point" | S_k is a per-probe, per-direction value from coefficient × share, rewritten each frame. w_k(d) interpolates the lobes. d is the reflected lookup direction, built from the normal and the view. S_k is a relative change multiplied onto captured colour, not a brightness | **Moves toward claim 6's first two elements.** The per-fragment form stores nothing per probe direction. The third element is arguable (reflected direction vs normal; relative change vs brightness). Claim 6 falls if claim 5 is absent |
| 3–4 / 11–12 | Static pixel: transfer "from the lightmap" | Shares live in probe records, not a lightmap | Appears absent |
| 2, 7, 8 | Illumination transform matrix; basis projection with virtual lights; a distant light's direction change | None | Appears absent |

**Reading.** Lever A adds no claim-1 element. It leaves the shipped ratio's exposure exactly where it is, and that exposure has no counsel answer yet. It does add claim 6's per-direction probe update in substance. The record's "not levers: probe vs pixel" note was written against claim 1; claim 6 is where probe vs pixel starts to matter. The case stays weak while claim 5's dynamic-object element is absent.

### Tencent WO2026113651A1, update chain

Claim 10 (machine translation): "When the update condition of the reflection probe is met, the illumination information collected by the reflection probe is updated to obtain updated illumination information; wherein the update condition includes at least one of … The light at virtual objects within the target area in the virtual scene changes … From the updated lighting information, obtain the lighting information of the reflected light." Claims 11 and 13 define that information as "color information obtained by coloring the virtual objects … based on the texture information".

| Claim | Precompute | Reading |
|---|---|---|
| 10 / CN 9 | A per-probe state is rewritten every frame before the fluid pass collects the reflection. The ordering is pinned (P1). The tile, the colour the probe collected, is never written. The sums are a per-direction relative modifier, not colour information | Appears absent: what the probe collected is not updated. **Nearer than per-fragment**: a stored per-probe state now changes with the lights ahead of collection. Counsel |
| 13 / CN 11 | No texture or G-buffer information, no re-shading | Appears absent |
| 14 / CN 12 | No faces updated in turn; every resident probe every frame | Appears absent. Moves toward 14 if the precompute is ever amortised over frames by probe or face |
| 19 | Same as 10 | Same as 10 |

### Prior art for Lever A

| Publication | Date | What it discloses | Elements | Evidence |
|---|---|---|---|---|
| id Software, Quake `R_AnimateLight` (WinQuake `gl_rlight.c`; called once from `R_SetupFrame`, `gl_rmain.c`) | 1996; GPL 1999 | Each light style's value is evaluated **once per frame** into `d_lightstylevalue[]`. Every consumer reads that array. Surfaces whose cached style value differs rebuild `blocklights[i] += lightmap[i] * scale` | Per-light, per-frame evaluation shared by all consumers (the brief's per-light rival). A stored per-light basis × current value, recomposed on change | **Verified**, source read 2026-11-21 (`id-Software/Quake`) |
| Jedi Academy `R_SetupEntityLightingGrid` with q3map2 `-bouncegrid` | 2003; GPL 2007 / 2013 | Σ style value × baked per-style ambient and directed grid values, trilinear, lighting dynamic entities | '451 claims 1, 5, 6 with a scalar coefficient | Verified in the prior record (`gi-rtx-lite/ip-check.md`) |
| Dorsey, Arvo & Greenberg, IEEE CG&A 15(2) | 1995 | Per-light basis images superposed by current intensity | The ratio's algebra | Prior record |
| Stefanov / Gilabert, "Deferred Radiance Transfer Volumes", Far Cry 3, GDC | 2012 | Probes store SH transfer and are relit on the fly for time of day | Per-probe, per-frame relighting from stored transfer (claims 1, 6–8) | Search excerpt, GDC Vault abstract |
| McAuley, "Rendering the World of Far Cry 4", GDC | 2015 | A mini G-buffer stored per cubemap, relit at runtime | Tencent WO 11–13 (re-shade stored texture information on light change) | Secondary only: cited and described in Elcott et al., FFXV, SIGGRAPH 2016 talk *(not read)* |
| Sloan, Kautz & Snyder, PRT, SIGGRAPH | 2002 | Transfer × lighting coefficients | '451 claim 1 | Prior record |

The Far Cry 4 row replaces `ip-research.md`'s unverified forum row ("relight stored cubemap G-buffers a few per frame"). It predates Tencent WO's 2024 priority by nine years. It discloses claims 11–13's shape, which neither path here uses.

### Fences for Lever A

| Rule | Avoids | Cost |
|---|---|---|
| The sums live in their own buffer. They never write the probe tile, a "live tile" or any record the capture owns | WO 10, 13, 19 | None (as briefed) |
| Every resident probe, every frame that has a reflecting draw: no faces, octants or probe subsets amortised over frames | WO 14 | None (as briefed) |
| No SH basis, transform matrix or virtual-light projection in the coefficient | '451 2, 7, 8 | None |
| If counsel reads claim 6 onto a static animated surface: take the brief's rival instead. Evaluate `current_l / ref_l` once per light per frame (Quake's shape) and keep the share × value sum per fragment, so no per-probe, per-direction state exists | '451 6, element 1; WO 10's "probe information updated" framing | Unmeasured per-fragment cost (`index.md` Path, rival 2). The claim-1 reading does not change either way |

## Lever C: cell path

### Qualcomm US12136166B2, "Meshlet shading atlas"

Inventors Neff, Müller, Steinberger, Schmalstieg. Priority 2021-09-30; filed 2022-09-21; granted 2024-11-05. Family: EP4409531B1, CN118043850A, KR20240093463A, TW202322043A. The face cites Mueller et al., "Shading Atlas Streaming" (2018), and Neff et al., "Meshlets and How to Shade Them" (2022).

Claim 1 (16, 25 and 26 are the same steps), verbatim:

| Element | Cell path | Reading |
|---|---|---|
| "divide at least one scene into a plurality of meshlets, each … including a plurality of primitives" | Fluid faces are compile-time brush polygons, each triangulated. The runtime divides nothing | Arguable. Counsel: whether a compiler's face set is the claimed division |
| "identify a visibility of each of the plurality of meshlets …, wherein each … is identified as visible or invisible, wherein each … identified as visible corresponds to at least one of a plurality of pixels in a meshlet atlas" | Faces are clipped to the screen, and a face with no on-screen part packs no rect. The room cull picks draws. The hidden-cell skip acts per cell, after allocation | Leans present (frustum visibility per face decides allocation) |
| "calculate a pair of texture coordinates for each of the plurality of vertices …" | The water's texel grid maps world position to texel cells. Claim 11 covers precomputed coordinates | Leans present. Whether coordinates are per vertex or a per-face affine fit (the spike's form) is a detail counsel may weigh |
| "select a size of each of the plurality of meshlets … based on the pair of the texture coordinates … and based on a perspective projection of each of the plurality of meshlets" | A rect holds the face's texel cells inside the screen clip. Cell resolution is fixed at one per water texel and never scaled by distance or projected size. The projection decides only which cells survive the clip | **Crux.** Leans absent if "size" means shading resolution, which the description and claims 4/19 tie to the subtended angle. Arguable-present if it covers a rect's extent after a projection clip |
| "calculate layout information in the meshlet atlas for each … based on the selected size" | Shelf pack, nearest first | Present |
| "shade each of the plurality of pixels in the meshlet atlas based on the calculated layout information" | The cell pass writes reflection colour and weight per cell | Present |
| Claim 8: "render each … identified as visible, wherein the meshlet atlas is sampled to determine a color of each … pixels in an output image" | The fragment reads its cell's record | Present |

**Reading.** This is the nearest filing to the cell path, and earlier notes had not read its claims. Whether claim 1 is met turns on two elements: "divide … into meshlets" and "select a size … based on … a perspective projection". The Shading Atlas Streaming paper (Dec 2018) predates priority. As we recall it, it packs per-frame blocks of visible triangles into an atlas, sized by projected area *(not read this session)*. It is on the face, so claim 1 was allowed over it, presumably on the meshlet and sizing combination.

**Fences:**
- Cell resolution stays fixed by the texel grid. No per-face LOD, mip level or cell size chosen from distance, projected area or the pixels-per-cell estimate. That estimate picks a path only.
- Pack whole texel rows and columns of a face, clipped only to bound the pool. Never resample a face to fit a budget.
- **Counsel question:** does screen-clipping a face's texel rect make its "size" "based on a perspective projection"? The alternative is to allocate whole faces and fall back per fragment for faces that do not fit, which loses large lake faces running behind the camera.

### Intel US10699475B1, "Multi-pass apparatus and method for early termination of graphics shading"

Barczak, Xiao, Apodaca, Raoux, Brownlee, Liktor. Priority and filing 2018-12-28; granted 2020-06-30; active; ≈2038-12-28 (Google).

Claim 1, verbatim from the grant PDF. Claim 10 (method) and claim 19 (medium) have the same steps:

| Element | Hidden-cell skip | Reading |
|---|---|---|
| "graphics processing circuitry to execute a sequence of visibility testing operations related to texels within a texture domain to generate visibility results" | Each cell's screen footprint is tested against the depth pre-pass. Claim 6 adds "compare the depth data with stored depth data" | Present |
| "a register or memory to store a texel mask" and "texel mask update circuitry/logic to update the texel mask based on the visibility results, the texel mask comprising a plurality of bits to indicate visibility of the texels …" | No mask. The test result lives in the invocation that evaluates the cell | Appears absent |
| "set a first bit to indicate whether any bits in the texel mask indicate a visible texel" | No any-visible flag | Appears absent |
| "a shader dispatcher to initiate conditional dispatch operations only if the first bit is set …, the shader dispatcher is to dispatch texel shaders for only those texels that the texel mask indicates may be visible" | Every packed cell is dispatched. A hidden cell runs and returns early | Appears absent |
| "a plurality of execution units (EUs) to execute the shaders dispatched" | GPU compute | Present |

Dependent claims 3–4 and 12–13 cover a bit per n×n tile; claims 6 and 15 a depth compare; claims 7 and 16 adaptive multi-frequency shading.

**Reading.** As briefed, the skip appears to lack the mask, the any-visible bit and the selective dispatch. The brief's own acceptance row ("dispatches its cells yet evaluates none") documents that. The obvious next optimisation is to compact visible cells or tiles into an indirect dispatch, or skip a face's dispatch when nothing is visible. Each of those would add the missing elements.

**Fences:**
- The visibility test runs inline, in the invocation that evaluates the cell.
- No pass writes a per-cell or per-tile visibility bitfield that a later dispatch reads.
- No any-visible flag gates a dispatch.
- No visible-cell compaction into indirect dispatch arguments.
- Owner IP review before any of these is built.

### Other texture-space filings

| Filing | Independent claim (verbatim, abridged with …) | Cell path | Reading |
|---|---|---|---|
| AMD US9728002B2, "Texel shading in texture space" (Hillesland) | "selecting a subset of the texels for shading based on previously cached texels that were shaded for a second frame; … caching the shaded subset of the texels with the previously cached texels" | Written and read in one frame; every read record is written that frame. The no-history gate covers the pool | Appears absent. Keep the gate |
| Intel US10535186B2 (Clarberg) | "pixel shading computation logic to facilitate computation of shading rate in a first pass …; and shading pass logic to facilitate texel shading operations in a second pass using the shading rate, … on a procedural texture, … to query the procedural texture to determine whether relevant texels … have already been shaded" | Path chosen per draw on the CPU from geometry. Cells are evaluated unconditionally, with no shaded-yet query | Appears absent. Never add a per-cell "already shaded" check or GPU-computed rate |
| NVIDIA US9754407B2 (Patney et al.) | "defining an object-space shading grid for at least one of the … triangle primitives …, wherein a shape of an object-space shading element corresponds to a shape of the at least one triangle primitive" | Square cells on the water texel grid | Appears absent |
| Qualcomm US20250095265A1 → US12602856B2, "Dicing oracle for texture space shading" | A1 cl. 1: "render the set of geometry units in order to obtain a second set of UV coordinates for a screen space and a set of derivatives …; calculate a resolution for a mip region map based on the set of derivatives … and the target number of pixels per region" | The pixels-per-cell estimate comes from camera, extent and face geometry, with no render and no derivatives. It picks a path, not a resolution | Appears absent on the A1 claim. Re-read the granted claims before cell resolution ever adapts to screen derivatives |
| Qualcomm US20200364926A1, "Adaptive object space shading" (Mueller, Neff, Schmalstieg) | Shade a primitive when the difference between its first- and second-frame shading values exceeds a threshold | No history | Abandoned. Prior art only |
| Meta US11721064B1 | A server builds an atlas with view-dependent and view-independent parts, sends it to a client, and regenerates only the view-dependent subset for a second viewpoint | Local, one frame, one view | Appears absent |

### Tencent WO2026113651A1 against the pool

| Claim | Pool | Reading |
|---|---|---|
| 1 / 20: "Obtain the texture image of the first virtual object, and determine the color value of the image pixel based on the texture image and the lighting information" | The pool is a texture-space image of the probe term, the lighting side. The water's albedo still comes from its material per fragment. The description defines "texture image" as the object's unshaded screen-space image | No new element. Claim 1 is already arguably present (`plans/done/reflection-probes/ip-check.md` §1). The pool sits on the "lighting information" side, not the "texture image" side |
| 10 / 19: probe-collected information updated before collection | The pool holds per-cell lookups from the water, not the probe's record of its range | Appears absent (low concern) |

### Path choice (C3)

No filing found claims a per-draw choice between texture-space and per-pixel evaluation keyed to on-screen pixels per texel, with or without hysteresis. Nearest, unread: Intel US10109039 selects a display-engine sample rate when texel size meets a texel-to-pixel threshold. Microsoft's VRS family selects screen-space shading rates. Neither was charted.

Prior art *(not read this session)*: the GPUOpen texel-shading article says texel shading "can be selectively applied on a per-object, or even per-fragment basis" (search excerpt). Hysteresis in level-of-detail switching dates from Funkhouser & Séquin, SIGGRAPH 1993.

### Prior art for Lever C

| Publication | Date | What it discloses | Bears on | Evidence |
|---|---|---|---|---|
| Cook, Carpenter & Catmull, REYES | 1987 | Shading on object-space micropolygon grids before visibility | All texture-space filings | Bibliographic |
| id Software, Quake surface cache | 1996 | Per-surface texel images composed from lightmaps and styles, then sampled by the rasteriser | Texture-space shading in general | Source verified (`gl_rsurf.c`) for the light composition; the software cache is from memory |
| Burns, Fatahalian & Mark, "A Lazy Object-Space Shading Architecture with Decoupled Sampling", HPG | 2010 | Visibility first, then shading only the object-space samples visible pixels need | Intel '475, Qualcomm '166 | *(not read)* |
| Ragan-Kelley et al., "Decoupled Sampling", ACM TOG | 2011 | Shading samples decoupled from visibility samples | Same | *(not read)* |
| Liktor & Dachsbacher, "Decoupled Deferred Shading", I3D | 2012 | Shading cache keyed by surface position, visibility-driven | Same | *(not read)* |
| Clarberg et al., "AMFS: Adaptive Multi-Frequency Shading", HPG | 2014 | Shading at multiple rates | Intel '475 cl. 7 (named in the claim) | *(not read)* |
| Baker, "Object Space Lighting", Oxide, GDC (Ashes of the Singularity) | 2016 | Per-object texel lighting each frame, then a raster pass that samples it | Qualcomm '166, AMD '002 | *(not read)*; PDF host blocked |
| Hillesland & Yang, "Texel Shading", Eurographics short | 2016 | Camera-view raster marks needed texel tiles; compute shades them; a pixel pass samples them | Intel '475 (mask and dispatch), Qualcomm '166, C3 | *(not read)*; host blocked |
| Greene, Kass & Miller, "Hierarchical Z-Buffer Visibility", SIGGRAPH | 1993 | Conservative occlusion of a bound against a depth pyramid | C2's footprint test | Bibliographic |
| Haar & Aaltonen, "GPU-Driven Rendering Pipelines", SIGGRAPH Advances | 2015 | Compute-shader occlusion of cluster bounds against Hi-Z from a depth pre-pass | C2 | *(not read)*; host blocked |
| Mueller et al., "Shading Atlas Streaming", ACM TOG 37(6) | 2018-12-04 | Per-frame atlas of visible triangle blocks, sized by projected area; visibility pass | Qualcomm '166 (on its face); Intel '475 (24 days before its priority) | *(not read)*; host blocked |
| NVIDIA Turing texture-space shading | 2018 | Footprint query finds texels; shade in texture space; sample in screen space | Intel '475, '186 | NVIDIA developer blog, search excerpt |

## For counsel

The `ip-research.md` §For counsel questions stand, narrowed by this chart:
1. **'451 claims 5–6 against Lever A.** Is an animated fluid surface on static geometry a "dynamic object"? If so, does a per-probe, per-lobe sum of share × `(current/ref − 1)`, interpolated by the reflected direction, meet claim 6's "updating light brightness corresponding to directions on the light probe … and a normal direction of the target pixel point"? The underlying claim-1 question on the shipped ratio (`ip-delta-tiles.md` §2) is still open and comes first.
2. **Tencent WO 10 / 19 against Lever A.** Is a per-probe modifier, rewritten each frame before collection but never written into the probe's collected colour, "updating the illumination information collected by the reflection probe"? Watch the US national phase (≈2027-05-28).
3. **Qualcomm US12136166B2 claim 1 against the cell pool.** Does clipping a face's fixed-resolution texel rect to the screen "select a size … based on … a perspective projection"? Is a compiler's face set "divide at least one scene into a plurality of meshlets"? Weigh Mueller 2018 (on the face) and Hillesland & Yang 2016 (not on the face, as far as the page shows).
4. **Intel US10699475B1.** Confirm that an inline per-invocation depth test with unconditional dispatch falls outside claims 1, 10 and 19. Confirm whether Hillesland & Yang 2016 or Burns 2010 anticipate the mask-and-dispatch combination, in case the build later wants indirect dispatch.
5. Granted claims of Qualcomm US12602856B2 and the pending continuation US20260203991A1 (the dicing oracle). Only the A1 claims were read.
6. Fee status of Baobab US11605198B2 (the 3.5-year window closed about 2026-09-14, or 2027-03-14 with surcharge).

## Not accessed

- Patent text: the claims of Microsoft US10152819B2 / US10147227B2 and Intel US10109039; every file wrapper; USPTO ppubs (anonymous access ended 2026-11-07); and certified English claims for Tencent WO.
- Papers: every paper marked *(not read)* in the prior-art tables.
- Litigation and PTAB: no PACER, Docket Navigator or PTAB access.
