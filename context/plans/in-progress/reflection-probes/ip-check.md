# reflection-probes — IP check

> **Engineering due diligence, not legal advice. 2026-10-16.** A technical comparison of published claims against the as-built technique, to brief the owner and a patent attorney. "Appears to have", "appears to lack" and "reads on" are engineering readings of claim text, not opinions on infringement or validity. Claim construction, validity and freedom to operate are counsel's.

Compared against: `rendering_pipeline.md` §7.3a (Reflection), `resource_management.md` §5, `crates/render-cpu/src/fluid_surface/reflection_search.rs`, `crates/level-compiler/src/reflection_probes/capture.rs`, branch `feature/reflection-probes` at d8f2b2712.

## Documents found

| Document | Applicant | Priority | Published | Status (per Google Patents, assumed) |
|---|---|---|---|---|
| WO2026113651A1 (PCT/CN2025/126544), "Image rendering method and apparatus, and device, storage medium and program product" | Tencent Technology (Shenzhen); inventor Gu Dezhen | CN 202411731795.3, 2024-11-28 | 2026-06-04 | Pending. 23 claims; independent 1 (method), 20 (apparatus); 21–23 device / medium / program product referring to 1–19. EP designated (legal event 2026-07-15). No US or EP national/regional entry listed yet. |
| CN122115679A (CN 202411731795.3) | Tencent | 2024-11-28 | 2026-05-29 | Pending; legal events show publication only (PB01), no examination request listed. 20 claims; independent 1 (method), 17 (apparatus). Same content as the WO set, renumbered: WO 15/16/17/18 = CN 13/14/15/16. |
| US20210012562A1 (US 16/926,529), "Probe-based dynamic global illumination" | NVIDIA; McGuire, Majercik, Luebke | US 62/873,101, 2019-07-11; US 62/986,337, 2020-03-06 | 2021-01-14 | **Abandoned 2023-08-23** after final rejection (2021-09-08), appeal, and Board decision (2023-06-07). No family members or continuations listed. |
| NVIDIA filing on McGuire, Mara, Nowrouzezahrai & Luebke, I3D 2017 light-field probes | — | — | — | **None found.** McGuire's GTC 2019 talk on the follow-on DDGI work (slide 13) says "No patents on the algorithm. No SDK or licensing." |

URLs: https://patents.google.com/patent/WO2026113651A1/en · https://patents.google.com/patent/CN122115679A/en · https://patents.google.com/patent/US20210012562A1/en · GTC 2019 slides: https://developer.download.nvidia.com/video/gputechconf/gtc/2019/presentation/s9900-irradiance-fields-rtx-diffuse-global-illumination-for-local-and-cloud-graphics.pdf

Claims were read from Google Patents' English machine translation of the Chinese text, through a summarizing fetch tool that would not return long verbatim quotes. The claim charts below are close paraphrases; counsel should work from the certified claims.

## 1. Tencent WO2026113651A1 / CN122115679A

What the description says the invention is: a reflection probe **generated at a position tied to the virtual camera and moved with it** (on a distance threshold, a grid-cell change, or a dwell time), holding a six-face G-buffer it **re-shades at runtime** when light changes. The reflected ray's hit point is found by casting rays from the reflecting surface ("using ray querying"); the probe is then sampled toward that hit. The probe stores the depth its six faces see; a hit farther than that depth is occluded, and occluded hits are filled by moving the probe or reading a preset lighting library. The "texture image of the first virtual object" is defined as the object's unshaded 2D image in screen space (a G-buffer-style image). Reflectors named: mirrors, flowing water, glass, metal. The description presents the method as removing manual baking.

The claims do not name ray query, hardware ray tracing, a bounding box or a proxy.

### Claim 1 (method; CN claim 1 is the same)

