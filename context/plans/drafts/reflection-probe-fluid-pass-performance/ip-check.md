# reflection-probe-fluid-pass-performance — IP check

> **Engineering due diligence, not legal advice. 2026-11-21.** This compares published claims and dated prior art against the brief's levers, to brief the owner and counsel. "Appears present", "appears absent", "arguable" and "leans" are engineering readings of claim text. They are not opinions on infringement, validity or freedom to operate, which belong to counsel. A negative search result means "not found", never "clear".

Compared against: `index.md` (Decisions, Path) and `research.md` §Precompute here; `rendering_pipeline.md` §7.3a (Live lighting, Style, Two pipelines). Builds on `ip-research.md` here (the landscape), `plans/done/reflection-probes/ip-check.md` (Tencent WO chart, §4 fences), `context/research/gi-rtx-lite/ip-check.md` §Tencent US12293451B2 (verbatim claim 1, design rules, prior art) and `plans/drafts/dynamic-reflection-layer/ip-delta-tiles.md` §2 (the shipped ratio's '451 shape). It does not redo those charts. It charts only what this brief changes.

**Sources.** Claim text is verbatim unless a row says otherwise:
- US and EP filings come from Google Patents page text. Long claims were read in segments; "…" marks a cut.
- Tencent WO2026113651A1 comes from Google's machine translation of the Chinese text.
- Intel US10699475B1 comes from its grant PDF on `patentimages.storage.googleapis.com`, through two-column text extraction.
- Status, priority, prosecution and fee events are Google Patents' legal-event data, unconfirmed against USPTO.
- Papers were read in full text on the second pass (2026-11-21) unless a row says otherwise. Each prior-art row names its source.

