# Area Lights — IP and Prior-Art Check

**Date investigated:** 2026-11-15. This is a first pass from a parallel run in the owner's local session, folded in here, answering `ip-questions.md`.
**Status:** Engineering triage for counsel, not legal advice. "Appears present", "appears absent" and "arguable" are engineering readings of claim text. They are not opinions on infringement, validity or freedom to operate. Every negative result means "not found", never "clear".

> **Read this when:** deciding whether the area-light brief is clear to build, choosing its visibility-weighting option, or charting a new filing against it.
> **Sources:** Google Patents and ppubs search interfaces could not be queried, so there was no CPC- or assignee-filtered sweep. Individual patent pages loaded through a summarising fetch. Only US9262860B2 had its full claims read. Every other reading is from summarised or partial claim text, and its *Source* column says which.
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
| US20170263043A1 (+ US20210279940) "bounce light map" | Imagination Technologies | 2016-03-11 | Google shows active, adjusted expiry 2037 (Google's estimate) | Independent claims 1/19/20 summarised, opening words verbatim; last ~80k characters of description unread | 1 | Lightmap-bake preamble arguable; initial lighting indications → rays to scene positions → texel value. No analytic polygon term, no analytic × weighted-visibility product. Nearest by subject; weak element overlap |
| US10891783B2, US11302068B2, US11763520B2, WO2019183149A1 "Area light shadows in computer generated scenes" | NVIDIA | 2018-03-19 | Google shows active, expiry ≈2039 (estimate) | '520 and '068 claims relayed in quoted segments; '783 claims not read | 2 | Independent claims require voxels and cone tracing: appear absent. '520 cl. 12 / 19 and '068 cl. 11 ("average visibility … based on the samples and a weight associated with at least one sample", weights from cone-count sample patterns) are arguable on weighted-average-visibility wording only. The weights are not each sample's share of the unshadowed integrand. Show counsel if option (a) is chosen |
| US12293451B2 | Tencent | 2021-04-02 | Granted 2025-05-06, active | Independent claims 1/9/17 from a partial page | 1, 3 | Dynamic-light relighting with radiance transfer: light-change step plus transfer parameter. Appears absent; arguable only on a generic direct + SH split. Already charted for the GI stack in `context/research/gi-rtx-lite/ip-check.md` |
| US9262860B2 | Solid Angle SL / Sony | 2011-02-17 | Active, expiry ≈2032 (estimate) | **Full claims read** | 2 | Claims 1 and 14 require homogeneous participating media and equi-angular sampling along a ray. Appears absent: the design samples directions to a polygon from a surface texel. Dependent claim 6 (quad lights) hangs from the media claim |
| US7864177B2 | Intel | 2006-07-24 | Expired (fees) | Claims 1/12/17 partly read | 1 | Per-polygon visibility map: appears absent. Claim 12 (incident light × precomputed visibility) is arguable at that level of generality only. No polygon area light, no sampled solid angle |
| US7817823B1 | Adobe | 2007 | Not checked | Summary | 1 | Runtime penumbra-map filtering. Appears absent |
| US7538766B2 | Microsoft | — | Expired 2025 | Summary | 1 | Precomputed shadow fields for dynamic shadows. Appears absent |
| US6226005B1 | Autodesk | — | Expired 2017 | Summary only | 1 | Not charted |
| US9501862B2 "Visibility silhouettes for masked spherical integration" | Not captured | — | Not checked | Snippet | 3 | Not charted |

Dismissed on snippet: US9639976B2 (circular-light shadows), US10740954B2 and US11145110B2 (NVIDIA runtime shadow denoising), US9171396B2, US6288721B1, US10096146, US9754406, US12109936, US6342887 (plain inverse-square attenuation).

## Readings by piece

| Piece | Finding | Confidence |
|---|---|---|
| 1. Polygon lightmap bake | No filing found combining a baked per-texel lightmap, a polygon or emissive-face light, an analytic form-factor term and sampled solid-angle visibility. Nearest is Imagination's bounce map, with weak overlap | Low: no CPC or assignee sweep. Sony, Valve, AMD, Silicon Studio and NetEase not searched |
| 2a. Ratio estimator | Heitz, Hill & McGuire 2018, full text read from NVIDIA's PDF: numerator and denominator are "computed with the same samples"; any sampling strategy works if both share them. The paper's own section says "The ratio estimator is biased" for fixed N, and consistent. No patent notice in the paper or on the author page. No filing by the authors, NVIDIA or Unity found. Nearest wording: NVIDIA's area-light-shadow family above | Medium: search snippets for the patent side; paper read in full |
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

## Follow-ups before the build gate lifts

By hand in Google Patents or ppubs, where automated queries fail:
- A CPC sweep over G06T 15/50, 15/506, 15/60 and 15/80, with "area light", "lightmap", "form factor" and "visibility".
- Assignee queries for Sony, Valve, AMD, Silicon Studio, NetEase, UC Regents, Unity, Activision, Ubisoft and Geomerics/ARM.
- Full claim PDFs for US20170263043A1, US10891783B2 and US12293451B2.
- Wang & Ramamoorthi 2018 from a mirror, for the order-1 statement and cost.
- Source 2 `light_rect`: bake method and date.

Counsel reads the result; the brief's build gate lifts on the owner's call.
