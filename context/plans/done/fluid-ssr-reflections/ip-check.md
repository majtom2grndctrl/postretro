# fluid-ssr-reflections — IP check

> **Engineering triage, not legal advice. 2026-10-19. Read at 0076ad0d0.** A technical comparison of published claim text against the design in this brief, to brief the owner and a patent attorney before the pre-ship claims check. "Appears to have", "appears to lack" and "reads on" are engineering readings of claim text, not opinions on infringement or validity. Claim construction, validity and freedom to operate are counsel's.

Compared against: `index.md` (this brief, at 0076ad0d0), `rendering_pipeline.md` §7.3a (Reflection), `crates/renderer/src/shaders/fluid_reflection.wgsl` (`fluid_reflection_gate`, `fluid_reflection_composite`), and `plans/done/reflection-probes/ip-check.md`, whose probe-term conclusions this note does not redo.

## The design, as the claims are compared against it

| # | Element (from the brief) |
|---|---|
| D1 | A separate reflection pass before the fluid pass writes colour + confidence to its own target at ¼, ½ or full scene extent (player tier); the fluid shader samples it (nearest for pixel style, bilinear for smooth). |
| D2 | It runs only for draws and fragments the landed probe gate admits (`shader_variant` reflecting, `fluid_reflection_gate`: probe-bound, reflectivity > 0, seen from above, all `LightTermMask` bits). |
| D3 | A linear screen-space march against the current frame's full-resolution depth (`textureLoad` at scaled coordinates), fixed step budget scaled with resolution. No Hi-Z or downsampled depth, no acceleration structure, no ray tracing. |
| D4 | One ray per target pixel (smooth) or per water texel cell (pixel); the count never depends on roughness or any material value. |
| D5 | Confidence c = hit confidence × screen-edge fade, nothing else; never a function of ray direction. A ray leaving the screen gets c = 0. |
| D6 | Composite: colour `F·[c·SSR + (1−c)·s·Probe]`, weight `W = F·[c + (1−c)·s]`, then `+ (1−W)·(surface over fog over scene)` at coverage `W + (1−W)·a`. F = Schlick(0.02 × reflectivity, cap 1) × fog proximity band, unshadowed; s = the probe's capture-shadow weight. |
| D7 | s never triggers, selects or gates SSR; SSR runs on every gated fragment whatever s is. |
| D8 | No temporal reuse, no runtime cubemap re-render, no planar reflection, no probe change. |

A note on F: §7.3a and the landed shader fold s *into* F (`F = Schlick × s × fog band`, composite `F·R + (1−F)·…`). The brief redefines F as unshadowed and writes s into the probe term only. At c = 0 the brief's expression equals the landed one exactly (`F_brief·s = F_landed`), so the restatement is correct. It matters for the IP reading below: in the brief's form, **SSR's contribution F·c·SSR does not depend on s at all** — the premultiplied colour it adds is identical at s = 0 and s = 1. Only the probe's share and the share of the surface beneath change with s.

## Summary

| Reference | Status | Closest claim | Verdict | Confidence | Workaround needed? |
|---|---|---|---|---|---|
| Tencent WO2026113651A1 / CN122115679A | Pending (WO; CN published, no exam request listed); no US/EP entry yet | WO 15 (CN 13), with 16/18 | SSR adds nothing that reads on 15–18: SSR is unconditional, and its contribution is the same whether or not the obstruction test fires. Claims 1–3 unchanged from the probe IP check | Medium (pending; "supplement" unconstrued; machine translation) | No. Pin s-independence (W0) |
| EA US10621774B2 (viewport exit) + KR102811807B1 + CN110874858B | Granted, active; US to 2038-09-15, KR/CN to ≈2039-03 | US 1/9/17; KR 1/8; CN 1/8/15 | Appears not to read: lacks roughness-driven ray count and ray tracing of failed rays | High | No |
| EA US10796474B2, US10818079B2 (same family) | Granted, active | 1/8/15 each | Appears not to read: roughness-based reuse; reflection-realm temporal reuse | High | No |
| EA US12354210B2 (ray-traced occlusion and reflections) | Granted 2025-07-08, active | 1/12/19 | Appears not to read: no fixed-distance ray set, no ray tracing (claim text via PatSnap summary) | Medium-high | No |
| Microsoft US10586375B2 / US10360720B1 | Granted, active to ≈2038-02-22 | '375 claims 1, 7, 10; '720 claims 1, 7, 11 | Appears not to read: every independent claim requires a reflected-toward-camera determination; the design makes none | High | No, if the direction ban is kept in implementation (W0) |
| Microsoft EP3756162A1 | **Intention to grant 2026-07-16**; granted text not read | A1 claims 6 and 11 (no direction element as published) | Unknown until the granted text is read. As published, 6 and 11 need "a raytracing technique" and selecting a technique per pixel; the design has neither on a plain reading | Low (text unverified) | Not on present information |
| Microsoft CN111788608B / CN119904565A | CN granted 2025-01-24 / divisional pending | 1, 7, 10 | Same as the US claims (direction element present) | Medium-high (machine translation) | No |
| **NetEase CN114748874B** (new) | **Granted** (published B 2025-07-22), active, CN only | 1 | **Possible read.** Claim 1: water pixel's reflection position on the reflected object; first and second reflection information "according to the reflection position", from the opaque texture at its UV and from preset environment reflection (probe); Fresnel; combine. The design determines the probe term from its own search, never from the SSR hit, which is the arguable missing element. Claim text from PatSnap's translation, **not verified** against a primary source | Low-medium | Keep W1 (free). Counsel to read the certified claim |
| Others screened (§6) | Mixed | — | Not plausibly relevant | Medium | No |

