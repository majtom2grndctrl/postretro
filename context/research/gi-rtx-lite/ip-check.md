# RTX-lite GI — IP and Cost Check

**Date investigated:** 2026-10-22, repo at `473185e88`
**Status:** Engineering triage for counsel, not legal advice. Companion to [index.md](index.md). "Appears present", "appears absent" and "reads on" are engineering readings of claim text, not opinions on infringement, validity or freedom to operate.

> **Read this when:** designing probe selection, a runtime radiosity update, or any probe that changes with lighting at runtime.
> **Key limit (read first):** every patent database (Google Patents, Justia, USPTO, Espacenet, Patentscope) was blocked by the session's network policy. **All claims below come from search-result summaries, not claim text.** Element charts are paraphrases of paraphrases; statuses, priority dates and expiry projections are unverified. Treat each verdict as a lookup list. Re-chart against certified claims before any design decision rests on it. Tencent WO2026113651A1 is charted from [the reflection-probe IP check](../../plans/done/reflection-probes/ip-check.md), itself read from a machine translation.

---

## Pieces under review

1. **Bake-time probe classification and relocation** (Majercik et al., JCGT 2021). Offline only.
2. **One probe type, two readers** (McGuire et al., I3D 2017). Per-cell radiance + inverse-L1 distance tiles; bake-derived irradiance mip and moments; glossy reader is today's distance search, hard-bound by cell; live share ratio; tiles never rewritten.
3. **Patch radiosity re-solved at runtime.** ~0.5 m patches, baked top-K links with crossed portals, CPU Jacobi on door and light changes, sealed-portal links zeroed.
4. **Probe-to-patch links.** Probe diffuse SH = Σ weight × patch radiance; reflection tiles not re-shaded.

## Filings found

| Document | Owner | Priority | Status (per summaries) | Pieces |
|---|---|---|---|---|
| US20210012562A1 "Probe-based dynamic GI" | NVIDIA | 2019-07-11 | Abandoned 2023-08-23; no continuation found; now prior art | 1, 2 |
| WO2026113651A1 / CN122115679A | Tencent | 2024-11-28 | Pending; US national phase ≈2027-05, EP ≈2027-06 | 2, 4 |
| US10137372B2 "Light probe culling based upon visibility" | Square Enix (Eidos Montréal) | filed 2016-12-14 | Granted 2018-11-27; projected expiry ≈2036-12 | 2, 4 |
| US12293451B2 "Illumination rendering method…" | Tencent | CN 2021-04-02 | Granted 2025-05-06; projected expiry ≈2042-03 | 3, 4 |
| US8558837B2 "Modular radiance transfer" | Disney | 2010-01-18 | Active; adjusted expiry 2032-06-30 | 3 |
| US10290148B2 / US10796479B2 | Suntracker (Ashdown) | CIP chain to 2012-04-13 | Active; ≈2032-04, counsel to confirm CIP term | 3 |
| US10672177 (from US20170116773A1) | Geomerics → ARM | 2015-10-23 | Active | 3 (bake) |
| US10008032 / 10672183 / 11127198 / 11734879 | Imagination Technologies | 2015-03-03 | Active | 2 |
| US11854143B2 "Light probe generation" | Tencent | CN 2019-12-05 | Granted | 1 |
| US20250037360A1 | Sony Interactive Entertainment | ≈2023 | Pending | 2 |
| US8451270B2 "Real-time radiosity system in a video game" | Microsoft | 2008-06-25 | Expired (fees); prior art | 3 |
| US7408550 (Bunnell) | Intel | 2006 | Expired (fees); prior art | 3 |

Not found: Geomerics/ARM filings on the Enlighten runtime solve or radiosity-derived probes; Epic (Lumen surface or radiance cache); EA, Activision or Unity probe placement; portal- or door-gated light transport. Filings published in the last 18 months are invisible to search.

**Prior art relied on:** Goral et al. 1984; Cohen & Greenberg 1985; George, Sillion & Greenberg 1990; Dorsey, Sillion & Greenberg 1991; Hanrahan et al. 1991; Drettakis & Sillion 1997; qrad 1997; Source/vrad 2004 (per-leaf ambient samples); Doom 3 2004 (area portals gate light); Martin & Einarsson SIGGRAPH 2010 (Enlighten in Frostbite 2, probes from the solution); Sloan et al. 2002; McGuire et al. 2017; Wang et al. I3D 2019; Majercik et al. 2019, 2021; Unity Adaptive Probe Volumes "Virtual Offset".

## Piece 1: probe classification and relocation

| Filing | Element | Proposal | Reading |
|---|---|---|---|
| NVIDIA US20210012562A1 | Probe active only near geometry; probes moved off grid; backface-fraction test | Same idea, offline | Abandoned; prior art |
| Tencent US11854143B2 cl. 1 | Voxelize a target object's shadow points, reduce vertices, place probes there | Keep probes seeing a receiver in their cell; move out of walls | Appears absent |
| Sony US20250037360A1 | Higher resolution in a region of one probe | Uniform tile | Appears absent |

