# reflection-probe-fluid-pass-performance — IP research

> **Research for the owner and counsel, not legal advice.** It describes resemblance and difference in technique only; it draws no infringement conclusions.
> **Method:** web research on 2026-10-17. Claim text was read through Google Patents page text, mostly by a summarizing fetch, so claim wording here is paraphrase. Counsel should work from certified claims.
> **Coverage gaps:** no PACER, Docket Navigator, PTAB or Patent Center access. "No litigation or PTAB found" means only that public web search surfaced none.
> **Spot-check:** the avoid-list attributions below were verified against the predecessor's files on 2026-10-17.

## Avoid-list sources
- `plans/done/reflection-probes/ip-check.md` lists Tencent WO2026113651A1 / CN122115679A and NVIDIA US20210012562A1.
- The earlier handoff patent scan (`plans/done/reflection-probes/research.md`, `distance-proposal.md`) adds:
  - Microsoft US10586375B2
  - ARM US10636213B2
  - the EA (Uludag) SSR-to-ray-trace family
  - Autodesk US9355475B1
  - for the live-lighting form, notes on Baobab and Tencent US12293451B2

## Lever A: per-probe live-lighting precompute

**Concern.** Per-probe, per-frame recomputation of lighting state may resemble "updating stored lighting" (Tencent claims 10/19) or per-light-coefficient shapes.

**Technical framing.** The probe tile is never written. The precompute hoists the existing per-fragment ratio into five RGB sums per probe: the same ratio, evaluated once per probe instead of once per fragment. As `ip-check.md` paraphrases them:
- Tencent 10/19 update the probe's stored lighting information;
- claims 11–13 re-shade a stored G-buffer.

The resemblance is limited to per-frame, per-probe state derived from light changes.