**Gaps:**
- No CPC- or assignee-filtered database query ran. The fresh sweep was web search restricted to `patents.google.com`, plus citation lists on the pages read.
- File-wrapper documents (office actions, reasons for allowance) were not read. USPTO Patent Center and Patentscope refused automated access. Prosecution dates and examiner-cited art come from Google's legal events and citation tables.
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
| US12293451B2 ('451) | Tencent | Granted 2025-05-06, active, ≈2043-01-19. One non-final action (2024-11-12), allowed 2025-01-28 | A | 1(c) transfer × coefficient; 6 per-direction probe update | Claim 1: no new element; the shipped ratio's exposure is unchanged. Claim 6: moves toward it, but needs claim 5's dynamic-object pixel. **The description defines a dynamic object by a changeable position or direction**, so claim 5 leans absent more firmly | Medium |
| WO2026113651A1 / CN122115679A | Tencent | Pending. No national or regional phase yet; EPO notified of EP designation 2026-07-15. US phase ≈2027-05-28, EP ≈2027-06-28 | A, C1 | 10 / 19 "update the illumination information collected by the probe" before collection | Appears absent: no tile is written. Nearer than per-fragment, because a per-probe state is rewritten before collection | Medium-low |
| US12136166B2 ('166) | Qualcomm | Granted 2024-11-05, active; priority 2021-09-30; one non-final action (2024-02-29), allowed 2024-08-14 | C1, C2 | 1: meshlet visibility, size from texture coordinates and perspective projection, atlas layout, shade atlas | Most elements read on the cell path. The crux is unchanged: "select a size … based on … a perspective projection". **Shading Atlas Streaming already sizes patches by projected screen area**; it groups 1–3 triangles, not meshlets | Medium |
| EP4409531B1 | Qualcomm | Granted 2026-07-22 | C1 | Same as '166 claim 1 **without the visibility element** | Same crux as '166. Broader by one element, so the cell path's face-level clip no longer matters | Medium |
| US12664715B2 (A1 US20250095266A1) | Qualcomm | Granted 2026-06-23 (publication date); priority 2023-09-14 | C1, C2 | 1: identify visible shading elements, offsets from counts of visible elements, allocate texture memory to visible elements only | **New.** Arguable, leaning absent: rects are allocated per face from the screen clip, and cells keep texel-grid offsets. **A visible-cell compaction would read on it** | Medium |
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

Read in `ip-research.md`, not re-read here: ARM US10636213B2 (stored intersections), Autodesk US9355475B1 (reprojection), EA's SSR-to-ray-trace family and Microsoft US10586375B2 / US10360720B1 (Lever B). The brief keeps no intersections or history, so those readings hold. NVIDIA US10424074B1 (priority 2018-07-03; continuation US10699427): claim 1 reports "a texture footprint" in answer to "a texture footprint query request" from a shader. The brief issues no footprint query, so this appears absent.

**Changes from the first pass:**
- '451 claims 5–6: the description's definition firms the "leans absent" reading for static fluid faces. It opens a new trigger: fluid on a mover (§Lever A fences).
- Qualcomm '166: verified prior art discloses the sizing element for triangle patches. The crux row stands; the counsel question gains a reference. EP's claim 1 drops the visibility element.
- Qualcomm '715 is new. With Intel '475 it fences visible-cell compaction from two sides.
- Far Cry 4: the slide text read does not show the cubemap G-buffer relight. The row rests on FFXV's description, now verified.
- Citation corrections: AMFS is SIGGRAPH 2014 (ACM TOG 33(4)), not HPG; '856 granted 2026-04-14, not 2026-04-01; Far Cry 3 DRTV was presented at GDC 2012 by Stefanov (Gilabert presented it at CEDEC 2012).

No reading moved toward present. No crux row changed. No existing fence weakened.

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

**EP4409531B1** (granted 2026-07-22, ≈2042-09-22). Claim 1 has divide, texture coordinates, "select a size … based on … a perspective projection", layout and shade. It has no visibility element. Claims 2–4 and 9–11 add visibility as dependents. EP designations and validations were not seen.

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

No hit was found from NVIDIA, AMD, Sony, Samsung, Epic, Unity, NetEase or miHoYo on these topics beyond filings already charted. Tencent's own '451 is cited by 78 families, mostly Tencent CN filings on GI; none was read at claim level.

## For counsel

The `ip-research.md` §For counsel questions stand, narrowed by this chart:
1. **'451 claims 5–6 against Lever A.** The description defines a dynamic object as one whose "position, a direction, or the like … is changeable". Does "or the like" reach an animated fluid surface on fixed geometry? Does a fluid volume on a mover fall inside it? If either does: does a per-probe, per-lobe sum of share × `(current/ref − 1)`, interpolated by the reflected direction, meet claim 6's "updating light brightness corresponding to directions on the light probe … and a normal direction of the target pixel point"? The prosecution record (one non-final action, 2024-11-12) is unread. The claim-1 question on the shipped ratio (`ip-delta-tiles.md` §2) is still open and comes first. Art to weigh: Far Cry 3 DRTV (2012), The Division (2016).
2. **Tencent WO 10 / 19 against Lever A.** Is a per-probe modifier, rewritten each frame before collection but never written into the probe's collected colour, "updating the illumination information collected by the reflection probe"? The search report cited none of the Far Cry 4, FFXV or Division art. Watch the US national phase (≈2027-05-28) and EP regional phase (≈2027-06-28).
3. **Qualcomm '166 claim 1 and EP4409531B1 claim 1 against the cell pool.** Does clipping a face's fixed-resolution texel rect to the screen "select a size … based on … a perspective projection"? Is a compiler's face set "divide at least one scene into a plurality of meshlets"? Weigh Mueller 2018 (on the face; sizes patches by projected area), Baker 2016 (sizes per object by "estimated projected area"; not of record) and Hillesland & Yang 2016 (not of record). The examiner's own art was index-buffer compression. EP claim 1 has no visibility element.
4. **Intel '475.** Confirm that an inline per-invocation depth test with unconditional dispatch falls outside claims 1, 10 and 19. If the build ever wants compaction or indirect dispatch: weigh Liktor & Dachsbacher 2012 (per-sample visible flags gating shading kernels after a depth pass) and Hillesland & Yang 2016 (tile work list); neither is of record.
5. **Qualcomm '715 against the pool.** Confirm that whole-rect allocation from a screen clip, with cells at texel-grid offsets, is not "a group offset … based on a number of visible shading elements" or an element offset from a count.
6. **Qualcomm '856 continuation (US20260203991A1).** Its pending claim 1 needs only a mip-region resolution from derivatives of visible meshlets. The cell path computes none. Watch for allowance.
7. Fee status of Baobab US11605198B2. Google shows no payment or expiry event. The 3.5-year window closed about 2026-09-14; the surcharge period runs to about 2027-03-14.

## Not accessed

- File wrappers: office actions, responses and reasons for allowance for Qualcomm '166 (17/934,159), Intel '475 (16/235,517) and Tencent '451 (17/985,107). USPTO Patent Center refused; ppubs anonymous access ended 2026-11-07. Only Google's legal events and citation tables were read.
- Tencent WO: X/Y/A categories of the four search-report citations and the written opinion (Patentscope refused). Certified English claims.
- EP4409531B1: the EPO search report, designations and validations.
- USPTO maintenance-fee records for Baobab '198.
- Litigation, PTAB and licensing: no PACER, Docket Navigator or PTAB database access. A web search for the three patent numbers found nothing.
- Papers: the AMFS PDF (host refused; abstract read); McAuley's Far Cry 4 speaker notes and video; the Far Cry 3 DRTV slides (login).
- A CPC- or assignee-filtered patent database query.
