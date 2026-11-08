# Square Enix '372 against direction-aware probe visibility

**Date investigated:** 2026-11-07, repo at `55f61eab2` (branch `feature/direction-aware-probe-visibility`)
**Status:** Engineering triage for counsel, not legal advice. "Appears present", "appears absent", "reads on" and "arguable" are engineering readings of claim text, not opinions on infringement, validity or freedom to operate. Companion to [ip-check.md](ip-check.md) §Piece 2 and counsel question 8. Brief: `context/plans/in-progress/direction-aware-probe-visibility/`.

> **Read this when:** changing how SH probes are weighted, stored or selected (Chebyshev floor, depth tiles, backface factor, validity), adding per-probe visibility data of any kind, or briefing counsel on US10137372B2.
> **Sources.** **Verified** means the primary text was read. *(snippet)* means search-result or summarizer text only. Patent text: Google Patents full text of US10137372B2, the A1 publication US20180093183A1, EP3306569B1 and CA2944435C, all read 2026-11-07 (**verified**). Not read: the US file wrapper (15/377,913). Patent Center's public API refused (HTTP 405), the USPTO assignment API was unreachable, and ppubs anonymous access ends today. Prior-art reads are in §Prior art. Column numbers are not given because Google's text has none, so passages are quoted instead.

---

## Summary verdict

**Needs counsel, on a narrower question than ip-check.md records.** Only one of nine independent claims is live against this change.

1. **ip-check.md misreads claim 1.** It charts claim 1 from the abstract, which omits a limitation. Granted claim 1 adds "wherein the zones for each selected probe are defined by intersections of pre-defined occlusion planes for the selected probe". That limitation is in claims 1, 34, 35, 36, 42 and 43, and claim 38 recites an operation on "each of a plurality of pre-defined occlusion planes". Claim 44 instead needs a visibility mask, a zone code and "a bit-wise AND". The A1 publication's claim 1 lacked the planes; they sat in A1 dependent claim 6. Comparing A1 with B2 therefore suggests the planes were added to claim 1 to get it allowed. That is an inference: the file wrapper was not read.
2. **Claim 37 is the only independent claim without planes or a mask.** It is A1 claims 1 + 2 + 5. Its elements are a region from "a grid of regions", zones, zone visibility, and combining the probes whose zone is visible. The region-grid element reads on our lattice-cell lookup. Claim 37 is therefore decided by "zone" and "determining visibility of said determined zone".
3. **The plane-based claims appear absent.** The engine stores no per-probe plane, zone or mask. Claims 1, 34–36, 38 and 42–44 and their dependents fall with those elements. One observation to put to counsel: octahedral texel boundaries are plane pieces through the probe centre (row 7 of the claim map).
4. **Claim 37 is arguable and leans toward absent**, on two grounds:
   - Our weight is a continuous Chebyshev bound evaluated for the *point at its distance* against a per-direction depth distribution.
   - Within one direction, near points read visible and far points read occluded, so visibility is never a property of a zone. No zone identity is ever computed.
5. **The 0.03 floor is evidence, not the escape.** Dependent claim 11 adds "the number of probes in the set is greater than the number of sighted probes". By claim differentiation, that suggests claims 1 and 37 can be met when every probe is "sighted". So "no probe is excluded" does not by itself defeat the combining element. It does show that no binary visible or not-visible outcome exists anywhere in the runtime.
6. **The examiner had per-direction probe visibility before them.** Mega Meshes (GDC 2011) is cited on the patent's face. It stores per probe "a distance to the nearest occluder in every direction" and ignores probes that cannot "see the object". The claims were allowed over it with "zones". Counsel should check whether that limits how far "zone" can be read toward per-direction depth.

**Status.** Active. Priority CA2944435 2016-10-04; US filed 2016-12-13 (not 2016-12-14, which is the EP filing date). Granted 2018-11-27. Google gives an adjusted expiry of 2037-02-01, about 50 days of PTA; that figure is Google's and was not confirmed with USPTO. Fees paid: 4th year 2022-05-18, 8th year 2026-05-20. The 12th-year fee falls due in the 2029-11 to 2030-05 window, by 2030-05-27 without surcharge.

Inventors are Luc Leblanc and Jean-François Dufort. Assignments: inventors to Eidos Interactive Corp. (effective 2017-12-06), then to Square Enix Limited, UK (effective 2018-02-21). Google lists Square Enix Ltd as current owner. No later assignment was checked: Eidos Montréal went to Embracer in 2022, but this patent was assigned to Square Enix Limited.

**Family.** CA2944435C, EP3306569B1 (granted 2024-05-01, lapsed in several states for missing translations or fees) and JP6970508B2, all Active per Google. Google lists only one US application, so no continuation or divisional is in the US record. A continuation needed to be filed while 15/377,913 was pending, before 2018-11-27. Every EP independent claim carries the occlusion-plane limitation; the EP set has no claim-37 analogue. CA claim 1 matches US claim 1 (planes).

**Cited on the face.** Patent citations: US6628298B1, EP1508877A2, US20090309877A1, US20140327690A1, and US20160260247A1 (Imagination; examiner-cited). Non-patent: Cupisz GDC 2012, Ramamoorthi 2001, Sloan "Stupid SH Tricks" 2008, Sugden "Mega Meshes" GDC 2011, Tatarchuk GDC-E 2005, Valient (Killzone) 2014, and the EP search report.

## Patent text (claims verbatim)

