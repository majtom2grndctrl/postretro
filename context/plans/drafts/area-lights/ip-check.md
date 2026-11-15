# Area Lights — IP and Prior-Art Check

**Date investigated:** 2026-11-15, from a parallel run in the owner's local session, answering `ip-questions.md`. The three closest filings' independent claims were then read in full in a browser on Google Patents.
**Status:** Engineering triage for counsel, not legal advice. "Appears present", "appears absent" and "arguable" are engineering readings of claim text. They are not opinions on infringement, validity or freedom to operate. Every negative result means "not found", never "clear".

> **Read this when:** deciding whether the area-light brief is clear to build, choosing its visibility-weighting option, or charting a new filing against it.
> **Sources:** Google Patents and ppubs search interfaces could not be queried, so there was no CPC- or assignee-filtered sweep. Individual patent pages loaded through a summarising fetch. Full independent claims were read for US9262860B2, US10325401B2, US12293451B2 and US10891783B2, as each *Source* entry states. Every other reading is from summarised or partial claim text.
> **Related:** `ip-questions.md` (the request and design statement) · `research.md` §5 (prior art) · `context/research/gi-rtx-lite/ip-check.md` (format, and filings charted for the GI stack)

---

## Pieces under review

The design is stated in claim-like terms in `ip-questions.md` §Design:
1. An offline lightmap from a polygon light: an analytic, horizon-clipped Lambert term × integrand-weighted sampled visibility.
2. The visibility weighting: (a) the Heitz–Hill–McGuire ratio estimator over solid-angle samples, or (b) projected-solid-angle sampling.
3. Probe L2 SH: analytic unshadowed coefficients plus a control-variate shadow term.
4. Runtime glow driven by the per-light descriptor that also scales the baked animated lightmap.
5. Reach window max(0, 1 − (d/R)⁴)².

## Filings found

| Document | Owner | Priority | Status | Source | Pieces | Reading |
|---|---|---|---|---|---|---|
| US10325401B2 (from US20170263043A1; EP3217361B1) "bounce light map" | Imagination Technologies | GB 2016-03-11; US filed 2017-03-13 | Granted 2019-06-18, active, adjusted expiry 2037-06-07 (Google) | Independent claims 1 and 18 read in full; the third (IC-description medium) not opened | 1 | §Claim charts. Initial light maps and importance sampling from their texels appear absent. A bounce-light patent |
| US10891783B2, US11763520B2 (+ US11302068B2, WO2019183149A1) "Improving area light shadows in computer generated scenes", inventor Panteleev | NVIDIA | 2018-03-19 (provisional 62/644,806; PCT/US2019/023043) | '783 granted 2021-01-12; '520 granted 2023-09-19; active, expiry ≈2039-03-19 (Google) | Every independent claim of '783 and '520 read in full in a browser. '068 not read directly; its summary reading stands | 2 | §Claim charts. Runtime soft shadows from a rectangular light by voxel cone tracing. Real subject overlap: a spherical polygon, a sample grid and weighted average visibility. Every independent claim requires cones. Show counsel if option (a) is chosen |
| US12293451B2 | Tencent | CN 2021-04-02 | Granted 2025-05-06, active, adjusted expiry 2043-01-19 | Claim 1 read in full; claims 9 and 17 summarised as the same five steps | 1, 3 | §Claim charts. The light-change step and the radiance transfer parameter appear absent. Also charted for the GI stack in `context/research/gi-rtx-lite/ip-check.md` |
| US9262860B2 | Solid Angle SL / Sony | 2011-02-17 | Active, expiry ≈2032 (estimate) | **Full claims read** | 2 | Claims 1 and 14 require homogeneous participating media and equi-angular sampling along a ray. Appears absent: the design samples directions to a polygon from a surface texel. Dependent claim 6 (quad lights) hangs from the media claim |
| US7864177B2 | Intel | 2006-07-24 | Expired (fees) | Claims 1/12/17 partly read | 1 | Per-polygon visibility map: appears absent. Claim 12 (incident light × precomputed visibility) is arguable at that level of generality only. No polygon area light, no sampled solid angle |
| US7817823B1 | Adobe | 2007 | Not checked | Summary | 1 | Runtime penumbra-map filtering. Appears absent |
| US7538766B2 | Microsoft | — | Expired 2025 | Summary | 1 | Precomputed shadow fields for dynamic shadows. Appears absent |
| US6226005B1 | Autodesk | — | Expired 2017 | Summary only | 1 | Not charted |
| US9501862B2 "Visibility silhouettes for masked spherical integration" | Not captured | — | Not checked | Snippet | 3 | Not charted |

Dismissed on snippet: US9639976B2 (circular-light shadows), US10740954B2 and US11145110B2 (NVIDIA runtime shadow denoising), US9171396B2, US6288721B1, US10096146, US9754406, US12109936, US6342887 (plain inverse-square attenuation).

## Claim charts

Read against `ip-questions.md` §Design. Elements are quoted from the claim.

**US10325401B2 claim 1** (claim 18, a GPU claim, repeats it with buffers holding the initial light maps):

