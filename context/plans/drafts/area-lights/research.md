# Polygon Area Lights — Research

**Date:** 2026-11-15. Repo read at `17c2f1e`. No lighting, compiler, renderer or render-data source changed since the cloud session's `eb44499`. The `context/lib` changes since then touch only networking, scripting, UI and player options.
**Status:** Pre-brief research for a draft session. Planning only.
- Every row is an engineering reading.
- Prior-art rows record dates and mechanisms for counsel. They reach no legal conclusion.
- Web agents could not fetch most primary PDFs, because those hosts are outside this session's network allowlist. Each row's confidence says whether it rests on full text, search excerpts, or a derivation checked numerically here.

> **Read this when:** drafting the area-light brief, or changing the bake's emitter shape, its unshadowed light term, or a light-reach cull.
> **Key finding:** the separable design (analytic unshadowed term × stored visibility) carries a polygon emitter with no change to the lightmap partition format. One condition: the stored visibility must be weighted by the unshadowed integrand (Heitz–Hill–McGuire ratio estimator). Three things do not carry over: the direct-SH single-direction projection, every sphere-shaped emitter lattice, and the two flush-emitter rules.
> **Related:** `context/lib/build_pipeline.md` §Compiler pipeline step 12, §Build Cache · `context/lib/rendering_pipeline.md` §4 · `context/research/gi-rtx-lite/index.md`, `ip-check.md` · `context/plans/roadmap.md` "surface lights"

---

## Grounding recheck

The cloud session's facts hold at `17c2f1e`. This session found five more:

- **Direct SH projects each light along one direction.** `sh_bake/probe.rs::bake_probe_direct_rgb_posed` calls `accumulate_sh_rgb` once per light, along `incident_radiance_at_point`'s point→light direction, then calls `apply_cosine_lobe_rgb`. The probe atlas works on L2 (`sh_math::sh_basis_l2`, 9 coefficients × RGB) and packs into octahedral irradiance tiles. A polygon that subtends a large solid angle cannot ride that path (§3).
- **More consumers of the unshadowed term and emitter shape than the handoff listed:**
  - `mover_probe_channel/{direct,visibility,posed_scene}.rs` read `incident_radiance_at_point`, `emitter_radius` and `area_sample_target`, and `PosedOccluder::may_meet` is a sphere test.
  - `billboard_direct_scatter_bake.rs` reads `incident_radiance_at_point`.
  - `surface_radiance/layer.rs` reads `bounce_contribution`.
  - About 25 compiler files read `falloff_range` as an origin-centred reach radius, among them `affinity_grid`, `chunk_light_list_bake`, `delta_drop_policy`, `entity_shadow_select`, `mover_light_gate/*`, `reflection_probes/sdf_lights` and `surface_radiance/reach`.
- **The lightmap partition record already fits the ratio estimator.**
  - A `lightmap_layer` partition stores only a reached texel index and raw soft visibility (`build_pipeline.md` §Build Cache).
  - The fold rebuilds irradiance and weighted direction from the unshadowed Lambert term at the texel centre.
  - So a polygon light changes the term and what "visibility" means. It does not change the record.
- **Promotion filters.** `entity_shadow_select::is_promotable_base_light` already refuses any type except Point and Spot. The animated path (`animated_baked_shadow_candidates_with_direct_delta`) has no type filter, so it needs an explicit refusal.
- **A closer authoring precedent than `func_group`.** `switch` is a brush entity that compiles as static world geometry and also carries a `name` for scripts (`build_pipeline.md` §Custom FGD). An area-light entity has the same shape: world brushwork plus a named light.

## 1. Unoccluded polygon term

| Claim | Source | Confidence | Symbol it changes |
|---|---|---|---|
| Lambert: E(p, n) = (L/2) Σᵢ Θᵢ (Γᵢ·n) = πL·F, where Θᵢ is edge i's subtended angle and Γᵢ is the unit normal of the plane through p and the edge. Winding sets the sign. Compute S = Σ Θᵢ Γᵢ, then flip its sign so S points toward a front-facing emitter (S·n_e ≤ 0) | Arvo 1995 <https://en-cg-web.coecis.cornell.edu/pubs/1995/Arv95a.pdf>; Arvo 1994 <https://www.graphics.cornell.edu/pubs/1994/Arv94.pdf>; Baum, Rushmeier & Winget 1989, DOI 10.1145/74334.74367; sign convention <https://users.cs.utah.edu/docs/techreports/2000/pdf/UUCS-00-012.pdf> | High. Excerpts plus a numeric check here | New polygon arm in `lightmap_bake::light_contribution_and_direction` and `sh_bake::light_eval` |
| Irradiance vector Φ = (L/2) Σ Θᵢ Γᵢ = L ∫ ω dω. E(n) = Φ·n holds only when the whole polygon is above the receiver's horizon. For a straddling polygon, Φ·n subtracts the part below the horizon | Arvo 1994; UCSD CSE168 notes <https://visualcomp.ucsd.edu/classes/cse168/sp21/lectures/168-lecture6.pdf> | High | Weighted direction in the fold: use normalize(Φ of the clipped polygon), weighted by E |
| Clip against the receiver plane n·(x − p) ≥ 0 (Sutherland–Hodgman) before applying Lambert. A quad can leave up to 5 corners. LTC's sphere approximation avoids clipping but is approximate. It is a GPU saving, not needed offline | LTC paper <https://history.siggraph.org/?p=102764>; Hill & Heitz 2016 <https://www.advances.realtimerendering.com/s2016/s2016_ltc_rnd.pdf> | High on the need; medium on sphere details | Polygon term: clip exactly, in f64 |
| Behind a one-sided emitter, E = 0. A coplanar receiver outside the polygon gets E = 0 (winding number 0). Approaching the interior from the front, E → πL·(n·−n_e), finite, and it drops to 0 across the plane | Derivation; checked numerically below | High | Early-out on signed plane distance d ≤ ε, so texels on the emitter's own face go dark |
| Numerics: acos(1 − δ) ≈ √(2δ) loses precision. Use Θ = atan2(\|a×b\|, a·b) on unnormalised f64 vectors, and form Θ·Γ as c·(Θ/\|c\|). When \|c\| ≈ 0 with a·b > 0 the receiver lies on the edge's line outside the segment, and the term is 0 | Snyder MSR-TR-96-11 <https://www.microsoft.com/en-us/research/wp-content/uploads/1996/03/arealights.pdf>; LTC code pattern | High on the maths | Polygon term implementation; no fitted approximation needed |
| Small-emitter limit: E → L·A·cosθ_e·cosθ_r/d², a point light of intensity I = L·A·cosθ_e. Relative error is O((size/d)²). Total flux is πLA | Derivation; numeric check below | High | Proof oracle; brightness convention (§Decisions) |
| Exact references: a centred parallel square of side s at height h, with X = s/(2h), gives E = 4L·X/√(1+X²)·atan(X/√(1+X²)). Off-axis parallel rectangles use the signed corner form factor (Howell B-3) by inclusion–exclusion | Howell catalog <https://thermalradiation.net/sectionb/B-3.html>; <https://fchartsoftware.com/ees/heat_transfer_library/shape_factors/hs301.htm> | High. Re-derived, and matches the polygon code to 6 digits | Proof oracle |