US10137372B2, 44 claims, verbatim from Google Patents. Source typos are kept: ",-wherein" in claims 18 and 19, and "defined in 28" etc. (no "claim") in claims 30–33. Independent claims: 1, 34, 35, 36, 37, 38, 42, 43, 44. **Verified.**

**1.** A computer-implemented method for determining an illumination component for a selected point in a multi-dimensional space, comprising:
- identifying a set of probes associated with the selected point, the probes located in the multi-dimensional space;
- for each selected one of the probes, determining which of a plurality of zones for the selected probe contains the selected point and determining visibility of said determined zone from the selected probe, wherein the zones for each selected probe are defined by intersections of pre-defined occlusion planes for the selected probe; and
- deriving an illumination component at the selected point by combining scene irradiance data associated with those of the probes from which the corresponding determined zone is determined to be visible.

**2.** The computer-implemented method defined in claim 1, wherein said identifying a set of probes associated with the selected point comprises determining a region in which the selected point is located and identifying a set of probes associated with the region.

**3.** The computer-implemented method defined in claim 2, wherein said steps of determining which of a plurality of zones and determining visibility are carried out for each probe in the set.

**4.** The computer-implemented method defined in claim 2, wherein identifying a set of probes associated with the region comprises consulting a memory to retrieve a set of probes that are predetermined to have potential influence over illumination of the selected point.

**5.** The computer-implemented method defined in claim 1, wherein the number of occlusion planes for the selected probe is three.

**6.** The computer-implemented method defined in claim 1, wherein said determining visibility of said determined zone from the selected probe comprises applying a visibility mask associated with the selected probe.

**7.** The computer-implemented method defined in claim 6, wherein the visibility mask associated with the selected probe encodes the zone or zones that are visible from the selected probe, or the fact that there is no zone visible from the selected probe.

**8.** The computer-implemented method defined in claim 7, wherein the determined zone is encoded by a code and wherein said applying a visibility mask associated with the selected probe comprises effecting a comparison between said code and said visibility mask associated with the selected probe.

**9.** The computer-implemented method defined in claim 8, further comprising consulting a memory to access the visibility mask associated with the selected code.

**10.** The computer-implemented method defined in claim 1, wherein said combining scene irradiance data associated with those of the probes from which the corresponding determined zone is determined to be visible, hereinafter a sighted probe, comprises (i) determining irradiance data for each sighted probe; (ii) determining a weight of each sighted probe; (iii) computing a weighted sum of the irradiance data for each sighted probe.

**11.** The computer-implemented method defined in claim 10, wherein identifying a set of probes associated with the selected point comprises determining a region in which the selected point is located and identifying a set of probes associated with the region and wherein, for the selected point, the number of probes in the set is greater than the number of sighted probes.

**12.** The computer-implemented method defined in claim 1, wherein the irradiance data is diffuse global illumination data.

**13.** The computer-implemented method defined in claim 1, wherein said combining scene irradiance data associated with those of the probes from which the corresponding determined zone is determined to be visible, hereinafter a sighted probe, comprises (i) determining an array of irradiance parameters for each sighted probe; (ii) determining a weight of each sighted probe; (iii) for each element of the array, computing a weighted sum of the corresponding irradiance parameter across the sighted probes to result in a corresponding blended irradiance parameter; (iv) computing a mapping function of the blended irradiance parameters, thereby to determine the illumination component.

**14.** The computer-implemented method defined in claim 13, wherein the weight of each sighted probe is based at least in part on a distance between the selected point and the probe.

**15.** The computer-implemented method defined in claim 13, wherein the weight of at least two sighted probes is proportional to a distance between the selected point and that probe and the weight of at least a third one of the sighted probes is not proportional to a distance between the selected point and the third sighted probe.

**16.** The computer-implemented method defined in claim 13, wherein the weighted sum is normalized by the sum of the weights of the sighted probes.

**17.** The computer-implemented method defined in claim 13, wherein the irradiance parameters are spherical harmonic coefficients and wherein the mapping function comprises a linear combination of the spherical harmonic parameters and spherical harmonic basis functions evaluated at a normal associated with the selected point.

**18.** The computer-implemented method defined in claim 17,-wherein the normal is the normal of the selected point.

**19.** The computer-implemented method defined in claim 17,-wherein the spherical harmonic basis functions are of order 3.

**20.** The computer-implemented method defined in claim 1, wherein said combining scene irradiance data associated with those probes from which the corresponding determined zone is determined to be visible, hereinafter a sighted probe, comprises (i) determining an array of irradiance parameters for each of at least two sighted probes; (ii) determining an illumination component for each of the sighted probes by computing a mapping function of the blended irradiance parameters for that sighted probe; (iii) determining a weight of each of the sighted probes; (iv) computing a weighted sum of the illumination components for the sighted probes, thereby to determine the illumination component.

**21.** The computer-implemented method defined in claim 20, wherein the weight of each sighted probe is based at least in part on a distance between the selected point and the probe.

**22.** The computer-implemented method defined in claim 20, wherein the weight of at least one sighted probe is not proportional to a distance between the selected point and that probe.

**23.** The computer-implemented method defined in claim 20, wherein the weighted sum is normalized by the sum of the weights of the sighted probes.

**24.** The computer-implemented method defined in claim 20, wherein the irradiance parameters are spherical harmonic coefficients and wherein the mapping function comprises a linear combination of the spherical harmonic parameters and spherical harmonic basis functions evaluated at a normal associated with the selected point.

**25.** The computer-implemented method defined in claim 24, wherein the normal is the normal of the selected point.

**26.** The computer-implemented method defined in claim 1, wherein the space is a two-dimensional space.