Bottom line: none of the three concerns the brief names reads on the design as written, and the brief's own levers (the direction ban, s outside SSR's coefficient, no ray tracing) are what keep it clear. The one new item worth the owner's attention is a granted Chinese NetEase patent on water reflection that combines a screen-space colour lookup with an environment/probe reflection under Fresnel; the design's best distinction from it is that the probe term is never derived from the SSR hit. Its claim wording was read from a third-party translation only.

## 1. Tencent WO2026113651A1 / CN122115679A

Status re-checked on Google Patents 2026-10-19: WO pending, EP designation event 2026-07-15 only; CN122115679A shows publication (PB01, 2026-05-29) only, no substantive-examination entry. Family: WO + CN only. Priority CN 202411731795.3, 2024-11-28. National-phase deadlines: US ≈ 2027-05-28 (30 months), EP ≈ 2027-06-28 (31 months). Claims read from Google Patents' English machine translation of the Chinese, extracted as page text (not through a summarizer this time).

### Claim 15 (WO; CN 13 is the same)

> "The method according to any one of claims 1 to 14, wherein, after performing parallax correction on the reflection probes … the method further includes: The acquisition direction for collecting illumination information by the reflection probe at the intersection point corresponding to the reflected light rays after parallax correction is obtained; Wherein, the intersection point corresponding to the reflection probe is the intersection position of the reflected light of the first virtual object and the second virtual object in the virtual scene; If there is an obstruction in the acquisition direction, the illumination information acquired by the parallax-corrected reflection probe is supplemented. The process of collecting illumination information of reflected light from the first virtual object using a parallax-corrected reflection probe includes: Obtain the illumination information of the reflected light obtained through completion."

CN 13's translation renders the operative step as "If a shielding object exists in the acquisition direction, supplementing the illumination information acquired by the reflection probe after parallax correction", and the last element as "acquiring illumination information of the reflected light obtained through complementation".

| Element | Design with SSR | Reading |
|---|---|---|
| 15-pre. Claim 1 (and whichever of 2–14 it is read with) | Unchanged by SSR. The probe IP check found claims 1–3 appear present, subject to the "texture image" construction | As before; SSR neither adds nor removes anything |
| 15a. Obtain the probe's acquisition direction toward the intersection of the first object's reflected ray with a second object | The probe term samples from C toward the search's hit (`point − C`) | Appears present (probe term, unchanged) |
| 15b-i. Condition: "if there is an obstruction in the acquisition direction" (the direction from the probe to *the intersection*) | s comes from the capture-shadow test at the fragment or cell start point (is the *water point* behind a surface C saw?) plus its azimuth fade taps, not from a test of C's view of the reflected hit. The search's own sign tests run at the box hit and sphere steps, but their outcome is where to sample, never s | A test exists, but at a different point from the claim's. Arguable either way; counsel |
| 15b-ii. "the illumination information acquired by the … reflection probe is supplemented" upon that condition | SSR runs on every gated fragment (D7). Its contribution, F·c·SSR, is identical at s = 0 and s = 1: nothing is added, switched or re-weighted for SSR when the obstruction test fires. What the test changes is the probe's own share (withheld, as landed) and the surface's share beneath | **Appears absent.** There is no step conditioned on obstruction that adds anything; the conditional structure of the claim ("if … supplemented") has no counterpart |
| 15c. The probe's collection "includes" obtaining the completed illumination | SSR is a separate layer composited beside the probe term; the probe's collected value is never altered by SSR | Appears absent |