| Element (paraphrase) | PostRetro as built | Reading |
|---|---|---|
| 1a. Parallax-correct a reflection probe in the scene based on the position of a first virtual object and the virtual camera's viewpoint | Per fragment, the reflected view ray from the fragment's world position meets the probe box (slab test) and the distance search corrects that hit; §7.3a Reflection, `reflection_search()` | Appears present |
| 1b. Collect the illumination of the first object's reflected light through the corrected probe | Colour tile sampled toward the corrected lookup from the capture point | Appears present |
| 1c. Obtain a texture image of the first virtual object | The fluid shader samples the water face's albedo per fragment in a forward pass. No screen-space unshaded image exists (deferred is a non-goal, `index.md` §4) | Present under a plain reading of "texture image"; arguably absent if the description's definition (unshaded screen-space image) governs. **Counsel** |
| 1d. Determine pixel colour from the texture image and the illumination | `F·R + (1−F)·(surface over fog over scene)`; §7.3a Weight and composite | Appears present |
| 1e. Render the first object's reflection image from those colours | The fluid draw | Appears present |

Claim 1 as published is broad: on this reading it would cover most parallax-corrected probe reflections on a textured surface, including Bjorke 2004 and Lagarde & Zanuttini 2012 (see §3). That is an examination and validity matter for counsel; claims of a pending application often narrow before grant.

### Claim 20 (apparatus; CN 17)

Correction, acquisition, determination and rendering modules performing claim 1's steps. Same reading as claim 1. Claims 21–23 (device, medium, program product) carry whichever method claim they are read with.

### Dependent claims (chart, abbreviated)

| Claim (WO / CN) | Adds | PostRetro | Reading |
|---|---|---|---|
| 2 | Find the intersection of the reflected ray with a second virtual object; set the probe's sampling direction from the probe position toward it | The search finds the point where the reflected ray crosses the surface the capture stored (status `Hit`) and samples toward `point − C`; with nothing stored in the box (status `BeyondBox`) it samples toward the box hit, a proxy, not an object | Appears present for `Hit`; the box-hit case depends on whether a proxy box is a "virtual object". Hit is estimated from the probe's own distance tile, not by a scene ray query, but the claim as paraphrased names no method |
| 3 | Build a scene coordinate system; direction = intersection coordinates − probe coordinates | Same subtraction (`reflection_lookup`) | Appears present (trivial) |
| 4–9 | Generate the probe based on the camera position; move it when the camera moves past a threshold, changes grid cell, or dwells; probe attributes (type, size, range, intensity) | Probes are placed at compile time from water geometry (capture point 2 mm above the plane, area-centroid rule; `resource_management.md` §5.1); the camera never places or moves a probe | **Appears absent** |
| 10, 19 | Before collection, update the probe's stored lighting when an update condition holds (relative position change and/or light change) | The stored tile is never rewritten after compile. At sample time the fragment multiplies the sampled colour by a per-light ratio from live light values (§7.3a Live lighting) | Appears absent as built: nothing stored is updated. **Closest element after 16.** Counsel to confirm a per-fragment post-sample scale is not "updating the stored lighting information" |
| 11–13 | Updated lighting = colour recomputed by re-shading stored texture information (re-shading the G-buffer on light change) | No re-shading at runtime; capture shading happens once, offline, against the static world (`capture.rs`) | Appears absent |
| 14 | Polyhedral probe, per-face local lighting, faces updated in turn over update periods | Single octahedral tile per probe, never updated | Appears absent |
| 15 (CN 13) | After correction, if an occluder lies in the sampling direction toward the intersection, **complete** the collected lighting | Capture shadow: a fragment behind a stored surface (h ≥ κ) draws **no** probe term; within κ and beside the shadow the term fades out. Nothing is filled in | Appears absent (no completion). **Counsel** on whether withholding the term could be read as "completing" |
| 16 (CN 14) | Occluder exists when the probe-to-intersection distance exceeds the probe's depth in the sampling direction | h(Q) = \|Q − C\|₁ · w(Q − C) − 1 ≥ 0, with w the stored inverse L1 distance: the same comparison, in L1 and inverted. Applied at the fragment (capture shadow), at the box hit (bracket close), at each sphere step and false-position sample (locating the hit), and at the shadow-fade taps | **Closest element.** The comparison is present in substance; the claim's Q is "the intersection", which matches PostRetro's box-hit test most closely. Depends on 15, whose completion step appears absent |
| 17 (CN 15) | Completion by moving the probe to an unobstructed position and re-collecting | No probe moves; no second probe is consulted | Appears absent |
| 18 (CN 16) | Completion by marking the intersection and sampling a preset scene lighting library | No fallback lookup; the probe term reads no SH or lightmap (§7.3a Live lighting; source-scan acceptance row) | Appears absent |