**27.** The computer-implemented method defined in claim 1, wherein the space is a three-dimensional space.

**28.** The computer-implemented method defined in claim 1, the illumination component being a first illumination component that is part of a global illumination component, the method further comprising:
- deriving a local illumination component; and
- rendering the selected point on a display device based on a combination of the local and global illumination components.

**29.** The computer-implemented method defined in claim 28, wherein the rendering is carried out in real-time and is dependent on user input.

**30.** The computer-implemented method defined in 28, wherein the local illumination component is derived in real-time.

**31.** The computer-implemented method defined in 30, wherein the first illumination component is derived in real-time.

**32.** The computer-implemented method defined in 31, wherein the plurality of zones for each selected one of the probes are determined in non-real-time.

**33.** The computer-implemented method defined in 32, wherein said determining visibility is based on a visibility mask that is pre-computed for each probe.

**34.** A system for determining an illumination component for a selected point in a multi-dimensional space, comprising:
- a processor;
- a memory storing information identifying a plurality of probes in the multi-dimensional space and information associated therewith;
- the processor configured for:
- identifying a set of probes associated with the selected point, the probes located in the multi-dimensional space;
- for each selected one of the probes, determining which of a plurality of zones for the selected probe contains the selected point and determining visibility of said determined zone from the selected probe, wherein the zones for each selected probe are defined by intersections of pre-defined occlusion planes for the selected probe; and
- deriving an illumination component at the selected point by combining scene irradiance data associated with those probes from which the corresponding determined zone is determined to be visible.

**35.** A computer-readable medium comprising computer readable instructions which, when executed by a computing device, cause the computing device to execute a method for determining an illumination component for a selected point in a multi-dimensional space, the method comprising:
- identifying a set of probes associated with the selected point, the probes located in the multi-dimensional space;
- for each selected one of the probes, determining which of a plurality of zones for the selected probe contains the selected point and determining visibility of said determined zone from the selected probe, wherein the zones for each selected probe are defined by intersections of pre-defined occlusion planes for the selected probe; and
- deriving an illumination component at the selected point by combining scene irradiance data associated with those probes from which the corresponding determined zone is determined to be visible.

**36.** A game apparatus, comprising
- an input/output interface allowing a user to control game inputs and perceive game outputs;
- a memory storing (i) information associating a plurality of zones in a multi-dimensional space with corresponding sets of probes in the multi-dimensional space; (ii) executable instructions;
- a processor configured for executing the executable instructions to derive an illumination component at each of a plurality of selected points in the multi-dimensional space from scene irradiance data associated with each of one or more probes from which a zone containing the selected point is visible, the one or more probes being selected from the set of probes corresponding to the zone;
- wherein the zones for each probe are defined by intersections of pre-defined occlusion planes for the probe.

**37.** A computer-implemented method for determining an illumination component for a selected point in a multi-dimensional space, comprising:
- identifying a set of probes associated with the selected point, the probes located in the multi-dimensional space;
- for each selected one of the probes, determining which of a plurality of zones for the selected probe contains the selected point and determining visibility of said determined zone from the selected probe; and
- deriving an illumination component at the selected point by combining scene irradiance data associated with those of the probes from which the corresponding determined zone is determined to be visible;
- wherein said identifying a set of probes associated with the selected point comprises determining a region in which the selected point is located and identifying a set of probes associated with the region; and wherein said determining a region in which the selected point is located comprises accessing a grid of regions.

**38.** A computer-implemented method for determining an illumination component for a selected point in a multi-dimensional space, comprising:
- identifying a set of probes associated with the selected point, the probes located in the multi-dimensional space;
- for each selected one of the probes, determining which of a plurality of zones for the selected probe contains the selected point and determining visibility of said determined zone from the selected probe; and
- deriving an illumination component at the selected point by combining scene irradiance data associated with those of the probes from which the corresponding determined zone is determined to be visible;
- wherein said determining which of a plurality of zones for the selected one of the probes contains the selected point comprises carrying out an operation involving the selected point and each of a plurality of pre-defined occlusion planes for the selected one of the probes to obtain a final result indicative of the determined zone.

**39.** The computer-implemented method defined in claim 38, further comprising accessing a memory to retrieve identification parameters of the occlusion planes for the selected probe.

**40.** The computer-implemented method defined in claim 39, further comprising creating the zones from the occlusion planes and storing identification parameters of the zones in the memory, in association with the selected probe.

**41.** The computer-implemented method defined in claim 40, wherein creating zones from the occlusion planes comprises defining a zone as being in front of or behind each of the occlusion planes.

**42.** A computer-implemented method for determining an illumination component for a selected point in a multi-dimensional space, comprising:
- identifying a set of probes associated with the selected point, the probes located in the multi-dimensional space;
- for each selected one of the probes, determining which of a plurality of zones for the selected probe contains the selected point and determining visibility of said determined zone from the selected probe; and
- deriving an illumination component at the selected point by combining scene irradiance data associated with those of the probes from which the corresponding determined zone is determined to be visible;
- wherein said determining visibility of said determined zone from the selected probe comprises applying a visibility mask associated with the selected probe;
- wherein the visibility mask associated with the selected probe encodes the zone or zones that are visible from the selected probe, or the fact that there is no zone visible from the selected probe;
- wherein the zones for the selected probe are defined by intersections of pre-defined occlusion planes for the selected probe and wherein the visibility mask associated with the selected probe encodes at least two zones that are adjacent in the multi-dimensional space.