The concern in the brief is real but narrow: where s ≈ 0 and c > 0, what the player sees is SSR alone, so the *effect* is that the capture shadow is no longer empty. The claim, as worded, reads on a method that detects obstruction and *then* supplements the probe's information (claim 17: move the probe; claim 18: mark the intersection and sample a lighting library). It does not, on this reading, reach a layer that is composited everywhere by its own hit confidence and happens to be the only layer left where the probe withdraws. The argument rests on one checkable fact: **SSR's coefficient contains no s.** If an implementation ever made SSR's weight, its execution, its march parameters or its target depend on s or on the distance tile, that argument weakens sharply.

Two further points that make an amendment toward screen space less likely to catch this design:

- The description discloses only two completion means: moving the probe (claim 17) and sampling a world-space "lighting information library" (claim 18). Its definition of that library is "a lighting information database used to store lighting information in the world space, including lighting information not in the screen space". No screen-space completion is described, so a claim narrowed to screen-space fill would lack written support in the CN/EP and, likely, US phases. Counsel to confirm.
- Claim 18's "lighting information library" read onto the frame's colour buffer would still need claim 15's conditional completion and claim 18's "marking the intersection points". SSR finds its own hit by marching; it does not take the probe's intersection, mark it, and look it up.

### Claims 16, 17, 18 (CN 14, 15, 16)

All depend from 15. Claim 16's distance-greater-than-stored-depth test is present in substance in the probe term (probe IP check §1); it is not changed by SSR. Claims 17 and 18 add completion means the design does not use. No change from the probe IP check.

### Verdict

SSR as briefed adds no element of claims 15–18 that the landed probe term lacked. Confidence medium: the application is pending, the claim term "supplement/complete" has not been construed by any office, and the text is a machine translation. The brief states s-independence as a rule and pins it with an acceptance row (§8).

## 2. EA / Uludag "Systems and methods for rendering reflections" family

Three US applications filed 2018-08-10 by Electronic Arts, inventor Yasin Uludag, with combined KR and CN counterparts.

| Document | Status (Google Patents, 2026-10-19) | Subject of independent claims |
|---|---|---|
| **US10621774B2** (US 16/101,019) | Granted 2020-04-14; active; 4th-year fee paid 2023-10-16; adjusted expiry 2038-09-15 | **The viewport-exit fallback** (claims 1, 9, 17) |
| US10796474B2 (US 16/101,098) | Granted 2020-10-06; active; expiry 2038-10-12 | Roughness-matched reuse of neighbours' hits across a checkerboard (1, 8, 15) |
| US10818079B2 (US 16/101,103) | Granted 2020-10-27; active; expiry 2038-09-25 | Temporal reuse via a "virtual reflection realm" across frames (1, 8, 15) |
| KR102811807B1 (KR 10-2019-0035373) | Granted 2025-05-22; anticipated expiry 2039-03-27 | Same as US10621774B2 (claims 1, 8; 15–23 deleted) |
| CN110874858B (CN 201910248999.4) | Granted 2024-04-09; anticipated expiry 2039-03-29 | Same as US10621774B2 (claims 1, 8, 15) |

No EP or JP member is listed.

### US10621774B2, claim 1 (claims 9 and 17 are the medium and device forms; KR 1/8 and CN 1/8/15 match in substance)

> "determining … a roughness value of a surface of an object in the pixel; determining … a number of rays to spawn for the pixel based on the roughness value, wherein a greater roughness value corresponds to a greater number of rays to spawn for the pixel; for each ray …, selecting … a ray direction …; for each ray …, performing … ray marching of the ray …; for each ray where ray marching is successful, storing … color information of an object intersected by the ray discovered via ray marching, wherein ray marching fails for a ray if the ray is ray marched to an edge of a viewport corresponding to the image without intersecting any other objects, and wherein ray marching is successful for a ray if the ray intersects an object before the ray is ray marched to the edge of the viewport; for each ray where ray marching fails, casting … the ray via ray tracing and storing color information of an object intersected by the ray discovered via ray tracing; and generating … reflection information for the pixel based on the color information of objects intersected by rays discovered via ray marching and the color information of objects intersected by rays discovered via ray tracing."

| Element | Design | Reading |
|---|---|---|
| Roughness value of the surface in the pixel | None. `surfaceReflectivity` scales Fresnel; there is no roughness input (§13) | **Appears absent** |
| Number of rays from roughness, more for rougher | One ray per target pixel or per texel cell, always (D4) | **Appears absent** |
| Ray direction per ray; ray marching | Mirror direction; screen-space march (D3) | Present |
| Store colour on success; failure = reaching the viewport edge without a hit | A ray leaving the screen gets c = 0; a hit stores scene colour | Present in substance (the failure definition matches) |
| On failure, **cast the ray via ray tracing** and store the colour it finds | On a miss the composite shows the pre-captured probe term, which is computed for every fragment whether SSR hit or not. Nothing is cast in response to the failure; there is no ray tracing (D3, D8) | **Appears absent** |
| Generate reflection from marched and traced colours | Composite of SSR and probe | Partly; the "traced" input is absent |