**Verdict: appears clear.**

**Costs (est.).** Bake: reuses the validity pass's 256 rays per probe; relocation ≈64 rays per invalid probe, ~15–40 s on the hallway (962,636 valid probes), noise against the SH stage. VRAM: stored tiles perhaps −20–40%; fog-reachable and entity-reachable air must stay "receiver", which caps the saving. Runtime: classification free (validity 0 exists); relocation needs a 3×i8 offset in the per-probe moment record, +0–2% forward, and relocated probes pin their nodes to L0.

**Levers.** Backface threshold, relocation step limit, receiver definition (surfaces, fog, navmesh air), classification aggressiveness as a bake setting.

**Cache keys.** Geometry, probe spacing, threshold constants. Not lights or materials.

## Piece 2: one probe type, two readers

| Filing | Element | Proposal | Reading |
|---|---|---|---|
| Tencent WO cl. 1–3 | Parallax-corrected probe reflection on a textured surface | Glossy reader = today's search | Present, as for water; exposure widens to every glossy surface |
| Tencent cl. 4–9 | Camera-placed or camera-moved probes | Bake-placed | Appears absent |
| Tencent cl. 10, 19 | Update stored probe lighting on a light change | Post-sample share ratio; tiles never rewritten | As landed: closest element, counsel |
| Tencent cl. 15–18 | Complete occluded lookups (move probe / lighting library) | No continuation; hard cell binding | Appears absent, provided a glossy fragment in capture shadow never falls back to the irradiance mip |
| Square Enix US10137372B2 | Find probes for a point; find its zone; check zone visible from probe (binary per-probe mask); combine visible probes' irradiance | Entities and fog blend nearby per-cell probes | **Near a live claim** on the zone-visibility element; continuous Chebyshev weighting arguably differs. Counsel |
| Imagination '879 family | Probes updated by runtime ray tracing; local probes per screen block | Bake only | Appears absent |