**43.** A computer-implemented method for determining an illumination component for a selected point in a multi-dimensional space, comprising:
- identifying a set of probes associated with the selected point, the probes located in the multi-dimensional space;
- for each selected one of the probes, determining which of a plurality of zones for the selected probe contains the selected point and determining visibility of said determined zone from the selected probe; and
- deriving an illumination component at the selected point by combining scene irradiance data associated with those of the probes from which the corresponding determined zone is determined to be visible;
- wherein said determining visibility of said determined zone from the selected probe comprises applying a visibility mask associated with the selected probe;
- wherein the visibility mask associated with the selected probe encodes the zone or zones that are visible from the selected probe, or the fact that there is no zone visible from the selected probe;
- wherein the zones for the selected probe are defined by intersections of the occlusion planes for the selected probe and wherein the visibility mask associated with the selected probe encodes only zones that are non-adjacent in the multi-dimensional space.

**44.** A computer-implemented method for determining an illumination component for a selected point in a multi-dimensional space, comprising:
- identifying a set of probes associated with the selected point, the probes located in the multi-dimensional space;
- for each selected one of the probes, determining which of a plurality of zones for the selected probe contains the selected point and determining visibility of said determined zone from the selected probe; and
- deriving an illumination component at the selected point by combining scene irradiance data associated with those of the probes from which the corresponding determined zone is determined to be visible;
- wherein said determining visibility of said determined zone from the selected probe comprises applying a visibility mask associated with the selected probe;
- wherein the visibility mask associated with the selected probe encodes the zone or zones that are visible from the selected probe, or the fact that there is no zone visible from the selected probe;
- wherein the determined zone is encoded by a code and wherein said applying a visibility mask associated with the selected probe comprises effecting a comparison between said code and said visibility mask associated with the selected probe;
- wherein said comparison comprises a bit-wise AND.

**Prosecution inference (A1 vs B2, file wrapper unread).** A1 claim 1 reads as granted claim 1 without the occlusion-plane "wherein". A1 claim 6 is that wherein. Granted 37 = A1 1 + 2 + 5 ("accessing a grid of regions"). Granted 38 = A1 1 + 8. Granted 42–44 add mask limitations to A1 1. This pattern matches a rejected broad claim 1 plus dependents indicated allowable and rewritten in independent form. Only the file wrapper can confirm it.

## Implementation in the patent

All passages are from the B2 description. **Verified.**

**Problem.** "known methods interpolate probes based on their proximity to the point of interest. However, this fails to take into consideration the effect of occlusion that certain elements in the scene may have, which results in certain nearby probes being unduly influential … the viewer may perceive unpleasant artifacts such as 'popping'."