| Element | Reading |
|---|---|
| "determining one or more initial light maps representing lighting within the scene" | Appears absent: there is no initial light map; the unshadowed term is analytic |
| "identifying one or more positions within the scene based on values of corresponding texels of the one or more initial light maps … using an importance sampling technique" | Appears absent: samples are drawn over the polygon's solid angle, not from a prior lightmap's texels |
| "tracing one or more sampling rays between a position in the scene corresponding to the texel position … and the … identified positions" | Arguable: generic per-texel visibility rays |
| "determining a lighting value for the texel position … using results of the tracing" | Arguable: generic |

**US12293451B2 claim 1:**

| Element | Reading |
|---|---|
| "determining light source change information when a light source changes in a virtual scene" | Appears absent: the bake is offline, with no runtime change step |
| "determining a current light source projection coefficient corresponding to the changed light source" | Arguable only loosely, as SH projection of a light |
| "determining an indirect illumination value … according to a radiance transfer parameter … and the current light source projection coefficient" | Appears absent: no radiance transfer parameter |
| "determining a direct illumination value … under the changed light source", and rendering from both | Arguable: a generic direct and indirect split |

**US10891783B2 claim 1:**

| Element | Reading |
|---|---|
| "generating, based on a rectangular area light, a spherical polygon on a unit sphere around a surface point" | Arguable: the design forms a horizon-clipped spherical polygon, though for any convex polygon, not only rectangles |
| "determining angular sizes of the rectangular area light along a first and a second dimension employing the spherical polygon" | Appears absent |
| "determining a number of cones needed to sample the rectangular area light … employing the angular sizes" | Appears absent |
| "generating a sample pattern for the first and the second dimensions based on the number of cones" | Appears absent |
| "creating a sampling grid within the spherical polygon based on the sample patterns" | Arguable: a sample set over the polygon (Ureña or Arvo) |
| "obtaining samples … by tracing cones, through a voxel representation of the scene, … wherein a total number of cones is a product of the number of cones along each dimension" | Appears absent: no cones and no voxel scene |
| "computing average visibility for the surface point based on the samples and a weight associated with each of the cones as determined by the sample patterns" | Arguable on weighted average visibility; the design's weight is each sample's share of the unshadowed integrand, not a cone-pattern weight |

**US10891783B2 claims 10 and 18.** Claim 10, a GPU system, claims "a memory configured to store a voxel representation of a scene" and "voxel cone tracing". Claim 18, a computer program product, claims "determining a number of cones" and "employing voxel cone tracing". Both cone and voxel elements appear absent; each claim's weighted-average-visibility tail is arguable.

**US11763520B2:**

| Claim | Elements | Reading |
|---|---|---|
| 1 | Generating voxels from geometry and light information; "tracing … one or more cones corresponding to an illuminant … through the one or more voxels, wherein a number of the one or more cones traced is based on sampling a surface of the illuminant"; illumination values from the traced cones | Appears absent |
| 12 (system), 19 (method) | "determining a number of cones for sampling the illuminant"; "obtaining … samples from a light surface of the illuminant by tracing the cones"; average visibility or illumination using "a weight associated with at least one sample … as determined by sample patterns". Claim 19 adds patterns that "dynamically correspond to changes in a size of the illuminant". The voxel wording is dropped | Cones appear absent: the description defines them as voxel-cone-trace cones with a preferred cone angle, though how "cones" is construed is counsel's question. Weighted visibility is arguable |

**Distance from the family.**
- What the design shares: a spherical polygon, a sample set and a weighted visibility average.
- Where it differs:
  - It traces no cones and keeps no voxel scene.
  - Its sample count does not come from the light's angular size.
  - Its weights are each sample's share of the unshadowed integrand (option a), or there are none (option b), never smoothness-driven pattern weights.
  - It bakes offline, per texel.
  - It takes any convex polygon, not only rectangles.
- Option (b) sits furthest from the family.
- The examiner cited Ureña 2013, LTC 2016, Kok 1992, Crassin's voxel cone tracing, Assarsson 2003 and Disney US20130328875A1. The claims are drawn around that art through cones and voxels.

**Choices that would move the design toward these claims, and stay out of it:**
- a voxel scene representation;
- a sample or cone count derived from the light's angular size;
- a two-axis sample grid whose zero-weight samples split as the light grows;
- describing samples as cones.

## Readings by piece

