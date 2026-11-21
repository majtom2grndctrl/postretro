# glossy-world-reflections — IP check: glow delta tiles

> **Engineering due diligence, not legal advice. 2026-11-21.** A technical comparison of published claims and dated prior art against a proposed technique, to brief the owner and a patent attorney. "Appears present", "appears absent", "arguable" and "reads on" are engineering readings of claim text. They are not opinions on infringement, validity or freedom to operate, which are counsel's. Every negative search result means "not found", never "clear".

Compared against: the brief (`index.md` here), `rendering_pipeline.md` §7.3a (Live lighting), `resource_management.md` §5 (capture shading at the reference state). Builds on `plans/done/reflection-probes/ip-check.md` (Tencent WO chart, §4 fences), `context/research/gi-rtx-lite/ip-check.md` (§Tencent US12293451B2 and its design rules), `plans/done/reflection-probes/research.md` §Prior art and `plans/drafts/reflection-probe-fluid-pass-performance/ip-research.md`.

**Sources and confidence.** Claim text came from Google Patents pages through a summarising fetch, which returns paraphrase, not verbatim claims. Each row's *Source* column says which. Google Patents' search endpoint rate-limited (HTTP 503 "Sorry") after two queries, so there was no CPC- or assignee-filtered sweep. `cs.dartmouth.edu`, `www.activision.com` and `history.siggraph.org` are outside this session's network allowlist, so the UberBake paper and the Infinite Warfare decks were read only through search-result excerpts. Filings from the last 18 months are invisible to any search.

## Technique under review

Per probe, the compiler bakes one sparse **delta tile** per animated glow source: an `area-lights` glass face or an `emissive_volume`'s faces. The tile holds that source's glow as the probe sees it at unit strength, stored as 8×8 blocks with prefiltered mips. Stored tiles are never rewritten.

| Variant | Runtime | Base tile |
|---|---|---|
| **S-A** (sample-time, base at reference) | Fragment samples base + each relevant delta and adds (current_s − ref_s) × delta_s | Today's capture: glow at its reference state (`resource_management.md` §5) |
| **S-B** (sample-time, base without animated glow) | Fragment adds current_s × delta_s | Capture with animated glow removed |
| **C** (compose, rejected) | A pass writes base + Σ deltas into a live tile when a source changes; fragments sample the live tile | Either |

In every variant, cast light keeps today's post-sample ratio, 1 + Σ_l share_l(d) × (current_l / ref_l − 1).

S-A and S-B are the same function of current_s; they differ in what is stored and in how a claim's wording maps onto them (§2).

## Documents found