| Technique | Source, date | Relation |
|---|---|---|
| Per-light basis images superposed by current intensity | Dorsey, Arvo, Greenberg, IEEE CG&A 15(2), 1995 | Prior art for the ratio form |
| Quake lightstyles | id Software, 1996 (from memory) | Same lineage |
| Ambient cube, six directional weights | Mitchell, McTaggart, Green, SIGGRAPH 2006 ([PDF](https://advances.realtimerendering.com/s2006/Chapter7-Shading_in_Valve's_Source_Engine.pdf)) | Prior art for fixed-lobe directional weights (background knowledge; PDF unread) |
| Ratio-rescaled probes | Lazarov, Black Ops (2011), Black Ops II notes (2013) ([PDF](https://blog.selfshadow.com/publications/s2013-shading-course/lazarov/s2013_pbs_black_ops_2_notes.pdf)) | Relight detail unverified |
| Relit radiance-transfer probes, time of day, local lights | Far Cry 3, GDC 2012 ([GDC Vault](https://www.gdcvault.com/play/1015326/Deferred-Radiance-Transfer-Volumes-Global)); The Division, GDC 2016 ([PDF](http://mrakobes.com/Nikolay.Stefanov.GDC.2016.pdf)) | Per-frame probe relighting (GI/SH probes, not reflection tiles) |
| Relight stored cubemap G-buffers a few per frame | Forum description, unverified | Rewrites stored data: closer to Tencent 11–14 than the brief |
| Every-frame or on-demand probe re-render | [Godot ReflectionProbe](https://docs.godotengine.org/en/4.1/classes/class_reflectionprobe.html) | Rewrites the probe; far costlier |
| CPU precompute of the same sums | The brief's rival | Same math, same resemblance; changes only where it runs |

**Related patents to read before relying on the "scalar ratio" framing:**
- **Tencent US12293451B2.** Priority 2021-04-02, granted 2025-05-06, active, expires 2043-01-19. It computes a per-light "projection coefficient" on light change and combines it with a precomputed radiance-transfer parameter per pixel. Claims not read; confidence low–medium.
- **Baobab US11605198B2.** Priority 2018-06-29, granted 2023-03-14, active, expires 2039-06-28. It blends two precomputed GI datasets by weight. Low resemblance; confidence medium.

**Closest to the predecessor's Keep row:** keep the per-fragment ratio and cut its cost without a per-probe state pass, for example with fewer stored lights. No technique was found that avoids per-probe light evaluation entirely.

## Lever B: view-dependent skip

US10586375B2 (Microsoft), as summarized:
- **Claim 1:** decides per pixel whether a reflection ray is reflected back toward the camera, then picks one of two reflection techniques.
- **Claim 7:** uses screen-space reflection when the ray is not directed at the camera.

A Fresnel-keyed skip chooses skip-or-evaluate on weight magnitude, not between two techniques by ray direction. The predecessor already records "no per-pixel choice by ray direction".

| Technique | Source | Notes |
|---|---|---|
| Roughness-driven traversal rate | [AMD FidelityFX SSSR](https://gpuopen.com/?p=55238) | Work scaled by a surface attribute; not a Fresnel skip |
| Stochastic SSR | Stachowiak, SIGGRAPH 2015 ([Frostbite](https://www.ea.com/frostbite/news/stochastic-screen-space-reflections)) | Ray reuse and budget |
| Branch on weight below ε | Common shader practice | No citable source found |

- No pre-2018 citable source for a Fresnel-gated skip was found; the family's priority date is 2018-02-22. Confidence low.
- Further from the pattern:
  - key on view elevation or reflectivity alone;
  - or gate per draw on the CPU from the bound probe's maximum weight.

  Both trade precision and saving.

## Lever C: cell-rate evaluation into same-frame storage

**Against the avoid list:**
- **Tencent:** nothing resembles the storage. Claim 1c's "texture image" is the object's own unshaded image.
- **NVIDIA:** probe irradiance; unrelated.
- **ARM:** a precomputed intersection lookup; the brief stores none.
- **Autodesk:** temporal reprojection; the brief keeps no history.
- **EA:** an SSR-to-ray-trace fallback; unrelated.

**Prior art:**

| Technique | Source, date |
|---|---|
| Object-space shading | REYES, Cook et al., 1987 |
| Quake surface cache | id Software, 1996 |
| Lazy object-space shading | Burns, Fatahalian, Mark, HPG 2010 ([link](https://diglib.eg.org/items/46dfa3bd-3edb-4c17-a5e7-02bd2f3ba1b7/full)) |
| Decoupled sampling | Ragan-Kelley et al., ACM TOG 2011 |
| Object Space Lighting, shipped in Ashes of the Singularity | Baker, Oxide, GDC 2016 ([PDF](https://www.oxidegames.com/wp-content/uploads/2016/03/Object-Space-Lighting-Rev-21.pdf)) |
| Texel Shading | Hillesland and Yang, AMD, Eurographics 2016 ([PDF](https://diglib7.eg.org/bitstream/handle/10.2312/egsh20161018/073-076.pdf)) |
| Turing texture-space shading | NVIDIA, 2018 ([article](https://www.nvidia.com/en-us/geforce/news/nvidia-adaptive-shading-a-deep-dive)) |

**Texture-space shading patents:** none were read in full.

| Patent | Assignee | Priority | Status, expiry | Resemblance |
|---|---|---|---|---|
| US9728002B2 | AMD | 2015-12-18 | Active, expires 2035-12-18 | Claim 1 selects texels to shade from texels cached in another frame: temporal reuse. The brief keeps no history. |
| US12136166B2 | Qualcomm | 2021-09-30 | Active, expires 2043-01-03 | Meshlet shading atlas; claims not read |
| US11721064B1 | Meta | 2020-12-11 | Active, expires 2041-06-16 | Atlas shading with reuse across viewpoints, server to client |
| US7768523B2 | Microsoft | 2006-03-09 | Expires 2027-08-24 | Texture-space linearization and MIP-maps; different subject |
| Intel, asynchronous texel shading | Intel | Unknown | Not identified by number | Lazy texel shading reused across primitives and frames |

## Patent status

| Number | Assignee | Priority | Expiry | Status | Litigation / PTAB found |
|---|---|---|---|---|---|
| WO2026113651A1 / CN122115679A | Tencent | CN 2024-11-28 | Pending | PCT filed 2025-10-09; WO published 2026-06-04; no US or EP entry yet | None |
| US20210012562A1 | NVIDIA | 2019-07-11 | None | Abandoned 2023-08-23 | None |
| US10586375B2 | Microsoft | 2018-02-22 | 2038-02-22 | Active; 4th-year fee paid 2023 | None |
| US10360720B1 (parent of US10586375B2) | Microsoft | 2018-02-22 | 2038-02-22 | Active | None |
| US10636213B2 | ARM | 2015-10-26 | 2036-08-27 | Active | None |
| US10621774B2, US10796474B2, US10818079B2 | Electronic Arts | 2018-08-10 | 2038 (adjusted) | Active | None |
| US9355475B1 | Autodesk | 2015-02-27 | 2035-02-27 | Active; 4th- and 8th-year fees paid | None |

Details and links: [Google Patents](https://patents.google.com/) entries for each number.

**Confidence.**
- **High:** bibliographic data as Google Patents displays it, the Tencent timeline, the NVIDIA abandonment, and the gist of Microsoft's claim 1.
- **Low:** absence of litigation or PTAB records, and the readings of unread claims.

## For counsel
- Is a per-frame, per-probe sum of light changes, written to a buffer and never to the probe tile, "updating stored lighting" under Tencent 10/19 and 11–14? Read Tencent US12293451B2's "projection coefficient" claims as well.
- Does a Fresnel-keyed skip (view and normal, not ray direction) fall outside US10586375B2 claims 1, 7 and 10, and its parent US10360720B1?
- Does same-frame, history-free texel storage stay outside AMD US9728002B2 (temporal texel cache), Qualcomm US12136166B2 and Meta US11721064B1? Also identify Intel's asynchronous texel shading filing.
- Tencent's pending claims 1 and 20 (US deadline about 2027-05-28, EP about 2027-06-28 per `ip-check.md`): the cell storage adds an intermediate texture on the colour path. Re-read the claims' "texture image" construction.
- Confirm litigation, PTAB and licensing history for ARM, EA, Microsoft and Autodesk through PACER, Docket Navigator and the PTAB.