**Is the screen-edge fade the problematic element?** No. The claims define failure as reaching the viewport edge, which the design shares (it is inherent to any SSR), but the screen-edge *fade* appears in no claim of the family. **Is the "other technique" limited to ray tracing?** In the US, KR and CN independent claims, yes: "casting … the ray via ray tracing". A pre-baked probe lookup is not that step on a plain reading. One construction risk to note: the probe term's distance search is in the "approximate ray-tracing with distance impostors" lineage (Szirmay-Kalos et al. 2005). Even if counsel thought that could be called ray tracing, the probe term is not *cast for each failed ray*; it runs unconditionally, and the roughness elements remain absent. Keep project docs calling it a "distance search", as §7.3a does.

US10796474B2 (checkerboard reuse gated on roughness difference and reachability within a roughness-shaped ray distribution) and US10818079B2 (two-frame reuse through a "virtual reflection realm" and a roughness-threshold match) each need roughness and cross-pixel or cross-frame reuse. The design has no roughness, no checkerboard, no reuse between pixels beyond a texel cell sharing one ray, and no temporal reuse (D8). **Appear not to read; high confidence.**

### EA US12354210B2 (filed 2022-03-31, granted 2025-07-08, inventor Hulm)

Claim text read only through PatSnap's page summary (Google Patents was rate-limiting; USPTO and Justia returned 403): independent claims 1/12/19 select a pixel, take a set of directions from a location, cast a fixed-distance ray per direction, take an occlusion value from a lighting probe for an unoccluded direction and **by ray tracing** for an occluded one, and accumulate. Dependent claim 5 mentions "obtaining a screen space reflection value for the direction based on one or more lighting probes". The design has no direction set, no fixed-distance rays and no ray tracing. **Appears not to read; medium-high confidence (claims not read verbatim).**

## 3. Microsoft "Hybrid raytracing approach for modeling light reflection"

Family (priority US 15/902,926, 2018-02-22; inventors Nevraev, Stanard): US10360720B1 (granted 2019-07-23, 4th-year fee paid), US10586375B2 (continuation, granted 2020-03-10, 4th-year fee paid), WO2019164699A1, **EP3756162A1 (status "grant of patent is intended", Rule 71(3) communication 2026-07-16)**, CN111788608B (granted 2025-01-24), CN119904565A (divisional, published 2025-04-29, examination requested). US anticipated expiry 2038-02-22.

### US10586375B2

> Claim 1: "… determining whether the reflection ray for the selected pixel is reflected back to a virtual camera position of the frame; if the reflection ray is determined to be reflected back to the virtual camera position of the frame, performing a first reflection ray technique …; and if the selected reflection ray is determined not to be reflected back to the virtual camera position of the frame, performing a second reflection ray technique …"
>
> Claim 7 (claim 10 is the system form): "adaptively selecting a rendering technique from among multiple available rendering techniques for use in rendering a pixel value representative of a reflection …; computing the pixel value using the selected technique; and buffering the pixel value in a hardware buffer, wherein the adaptively selecting the rendering technique comprises: determining that a reflection ray from the pixel location is not reflected toward a virtual camera position of the computer-generated environment; and performing a screen space reflection technique for the reflection ray."

| Element | Design | Reading |
|---|---|---|
| A list of reflection ray origins and directions (cl. 1) | The reflection pass computes them per target pixel or cell | Present |
| **Determining whether / that the reflection ray is (not) reflected toward the camera** (cl. 1, 7, 10) | No such determination anywhere. c = hit × screen-edge fade (D5). The gate's "seen from above" tests the view against the surface normal, not the reflected ray against the camera | **Appears absent** |
| Technique chosen by that determination (cl. 1); SSR performed upon it (cl. 7, 10) | SSR runs on every gated fragment; the probe term runs on every gated fragment; they blend by c | Absent (follows from the above) |
| Adaptively selecting a technique per pixel (cl. 7, 10) | A continuous per-pixel weight between two always-computed layers | Arguable on its own, but the claims' "wherein" clause requires the direction determination, which is absent |
| Dependents 11–13: reduced-resolution image for one technique ("one-quarter") | D1 runs SSR at ¼/½/full | Present in substance, but these depend from claim 10 and fall with it |

US10360720B1 claims 1, 7 and 11 add "a raytracing technique" selected when the ray is reflected toward the camera; the design lacks both that technique and the determination. **Appears not to read; high confidence**, provided the implementation never adds a ray-direction early-out or weight (see W0: a march that skips or zeroes rays heading toward the camera would be the "determining … not reflected toward a virtual camera position" step). A ray heading toward the camera must march like any other and end by step budget or by leaving the screen or near plane, with c from hit and edge alone.