**Checked here.** Reference values from a scratch implementation (Lambert with f64 atan2 and horizon clip), cross-checked against midpoint integration and the closed form. Unit square, L = 1, emitter facing down:

| Receiver | E | Check |
|---|---|---|
| On-axis, facing up, h = 0.1 / 0.5 / 1 / 2 / 5 / 10 | 3.042128 / 1.740840 / 0.752275 / 0.230837 / 0.039474 / 0.009967 | Closed form equal to 6 digits |
| h = 1, normal tilted 0° / 30° / 60° / 85° / 90° / 120° | 0.752275 / 0.651489 / 0.376137 / 0.123245 / 0.087510 / 0 | Midpoint integration within 1e-6 |
| Offset x = 2, h = 1, facing up | 0.0437426 | Midpoint within 1e-7 |
| On an edge's line extension (x = −0.5, y = −2) | 0.0396548 | Midpoint within 1e-7 |
| Coplanar (z = 1, x = 2), any normal | 0 | — |
| Grazing: 0.1 mm below the plane, facing the emitter | 1.3554e-5 | Midpoint within 1e-9 |
| Behind the emitter | 0 | — |
| h = 1 mm, centred | 3.14158 (→ π) | — |
| Small limit: s = 0.1 / 0.01 / 0.001 at a skewed pose, ratio to L·A·cosθ_e·cosθ_r/d² | 0.99992 / 0.9999992 / 0.99999999 | O((s/d)²) |
| f32 corners near (1000, 2000, 12) m vs f64 corners | Relative difference 0 at 1e-16 | Subtraction in f64 before the cross product |

**Near-field size of the point-light error.** For a 4 × 4 m skylight, compare a single point light at its centroid with the same on-axis flux (I = L·A) against the true term, at the floor directly below. At 1 m the point term reads 6.1× too bright, at 2 m 2.3×, at 3 m 1.6×. For a unit square at 0.1 m it reads 33× too bright. A template grid of points shrinks the factor but keeps a ripple at each copy's foot. This is the measured form of the owner's hot-spot reason.

## 2. Analytic term with stochastic shadows

| Claim | Source | Confidence | Symbol it changes |
|---|---|---|---|
| Ratio estimator: E = E_analytic × [Σ f(xᵢ)V(xᵢ)/p(xᵢ)] / [Σ f(xᵢ)/p(xᵢ)], with the same samples in both sums. It is exact with zero variance when V is all 1 or all 0. It is bounded to [0, E_analytic] and consistent, with O(1/N) bias | Heitz, Hill & McGuire, I3D 2018 <https://research.nvidia.com/publication/2018-05_Combining-Analytic-Direct>, DOI 10.1145/3190834.3190852 | High on the form; medium-high on the bias wording (abstract and summaries; full text not fetched) | Defines what a polygon light's stored "visibility" means in the `lightmap_layer` record |
| Today's "term × visibility fraction" already is this estimator, provided samples are drawn with p ∝ f. A sphere light seen from afar nearly satisfies that. A large polygon close by does not: under a 4 × 4 m light at 1 m, cosθ_e·cosθ_r/d² varies 81× from centre to corner (3.6× at 3 m). An unweighted area fraction over-counts blockers near the far edges | Derivation from the estimator; figures by the agent | High | `area_sample_target`, `SoftProbes::new`, `texel_footprint` for the polygon arm |
| Two equal ways out: (a) sample by projected solid angle (Peters 2021), so plain counting is exact; (b) sample by area or solid angle and store Σ wV / Σ w, with w = f/p. Option (b) is one multiply per ray | Peters 2021 <https://cg.ivd.kit.edu/polygonal_lights.php>, DOI 10.1145/3450626.3459672 | High | Same |
| Solid-angle sampling maps stratified 2D points to a light. Ureña et al. 2013 is exact for rectangles. Arvo 1995 handles spherical triangles (fan a convex polygon and pick triangles by solid angle). Both keep existing stratification and seeds | Ureña, Fajardo & King, EGSR 2013 <https://www.ugr.es/~curena/publ/2013-egsr/>; Arvo 1995 <https://www.graphics.cornell.edu/pubs/1995/Arv95c.pdf> | High | Polygon emitter lattice |
| Jittered stratified penumbra error falls about N^-0.75, against N^-0.5 for random sampling. Going from 4 to 32 rays cuts penumbra RMS noise about 4.8× | Mitchell 1996 <https://people.csail.mit.edu/ericchan/bib/pdf/p277-mitchell.pdf>; Subr & Kautz 2014 <https://jankautz.com/publications/ErrorAnalysis_EGSR2014.pdf>; Ramamoorthi et al. 2012 <https://cseweb.ucsd.edu/~ravir/a121-ramamoorthi.pdf> | High | Sample-count proof row |
| Lightmass uses the same two tiers as this bake: NumShadowRays per texel, then NumPenumbraShadowRays in penumbra. A community source cites about 16 at Production quality, raised for big lights. No published per-texel count exists for large area lights | Community wiki and forum only (not Epic docs) | Medium | None; counts come from measurement |
| No evidence of a ratio estimator in Lightmass, Unity PLM or Bakery | Absence of evidence | Medium | Choosing it here is deliberate, not borrowed |