**Summary.** On this reading PostRetro appears to have every element of independent claims 1 and 20 (subject to the "texture image" construction), and of claims 2–3. It appears to lack at least one element of every claim from 4 onward: camera-placed probes (4–9), runtime update of stored probe lighting (10–14, 19), and completion of occluded lighting (15–18). The occlusion comparison of claim 16 is present in substance but sits under claim 15's completion step.

## 2. NVIDIA

### US20210012562A1 (abandoned)

| Claim 1 element (paraphrase) | PostRetro | Reading |
|---|---|---|
| Compute an irradiance field probe, one of a plurality placed in a scene volume | One probe per bound face group; placed per water plane, not in a volume grid | Partly |
| Compute the probe's **diffuse irradiance** | Stores radiance (colour per direction), not irradiance; diffuse GI is the separate SH volume, not these probes | Appears absent |
| Compute **distance-distribution statistics** | Per texel the mean of 1/\|hit − C\|₁ over four supersamples; no variance, no squared distance, no Chebyshev test | A single mean statistic; counsel would decide whether that is a "distance-distribution statistic" |
| Encode the probe with both | Colour tile + R16F distance tile | Partly |

Independent claims 19 (media) and 20 (system) mirror claim 1; claim 22 is a client–server rendering system using it. The application is abandoned with no continuation listed, so on the record it confers no rights; counsel to confirm no continuation or foreign counterpart exists.

### I3D 2017 light-field probes