**Offline process** (FIG. 4; the description's sub-process numbering is internally inconsistent: step 530 cites 430 for probe sets, and the visibility test cites 420 for the mask):
- **Regions and probe sets.** "the multi-dimensional space is subdivided into regions, e.g., according to a regular hierarchical grid … For each region, the processor 10 generates a set of potentially influential probes … based on factors such as distance between the region and the probe, and the probe's radius". The set is stored per region (FIG. 8A), "say, 8 to 20" probes.
- **Irradiance.** Order-3 SH per probe (FIG. 8B). "alternatives to spherical harmonics may be used, including cube maps, spherical Gaussians or other spherical functions."
- **Occluders and zones.** Occluders: "Examples of occluding elements include walls, ceilings, floors, doors, vehicles, etc." Zones: "a set of N 'occlusion planes' is defined for each probe … each occlusion plane may be defined by the coordinates of a point in the plane and the parameters of a normal to the plane … The N occlusion planes yield a total of up to a maximum of P(N) 'zones' per probe. In an embodiment, P(N)=2^N".
- **Mask.** "the visibility mask may be a binary string of P(N) bits, where the position of a bit indicates the zone and the value of the bit in that position indicates whether the zone is visible from the probe." FIG. 7 example: N = 3 in 2D gives 7 zones and the mask "1001011". "For a 3D space there could be up to 8 zones for 3 occlusion planes."
- **Storage.** Plane parameters and the mask are stored per probe (FIG. 8C).
- **Not described:** how the planes are chosen ("defined for each probe", "pre-defined"), and how a zone's visibility is computed. The text names no ray casts and no authoring step.

**Real-time process** (FIG. 5), per point:
- **Steps 515 and 530.** The point's region comes from the grid, and that region's stored probe set is read.
- **Step 540.** A weight per probe, "based in part on relative proximity (distance) … could also be such that the weight … is not proportional to its distance … other factors, such as probe radius, and/or the orientation".
- **Step 550.** The zone comes from the sign of "a four-dimensional dot product of the point (x, y, z, 1) with the selected occlusion plane", repeated over the N planes. The zone is encoded "as a binary string of P(N) bits, where only one of the bit positions is a 1".
- **Step 560.** The point's zone code is ANDed with the probe's mask. A surviving 1 means a "'sighted' probe", which "should influence" the result. All 0s means a "'blind' probe and thus should NOT influence the diffuse global illumination at the selected point".
- **Steps 570–590.** Sighted probes only: weighted SH accumulation, evaluation at the normal, "normalizing the sum by dividing it by the sum of the weights of the probes used in the calculation".
- **Effect of blind probes.** "not all potentially influential probes actually turn out to be influential … This effectively discounts the influence of certain potentially influential probes".

**Readings of the three key terms:**
- **Zone:** a region of space bounded by per-probe planes, with an integer identity and a single visibility bit.
- **Visibility:** binary per zone, pre-computed, independent of the point's distance from the probe.
- **Culling:** an all-or-nothing exclusion of a blind probe from the sum and from the normaliser.
- **No continuous visibility.** The description never mentions partial or continuous visibility, and never uses depth, distance distributions or moments to judge visibility. Distance appears only in the step-540 proximity weight.

## Claim map

Builds read on are: `sh_sample.wgsl` (`sh_corner_depth_visibility`, `sh_chebyshev_visibility`, `sh_depth_tile_moments`, `sh_probe_weight` and the four blend paths), `sh_bake/depth_tiles.rs` and `probe_depth_tiles.rs`.

| # | Claim element | What the patent means by it | What we built | Reading |
|---|---|---|---|---|
| 1 | "identifying a set of probes associated with the selected point" (all independents) | A stored per-region set of 8–20 probes | The 8 lattice corners of the cell containing the point (`sh_corner_index`), computed arithmetically | Appears present |
| 2 | Cl. 37 / cl. 2: "determining a region in which the selected point is located … accessing a grid of regions" | Regular hierarchical grid (FIG. 9) | `gi` = the point's cell in the regular SH lattice | **Reads on.** Old art: Greger 1998, Tatarchuk 2005, Far Cry 3 2012 |
| 3 | Cl. 4: "consulting a memory to retrieve a set of probes that are predetermined to have potential influence" | FIG. 8A table per region | No stored set; corners derived from the cell index | Appears absent (dependent of 1 anyway) |
| 4 | "determining which of a plurality of zones for the selected probe contains the selected point" | Signs against N stored planes → a one-hot code naming one of ≤2^N zones | Tiled probe: `oct_encode_l1(probe→point)` gives a continuous 2-D coordinate. One hardware-bilinear tap blends 4 texels of (E[d], E[d²]). No zone index is computed, compared or kept. Untiled probe: one omnidirectional pair, with no direction at all | **Arguable.** For us: the output is continuous in direction, and no plurality of zones is enumerated or identified. Against us: the 2×2 bilinear footprint, or the texel under the direction, is a discrete set of directions picked per point. A nearest-texel lookup would strengthen that argument (§Design levers) |
| 5 | "determining visibility of said determined zone from the selected probe" | Bit test (AND) of a pre-computed per-zone mask; the outcome is sighted or blind | `sh_chebyshev_visibility`: var / (var + (d − μ − bias)²), 1 when d ≤ μ + bias, clamped to [0.03, 1]. It depends on the point's distance d: along one direction, near points read 1 and far points read toward 0.03. No yes/no outcome exists | **Arguable, leans absent.** What is evaluated is the visibility of a *point*, not of a zone, and nothing is "determined" to be visible. Counsel Q1 |
| 6 | "combining scene irradiance data associated with those of the probes from which the corresponding determined zone is determined to be visible" | Sum over sighted probes only, normalised by their weights | Every valid probe enters with weight trilinear × validity × backface (forward only) × visibility, renormalised. Depth visibility never removes a probe (floor 0.03). L2 whole-cell path: the weights only gate one node sample | **Turns on row 5.** Claim 11's differentiation implies the element can be met with all probes sighted, so the floor alone is not the escape (counsel Q2). If row 5 is absent, so is row 6 |
| 7 | Cl. 1, 34, 35, 36, 42, 43: "zones … defined by intersections of pre-defined occlusion planes for the selected probe"; cl. 38: an operation with "each of a plurality of pre-defined occlusion planes" | N planes per probe, stored (FIG. 8C), tied to occluders | No planes stored or derived per probe. Note for counsel: inside each octant, an octahedral texel's boundaries are planes through the probe centre (x/(|x|+|y|+|z|) = c is linear). Those planes are one fixed parameterization shared by every probe, unrelated to occluders, never tested against the point, and visibility is not constant within the cone they bound | **Appears absent.** "Occlusion" and "for the selected probe" both fail for a fixed angular grid. The A1→B2 amendment (if confirmed) may also bar equivalents |
| 8 | Cl. 6–9, 42–44: "visibility mask … encodes the zone or zones that are visible"; zone "code"; "bit-wise AND" | P(N)-bit string per probe | No mask. The tile stores depth moments (distances), from which visibility follows only given the point's distance. The G-channel reference is a tile index, not visibility | Appears absent |
| 9 | Cl. 10, 13, 16, 20–23: weighted sum of sighted probes, distance-based weight, normalised | Steps 540–590 | Trilinear (distance-based) × factors, renormalised. Per-probe atlas sample at the normal, then a weighted sum: the claim-20 shape | Shape present, but each depends on claim 1 (planes) |
| 10 | Cl. 17, 19, 24: SH coefficients and SH basis at the normal | Order-3 SH | Octahedral irradiance tiles sampled at the normal; no SH coefficients (the "SH" name is historical) | Appears absent; dependent of 1 anyway |
| 11 | Cl. 28–31: local + global illumination, real-time | Generic | Present in form | Dependent of 1 |
| 12 | Cl. 32–33, 40–41: zones in non-real-time; "visibility mask that is pre-computed for each probe"; zones from planes, front/behind | Offline 430 | The bake stores depth moments, plus tiles for some probes. The selection score (largest \|Δvisibility\| toward any valid neighbour, threshold 0.1, cap 32,768) is a ranking key that is discarded: id 59 holds only indices and moments | Appears absent. The score is never stored and decides only data representation, never a probe's inclusion |
| 13 | Cl. 36: memory associating zones with probe sets, zones from planes | FIG. 8A + 8C | None | Appears absent |
| 14 | Sparse tile + reference vs per-probe plane and mask record | FIG. 8C: per-probe planes + mask | Per tiled probe: 64 bordered texels of (E[d], E[d²]), found through a sign-flagged index in the moments texel. The structural resemblance is only "per-probe side data found by index" | Appears absent; the content is a distance distribution, not zones or visibility bits |
| 15 | Pre-existing zero weights (not this change) | — | Weight is exactly 0 for: an invalid probe (inside solid; a per-probe flag independent of the point); a probe behind the receiver's geometric plane (`max(dot, 0)`, forward path only); trilinear 0 at a cell face. The L1 whole-cell path skips zero weights (`continue`) | Claim 37 is the only candidate. Validity: no zone and no point dependence. Backface: a half-space of the *receiver's* plane, not a zone "for the selected probe". Arguable-low; flagged because it is the only binary exclusion that depends on the point (lever 5) |

**What the floor's residue shows.** Behind the closed static door, station 3 reads 0.033 on this build (0.495 untiled; the spike measured 0.04 and 0.50), with a faint door-edge outline. That residual glow is the hidden probes' light, entering at ≥ 0.03 × trilinear and renormalised. It is observable evidence that every valid probe keeps a nonzero weight at runtime, where the patent's blind probe contributes nothing. It answers "do you cull?". It does not answer "do you determine zone visibility?" (row 6). Record the capture with the counsel packet.

## Prior art

Priority 2016-10-04. "On face" means cited on the '372 face, so the art was before the examiner. Status labels:
- **Verified:** read from the author's file or an archived copy of it.
- **Verified, 3rd-party copy:** read in full, but from a transcription or rehost.
- *(Snippet):* search-result or summarizer text only.

Dates are venue-level unless stated.

| Art | Date | Element | Key passage | Status |
|---|---|---|---|---|
| Sugden & Iwanicki, "Mega Meshes", GDC 2011 (Lionhead, *Milo and Kate*) — **on face** | 2011-02/03 | Per-probe per-direction occluder distance; visibility from probe to receiver; blind probes ignored (cl. 37 rows 1, 5, 6, binary) | "Each probe stores its area of influence – which in case of our 2d world is just a distance to the nearest occluder in every direction. So when we grab SH coefficients for an object, we check which of the four nearest probes can actually see the object and simply ignore the rest." (slide 63 notes) | Verified (Wayback of miciwan.com) |
| Cupisz, "Light Probe Interpolation Using Tetrahedral Tessellations", GDC 2012 — **on face** | 2012-03 | Per-probe baked visibility as common practice; runtime raycast probe selection | "Milo and Kate solves it by baking per-probe explicit visibility information, which limits each probe's influence up to the nearest obstacle. This seems to be what most games do nowadays." / "Some engines sample n nearest probes, raycast to test visibility and fade over time to avoid popping." | Verified, 3rd-party copy (sambuz transcription) |
| Tatarchuk, "Irradiance Volumes for Games", GDC-E 2005 — **on face** | 2005 (PDF dated 2005-09-12) | Probe grid, trilinear or distance-weighted sum (rows 1, 2, 9); per-probe depth cubemap (used for subdivision, not weighting) | "Interpolate surrounding samples (trilinear) – Find a weighted sum of surrounding samples (weighted by 1/distance)" / "render the scene into each face of a floating point cubemap. The scene should be drawn with a shader that outputs: 1/depth" | Verified (Wayback of ati.com) |
| Greger, Shirley, Hubbard, Greenberg, "The Irradiance Volume", IEEE CG&A 18(2) | 1998-03/04 | Grid of regions, 8-corner interpolation (rows 1, 2) | "A cell represents a box in space bounded by eight samples, one at each of its corners." / "interpolate to get the irradiance." | Verified (Wayback of cs.utah.edu) |
| Donnelly & Lauritzen, "Variance Shadow Maps", I3D 2006 | 2006-03 | Depth moments + Chebyshev bound as continuous visibility, our exact formula (row 5 mechanism) | "we store the mean and mean squared of a distribution of depths … Using the variance, we derive an upper bound on the fraction of a shaded fragment that is occluded." / "P(x ≥ t) ≤ pmax(t) ≡ σ²/(σ² + (t − µ)²)" | Verified (rehosted PDF) |
| Lauritzen, GPU Gems 3 ch. 8 | 2007 | Same; "If t ≤ μ, pmax = 1 and the surface is fully lit." | "apply Chebyshev's inequality to compute an upper bound on the probability that the currently shaded surface (at depth t) is occluded" | Verified |
| Laine et al., "Incremental Instant Radiosity", EGSR 2007 | 2007-06 | Per-point (VPL) omnidirectional depth looked up toward each receiver | "performs the shadow lookups from the paraboloid maps of the assigned VPLs, and computes the incident illumination based on position and normal of the pixel" | Verified (garbled text layer; clean fragments only) |
| Ritschel et al., "Imperfect Shadow Maps", SIGGRAPH Asia 2008 | 2008-12 | Same, per VPL, binary test | "We propose to use imperfect shadow maps to determine visibility for each VPL" | Verified (KAIST copy) |
| Kontkanen & Laine, "Ambient Occlusion Fields", I3D 2005 | 2005-04 | Per-direction field around a point looked up by receiver direction | "these fields are stored as radial functions into a cube-map surrounding the occluder." | Verified |
| Annen et al., "Exponential Shadow Maps", GI 2008 | 2008-05 | Prefiltered continuous shadow test | "approximating the shadow test using an exponential function. This enables us to pre-filter the shadow map" | Verified |
| Sloan, Kautz, Snyder, PRT, SIGGRAPH 2002 | 2002-07 | Transfer on a 3-D grid around an object | "precomputes the transfer matrix … at each point within a simple 3D grid surrounding O." | Verified |
| Engelhardt & Dachsbacher, "Octahedron Environment Maps", VMV 2008 | 2008 | Octahedral direction parameterization (our tile layout) | "we examine the octahedron as a parameterization scheme to represent environment maps … a means for storing directional data in general." | Verified, 3rd-party copy |
| Gilabert & Stefanov, Far Cry 3 DRTV, GDC 2012 | 2012-03 | Grid of regions → probe (row 2); bleeding fixed by authored volumes; per-probe sky-visibility function | "we create a 3D grid. The grid's cells are a single byte that represents the index of the nearest probe" / "the so-called probe bleeding … closed 3D volumes that our artists can place" | Verified, 3rd-party copy |
| Mitchell, McTaggart, Green, "Shading in Valve's Source Engine", SIGGRAPH 2006; Source SDK 2013 `bspfile.h`, `leaf_ambient_lighting.cpp` | 2006 / 2013 | Probe sets per BSP leaf (a region bounded by planes, but the leaf's, not the probe's); inverse-square weighted, normalised | "each leaf contains N samples of the ambient lighting" / "do an inverse squared distance weighted average of the samples to reconstruct the original function" | Verified |
| Hooker, "Volumetric Global Illumination at Treyarch", SIGGRAPH 2016 Advances | 2016-07 (before priority) | Leak and visibility problem named; "Planes" listed as a rejected voxel datum; authored convex volumes instead | "Visibility Is A Problem • Where the probe doesn't see" / "Common Approach: Adjust trilinear Based on normal [SILVENNOINEN15]" / "More Voxel Data: Planes, Signed distance field, Bad artifacts" | Verified (slides; image-heavy) |
| Silvennoinen & Timonen, Quantum Break, SIGGRAPH 2015 Advances | 2015-08 | Per-voxel probe visibility by traced rays (reflection probes) | "trace visibility rays back to the reflection probes" / "Store best reflection probes in the voxel" | Verified |
| Unity 5.4 Light Probe Proxy Volume docs | 2016 (month unconfirmed) | Leaking admitted; position offset only | "some of the interpolated Light Probes pass through walls or other geometries and cause light leaking." | Verified |
| UE4 Indirect Lighting Cache | ≈2014 | Leaking from sparse samples, no visibility | "Placed with high-density only on top of static surfaces resulting in leaking from low-density samples" | *(Snippet)* |
| Iwanicki, *The Last of Us*, SIGGRAPH 2013 | 2013-07 | Negative: no probe visibility; analytic and 3-D-texture occluders only | — | Verified (negative) |
| **Context, after priority:** McGuire, Mara, Nowrouzezahrai, Luebke, "Light Field Probes", I3D 2017 | PDF built 2016-12-21; NVIDIA page 2017-02-01; no earlier preprint found | Our design almost exactly: per-texel radial-distance moments, a Chebyshev test, × trilinear, normalised | "we combine the trilinear weighing with … a Chebyshev visibility test of the local radiance distance distribution, using the two filtered moments as spherical analogues of a variance shadow map test … We normalize the result by the sum of all eight weights." | Verified |
| **Context, after priority:** Majercik et al., DDGI, JCGT 8(2) | 2019 | Chebyshev-weighted probe interpolation, the basis of the brief | "we apply mean- and variance-biased Chebyshev interpolants, as detailed in the variance shadow-mapping method, to our visibility queries" | Verified |

**Reading of the art.**
- **Elements 1 and 2 are old.** Grid, per-region probe sets and distance-weighted normalised blending: Greger 1998, Tatarchuk 2005, Source 2006, Far Cry 3 2012.
- **Per-probe visibility with binary exclusion was common before 2016.** Mega Meshes and Cupisz both describe it, and both were on the face. The patent's contribution over them, as allowed, is the plane-bounded zone and mask encoding.
- **The Chebyshev mechanism dates from 2006.** Moments looked up per direction at a point date from 2005–2008. No pre-priority reference combines them for probe weighting; that combination first appears in McGuire (Dec 2016 / Feb 2017), after the priority date.
- **Not found:** any pre-2016 cube or omnidirectional VSM source, the Far Cry 4 slides (gated), any earlier Light Field Probes tech report, and primary Epic docs (403).

## Design levers

Each lever names the claim language it moves away from. None costs quality at today's settings unless stated.

1. **Keep the hardware-bilinear tile tap. Never use `textureLoad` or a nearest sampler for tiles.** A nearest lookup picks one texel per point, which is closer to "determining which of a plurality of zones … contains the selected point" (row 4). Bilinear returns a continuous blend, so no single cell is chosen. Today's `sh_atlas_sampler` is Linear (`sh_volume.rs`). Pin that with a test, or a comment at the sampler, that cites this doc.
2. **Keep distance inside the visibility function.** The decisive distinction from "visibility of said determined zone" is that our weight depends on where along the direction the point sits (row 5). Never precompute, per probe or per texel, a "blocked" flag, a "visible" bit, a threshold distance turned into a bit, or a per-direction yes/no mask, even as an optimisation. Such a mask reads toward claims 6–9 and 42–44 and toward a "visibility" determination. Store moments, never verdicts.
3. **Floor: keep 0.03 until counsel answers. Never 0.**
   - At 0, the L1 whole-cell path's `if (outer_weight <= 0.0) continue;` would drop the probe literally, with no normaliser share: the patent's blind probe.
   - Lowering toward 0.003 stays continuous mathematically, but a 0.3% share beside weights near 1 is exclusion in substance and invites a doctrine-of-equivalents argument.
   - The spike's 0.003 gain (station 3 from 0.04 to about 0.03) is small. Since claim 11 suggests the floor is not the escape, the floor's value is evidentiary: it makes the "every probe contributes" fact visible.
4. **Remove the floor from the argument, not from the code.** The argument should rest on rows 4–5 (no zone, point-distance-dependent visibility), with the floor as corroboration. Nothing to build.
5. **Backface factor: consider a floored or smooth form.** `max(dot, 0)` is the one point-dependent binary zero in the blend (row 15). It is pre-existing and not a probe-zone test, but under claim 37 it is the easiest thing to call a determination of visibility. A DDGI-style smooth weight such as ((dot + 1)/2)² + small ε never reaches 0. This costs some backface leak and needs a capture A/B, so the owner should decide. Validity 0 (a probe inside solid) is point-independent and can stay.
6. **Never store per-probe planes, half-space tests or region-to-probe tables.** That rules out an occlusion plane per probe (already a rejected rival in the brief), FIG. 8A-style stored probe sets per region (claims 4, 36), and zone codes. Keep corner lookup arithmetic.
7. **Keep tile selection a representation choice.** The score ranks candidates and is discarded. A probe without a tile still weighs by averaged moments. Never let tile presence or absence gate a probe's inclusion, and never ship the score. Later mover tiles (the probe channel) should switch or blend *moment sets* by door state, never a per-probe "occluded" flag or a weight forced to zero when a door closes.
8. **Describe it as it is.** `rendering_pipeline.md` §Indirect says "Surviving weights are renormalized" and "backface rejection". Both refer to validity and backface, not depth visibility. When those lines are next edited, say "every valid probe keeps a Chebyshev weight ≥ 0.03" so internal docs match the code. This is accuracy, not concealment.

## Questions for counsel, ranked

These sharpen ip-check.md item 8.

1. **Claim 37 construction.** Does "determining which of a plurality of zones for the selected probe contains the selected point" read on encoding the probe-to-point direction and bilinearly sampling a per-probe octahedral tile of depth moments, where no zone index is produced? Does "determining visibility of said determined zone" read on a continuous Chebyshev bound that depends on the point's distance, so that one direction yields different values for near and far points? The description defines a zone by plane signs, with one stored visibility bit per zone and a sighted/blind outcome.
2. **Claim differentiation and the floor.** Claim 11 ("the number of probes in the set is greater than the number of sighted probes") suggests claims 1 and 37 cover the case where every probe is sighted. Does a floored continuous weight (no probe ever excluded) then carry any weight against the combining element, or only as evidence that no visible/not-visible determination exists?
3. **File wrapper 15/377,913.** Confirm that the occlusion planes were added to claim 1 to overcome art, and why A1 claim 5 ("grid of regions") made claim 37 allowable. Check whether the applicant argued over Mega Meshes or Cupisz (per-direction occluder distance, probes ignored) by pointing to zones, which would bear on reading "zone" toward per-direction depth. Check for prosecution-history estoppel on the plane-based claims. ppubs anonymous access ends 2026-11-07, so Patent Center is the route.
4. **Octahedral texel cones.** Within an octant, each tile texel's cone is bounded by planes through the probe centre. Could a fixed direction parameterization shared by all probes be "zones … defined by intersections of pre-defined occlusion planes for the selected probe"? Our reading is no: not occlusion planes, not per probe, not tested against the point, and not a unit of visibility.
5. **Validity of claim 37** over art not on the face: Greger 1998 (grid, 8 corners), Source 2006 (per-leaf sets), Far Cry 3 2012 (grid → probe), VSM 2006 (Chebyshev visibility), the per-VPL directional depth lookups of 2007–2008, and Hooker 2016 (pre-priority, planes considered). Also, combined with the on-face art Mega Meshes 2011 and Cupisz 2012.
6. **Backface and validity zeros.** Does `max(dot(probe − point, receiver normal), 0)`, a half-space of the receiver's plane, risk being read as a zone determination under claim 37? Does that justify lever 5?
7. **Floor level and equivalents.** Is there a floor below which a continuous weight is treated as exclusion in substance (0.03 today, 0.003 tested)?
8. **Ownership and status.** Confirm the current owner after Embracer's 2022 acquisition of Eidos Montréal (the record shows assignment to Square Enix Limited, UK, 2018). Confirm the 2037-02-01 adjusted expiry. Confirm that no reissue, reexamination or IPR exists. JP6970508B2 and EP3306569B1 matter only if the engine is distributed there. EP claims all carry planes.
9. **Future mover tiles** (probe channel). Is switching a probe's tile reference by door state, while still a continuous moment set, any closer to "a visibility mask … pre-computed for each probe" (claim 33) than a static tile is?

**Corrections to ip-check.md** (applied 2026-11-07, with the design rules and prior-art summary):
- Piece 2 row: claim 1 includes the occlusion-plane limitation, not just the abstract text; claim 37 is the plane-free claim.
- Filings table: "filed 2016-12-14" is the EP date; the US was filed 2016-12-13, with CA priority 2016-10-04.
- Status: expiry ≈2037-02-01 (Google adjusted); 8th-year fee paid 2026-05-20.
- Source column: full claims verified 2026-11-07.