**Reading for this repo.**
- Keep the separable fold. A polygon light stores the weighted visibility Σ wᵢVᵢ / Σ wᵢ.
- Weights are computed at the texel centre (point and normal), as N·L and falloff are today. The footprint points supply the Vᵢ.
- Fully lit and fully shadowed texels stay exact 1 and 0, so the nine-probe early-out keeps its meaning.
- Lattice choice: Ureña sampling for rectangles, which the skylight and light-table cases are, with an Arvo fan for other convex polygons. Store w = cosθ_r per sample, since solid-angle sampling already accounts for distance and the emitter cosine.
- **Lattice dependence.** The polygon lattice depends on the light and on the receiver (solid angle is per point). So the per-thread point/spot snap in `SoftProbes::new`, keyed by sample count alone, must not serve polygon lights. The receiver-floor sector pairing assumes a sphere offset projected onto the receiver; for a polygon, pair by the sample's projected offset in the same way.
- Whether 32 escalated rays are enough for a 4 m light at 1–3 m is a measurement, not a literature answer. It goes in a Proof row.

## 3. SH projection for probe bakes

**Repo encoding.** Probes project into real L2 SH (9 × RGB, Ramamoorthi–Hanrahan basis without Condon–Shortley phase), convolve with the cosine lobe, and pack octahedral irradiance tiles. Direct SH (id 35), animated direct (45), entity-shadow deltas (41) and billboard scatter (47/48) all use the single-direction projection.

| Claim | Source | Confidence | Symbol it changes |
|---|---|---|---|
| Wang & Ramamoorthi 2018 gives exact SH coefficients of a uniform polygon at any order. It uses a zonal-harmonic recurrence over edges plus a sparse rotation. Cost is trivial at N ≤ 2 | <https://cseweb.ucsd.edu/~viscomp/projects/ash/>; <https://cseweb.ucsd.edu/~ravir/ash.pdf> | High on scope; low on exact cost | Unshadowed oracle for L2 |
| Closed forms for orders 0 and 1: c₀₀ = L·Ω·Y₀₀ with Y₀₀ = 1/(2√π). (c₁,₋₁, c₁,₀, c₁,₁) = L·√(3/4π)·(Φ_y, Φ_z, Φ_x), with Φ the unscaled vector ½ΣΘΓ. Signs follow the repo basis (`sh_basis_l2` negates y and x). L2 needs Arvo's second-moment tensors or the recurrence | Arvo 1995; derivation | High on the maths | Test oracle for the probe projection |
| Solid angle of a triangle (Van Oosterom & Strackee): tan(Ω/2) = \|a·(b×c)\| / (abc + (a·b)c + (a·c)b + (b·c)a). Evaluate with atan2, and fan-sum for convex polygons | IEEE TBME 30(2), 1983, DOI 10.1109/TBME.1983.325207 | High | Solid-angle lattice weights; Ω for L0 |
| A per-coefficient ratio estimator is unstable for SH, because a signed coefficient's denominator can be near zero. Stable options are a control variate, c = c_analytic + (Ω/N)·Σ(Vₖ − 1)·L·Y(ωₖ), or plain stratified sampled projection | Agent analysis on Heitz 2018 | Medium-high | Direct SH polygon arm |

**Checked here.** A 4 × 4 m light, probe below its centre. Each row compares irradiance reconstructed from L2 SH with the exact Lambert value, for four receiver normals:

| h | Normal | Exact | L2, sampled projection | L2, single direction (today's path, matched facing up) |
|---|---|---|---|---|
| 1 m | up / 45° / side / down | 2.611 / 1.914 / 0.781 / 0 | 2.566 / 1.934 / 0.761 / −0.045 | 2.611 / 1.675 / 0.230 / 0.154 |
| 2 m | up / 45° / side / down | 1.741 / 1.231 / 0.350 / 0 | 1.755 / 1.229 / 0.343 / 0.014 | 1.741 / 1.117 / 0.154 / 0.102 |
| 3 m | up / 45° / side / down | 1.124 / 0.795 / 0.167 / 0 | 1.155 / 0.780 / 0.173 / 0.031 | 1.124 / 0.721 / 0.099 / 0.066 |

At 1 m the single-direction path under-lights a side-facing entity 3.4× and leaks light onto down-facing normals. Projecting over the polygon's solid angle lands within L2's own ringing, a few percent.

**Reading for this repo.**
- The probe arm projects over the polygon. It does not use one direction.
- With 1 m probe spacing, a probe can sit inside a skylight's near field.
- Recommended form: the control variate on the probe's 4-ray set (escalated in penumbra).
  - Unshadowed coefficients come analytically: L0 from Ω, L1 from Φ, and L2 from Wang–Ramamoorthi or a fixed dense quadrature that traces no rays.
  - Fast paths: every sample visible writes the analytic value; none visible writes zero.
- Sampled projection alone also works, but a 4-ray direction estimate of a large light is noisy in L1/L2.
- The scalar-ratio shortcut (L0 visibility ratio applied to every coefficient) matches how today's sphere lights treat partial occlusion. It is the cheapest fallback.

## 4. Conservative bounds for culls

| Claim | Source | Confidence | Symbol it changes |
|---|---|---|---|
| Haines & Wallace 1991 shaft: the AABB of both boxes plus connecting planes through edge pairs. Boxes are tested by their nearest corner | DOI 10.1007/978-3-642-57963-9_13; Haines, JGT 2000 <https://www.realtimerendering.com/erich/ShaftCulling.pdf> | High on the citation; medium on the plane rule | `lightmap_layer/shaft_cull.rs` polygon variant |
| The convex hull of S ∪ T, for convex S and T, is exactly the union of segments from S to T. So hull(polygon ∪ receiver box) is the tightest conservative shaft. An axis-aligned ceiling rectangle is its own AABB, so the box-to-box shaft is exact for it | Convex-join result (Mathlib `convexHull_union`) | High | Same; `shaft_cull/margin.rs` proofs need a polygon form |
| Emitter half-space cut: a receiver behind a one-sided emitter takes nothing (Tampieri's `CanCull`). Clipping both sets to half-spaces keeps them convex, so the hull argument still holds | Tampieri, Graphics Gems III <https://www.cs.uaf.edu/2007/spring/cs481/lecture/04_10_radiosity/formfactor_gem.pdf> | High | Every cull: front half-space test |
| A bounding sphere around a flat 4 × 4 m light has radius 2.83 m. About half of it lies behind the emitter, in the slab and the room above, and it pulls ceiling brushes into every shaft. A 4 × 0.25 m strip is about 16× too wide across. At 10 m reach, a centroid ball holds about 2.5× the true front "rounded slab" volume (1.26× with only the front half) | Derivation (Steiner formula) | High | Cost only; never correctness |
| Bound: E ≤ L·min(π, A/d_min²), where d_min is the distance to the nearest point of the polygon. The reach for cutoff ε is d_reach = √(L·A/ε), measured from the polygon. For a Lambertian emitter of flux Φ, L·A = Φ/π, so the reach depends on flux, not area. A centroid ball is conservative only with radius d_reach + circumradius | PBRT 4e §4.2.3 <https://pbr-book.org/4ed/Radiometry,_Spectra,_and_Color/Working_with_Radiometric_Integrals>; derivation | High | Each `falloff_range` cull reader (§Grounding) |

**Reading for this repo.**
- v1 can feed every sphere cull a conservative sphere: centroid plus circumradius plus reach. Every exact-skip proof stays valid, because those proofs show zero contribution outside a volume, and a containing sphere still proves it.
- The cost lands on ceiling skylights, whose sphere pulls in the slab. Measure that on the hallway map before building a tight polygon hull.
- Two cheap tightenings, if the measurement asks for them:
  - the front half-space test in each cull;
  - the box-to-box shaft with the polygon's AABB.
- Tightening is a follow-on, not v1 scope, unless the bake-time row fails.
- **Self-occlusion.** Shadow rays end on the emitter's own brush face. Lift samples off the plane along n_e by a small offset above the tracer tolerance τ (0.5 mm), in the spirit of ericw-tools' `_surface_offset` (§5). Alternatively, exclude the emitter's own faces from the hit test. How `segment_clear` treats an endpoint lying on a triangle is not verified.

## 5. Prior art in game tools

Sources: rows 1–4 come from primary source code on GitHub. Rows 5–6 come from search extracts of official docs. Quake map units are converted at 1 unit = 0.0254 m, the engine's adapter scale.

| Year | Tool | Emitter model | Flush emitter | Near field | Source | Confidence |
|---|---|---|---|---|---|---|
| 1997 (GPL 2001) | Quake 2 `qrad3` | Faces flagged `SURF_LIGHT` are chopped into radiosity patches (`-chop`, default 64 units). `CreateDirectLights` turns each bright patch into one `emit_surface` point light of intensity ∝ patch area, then removes that energy from the patch so it is not bounced twice | Patch origin +1 unit (2.5 cm) along normal | None: 1/d² × both cosines | <https://github.com/id-Software/Quake-2-Tools/blob/master/bsp/qrad3/lightmap.c>, `patches.c` | High |
| 1998 | Half-Life `hlrad`, `lights.rad` | Texture → RGB table (`texlight`); same `emit_surface` points as qrad3 | +1 unit | Clamps dist ≥ 1 unit | <https://github.com/ValveSoftware/halflife/blob/master/utils/qrad/lightmap.c> | High |
| 1999 (GPL 2005) | id `q3map` | **Polygon area light.** Each emitting surface (optionally subdivided by `q3map_lightSubdivide`, default 999) becomes an `EMIT_AREA` light that keeps its winding. `LightContributionToSample` uses the exact `PointToPolygonFormFactor` (edge angle × facing over 2π). One shadow ray to the winding centre +1 unit | Receivers within ±8 units (0.2 m) of the plane are pushed 8 units in front, "so that surfaces meeting a light emiter don't get black edges". Receivers behind are skipped unless two-sided. `q3map_backsplash` adds a point light 23 units out carrying 5% of the output, against "weird pools of light near edges" | Exact form factor. The approximate path clamps dist ≥ 16 units "to prevent super hot spots" | <https://github.com/id-Software/Quake-III-Arena/blob/master/q3map/light.c> | High |
| ~2002 | `q3map2` | Same polygon model, plus a `-faster` point approximation (code comment dated 2002-09-30) | Same, plus rejection of coplanar receivers within 3 units | Same | <https://github.com/TTimo/GtkRadiant/blob/master/tools/quake3/q3map2/light.c> | High on code; medium on first-release year |
| 2004 (GitHub 2013) | Source `vrad` | `emit_surface` points per patch. Radiosity transfers switch to an analytic polygon-to-differential form factor (`FormFactorPolyToDiff`) when a patch is near relative to its area (the "5 times rule") | Trace start pushed `DIST_EPSILON` | No clamp on `emit_surface` | <https://github.com/ValveSoftware/source-sdk-2013/blob/master/src/utils/vrad/lightmap.cpp> | High on code; 2004 date not verified |
| 2009 | UE3 Lightmass | `bUseEmissiveForStaticLighting`: each connected group of emissive texels becomes a "mesh area light". Influence radius is derived from brightness and area (`EmissiveBoost`), with a falloff exponent and an explicit radius override | Not found | Not found | <https://docs.unrealengine.com/udk/Three/Lightmass.html> | Medium |
| ~2010 | Enlighten | Emissive is an input to the radiosity solve, not a direct light, and can be updated at runtime | n/a | Limited by solve resolution | Martin & Einarsson, SIGGRAPH 2010 Advances; Enlighten SDK docs | Medium |
| 2015 | ericw-tools `_surface` | A light entity copied to each subdivided face centroid (`-surflight_subdivide`, code default 128) of a named texture; `_surface_spotlight`, `_surflight_group` | `_surface_offset`, default 2 units (5 cm) | Inherits the template's falloff | <https://github.com/ericwa/ericw-tools> `light/entities.cc`, `docs/changelog.rst` (2015-04-27) | High |
| ~2022 | ericw-tools 2.0 emissive | Points per face (`-emissivequality`), intensity × face area split evenly | +1 unit, `FixLightOnFace` | Clamps d ≥ 16 units | Same repo, `light/surflight.cc` | High on code; date not checked |
| 2014–2016 | Godot 1.0–2.1 `BakedLight` | Octree light tracer. Emission is ignored; lights are points with a post-blur | n/a | None | <https://raw.githubusercontent.com/godotengine/godot/2.1-stable/tools/editor/plugins/baked_light_baker.cpp> | High |
| 2018–2020 | Godot 3.0–3.2 `BakedLightmap` | Material emission is voxelised as a bounce source, gathered by cones or rays. No geometric emitter | n/a | Ray start offset 1.5–2.5 cells | <https://raw.githubusercontent.com/godotengine/godot/3.0/scene/3d/voxel_light_baker.cpp> | High |
| 2021 | Godot 3.3/3.4 CPU lightmapper | Emission is a path-traced bounce source only. From 3.4, a light's `size` jitters shadow rays over a disk of radius `size/dist`, with an early-out after half the rays. Irradiance is still taken from the light's centre | n/a | Ray bias; texels pushed out of geometry | <https://raw.githubusercontent.com/godotengine/godot/3.4-stable/modules/lightmapper_cpu/lightmapper_cpu.cpp> | High |
| 2023–2026 | Godot 4.0–4.6 `LightmapGI` (GPU) | Emission is a bounce source (atlas texel, later path-traced). Shadows use disk jitter × `shadow_blur` | n/a | Bias plus an unocclude pass | <https://raw.githubusercontent.com/godotengine/godot/4.3-stable/modules/lightmapper_rd/lm_compute.glsl> | High |
| 2026 (4.7, June; PR opened 2025-07-02) | Godot 4.7 `AreaLight3D` | **Baked rectangle area light.** Analytic, horizon-clipped polygon diffuse term (`ltc_evaluate_diff`, `lm_area_lights_inc.glsl`). Shadow rays aim at the rectangle's **centre**, disk-jittered by `size/dist`; the rectangle itself is not sampled. Optional texture | Receivers in the light's plane get 0 | `dist ≥ 1e-4` to the nearest point | <https://raw.githubusercontent.com/godotengine/godot/4.7-stable/modules/lightmapper_rd/lm_area_lights_inc.glsl>; PR <https://github.com/godotengine/godot/pull/108219> | High on code; medium on release date |
| 2020s | Strata Source (P2CE, Portal: Revolution, Momentum Mod) | No area or rectangle light documented publicly; `light`, `light_spot`, `light_rt*` only. An engine issue reports circular specular from rectangular strips | n/a | n/a | <https://github.com/StrataSource/Engine/issues/1623>; <https://wiki.stratasource.org/shared/reference/util/vrad> | Medium (search extracts; a negative result) |
| 2020s | VRAD++ (ficool2) | `_light_radius` makes a `light` or `light_spot` a sampled sphere or disk; `-extrasoft` multiplies samples. Texlights are unchanged | n/a | n/a | <https://developer.valvesoftware.com/wiki/VRAD%2B%2B> | Medium |
| ~2023 | Source 2 / CS2 `light_rect` | Rectangle or disc **area light, baked only**, by the GPU ray tracer (VRAD3). Whether it is solved analytically or sampled is not documented | Not documented | Not documented | <https://developer.valvesoftware.com/wiki/Light_rect>; <https://developer.valvesoftware.com/wiki/VRAD3> | Medium |

**What the record shows** (mechanisms only, no legal reading):
- Both models shipped early in game tools.
  - Templated points: qrad3 1997, hlrad 1998, vrad, ericw-tools.
  - A polygon emitter with an exact unoccluded point-to-polygon form factor: id q3map 1999, q3map2.
- The academic sources predate both: Lambert 1760, Baum et al. 1989, Arvo 1995.
- Every tool offsets the emitter or receiver off the plane, by 2.5 cm to 0.2 m.
- Every point model clamps the near field, or accepts hot spots.
- Every area-scaled tool makes output grow with emitter area.
- Godot treats emission only as a bounce source in every lightmapper version.
  - Its one shaped direct light is the 4.7 rectangle (2026). It has an analytic clipped-polygon diffuse term, but its shadows are traced from the rectangle's centre.
  - It is the nearest open reference to this design's diffuse term. The source is MIT-licensed; whether to reuse any of it is the owner's call.
  - This design differs in the shadow term: samples over the polygon itself, weighted by the ratio estimator (§2).
  - Reading its shader is a cheap cross-check of the polygon maths, not a source of the shadow method.
- Source-engine forks kept texlights as templated points. Area lights arrive only as sized point lights (VRAD++) or, in Source 2, as `light_rect`, a baked-only rectangle (~2023).
- This session searched for prior art only. It ran **no patent search** on polygon area-light baking or on the ratio estimator. The roadmap's "IP check before drafting" is therefore half done (§Handoff, Not verified).

## 6. Future runtime specular (door only)

| Claim | Source | Confidence | Symbol it changes |
|---|---|---|---|
| LTC (Heitz, Dupuy, Hill & Neubelt 2016). A BRDF lobe is a clamped cosine under a 3×3 matrix. Shading transforms the polygon by M⁻¹ and evaluates Lambert's edge formula. Runtime needs two 64 × 64 tables and the light's vertices, plus horizon clipping or the sphere-approximation lookup | Hill & Heitz 2016 <https://www.advances.realtimerendering.com/s2016/s2016_ltc_rnd.pdf>; Heitz & Hill 2017 <https://blog.selfshadow.com/publications/s2017-shading-course/heitz/s2017_pbs_ltc_lines_disks.pdf>; fitter <https://github.com/selfshadow/ltc_code> | Medium-high (PDFs not fetched) | `forward.wgsl::world_static_light_specular`, `SpecLight` |
| The fitter takes any isotropic BRDF through an eval/sample interface. A Blinn-Phong fit is feasible, or a Beckmann table can be reused with α = √(2/(n+2)) | ltc_code; Walter et al. 2007 mapping | Medium | Same; one extra bound texture against the 16-texture floor |
| Representative point (Karis 2013) is defined for spheres and tubes: the closest point to the reflection ray, plus α′ = saturate(α + r/(2d)) energy widening. Drobot 2014 (GPU Pro 5) and Frostbite 2014 extend it to rectangles and note its artefacts | Karis <https://blog.selfshadow.com/publications/s2013-shading-course/karis/s2013_pbs_epic_notes_v2.pdf>; Lagarde & de Rousiers <https://blog.selfshadow.com/publications/s2014-shading-course/frostbite/s2014_pbs_frostbite_slides.pdf> | High on concepts; medium on URLs | Same |

**Reading for this repo.**
- One record shape opens both doors: centre, two half-extent vectors (the normal is their cross product), colour and intensity.
- A representative point reuses the Blinn-Phong-to-a-point code with a moved light vector and a lowered exponent, and needs no tables.
- LTC draws strip- and rectangle-shaped highlights but costs a table binding and four edge integrals per pixel.
- v1 stays at no specular, as decided.
- The `AlphaLightRecord` polygon payload should carry the rectangle's centre and half-extents now. A later specular change then needs no second section bump. Whether a non-rectangular convex polygon is in scope settles how that payload generalises (§Decisions).

## What the web changed

- **Visibility semantics** (new). The handoff's "term × visibility fraction" stays, but for a polygon the fraction must be integrand-weighted (ratio estimator), or sampled by projected solid angle. An unweighted fraction is wrong near the light, which is exactly the skylight case. The partition format does not change.
- **Direct SH needs a polygon projection** (new). The single-direction path is 3.4× off for side-facing receivers at 1 m. This adds the id-35, 41, 45, 47/48 and mover-probe-channel bakes to the polygon arm's scope, not just the lightmap.
- **The SH ratio must not run per coefficient** (new). Use a control variate or a scalar ratio.
- **Culls can stay spheres for v1** (narrows scope). A containing sphere keeps every exact-skip proof. A tight hull is a measured follow-on.
- **Reach is measured from the polygon**, and depends on flux, not area (new, for the brightness convention).
- **Prior art** shows both polygon-native area lights and templated points in shipped tools (§5). This informs owner question 1. It is not a legal reading.

---

## Handoff

Changes from the cloud session's handoff are marked **[web]** where research changed them and **[source]** where this session's source reads did.

```markdown
Problem: A designer cannot make a big ceiling skylight or a glowing light-table
floor light a room. Emissive maps light nothing. The only workaround is a grid of
point lights. That grid leaves hot spots on nearby surfaces: under a 4 × 4 m panel
a single point reads 6.1× too bright at 1 m. It also loses specular past the 4
shadowmask owners per face (id 42), and lightmap bake cost grows with every copy.

Outcome: A brush entity whose brushes compile as world brushwork and whose emitting
faces become one static polygon area light, with its own colour and intensity.
- It lights the lightmap (direct), the SH bounce (through its own patch layer),
  and dynamic receivers (direct SH).
- Soft shadows are correct near the emitter, with no hot spots.
- Scripts can animate it through the existing weight-map and delta paths.
- v1 has no specular, no promotion and no SDF shadows.

Read at: 17c2f1e (lighting sources unchanged since eb44499)

Verified facts: (by symbol)
- Cloud session's list, all still current.
- [source] Direct SH projects each light along one point→light direction:
  sh_bake/probe.rs::bake_probe_direct_rgb_posed → accumulate_sh_rgb, then
  apply_cosine_lobe_rgb.
  - Probes are real L2 SH (sh_math::sh_basis_l2, 27 floats) packed into octahedral
    irradiance tiles.
  - The same single-direction path feeds ids 35, 41, 45, 47/48 and the mover
    probe channel.
- [source] More consumers of the term and emitter shape than listed:
  - mover_probe_channel/{direct,visibility,posed_scene}.rs: emitter_radius,
    area_sample_target, and a PosedOccluder::may_meet sphere test
  - billboard_direct_scatter_bake.rs: incident_radiance_at_point
  - surface_radiance/layer.rs: bounce_contribution
  - about 25 compiler files that read falloff_range as an origin-centred reach
    radius
- [source] The lightmap_layer partition record holds only a texel index and raw
  visibility. The fold rebuilds irradiance and direction from the unshadowed term
  at the texel centre. So a polygon light needs a new term and a weighted
  visibility, and no record change.
- [source] entity_shadow_select::is_promotable_base_light already refuses any type
  except Point/Spot. The animated promotion path needs its own refusal.
- [source] `switch` is the closer precedent: a brush entity that compiles as world
  geometry and carries a script `name`.
- [web] Unoccluded term: Lambert/Arvo E = (L/2) Σ Θᵢ (Γᵢ·n) on the polygon clipped
  to the receiver's horizon. The irradiance vector Φ of the clipped polygon gives
  the stored direction. Checked to 6 digits against Howell's closed form and
  midpoint integration (research.md §1).
- [web] Ratio estimator (Heitz, Hill & McGuire 2018): "term × visibility" stays
  exact for a polygon only if visibility is weighted by the unshadowed integrand,
  or sampled by projected solid angle. An unweighted area fraction is 81×
  mis-weighted at 1 m under a 4 m light.
- [web] Single-direction SH is 3.4× low for a side-facing receiver at 1 m and leaks
  onto down-facing normals. A projection over the polygon's solid angle is within
  a few percent (§3).
- [web] Any containing sphere keeps every exact-skip proof valid. A tight bound is
  hull(polygon ∪ box), and the reach is √(L·A/ε) measured from the polygon (§4).
- [web] Prior art: templated points in qrad3 1997, hlrad 1998 and ericw-tools 2015;
  exact polygon form factor in id q3map 1999 and q3map2. Baked rectangle area
  lights in Source 2 `light_rect` (~2023) and Godot 4.7 `AreaLight3D` (2026, MIT,
  analytic diffuse, shadows from the centre). Godot otherwise treats emission as
  bounce only, and Strata Source documents no area light. Every game tool offsets
  the emitter off the plane or biases the ray (§5).

Not verified:
- IP: no patent search was run. Only game-tool prior art was collected. The
  roadmap's IP check still needs a claim-level patent search on polygon area-light
  baking (and on the ratio estimator, if the brief adopts it).
  - patents.google.com is reachable from the default environment per gi-rtx-lite
    index.md.
  - Legal conclusions are for the owner or counsel.
- How segment_clear treats a segment whose endpoint lies on a triangle. This
  decides between lifting samples off the plane and excluding the emitter's own
  faces.
- Whether 32 escalated rays are enough for a 4 m emitter at 1–3 m. No published
  count exists, so it must be measured.
- Lightmass internals and Enlighten near-field behaviour (docs only, medium).
- The full text of Heitz 2018, Wang & Ramamoorthi 2018, LTC, Arvo 1995 and Howell
  B-3. These hosts were outside the allowlist; facts were checked against
  excerpts, and the polygon maths numerically.
- How a face's visible glow intensity is sourced today, which owner question 3
  depends on.

Decisions: (owner, settled)
- Opt-in per placement through a brush entity. Its brushes are world brushwork; its
  emitting faces form one light. The brief states both divergences (roadmap
  "surface lights"; gi-rtx-lite "Emissive bounce: opt-in per material") and
  updates both records.
- Do not build on Material::emissive_strength or `neon_`. The entity carries its
  own colour and intensity.
- Script-animatable through bake-time weight maps and deltas, scaled by runtime
  curves.
- v1: no area-light specular (keeps a placeholder SpecLight record, refused as an
  owner), no promotion on either path, SDF shadow type refused.
- The emitter's bounce comes from its own patch layer. Its glow is not also put
  into the surface radiance cache (no double count, index.md §2).

Decisions: (brief to settle; recommendation from the research)
- [web] Visibility for polygon lights is the ratio estimator. Use Ureña
  solid-angle sampling for rectangles and an Arvo fan for other convex polygons;
  store Σ wV / Σ w with w = cosθ_r at the texel centre. The polygon lattice is
  per light and per receiver, so it never uses the count-keyed point/spot snap.
- [web] Probe bakes project over the polygon. Use analytic unshadowed coefficients
  (L0 from Ω, L1 from Φ, L2 from Wang–Ramamoorthi or a fixed quadrature) plus a
  control-variate shadow term, with all-visible and none-visible fast paths. Never
  a per-coefficient ratio.
- [web] Culls in v1 take a containing sphere: centroid, radius = circumradius +
  reach. Tight hulls follow only if the hallway bake-time row fails.
- [web] Brightness is radiance-like (per area): output grows with emitter area, as
  in every area-scaled tool (§5). On the emitter's axis the small-emitter limit is
  a point light with I = L·A.
- [web] Reach: derived from flux by the bound √(L·A/ε), with an authored override,
  zero beyond. This mirrors Lightmass's derived influence radius. ε is a constant
  chosen by measurement.
- Flush handling:
  - Shadow-ray sample points lift off the emitter plane (q3map, qrad and
    ericw-tools precedent), unless the endpoint check says to exclude the
    emitter's own faces instead.
  - buried_lights and the embedded-emitter lattice skip this light type.
  - The term's d ≤ ε early-out darkens the emitter's own face.
- The emitter's geometry comes from exposed brush sides before the BSP splits
  them. The light is a set of convex polygons; the term sums over them, and the
  lattice spreads samples across them by solid angle.
- The AlphaLightRecord polygon payload carries enough to open a later specular
  door (rectangle centre + half-extents, or a bounded polygon list) in the same
  section bump.

Non-goals:
- Runtime specular from area lights. LTC or a representative point are recorded
  doors (§6), not v1.
- Entity-shadow promotion and SDF shadows for area lights.
- Template ("_surface") lights across every face of a texture. This is the
  roadmap entry the owner's ruling replaces.
- Tight polygon shaft/reach hulls, unless measurement asks for them.
- Two-sided emitters (q3map's fog case); none of the motivating content needs one.
- Retiring `neon_` (separate work).

Proof:
Automated, polygon term (§1 values are the oracle):
- A unit square, L = 1, facing down, receivers facing up:
  - h = 0.1 / 0.5 / 1 / 2 / 5 / 10 → 3.042128 / 1.740840 / 0.752275 / 0.230837
    / 0.039474 / 0.009967, to 1e-5 relative.
  - These must equal 4X/√(1+X²)·atan(X/√(1+X²)) with X = 1/(2h).
- At h = 1, receiver tilts 0/30/60/85/90/120° → 0.752275 / 0.651489 / 0.376137 /
  0.123245 / 0.087510 / 0. The 120° case pins the horizon clip; without the clip
  it reads negative or nonzero.
- Off-axis (2, 0, 0) → 0.0437426, matching inclusion–exclusion of Howell B-3. On
  an edge's line extension → 0.0396548, with no NaN.
- Grazing: 0.1 mm below the plane, facing the emitter → 1.3554e-5, finite.
  Coplanar → 0. Behind the emitter → 0. The emitter's own face → 0.
- Permit side: h = 1 mm under the centre → within 1e-4 of π.
- Small-emitter limit: s = 0.1 / 0.01 / 0.001 at a skewed pose. The ratio to
  L·A·cosθ_e·cosθ_r/d² is within 1e-3 / 1e-5 / 1e-7. On the emitter axis this
  equals the existing InverseSquared point term with I = L·A and range beyond d.
- f32 corners at about 1 km from the origin agree with f64 corners.
Automated, visibility:
- Fully lit and fully shadowed texels store exactly 1 and 0.
- A half-plane occluder covering the near half of a large emitter versus one
  covering the far half gives different stored visibility, weighted by
  integrand. An unweighted fraction would tie; this row fails without the ratio
  estimator.
- Fold output equals the unweighted reference for a tiny emitter.
Automated, probes:
- With nothing occluding, the probe projection's L0 matches Ω and its L1 matches
  Φ.
- 4 × 4 m at 1 m: a side-facing reconstruction within 5% of 0.781, and a
  down-facing one within 0.05 of 0. Single-direction projection would give 0.230.
Automated, flush, culls and identity:
- An area light on a ceiling face is neither dropped by buried_lights nor flagged
  embedded. A point light buried in solid still is.
- A wall texel adjacent to the emitter edge is not black (q3map's "black edges"
  failure).
- Every bake output is byte-identical with exact skips on and off for an
  area-light map.
- Changing any polygon vertex re-keys the lightmap layer, LightDigest::of and the
  direct-SH memos. An unrelated light's entry does not move.
- Promotion refuses area lights on both the base and the animated path. The SDF
  shadow type is refused at compile. The placeholder SpecLight is never an id-42
  owner.
- Animated: a weight map with curve = 1 equals the static bake of the same light.
Measured (hallway map, recorded in the PR):
- Cold and warm bake time: one area-light skylight against the same patch lit by
  a 4 × 4 template grid of point lights. Then the same with the sphere cull, to
  decide on tight hulls.
- Penumbra noise (RMS against a 256-ray reference) at 32 and 64 escalated rays,
  1–3 m from a 4 × 4 m emitter. Decide the polygon's sample count from this.
Manual:
- Owner A/B of a skylight room against a template grid: hot spots and wall
  ripple.
- A scripted flicker with the face glow in sync (if owner question 3 is yes).
- Entities walking under the skylight match the walls beside them.

Open owner questions:
1. True polygon light or template lights (roadmap)?
   Recommend the polygon light.
   - The web adds two reasons. First, the approach has game-tool precedent:
     id q3map shipped an exact polygon form factor in 1999, and Source 2
     (~2023) and Godot 4.7 (2026) bake rectangle area lights. Second, the
     separable bake absorbs it with no partition format change.
   - Cost: a polygon arm in the probe bakes, and a sampler that is not a sphere.
   - IP: prior art is recorded, but no patent search has been run. Run one before
     the brief is promoted. Its reading is for the owner or counsel.
2. Which faces emit?
   Recommend exposed faces whose texture has an `_e` map, the cloud session's
   choice.
   - It picks the glass pane over the frame in one mixed entity, and it is the
     material link the gi-rtx-lite ruling expected.
   - Add a compile error when no face qualifies.
   - "Every exposed face" lights a light table's sides and frame.
   - "Face nearest angles" fails a multi-pane skylight.
3. Should the entity's brightness also drive its faces' glow?
   Recommend yes, together with animation.
   - A flickering light with a steady glowing pane reads as a bug.
   - This needs a per-face emitter id and a runtime table, following the
     `animated_block` precedent.
   - A static-only first slice could defer it, but script animation is in scope,
     so splitting would land a known-visible defect.
New, from this research:
4. Should brightness be per area, so a larger brush gives more light? Recommend
   yes: every area-scaled tool does this.
   The alternative is total output fixed regardless of size. That needs a
   modder-facing unit either way.
5. Should reach be derived from brightness with an override (Lightmass)? Or
   authored as falloff_range like point lights? Recommend derived, with an
   override.

Route: brief

Why this route: Several decisions must be reviewed before code exists:
- a modder-facing FGD entity and its brightness and reach units;
- a PRL section version bump (AlphaLightRecord) that sizes the later specular
  door;
- two recorded rulings to overturn;
- an IP check that is still half done.
One integrating executor can own the work: the term, the sampler, the probe
projection, the flush rules, the cull feed and the runtime refusals.

Next action: discuss
```
