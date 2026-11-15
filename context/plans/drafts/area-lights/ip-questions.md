# Area Lights — IP Questions

**Status:** The request behind the patent search the brief's one build-blocking question names. The first pass is in `ip-check.md`; its follow-up list is what remains. Engineering triage for counsel; not legal advice. Findings are in the sibling `ip-check.md`.

> **Read this when:** running or reviewing the area-light patent search.
> **Related:** `index.md` (Open questions) · `research.md` §5 (prior art held) · `context/research/gi-rtx-lite/ip-check.md` (format)

---

## Format and certainty

- For each filing record:
  - Document number, assignee, priority date, status, and expiry if found.
  - Source quality: full claims read, abstract, or snippet.
  - Each independent claim's elements, quoted, read against the design below as *appears present*, *appears absent* or *arguable*.
- *Appears absent* needs full claim text. Snippets are triage and are labelled as such.
- No conclusion on infringement, validity or freedom to operate. That reading belongs to the owner and counsel.
- Sources: Google Patents and USPTO ppubs. Google Patents rate-limits (HTTP 503). Patentscope, Justia and Espacenet have refused automated queries.

## Design, in claim-like terms

Bake runs offline in the level compiler. Runtime is the forward renderer.

- A brush entity's faces carrying an emissive map become one light made of planar convex polygons.
- Unshadowed per-texel irradiance is analytic: the Lambert/Arvo polygon form factor, clipped to the receiver's horizon. The stored direction is the clipped polygon's irradiance vector.
- Shadowing is a per-texel visibility, weighted by each sample's share of the unshadowed integrand. One of two methods:
  - (a) The Heitz–Hill–McGuire 2018 ratio estimator over solid-angle samples: Ureña 2013 for rectangles, an Arvo 1995 triangle fan otherwise.
  - (b) Projected-solid-angle sampling (Peters 2021) with plain counting.
- A lightmap texel is the analytic term × the weighted visibility.
- Probe direct light (L2 SH) uses analytic unshadowed coefficients, plus a control-variate shadow correction:
  - L0 from solid angle.
  - L1 from the irradiance vector.
  - L2 by Wang & Ramamoorthi 2018 or by quadrature.
- Reach uses the window max(0, 1 − (d/R)⁴)² (Karis 2013), with d measured from the polygon.
- At runtime, an emitting face's glow is emissive texel × authored glow scalar × the light's live brightness and colour.
  - It reads the same per-light descriptor that scales the baked animated lightmap, on the same frame.
  - Animated baked lights are per-light weight maps × runtime curves (Quake lightstyle prior art).

## Questions

| # | Question | Search leads | Feeds | Certainty |
|---|---|---|---|---|
| 1 | Do patents claim offline lightmaps from area or polygon lights with an analytic unoccluded term combined with sampled visibility? | Terms: "area light", "lightmap", "form factor", "visibility", bake / precompute, "analytic", "unshadowed". Assignees: NVIDIA, Unity, Epic, Microsoft, Sony, Tencent, NetEase, Imagination, AMD, Intel, Geomerics/ARM, Silicon Studio, Valve | The brief's build-blocking question | Claim level |
| 2 | Is the I3D 2018 ratio estimator patented? Confirm from the paper's full text that numerator and denominator share samples, and record its stated bias | Inventors Heitz, Hill and McGuire, with "stochastic shadows", "ratio", "analytic direct illumination" | Visibility option (a) vs (b) | Claim level if found |
| 3 | Is projected-solid-angle polygon sampling (Peters 2021) or spherical-rectangle sampling (Ureña et al. 2013) patented? | KIT; Solid Angle, Autodesk, Sony Imageworks | Visibility option (a) vs (b) | Claim level if found |
| 4 | Are analytic SH coefficients for polygonal lights (Wang & Ramamoorthi 2018), or analytic-plus-control-variate probe baking, patented? | UCSD | Probe projection method | Triage |
| 5 | Is an emissive surface's glow driven by the same runtime animation that scales its baked light patented? | "emissive", "light animation", "synchroniz*", "lightmap", "runtime intensity" | Glow-table design | Triage |
| 6 | Is a windowed inverse-square falloff patented? | Epic, Karis 2013 | Reach window | Triage |

## Sources this session could not read

| Source | Wanted |
|---|---|
| `research.nvidia.com/publication/2018-05_Combining-Analytic-Direct`, `eheitzresearch.wordpress.com/705-2/` | Heitz 2018 full text: the estimator, shared samples, bias, any patent notice |
| `cseweb.ucsd.edu/~ravir/ash.pdf` | Wang & Ramamoorthi 2018: the order-1 statement and cost |
| `w1.graphics.cornell.edu/pubs/1995/Arv95a.pdf`, `graphics.cornell.edu/pubs/1995/Arv95c.pdf` | Arvo 1995: the polygon irradiance formula and spherical-triangle sampling, for prior-art dates |
| `thermalradiation.net/sectionb/B-3.html` | Howell B-3 confirmation (already verified numerically) |
| `developer.valvesoftware.com/wiki/Light_rect`, `/wiki/VRAD3` | Whether Source 2 `light_rect` bakes analytically or by sampling, and its date |
| `wiki.stratasource.org` | Any area-light documentation |
| `realtimerendering.com/erich/ShaftCulling.pdf` | Haines–Wallace shaft planes (engineering, low priority) |

## Prior art already held

`research.md` §5 records each with source and date:
- id q3map 1999: exact polygon form factor.
- q3map2.
- qrad3 1997 and hlrad 1998: per-patch points.
- Source vrad: `FormFactorPolyToDiff`.
- ericw-tools 2015.
- Lightmass 2009.
- Source 2 `light_rect`, about 2023.
- Godot 4.7 `AreaLight3D`, 2026, MIT.
- Papers: Lambert 1760; Baum, Rushmeier & Winget 1989; Arvo 1995; LTC 2016; Karis 2013.