Per-pixel confidence on its own does not read on any independent claim: every US and CN independent claim ties the per-pixel choice to the reflected-toward-camera determination.

### EP3756162A1 — the open item

The published A1 claims are broader than the US grants. Independent claim 6: "adaptively selecting a rendering technique from among multiple available rendering techniques …, the multiple available rendering techniques including a raytracing technique; computing the pixel value using the selected technique; and buffering the pixel value in a hardware buffer." Independent claim 11: "selecting a technique from among multiple available techniques …, the multiple available techniques … including a raytracing technique and a screen space reflection technique; computing the pixel value using the selected technique; and rendering the frame …". Neither A1 claim recites the camera-direction determination.

The EPO issued a Rule 71(3) communication on 2026-07-16; the text intended for grant is on the EP Register, which returned 403 / a JavaScript challenge in this session. **Not verified.** Given that the US (twice) and CN offices each granted only with the direction element, the EP text is likely narrowed similarly, but that is an inference. Even read as published, claims 6 and 11 need "a raytracing technique" among the selectable techniques and the selection of one technique to compute the pixel value. The design has SSR and a pre-captured probe lookup, blended; no ray tracing. **Low confidence until the granted text is read**; on present information no workaround is needed.

### CN111788608B / CN119904565A

Granted claims 1, 7 and 10 of CN111788608B match US10586375B2's in substance (the direction determination is in each; claim 7's machine translation garbles it as "determining a virtual camera position at which a reflected ray … is not reflected toward"). CN119904565A (divisional, pending) has the same claim 1/7/10 structure with "first/second reflected ray technique". Medium-high confidence, machine translation.

## 4. NetEase CN114748874B — "Water body reflection processing method, device, equipment and storage medium" (new)

NetEase (Hangzhou) Network Co Ltd; inventor Qiu Xudong; application CN 202210346543.3 filed 2022-04-02; A published 2022 (CN114748874A); **B (granted) published 2025-07-22**; status active; expiry ≈ 2042-04-02. No US, EP or WO family member listed (PatSnap; Google Patents rate-limited before the family could be confirmed).

**Claim text not verified from a primary source.** Google Patents blocked further requests mid-session. The claim below is PatSnap Eureka's English translation, read in consecutive quoted fragments through a fetch tool; it was not checked against the CNIPA original or Google's translation. Treat it as unverified.

> Claim 1 (PatSnap translation): "A method for processing water body reflection, characterized in that, Including: Obtaining the reflection position of the light reflected by each pixel on the water surface on the target object, where the target object is an object reflected by the water; Determining, according to the reflection position corresponding to each pixel, a first reflection information and a second reflection information corresponding to each pixel; the first reflection information is sampled from an opaque texture declaration map, and the second reflection information is sampled from a preset environmental reflection information; Determining, according to the normal and the viewing vector at each pixel, the Fresnel information corresponding to each pixel; Obtaining the target reflection information corresponding to each pixel according to the Fresnel information, the first reflection information and the second reflection information corresponding to each pixel; Rendering each pixel according to the target reflection information; The determining, according to the reflection position corresponding to each pixel, a first reflection information and a second reflection information corresponding to each pixel includes: For each pixel, converting the reflection position corresponding to the pixel in world space to clip space to obtain the reflection position of the pixel in clip space; Converting the coordinates of the reflection position in clip space to a preset interval range to obtain the UV coordinates corresponding to the reflection position; Sampling in the opaque texture declaration map according to the UV coordinates to obtain the first reflection information corresponding to the pixel, and sampling in the preset environmental reflection information to obtain the second reflection information corresponding to the pixel."

The description names the environment source as an "environmental reflection map or reflection probe". Dependents (PatSnap summary): claim 2, the reflection position is a **preset distance** along the reflection direction (no march); claim 3, Fresnel from dot(view, normal); claim 4, Fresnel is the lerp mask between the opaque-texture and environment samples (`Lerp(A, B, x)`, x the Fresnel value). Claims 5–8: device, electronic device, medium, program product.