No patent or application found (Google Patents, Justia and web searches; Justia's inventor page returned 403). The paper is prior art, not a claim set. For the record, where PostRetro differs from it: one probe per face with no probe selection, no tracing across probes and no probe-to-probe continuation; inverse L1 distance, not radial distance with a low-resolution min-distance mip; a bounded 2 + ≤3 + 2 tap search with no polyline segmentation of the ray by octant and no hierarchical march; no irradiance or variance from these probes; no diffuse GI on this path.

## 3. Prior art and priority dates

Tencent priority 2024-11-28; NVIDIA priority 2019-07-11. Every publication below predates both.

| Publication | Date | Elements it discloses, as we read it |
|---|---|---|
| Bjorke, "Image-Based Lighting", GPU Gems ch. 19 | 2004 | Localized cube map: lookup corrected from the surface point and view ray against a proxy, sampled from the probe centre toward the proxy hit (Tencent 1a–1e; 2–3 if a proxy counts as the second object) |
| Szirmay-Kalos, Aszódi, Lazányi & Premecz, "Approximate Ray-Tracing on the GPU with Distance Impostors", CGF 24(3) | 2005 | Environment map with per-texel distance from a reference point; iterative search for the reflected ray's hit; lookup toward the hit from the reference point (1a–1e, 2, 3); in-front/behind tests of a ray point against stored distance (the comparison in 16) |
| Umenhoffer, Patow & Szirmay-Kalos, "Robust Multiple Specular Reflections and Refractions", GPU Gems 3 ch. 17 | 2007 | Distance maps; linear search then secant, each sample classified by comparing its distance from the reference point with the stored distance in that direction (the comparison in 16; PostRetro's search lineage) |
| Lagarde & Zanuttini, "Local image-based lighting with parallax-corrected cubemaps", SIGGRAPH talk | 2012 | Box proxy; per-pixel intersection of the reflected ray with the box from the pixel position and view; sample from the probe position toward the intersection (1a–1e, 2–3 with the box as proxy) |
| McGuire, Mara, Nowrouzezahrai & Luebke, I3D | 2017 | Per-probe radial distance; hit/miss/unknown classification of ray segments against stored distance (16's comparison); continuing an unresolved trace in another probe (close to 15/17) |
| Williams, "Casting curved shadows on curved surfaces", SIGGRAPH | 1978 | Shadow mapping: a point is occluded when its distance from the source exceeds the depth stored in that direction (the generic form of 16's test) |

Dates are publication years; exact publication days were not verified. The international search report's citations (Fujifilm CN104221370A / US20150009299A1; Tencent CN112870707A, CN114299220A; Lijie CN116758208A) include none of the above; their X/Y/A categories could not be read.

## 4. Design-around options

None appear needed for the dependent chains on this reading: PostRetro already sits outside 4–19 by construction (offline capture, static placement, no completion). The live risk is claims 1–3 as published, which no cheap change avoids without dropping parallax correction itself; that is a prior-art and examination question for counsel, not an engineering one.

Keep the current distance from the dependents, at no cost:

| Keep | Avoids | Cost |
|---|---|---|
| Probes placed and captured at compile time; never placed by or moved with the camera | 4–9 | None (current design) |
| Never rewrite, re-shade or time-slice the stored tile at runtime; live lighting stays a post-sample scalar ratio | 10–14, 19 | None (current design). A future load-time or runtime recapture would move toward 10–13; already rejected by `resource_management.md` §9 |
| Capture-shadow water draws **no** term, rather than being filled | 15–18 | None (current design) |

Watch before building:

| Future idea already in the plan or notes | Moves toward | Note |
|---|---|---|
| Probe-to-probe continuation for hidden water (brief Non-goals) | 15 + 17; the 2017 paper | Have counsel read the claims as granted first |
| Filling capture-shadow or unresolved water from the SH volume or lightmap at the hit (the "SH in the reflected direction" candidate is keyed by binding, for dropped probes, which is different) | 15 + 18 | Keep any SH fill keyed by binding, never triggered by the occlusion test |
| Screen-space layer filling the capture shadow | 15 (completion) | `fluid-ssr-reflections` compositing over the probe term on its own hits is a separate technique; filling specifically where the distance test flags occlusion is closer to 15 |

If counsel finds claim 16's comparison material on its own (for example, if it is rewritten into an independent claim during examination), a fallback exists: bake the capture-shadow decision per water texel at compile time (the compiler already evaluates the same test for its hidden-water warning) and drop the runtime origin test. The search's sign test would still compare distances, so this only narrows the overlap; cost is a per-face visibility mask in id 54, a format bump and a loss of the per-fragment fade.

## 5. For counsel

- Claims 1 and 20 of WO2026113651A1 / CN122115679A as published, against Bjorke 2004, Szirmay-Kalos 2005, Umenhoffer 2007 and Lagarde 2012; and how "texture image of the first virtual object" is construed (description: unshaded screen-space image).
- Claims 15–16 (CN 13–14): whether withholding the term (capture shadow) or resolving the lookup to a nearer stored surface could be read as "completing" the lighting.
- Claims 10 and 19: whether a per-fragment ratio applied after sampling is "updating the lighting information the probe records".
- Watch: CN examination (no request listed yet), EP regional phase (EP designated; 31-month deadline ≈ 2027-06-28), US national phase (30-month deadline ≈ 2027-05-28). Claims may narrow or broaden by amendment before grant.
- Confirm US20210012562A1 has no continuation or foreign counterpart.

Not accessed: Patentscope (403) and therefore the international search report's X/Y/A categories and written opinion; certified English claims (only Google's machine translation, read through a summarizing tool); Justia inventor listings (403); the full I3D 2017 and 2005 papers were not re-read for this note; Google Patents cannot be queried by inventor without a browser, so an NVIDIA filing under different wording could have been missed. Unpublished applications (filed within the last 18 months) are invisible to any search.