| Piece | Finding | Confidence |
|---|---|---|
| 1. Polygon lightmap bake | No filing found combining a baked per-texel lightmap, a polygon or emissive-face light, an analytic form-factor term and sampled solid-angle visibility. The nearest, Imagination's bounce map, claims initial light maps and importance sampling from their texels, which appear absent | Low: no CPC or assignee sweep. Sony, Valve, AMD, Silicon Studio and NetEase not searched |
| 2a. Ratio estimator | Heitz, Hill & McGuire 2018, full text read from NVIDIA's PDF: numerator and denominator are "computed with the same samples"; any sampling strategy works if both share them. The paper's own section says "The ratio estimator is biased" for fixed N, and consistent. No patent notice in the paper or on the author page. No filing by the authors, NVIDIA or Unity found. Nearest: NVIDIA's '783 family, whose independent claims read so far all require voxel cone tracing | Medium: search snippets for the patent side; paper read in full |
| 2b. Projected-solid-angle / spherical-rectangle / spherical-triangle sampling | No filing found for Peters 2021, Ureña 2013 or Arvo 1995 sampling. No patent notice on Ureña's EGSR slides, arXiv 1805.09048 or Peters' KIT paper. Peters' code licence unconfirmed | Medium |
| 3. Analytic SH for polygons; control-variate probe bake | Not found; shallow search. Wang & Ramamoorthi 2018 and its 2020 follow-up show no notice in snippets. Their order-1 statement and cost remain unverified (`cseweb.ucsd.edu` failed on a certificate error). A third-party MIT implementation exists (`github.com/LuniumLuk/AnalyticSHAreaLight`); its licence says nothing about patents | Low |
| 4. Glow synced to light animation | Not found; one query only. Quake lightstyles (1996) are prior art for per-light curves scaling baked light | Low |
| 5. Reach window | No Epic filing found. Karis 2013 publishes the formula (<https://blog.selfshadow.com/publications/s2013-shading-course/karis/s2013_pbs_epic_notes_v2.pdf>), and it is widely documented since | Medium |

## Prior art

`research.md` §5 holds the game-tool record. This pass confirmed or corrected it:
- **id q3map 1999, exact polygon form factor: confirmed in id's released source.** `q3map/light.c` sets `exactPointToPolygon = qtrue`. For `emit_area` lights it calls `PointToPolygonFormFactor(pushedOrigin, normal, light->w)`, with the "black edges" receiver push and the "super hot spots" distance clamp beside it (<https://github.com/id-Software/Quake-III-Arena/blob/master/q3map/light.c>, read 2026-11-15). The q3map2 compile manual describes its surface lights as subdivided points plus backsplash: a later tool's default path, not id's 1999 exact path.
- **Arvo.** Three distinct papers:
  - "The Irradiance Jacobian for Partially Occluded Polyhedral Sources", SIGGRAPH 94, pp. 343–350. This gives the Lambert edge-sum polygon irradiance.
  - "Applications of Irradiance Tensors to the Simulation of Non-Lambertian Phenomena", SIGGRAPH 95, pp. 335–342.
  - "Stratified Sampling of Spherical Triangles", SIGGRAPH 95, pp. 437–438.
  
  Neither pass read their PDFs. The dates come from the SIGGRAPH history records.
- **Howell B-3** covers a differential element to a parallel rectangle, with its normal through a corner. That is the corner form factor the oracle uses, not a general-polygon result.
- **ericw-tools.** The earliest surface-light changelog entry is 2015-04-27. Its surface lights are subdivided point lights.
- **Source 2 `light_rect`.** A CS2 point entity, rectangle or disc, static-only, baked by Hammer's GPU ray tracer. Whether the bake is analytic or sampled is not stated, and the ~2023 date is unverified (Valve's wiki returned 403).
- **Lightmass.** UDK docs confirm area lights and emissive mesh area lights, but the page is undated, so 2009 is unverified.
- **Strata Source.** The wiki is behind bot protection; no area-light documentation was found, and absence is not confirmed.

## Prior-art basis by element

The design practises published methods. Its analytic × sampled-visibility pairing is Heitz, Hill & McGuire's own idea, so the design does not rest on a novel combination. This is not a clearance.

| Element | Published basis |
|---|---|
| Analytic polygon irradiance, horizon clip, irradiance vector | Lambert 1760; Arvo 1994 |
| Polygon sampling | Ureña et al. 2013; Arvo 1995; Peters 2021 (code licence unconfirmed) |
| Analytic term × weighted sampled visibility | Heitz, Hill & McGuire 2018 (full text read; no patent notice) |
| Probe SH from polygon lights | Wang & Ramamoorthi 2018 (order-1 statement unverified); control variates are generic |
| Reach window | Karis 2013 |
| Glow synced to a baked animated light | Quake lightstyles 1996 (one search run) |
| Polygon area lights in a level compiler | id q3map 1999 (exact form factor, confirmed in source); Source 2 `light_rect` (method and date unknown) |

## Follow-ups before the build gate lifts

By hand in Google Patents or ppubs, where automated queries fail:
- A CPC sweep over G06T 15/50, 15/506, 15/60 and 15/80, with "area light", "lightmap", "form factor" and "visibility".
- Assignee queries for Sony, Valve, AMD, Silicon Studio, NetEase, UC Regents, Unity, Activision, Ubisoft and Geomerics/ARM.
- The unread claims: US11302068B2 in full; US10325401B2's IC-description claim; US12293451B2 claims 9 and 17.
- A full read of the 159-hit "area light / polygonal light + lightmap / texel" result set; only its first ten hits were seen.
- Wang & Ramamoorthi 2018 from a mirror, for the order-1 statement and cost.
- Source 2 `light_rect`: bake method and date.

Counsel reads the result; the brief's build gate lifts on the owner's call.