| Element | Design | Reading |
|---|---|---|
| 1a. Water pixel's reflection position on the reflected object | The march's hit point on the depth surface. (The probe term's search also estimates a hit, but from the distance tile, not a screen position) | Appears present (SSR hit) |
| 1b. First **and second** reflection information both determined "according to the reflection position" | First (SSR colour) comes from the SSR hit. Second (probe) comes from the probe term's own search, started at the fragment and sampled from C; it never reads the SSR hit or its UV | **Arguably absent.** The design has no single reflection position from which both are determined. Turns on whether "according to the reflection position" binds the second information or only the first |
| 1b-wherein. World → clip → UV of that position; opaque texture sampled at the UV | Scene colour holds exactly the opaque scene (Decisions); the hit's screen coordinate is where it is read | Appears present for the first information |
| 1b-wherein. "sampling in the preset environmental reflection information to obtain the second" | Probe colour tile sampled toward the search hit from C | Present in isolation (the clause names no sampling coordinate) |
| 1c. Fresnel from normal and view | Schlick from view and flat normal | Appears present |
| 1d. Target reflection from Fresnel, first and second | `F·[c·SSR + (1−c)·s·Probe]` | Present on a plain reading; the mixing is by c, not by Fresnel (claim 4's structure is absent) |
| 1e. Render | Fluid pass | Present |
| Claim 2. Position at a preset distance, no search | A depth march | Absent |
| Claim 4. Fresnel as the mix between screen and environment | Fresnel scales both; c mixes them | Absent |

**Verdict: possible read on claim 1; low-to-medium confidence that it does not read, resting on element 1b.** The design's distinguishing feature is that the environment term is never determined from the screen-space reflection position. That is true of the brief as written and should be kept as a constraint (W1). If counsel reads 1b loosely, claim 1 would also cover Unity's 2017 SSR (screen colour at the hit, `lerp` over reflection probes, Fresnel applied) and most SSR-over-probe water from before 2022, which is a validity question (§6). Territorial scope is China only: it matters if the game is made, sold, offered or imported there.

## 5. Others screened

Google Patents full-text queries (priority ≥ 2009; US/EP/WO/CN/JP/KR) on SSR with reflection probes, cubemaps or environment maps; fallback, blend, confidence, edge fade; reduced resolution with upsampling; water. Assignee-oriented web searches for Sony, AMD and others. Each item below was read only to the depth needed to see that an independent claim needs something the design lacks; most were read through PatSnap summaries, not verbatim.

| Document | Assignee | Status | Why it does not appear to read |
|---|---|---|---|
| CN118096983A (2024-03-14) | Booming Tech | Pending | SSR, then **ray tracing** for pixels SSR left undetermined |
| CN117496039A (2023-11-07) | NetEase | Pending | Static objects from a reflection probe, dynamic objects from a **planar reflection map**, mixed; no SSR (planar is a non-goal here) |
| CN115120973B (2022-06-15) | NetEase | Granted 2025 | Mirrored-point model sampling under a terrain-difference gate; SSR only as an alternative |
| CN120022596A (2024-12-30) | NetEase | Pending | Pre-built model texture under a camera angle and a relative-position relation; disclaims SSR and capture |
| CN116637366A (2023-04-28) | NetEase | Published | Sky reflection from a texture map with noise; SSR/probes only as background |
| CN116778049B (2022-03-07) | Tencent | Granted 2026 | Multi-threaded mirror (planar, second camera) pipeline |
| WO2022227553A1 (2021-04-30) | Perfect World | Unknown | SSR step-and-compare classifying the reflected object as static or dynamic; the static/dynamic colour sources and any blend could not be read (abstract only). **Low-confidence clear; worth one look in the pre-ship check** |
| US20210225067A1 (2018-12-13) | Tencent | Not read (404 on PatSnap) | Title-level only; flagged for the pre-ship check |

Nothing turned up from Sony, Nintendo, Nvidia, AMD, Activision, Ubisoft, Epic, Unity, Apple, Qualcomm, Imagination, Arm or Samsung that combines SSR with a probe or environment map by confidence or hit. Screening was keyword-based and Google Patents rate-limited mid-session, so this is not a clearance search.

## 6. Prior art

Every item predates every priority date above (Microsoft 2018-02-22; EA 2018-08-10; NetEase 2022-04-02; Tencent 2024-11-28) unless noted.

| Publication | Date | What it discloses, as we read it | Read |
|---|---|---|---|
| Sousa, Kasyan & Schulz, "Secrets of CryENGINE 3 Graphics Technology", SIGGRAPH 2011 (slides 29–32) | 2011 | Screen-space ray-traced local reflections in Crysis 2, composited over cubemap reflections | Secondary sources only |
| Valient, "Taking Killzone Shadow Fall Image Quality into the Next Generation", GDC 2014 — https://www.guerrilla-games.com/read/taking-killzone-shadow-fall-image-quality-into-the-next-generation-1 | 2014-04 | Half-resolution screen-space ray march; output "hit color, hit mask"; "Each stage falls back to next one on miss" (SSR → localized cubemap zones → background cubemap); composing with cubemaps where "the ray-trace mask controls the blending". Uses temporal reprojection, which this design does not | Slide text via SlideShare |
| McGuire & Mara, "Efficient GPU Screen-Space Ray Tracing", JCGT 3(4):73–85 — https://jcgt.org/published/0003/04/04/ | 2014 | Screen-space ray march against a depth buffer (perspective-correct DDA), near-plane clipping, thickness test; GLSL listing | Abstract |
| Unreal Engine 4 (release 2014-03): SSR composited over reflection captures, with a screen-edge fade on the SSR alpha | 2014 | The SSR-over-captures arrangement and edge fade; Epic's docs describe SSR combined with static Reflection Captures (https://dev.epicgames.com/documentation/en-us/unreal-engine/reflections-environment-in-unreal-engine) | Not re-read; engine source is access-controlled. Counsel should pull the 4.0 shader |
| Wronski, "The future of screenspace reflections" (bartwronski.com, 2014-01) | 2014 | SSR limits and fallback discussion | Not re-read |
| Stachowiak, "Stochastic Screen-Space Reflections", SIGGRAPH 2015 Advances (http://advances.realtimerendering.com/s2015/) | 2015 | Frostbite SSR: half-resolution tracing, hit-dependent blending with probes | Not re-read |
| Unity Post-processing Stack v2, `ScreenSpaceReflections.hlsl` (https://github.com/Unity-Technologies/PostProcessing, v2 branch; first commit 2017-08-28; verified at commit 52e8999, 2017-11-23) | 2017 | `Attenuate()` screen-edge fade; `float confidence = test.w * Attenuate(test.xy) * Vignette(test.xy);` (hit × edge × vignette); composite `resolve.rgb = lerp(reflectionProbes.rgb, resolve.rgb, confidence * fade);` — the brief's `c·SSR + (1−c)·Probe` exactly. **Note:** the same file's composite also multiplies in a ray-direction term, `saturate(2.0 * dot(-eye, normalize(reflect(-eye, normal))))`, which the brief deliberately omits | Read (raw source at that commit) |
| Godot 3.0 (stable 2018-01-29) `drivers/gles3/shaders/screen_space_reflection.glsl` | 2018 | `margin_blend` screen-edge fade; output alpha `fade * margin_blend`; SSR blended with reflection probes (Godot docs) | Read (raw source) |
| Lagarde & de Rousiers, "Moving Frostbite to Physically Based Rendering 3.0", SIGGRAPH 2014 course notes §4.10 | 2014 | Specular occlusion applied to probe reflections; a structural analogue of a visibility weight on the probe term alone | **Not verified** whether SSR is exempted there; counsel to check before relying on it |
| Szirmay-Kalos, Aszódi, Lazányi & Premecz, CGF 24(3) | 2005 | Distance-impostor search behind the probe term (probe IP check §3) | Per probe IP check |

**Where the design practises the prior art.** The SSR layer and its composite are, element for element, Unity PPv2 (2017) without its direction term: a screen-space march, confidence = hit × screen-edge fade, `lerp(probe, SSR, confidence)`, with Fresnel applied to the result. Killzone Shadow Fall (2014) adds the half-resolution trace and the miss-to-cubemap handoff; McGuire & Mara (2014) the march; Godot 3.0 the edge fade and probe blend. Every element of D1–D6 except s and pixel style appears in public sources before 2018.

**Where the design adds something.** (a) The capture-shadow weight s on the probe term only, from a distance-tile test — which comes from the landed probe term, not from this brief, and is exactly where Tencent claim 15 sits; (b) pixel style: one ray per water texel cell from the cell point the probe term uses, so both layers break into the same blocks; (c) a player resolution ladder over a fixed reach. None of (b) or (c) appears in any claim found.

## 7. Workarounds

None is required on the claims read. Ranked by cost:

| # | Change | Removes | Cost | New risk |
|---|---|---|---|---|
| **W0 (recommended, free)** | Pin the two facts the clearances rest on, as acceptance rows and shader comments: (i) SSR's premultiplied contribution is bit-identical for s = 0 and s = 1 at fixed F and c, and the reflection pass reads neither s nor the distance tile nor any probe colour (it may read the probe table's box and capture point for the pixel cell point, and the probe index and reflectivity for the gate; none of these carries the obstruction test); (ii) c reads no ray-direction quantity — no early-out, skip, weight or step change for rays heading toward the camera (view- or projected-space z sign), and the march carries no `dot(view, reflect)` term of the kind Unity PPv2's composite has | Keeps Tencent 15b-ii absent; keeps Microsoft's direction determination absent | None (rules and rows now in the brief, §8) | None |
| **W1 (recommended, free)** | Keep the probe lookup keyed to its own search from the fragment or cell point, never to the SSR hit, its depth or its UV. This is already the design; state it as a constraint so a later change (for example, sampling the probe toward the SSR hit "for consistency", or using the SSR hit to seed the distance search) does not add the element | NetEase 1b (both informations "according to the reflection position") | None now | None |
| W2 (only if counsel reads NetEase claim 1 broadly and China matters) | No engineering change avoids a broad "screen colour + environment + Fresnel → water reflection" claim short of dropping the screen-space layer for Chinese distribution or dropping Fresnel from one layer. Better route: invalidity on Unity PPv2 (2017), UE4 (2014), Killzone SF (2014) | — | Visual: high (if a layer is dropped) | — |
| W3 (only if counsel reads Tencent 15 on the effect) | Put s into SSR's coefficient too (`F·s·[c·SSR + (1−c)·Probe]`) so SSR never shows where the probe withdraws | 15b-ii entirely | **High**: loses the camera-facing side of objects in the capture shadow, a stated goal and a Manual acceptance row; the composite at c = 0 is unchanged | SSR would then read s, which is the coupling the clearance argues against; it also makes SSR's visibility depend on the probe's occlusion test |
| W4 (not recommended) | Replace instead of blend (SSR where hit, probe where not, hard switch) | Nothing found requires it | Visual: hard seams at screen edges, against the Manual row | A per-pixel technique *selection* is closer to Microsoft's "adaptively selecting" language, not further |
| W5 (not recommended) | Hard screen-edge cutoff instead of a fade | Nothing: no claim found recites the fade | Visual: a hard line, against the Manual row | None |

## 8. Brief wording

Applied to `index.md` on 2026-10-19:
- **Decisions.** The EA "resembles the SSR-exit pattern" line is removed; §2 records the elements the design lacks. W0 and W1 are stated as layering rules, W0 narrowed to probe colour so the pixel cell point may still read the probe table's box and capture point.
- **Problem and Manual.** The capture shadow no longer appears as a motivation. The goal is camera-visible and dynamic content, including the camera-facing sides of objects standing in the water.
- **Acceptance.** Rows pin s-independence (bit-identical at s = 0 and 1), a reflection pass with no probe or s input, and direction-blind marching. Path asks for code comments stating the three rules.
- **`research.md` §IP** summarizes this note instead of the "resembles" framing.

Applied to `rendering_pipeline.md` §7.3a at promotion:
- the screen-space layer bullet states that s scales the probe's share only and SSR's contribution never depends on it; that is the sentence counsel will look for;
- Limits says water C cannot see stays unreflected, and never describes the screen-space layer as filling the capture shadow.

## 9. For the owner's pre-ship check

- **EP3756162 (Microsoft):** read the text intended for grant (Rule 71(3), 2026-07-16) on the EP Register, then the B1 when it publishes; compare against the design, especially whether independent claims keep the reflected-toward-camera element and whether "raytracing technique" could be read onto a probe distance search. The opposition window is 9 months from the mention of grant. Note the validation states / Unitary Patent.
- **CN114748874B (NetEase):** obtain the certified claim 1 text and confirm whether "according to the reflection position" binds the second (environment) information; confirm there is no foreign family; assess validity against Unity PPv2 (2017), UE4 (2014) and Killzone SF (2014). Relevant only for distribution in China.
- **Tencent WO2026113651A1 / CN122115679A:** watch CN substantive examination, US national phase (≈ 2027-05-28), EP regional phase (≈ 2027-06-28); read any amendment of claim 15's "supplement/complete" and any new claim on screen-space completion (and whether the description supports one). Carry forward the probe IP check's items for claims 1–3, 10, 16, 19.
- **EA US10621774B2 / KR102811807B1 / CN110874858B:** confirm no continuation was filed in the US (none listed) and that EA has no later filing claiming SSR handoff to a non-ray-traced fallback.
- **WO2022227553A1 (Perfect World)** and **US20210225067A1 (Tencent)**: read the claims; both were screened at abstract level only.
- **Microsoft CN119904565A:** pending divisional; check its granted text if it grants.
- **Size-based capture exclusion (`reflection-probe-capture-exclusion`):** compare with UE Visible in Reflection Captures / Cull Distance Volumes, Unity Reflection Probe Static / culling mask / near clip; read Perfect World WO2022227553A1. Engineering triage, not legal advice.

Not accessed: EP Register and Patentscope (403 / JavaScript challenge); USPTO PPUBS PDFs and Justia (403). Google Patents rate-limited this session after the Tencent, EA and Microsoft families were read, so NetEase, Perfect World, Booming and EA US12354210B2 were read only through PatSnap Eureka pages via a summarizing fetch tool. Chinese and Korean claims are Google's machine translations. Unpublished applications (filed within the last 18 months) are invisible to any search; the UE4 4.0 shader source, the Frostbite 2014/2015 material and Wronski 2014 were not re-read.