| Document | Applicant | Priority | Published / granted | Status as shown (Google Patents, assumed) | Source |
|---|---|---|---|---|---|
| WO2026113651A1 (PCT/CN2025/126544) | Tencent | CN 2024-11-28 | WO 2026-06-04 | Pending; US national phase ≈2027-05-28, EP ≈2027-06-28 | Claims 1, 10–14, 19 re-read 2026-11-21 (machine translation, summarised) |
| CN122115679A | Tencent | 2024-11-28 | 2026-05-29 | Pending; publication only | Claims 9–12 re-read 2026-11-21 (summarised): CN 9 ≈ WO 10, CN 10 ≈ WO 11–12, CN 11 ≈ WO 13, CN 12 ≈ WO 14. No CN counterpart to WO 19 was seen |
| US12293451B2 "Illumination rendering method…" ('451) | Tencent | CN 2021-04-02 | Granted 2025-05-06 | Active; ≈2043-01-19 | Full claims, from the 2026-11-03 read in `gi-rtx-lite/ip-check.md`; not re-fetched |
| US11605198B2 "…dynamic real-time high-quality lighting for digital animation" (from US20200005525A1) | Baobab Studios (small entity) | US prov. 62/692,594, 2018-06-29 | A1 2020-01-02; granted 2023-03-14 | Active per Google; anticipated expiry 2039-06-28. Allowed 2022-11-14 after three final rejections. WO2020006505A1 ceased; EP and DE non-entry; no continuation listed. 3.5-year fee window opens about 2026-03 and closes about 2026-09-14, or 2027-03-14 with surcharge. Payment not seen | Claims 1–3, 6, 10, 12 summarised with quoted fragments, 2026-11-21 |
| US8368693B2 (from US20100045676A1) "…fast real-time rendering of multiple light sources" | Take-Two Interactive | 2008-08-20 | Granted 2013-02-05 | Active per Google; adjusted expiry 2031-03-11 | Independent claims 1, 10, 19 summarised |
| US6078332A "Real-time lighting method using 3D texture mapping" | SGI (now Microsoft) | 1997-01-28 | Granted 2000-06-20 | **Expired** 2017-01-28 | Claims 1, 18 summarised |
| US7262770B2 (from US20030179197A1), Sloan, Snyder & Kautz PRT | Microsoft | 2002-03-21 | Granted 2007-08-28 | **Expired**, fee related; adjusted expiry 2024-05-26. Continuation US7609265B2's status not seen | Claims truncated on the page; the description covers glossy transfer |
| CN112215938B | NetEase | 2020-10-14 | Granted 2024-02-23 | Active; ≈2040-10-14 | Claim 1 summarised |
| CN113034656B | Perfect World | 2021-03-30 | Granted 2022-03-22 | Active | Claim 1 summarised |
| CN117218273A | Kingsoft (Zhuhai) | 2023-09-26 | 2023-12-12 | Pending | Claim 1 summarised |
| US20210012562A1 | NVIDIA | 2019-07-11 | 2021-01-14 | Abandoned 2023-08-23 (per prior record) | Prior record |

URLs: `https://patents.google.com/patent/<number>/en` for each.

Not found: any filing that claims per-light or per-emitter delta environment maps or reflection probes summed at runtime, sparse delta cubemaps, or emissive animation in reflection probes. The searches were Google Patents full text (two queries before rate limiting), web search in English and Chinese, and citation lists on the pages above. This is weak evidence of absence, given the gaps.

## 1. Tencent WO2026113651A1 / CN122115679A

Claim 1 reads as `plans/done/reflection-probes/ip-check.md` §1 charts it: parallax-corrected probe reflection on a textured surface, which glossy world faces already widen. Delta tiles do not change that reading, and the brief already gates build on it. The update chain is the one delta tiles touch.

| Claim (WO / CN) | Element (paraphrase) | S-A / S-B (sample-time sum) | C (compose) |
|---|---|---|---|
| 10 / 9 | When an update condition holds, **update the illumination information the probe has collected**, then read the reflected light from the updated data. Conditions: relative position to objects in range changes, or light at those objects changes | Nothing stored is written. Base and delta tiles are fixed after compile; each fragment combines them at lookup. **Appears absent as built**, on the same footing as the shipped ratio. It is the closest element: the combined value is direction-resolved probe radiance, exactly what an updated probe would hold, so this sits nearer the claim than a scalar ratio. Counsel | Writes a live tile when a glow source changes. **Appears present**, if a glow change is a "change in light at objects in the target range". An emissive face changing its own emission is arguably that; counsel |
| 19 / — | Before collection, if the scene's illumination was updated, take the reflected light from the updated data | Same as 10: no updated data exists, only fixed tiles combined per fragment. Appears absent; counsel | Appears present (same as 10) |
| 11–12 / 10 | Relative-position change: refresh texture (G-buffer) information, then recompute colour from it | No position change, no stored G-buffer. Appears absent | Appears absent |
| 13 / 11 | Light change: **keep existing texture information and recompute colour information from it** | No recompute. Appears absent | Arguable, leaning absent. Compose recomputes colour from stored *colour* tiles, not from texture or G-buffer information. A broad reading of "texture information" as any stored image is counsel's |
| 14 / 12 | Polyhedral probe; each face records local illumination; faces updated in turn, at least one update period apart | No update. Appears absent | Absent while compose writes the whole tile in one pass. **Moves toward 14** if compose is amortised by octant or face across frames: an octahedral map is unfolded from an octahedron, a polyhedron |

**Base variant.** S-A has the base reflect the reference state, corrected at lookup by a change term (current − ref). In wording, that sits nearer "update the collected information when light changes" than S-B's composition from parts. Neither writes anything, so the difference is one of framing, not an element.

## 2. Tencent US12293451B2 ('451)

Claim 1, verbatim from the prior record: "(a) determining light source change information when a light source changes … (b) determining a current light source projection coefficient corresponding to the changed light source … (c) determining an indirect illumination value of a target pixel point … according to a radiance transfer parameter corresponding to the target pixel point … and the current light source projection coefficient; (d) determining a direct illumination value … under the changed light source; and (e) performing illumination rendering … according to the direct illumination value and the indirect illumination value." Claims 9 and 17 recite the same steps.

| Element | Delta tiles (S-A, S-B; C the same) | Reading |
|---|---|---|
| (a) light source change | Area-light glass: its glow follows its light's descriptor, so the light changes. `emissive_volume`: a scripted change in a face's emission. Volumes cast no light unless emissive bounce lands (parked, `gi-rtx-lite/index.md` row 12) | Area light: **leans present**, as for id 27 (col. 5 lists intensity change). Volume: arguable, whether an emitting surface that casts no light is a "light source"; the description does not discuss emissive surfaces |
| (b) current projection coefficient | current_s (S-B) or current_s − ref_s (S-A): a scalar or RGB per source | **Arguable; the same crux as id 27.** Paras. 32 and 35 and FIG. 4 support reading a per-light intensity scalar as the coefficient. Per the prior record, a scalar or RGB term is no design-around. S-A's multiplier is literally derived from the change, as (b) words it |
| (c) indirect value from a per-pixel radiance transfer parameter × coefficient | Delta tile = the source's unit-strength radiance per direction from the probe, with capture visibility and prefiltered blur. Read per fragment through parallax correction. Glow-only: no interreflection off other surfaces | **Arguable, nearest-risk element.** For: the description stores transfer "per probe face" (cols. 20, 24) and says it does not change when the light changes (col. 17), which a fixed unit-strength delta matches. Against: the claim's indirect is interreflection; a glossy texel reflecting an emitter receives one-bounce light *from the source*, arguably direct specular, not indirect. Engines call probe reflection "indirect specular". Counsel |
| Dependent 3–4 / 5–6 | Static pixel: transfer from a lightmap. Dynamic-object pixel: transfer from a light probe, with per-direction probe brightness updated, interpolated and evaluated with the normal | Glossy world faces are static, yet read a probe, not a lightmap: 3–4 appear absent (no lightmap holds the delta), and 5–6 appear absent (no dynamic object). Neither changes claim 1 |
| (d), (e) direct under the changed light; render both | The area light's direct reaches the floor through the animated lightmap. The brief draws no highlight from the area light | Present, as for id 27 |

**Base variant.** S-B (base without glow + current × unit delta) is the literal superposition FIG. 4 describes: transfer × coefficient per light. S-A (reference base + change × delta) equals the shipped ratio's algebra, `L_rest + Σ (share_l·L)(b_l / b̄_l)` (`plans/done/reflection-probes/research.md`), at image resolution instead of five lobes. Both break the prior record's high-confidence rule: "No baked quantity that stays fixed while a light changes is multiplied or weighted at runtime by a value tied to that light's state". No base choice escapes it. S-A at least adds no shape that shipped water does not already have.

**Carried observation.** The shipped water ratio (baked per-light, per-probe lobe shares × current_l / ref_l) has the same '451 shape at lobe resolution. The repository's '451 claim charts cover id 25, 27, 34, 45 and the mover channels. The fluid-pass IP research names '451 against the ratio but did not read its claims. No claim-level chart of the ratio against '451 was found in `context/`. Delta tiles extend that exposure to glow; they do not open a new one.

## 3. Baobab US11605198B2

| Element (claim 1, summarised; quoted fragments) | Delta tiles | Reading |
|---|---|---|
| Receive two **static lighting setups**; precompute a GI set for each | Per-source unit deltas, not whole precomputed setups | Arguable. A delta equals the difference of two setups (source on, source off) |
| Each set includes light-probe data with "a first set of spherical harmonics coefficients" and "lightmap data" | The reflection delta alone has neither. The engine elsewhere carries per-light SH deltas (id 27) and animated lightmap weights | Absent for the reflection path alone. Whether claim 1 is read against the engine as a whole is counsel's |
| Change a weight over time (cl. 6) | current_s from script or curve | Present |
| Determine that one set is lower-resolution and, "in response", up-sample it before blending | Delta blocks stored at base resolution per mip; sparsity drops blocks, not resolution | **Appears absent**, if deltas never sit at a lower resolution than the base. Avoid sub-resolution deltas that are up-sampled to blend |
| Blend by weight via "interpolation of at least one spherical harmonics coefficient" between co-located probes | Additive sum, not interpolation between two absolute sets | Arguable by equivalence: lerp(A, B, w) = A + w(B − A) = base + w × delta |
| Cl. 2–3: blend reflection probe data by "interpolation of cube map pixels", probes co-located | Base and delta tiles share one probe and capture point | Closest element. Reachable only through claim 1, which requires SH, lightmap and up-sampling elements |

Reading: claim 1's SH, lightmap and up-sampling elements sit outside the reflection technique, and the up-sampling element appears absent. The prior record rated Baobab low; that stands.

## 4. Other filings, dismissed or noted

| Filing | What it claims (summarised) | Reading |
|---|---|---|
| Take-Two US8368693B2 | Several lights' data in one texture's channels; a per-light, analytic Blinn-Phong specular | Precomputed attenuation per light, not environment or reflection data. Appears absent |
| SGI US6078332A (expired) | Per-light 3D light-intensity textures; claim 18 sums the maps of lights that are on into a resultant map, then renders | **Compose-variant art** (1997). Expired |
| Microsoft US7262770B2 (expired) | PRT, including glossy transfer convolved at runtime | Art for '451 (c) and for a glossy per-source transfer. Expired. US7609265B2's status not seen |
| NetEase CN112215938B | A self-luminous object stands in for a point light; local reflection probes nested by priority bake its reflected light | Emissive in a reflection probe bake, but static: no runtime change, no per-source data. Appears absent |
| Perfect World CN113034656B; Kingsoft CN117218273A | Viewpoint-updated probe grids with CPU-built indirection textures; distributed voxel baking | Different subject. Appears absent |

## 5. Prior art (dated, public)

Tencent WO priority is 2024-11-28; '451 priority is 2021-04-02; Baobab priority is 2018-06-29. Closeness is to sample-time summing of baked per-source deltas.

| Technique | Date | Mechanism | Closeness | Evidence |
|---|---|---|---|---|
| Dorsey, Arvo & Greenberg, "Interactive Design of Complex Time-Dependent Lighting", IEEE CG&A 15(2) | 1995 | Time-varying lighting as linear combinations of static images, one per lighting condition, weighted by current intensity | **The same math** (per-source baked image × current scalar, summed), in screen space, not a probe | Search excerpt and prior record |
| Nimeroff, Simoncelli & Dorsey, "Efficient Re-rendering of Naturally Illuminated Environments", EGWR | June 1994 | Mixing precomputed basis images by the linearity of rendering in illumination (sun and sky) | Same math; basis, not per source | Search excerpt (abstract level) |
| Haeberli, "Synthetic Lighting for Photography" (Grafica Obscura) | 1992 | Photographs under each light, summed with weights | Same math | From memory, not re-verified |
| Quake lightstyles (`gl_rsurf.c`) | 1996 | Up to 4 styled lightmaps per surface × style value, summed into a block when built | Per-source baked layers × scalar, **composed** into a cache: art for variant C | Verbatim in the prior record |
| SGI US6078332A | Priority 1997 | Per-light 3D maps summed for the lights that are on | Compose (C) | Patent text, summarised |
| Debevec et al., "Acquiring the Reflectance Field of a Human Face", SIGGRAPH | 2000 | Relighting as linear combinations of one-light-at-a-time images | Same math | From memory |
| Sloan, Kautz & Snyder, PRT, SIGGRAPH; Kautz, Sloan & Snyder, arbitrary-BRDF glossy, EGWR | 2002 | Precomputed transfer × runtime lighting coefficients; glossy view-dependent transfer | '451 (c) shape; glossy precedent | Prior record (verified) |
| q3map2 2.5.11 `-bouncegrid` | 2003-12-01 | Bounce baked per light style into grid probes | Per-source probe layers | Verbatim in the prior record |
| Lazarov, Black Ops (2011), Black Ops II notes, SIGGRAPH | 2013 | Reflection probes normalised and rescaled by local irradiance | Post-sample ratio lineage | Search excerpt |
| Enlighten, Martin & Einarsson, SIGGRAPH Advances | 2010 | Runtime radiosity also refreshes cubemaps (box-projected, about 8² per face, mips regenerated) | Runtime rewrite of reflection data: art for C and WO 10 | Search excerpt (SDK docs 3.10–4.03; doc dates not seen) |
| Far Cry 3 deferred radiance transfer volumes, GDC | 2012 | Probes relit on the fly for time of day | Relit probes (GI, not reflections) | Prior record |
| Unity 5 realtime reflection probes, `IndividualFaces` time slicing | 2015 | Probe re-rendered at runtime, faces in turn | Art for WO 14 | Prior record (verified) |
| McAuley, "Rendering the World of Far Cry 4", GDC | 2015 | Mini G-buffer per cubemap, relit at runtime, mips regenerated | **Art for WO 11–13** (re-shading stored texture information) | Secondary sources (FFXV SIGGRAPH 2016 talk, Monster Hunter Online slides) |
| Infinite Warfare, Activision rendering talk, SIGGRAPH | 2017 | "Relightable" reflection probes: SH luma stored per probe, rescaled to the lightgrid at shading; "Delta Light SH2 from probes" added to ambient | Probe relighting by stored per-probe data at shading time; per-light probe deltas, though in SH | Search excerpt |
| UE4 lighting scenarios; reflection captures in BuildData from 4.19 | 2018 | Swap whole baked states, captures included | Absolute-state selection, not a sum | Epic forum and docs excerpts |
| Baobab US20200005525A1 (published application) | 2020-01-02 | Weighted blend of precomputed GI sets, reflection-probe cube-map pixels included | Reflection-domain per-state blend by a runtime weight | Patent text |
| Seyb, Sloan, Silvennoinen, Iwanicki & Jarosz, "The Design and Evolution of the UberBake Light Baking System", ACM TOG 39(4) | July 2020 | Each light's or light set's GI contribution "controlled individually at run-time"; sparse storage for texels touched by up to 8 dynamic light sets; fast runtime combination | **Closest game art**: sparse per-light baked deltas × runtime intensity. Predates '451's priority. Whether reflection probes carry light-set deltas was not seen | Search excerpt only (abstract, §3.3 fragment) |

**Strongest dated art.** For the math: Dorsey, Arvo & Greenberg 1995, with Nimeroff 1994. For per-source sparse deltas in a shipped game baker: UberBake (July 2020) and Baobab's application (2020-01-02). Both predate '451's 2021-04-02 priority, and none of these was before '451's examiner (prior record). Everything here predates Tencent WO. No publication found stores per-emitter delta **reflection** tiles and sums them at lookup. That gap is where the technique is new in detail, which bears on novelty, not exposure.

## 6. Implementation choices that move the reading

| Choice | Moves | Why |
|---|---|---|
| Sample-time sum (S) instead of compose (C) | Away from WO 10/19 (C appears present); away from 14 | Nothing stored changes; the existing "tiles never rewritten" fence holds |
| Compose amortised by octant or face | Toward WO 14 | Faces updated in turn over a period |
| Deltas at base resolution, sparse by block omission | Away from Baobab's up-sampling element | Sub-resolution deltas up-sampled to blend would supply it |
| Dense vs sparse | Neutral for Tencent and '451 | UberBake's sparse light-set storage is art either way |
| Glow-only deltas | Keeps '451 (c) arguable | The tile holds the emitter's own image, with no interreflection |
| Cast-light deltas (the light's effect on the room, per source, in the tile) | **Toward '451 (c)** squarely | Per-light unit bounce × intensity, probe-resolved: the id-27 shape at image resolution |
| Cast light on today's ratio | Unchanged exposure | Already '451-shaped at lobe resolution; not yet charted (§2) |
| S-A (reference base + change × delta) | Same function as S-B; adds no shape that water lacks; keeps base tiles byte-equal to today (water parity) | Wording nearer WO 10's "update" and '451 (b)'s "change information" |
| S-B (base without glow + current × unit delta) | Literal '451 FIG. 4 superposition | Also changes today's base tiles and the §5 reference-state contract |
| `emissive_volume` sources, not area lights | Away from '451 (a), while volumes cast no light | Landing emissive bounce would make a volume a light source |
| Cap on deltas per probe; sources past it hold their reference | Neutral | Same rule as lights past the stored few |
| Fallback past the cap through a lobe-aggregate glow share (`research.md` §Volume glow share) | Same '451 shape as the ratio | Linear in current_s |
| Fallback past the cap through a runtime re-capture or compose | Toward WO 10–13 | Rewrites the probe |
| G-buffer stored in probes for relighting | Toward WO 11–13 (Far Cry 4 lineage) | Do not store it |
| Event-triggered vs per-frame evaluation | Not a lever for '451 (prior record); for WO 10 it matters only if something is written | — |

## 7. Design fences

| | Rule | Avoids | Note |
|---|---|---|---|
| **Keep** | Base and delta tiles written once, at compile; no pass writes the probe arrays after install (the brief's source gate) | WO 10, 19, 13–14 | Extends the brief's existing gate to delta arrays |
| **Keep** | Deltas carry glow only; cast light stays on the existing ratio | '451 (c) widening | — |
| **Keep** | Deltas at base resolution per mip; sparse by block omission | Baobab up-sampling | — |
| **Keep** | No G-buffer, normal or albedo stored per probe | WO 11–13 | — |
| **Prefer** | S-A over S-B | Keeps water parity and today's reference-state contract; adds no new shape | IP difference between them is framing only |
| **Avoid** | Compose into a live tile (variant C), at any cadence | WO 10, 19; 14 if amortised | Rejected already |
| **Avoid** | Per-source cast-light deltas (bounce or room lighting in the tile) | '451 (c), cl. 5–6 analogue | The id-27 shape |
| **Avoid** | Runtime or load-time re-capture as a fallback | WO 10–13; `resource_management.md` §9 | — |
| **Watch** | Glow deltas for area lights | '451 (a)–(c), the same counsel question as id 27 | Build gate: counsel on '451 |
| **Watch** | `emissive_volume` deltas once emissive bounce lands | '451 (a) | A volume then casts light |
| **Watch** | The shipped water ratio | '451 claim 1 | Uncharted at claim level (§2) |
| **Watch** | Tencent WO national phase: US ≈2027-05-28, EP ≈2027-06-28; CN examination | Claims 10/19 may be amended | Re-read the claims as entered |
| **Watch** | Baobab's 3.5-year maintenance fee (grace to ≈2027-03-14) | Claims 1–3 | A lapse would end it |

## 8. For counsel

1. '451 claim 1 against glow delta tiles (either base variant). Is an emitting surface's own emission a "light source", in particular an `emissive_volume` that casts no light? Is the reflection of an emitter in a glossy face "indirect illumination"? Is a fixed per-probe, per-source directional delta a "radiance transfer parameter"? This is the same question already open for id 27, and for the shipped water ratio, which has not been charted.
2. WO 10 / CN 9 and WO 19: is combining several fixed stored tiles at lookup "updating the illumination information the probe has collected"? Contrast compose, which appears to read on 10 if a glow change is a "change in light".
3. WO 13 / CN 11: under compose, is recomputing colour from stored colour tiles recomputation "from texture information"?
4. Baobab US11605198B2 claims 1–3: interpolation versus base + weight × delta (equivalents); whether claim 1 is read against the engine's other per-light SH and lightmap data; maintenance-fee status.
5. Validity art for '451 not before its examiner: UberBake (July 2020), the Baobab application (2020-01-02), Dorsey, Arvo & Greenberg 1995, Nimeroff 1994, and the art already listed in `gi-rtx-lite/ip-check.md`.

**Not verified.** No verbatim or certified claim text for any filing; WO and CN came through a machine translation, summarised. Not checked: WO 19's CN counterpart, Baobab's fee payment, the status of Microsoft continuation US7609265B2, and the status of Take-Two US8368693B2's fees. UberBake's treatment of reflection probes and its runtime combination path are unread (host blocked). The Infinite Warfare and Far Cry 4 details come from secondary sources. Haeberli 1992 and Debevec 2000 are cited from memory. There was no assignee or CPC sweep (Google Patents rate limit), and Patentscope, Espacenet and CNIPA were not tried, given prior refusals recorded in `gi-rtx-lite/ip-check.md`.