**Verdict: needs counsel** (Square Enix '372 on the diffuse reader).

**Free design-arounds.** Select diffuse probes by cell adjacency (own cell, then portal-adjacent) with continuous moment weights; store no per-probe zone or cell visibility mask and make no binary visible decision. Glossy capture shadow draws no probe term and is never filled.

**Costs (est.).**
- Probe count: hallway 2,082 cells → 2.1k–6.2k probes; campaign 198 cells → 200–600.
- Per probe: 128² BC6H radiance 16 KiB + R16F distance 32 KiB + mips ≈ 64 KiB; irradiance mip + moments ≈ 1.6 KiB.
- Disk: hallway 131–397 MiB at 128², 33–99 MiB at 64². Resident tiles 4–38 MiB; diffuse data ≈10 MiB whole-map. Replaces the SH grid (hallway: 193.8 MiB across 3,385 clusters, worst mandatory set 49–125 MiB).
- Bake: ≈45 ms per probe captured → 94–279 s at 128². Removes the SH bake only if world indirect gets another source (piece 3 or a baked bounce lightmap).
- Runtime GPU: entities ±20% of today (4 irradiance + 4 moment taps vs 8 + 8). Fog +0–30% (needs a coarse 3D probe index). Glossy world surfaces 0.5–1.2 ms per screen-megapixel on the 1660, 1.5–2.5× on the 5300M; needs a per-material pipeline variant. SH compose passes removed.
- **Binding:** the forward pass has a fixed 16-texture floor; dropping SH atlases (−2) and adding radiance, distance and radiosity (+3) is net +1, so something must consolidate.
- **Scope gap:** tiles carry no emitters. Direct light from baked lights onto entities and billboards (ids 35/41/45/47/48) needs a carrier, e.g. per-pixel direct for entities.

**Levers.** Tile resolution, probes per large cell, moment filter width, irradiance mip size, index voxel size; player tiers for glossy on/off per material class, mip bias, search taps.

**Cache keys.** Split capture into a geometry pass (hit, distance, moments: geometry, placement, tile size) and a shading pass (lights, albedo, emissive, the bounce source). Capture animated lights at zero and carry them in the share table so curve edits do not re-key tiles.

## Piece 3: patch radiosity

| Filing | Element | Proposal | Reading |
|---|---|---|---|
| Tencent US12293451B2 cl. 1 | On a light change, projection coefficient of the changed light onto basis functions × per-pixel transfer → indirect | E = Σ scalar intensity × baked unit per-light patch irradiance; Jacobi over links | **Near a live claim** on "projection coefficient"; a scalar is arguably not a basis projection. Counsel |
| Disney US8558837B2 | Per-shape spectral decomposition of one-bounce transport | Explicit top-K links | Appears absent; keep links uncompressed |
| Suntracker '148/'479 | Per-channel canonical solutions in a texture, summed per vertex in a vertex shader × channel intensity | CPU solve | Appears absent; keep any per-light baked bounce out of a per-vertex shader sum |
| ARM US10672177 | Final-gather rays distributed by directional irradiance | Cosine-distributed link rays | Appears absent if rays never follow a prior solution |
| Portal- or door-gated transport | None found | Sealed-portal links zeroed | Prior art: Drettakis & Sillion 1997; Doom 3 2004 |

**Verdict: needs counsel** (Tencent '451).

**Free design-arounds.** Never project lights onto SH or any basis; no low-rank link compression; cosine link rays.

**Costs (est.).**
- Patches: hallway ≈1.0–2.0M at 0.25 m² (inferred from pool layers, not measured); campaign 0.1–0.4M.
- Links: 144–512 MB on the hallway, CPU RAM and disk; must stream per cell.
- Bake: ≈0.6–1.3G rays → 105–430 s; per-light emission from cached lightmap layers; +0.5–1 GiB peak.
- VRAM: radiosity map 6–12 MB.
- Runtime GPU: 1–2 taps per world fragment, replacing the 8-probe stencil.
- Runtime CPU per door event: ≈0.7M link-ops per sweep, ≈1 ms on 4 threads for 4 sweeps.
- **Animated worst case** (50 lights changing every frame): ≈3.6 ms per frame on 4 threads, over budget. Fix: B = B_door + Σ c_l(t)·B_l with per-light baked bounce; re-solve only on door events.

**Levers.** Patch size (integer multiple of lightmap texels), K, rays per patch, sweeps, update cadence, CPU ms budget, per-region patch scale.

**Cache keys.** Links: geometry, portal polygons, mover sealed-portal ids, patch size, K, ray count, seed; stable patch ids (face + grid coordinate). Not lights, colours, curves, albedo (store ρ separately), lightmap density or block order.

## Piece 4: probe-to-patch links

| Filing | Element | Proposal | Reading |
|---|---|---|---|
| Tencent WO cl. 10 | Update the probe's stored lighting before collection | Diffuse SH recomputed; tile untouched | Arguably absent; counsel on whether co-located data is "the probe" |
| Tencent WO cl. 11–13 | Re-shade stored G-buffer information on a light change | Sum of weight × patch radiance | Appears absent |
| Rejected variant: per-texel hit, UV and material re-shaded at runtime | — | — | **Reads directly on cl. 11–13**; rejected |
| Tencent US12293451B2 | Probe transfer × light projection coefficient | SH links × patch radiance | Same question as piece 3 |
| Square Enix '372 | Zone-visibility culling | Same reader as piece 2 | Same as piece 2 |

**Verdict: needs counsel.**

**Free design-arounds.** Runtime SH in a separate resource from the reflection probe record; the glossy reader never reads it; no per-texel re-shading; no time-slicing of tile faces.

**Costs (est.).** ≈0 extra bake rays (from piece 2's capture); 9.5–17.5 MB links; ≈0.2 ms per door event; worst case all probes 10–20 ms, so dirty-track changed patches and resident cells only.

## Summary

| Piece | IP verdict | Watch | Bake Δ (est.) | Memory Δ (est.) | Runtime (est.) |
|---|---|---|---|---|---|
| 1 | Appears clear | — | +15–40 s | SH −20–40% | ≈0 |
| 2 | Needs counsel | Square Enix '372; Tencent WO cl. 1, 18 | +94–279 s; −SH bake only with a new bounce source | −SH grid (49–125 MiB mandatory on the hallway); +4–38 MiB resident; 131–397 MiB disk | Entities ≈0; fog +0–30%; glossy 0.5–1.2 ms/MP (1660) |
| 3 | Needs counsel | Tencent '451 | +105–430 s | +6–12 MB VRAM; 144–512 MB links | Door ≈1 ms (4 threads); animated needs hybrid |
| 4 | Needs counsel | Tencent WO cl. 10; '451 | ≈0 | +9.5–17.5 MB | ≈0.2 ms per door event |

**Top IP risks.** (1) Square Enix US10137372B2 against the diffuse reader; read certified claims before designing per-cell probe lists. (2) Tencent WO2026113651A1: claim 1 widens to every glossy surface; claim 10 on runtime data beside a reflection probe; claims 11–13 keep re-shading rejected. (3) Tencent US12293451B2 against any light-change-triggered transfer update; keep per-light terms scalar.

**Top cost unknowns.** (1) Patch count, link bytes, link bake time and K-truncation energy loss on the hallway and campaign. (2) Glossy world-surface cost on the 5300M, plus the 16-texture fix. (3) CPU Jacobi under animated load, validating the hybrid and its error when doors cut a light's region.
