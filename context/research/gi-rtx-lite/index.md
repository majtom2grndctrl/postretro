# RTX-lite GI and Reflections — Exploration

**Date investigated:** 2026-10-22
**Status:** Pre-spec exploration. Not a draft plan. Records an owner-led ideation thread on reflections and the whole GI stack, the owner's stated requirements, a ranked set of grafts, and the measurements that gate any redesign. Costs marked *est.* are unmeasured.

> **Read this when:** scoping a lighting or GI spec, reconsidering the SH probe grid, adding door or mover influence on light, generalising reflection probes beyond water, or evaluating an "RTX without RTX" idea against prior conclusions.
> **Key invariant:** today's stack is shipped, measured and tuned; nothing here is. Every graft lands only after its measurement gate, and the full redesign has no more than coin-flip confidence of looking better.
> **Related:** [IP and cost check](ip-check.md) · [Rendering Pipeline §4, §7.3a](../../lib/rendering_pipeline.md) · [Resource Management §5](../../lib/resource_management.md) · [Dynamic shadow survey](../dynamic-shadow-technique-survey.md) · [Baked promoted depth](../baked-promoted-depth.md) · [Reflection-probe IP check](../../plans/done/reflection-probes/ip-check.md)

---

## Owner requirements

Stated in this thread; treat as design inputs.

- **Doors and movers change how light flows.** Direct and bounce. Today doors occlude the camera only (`E17--doors-as-occluders`).
- **Indirect light in empty space is tech debt,** whatever else changes.
- **Shadow edges never show their storage lattice.** Soft is fine; chunky texel stair-steps are not. A crisp edge is crisp per scene pixel. Reflections match the geometry's render resolution, except where material roughness justifies blur. "Resolution" means the scene extent (surface ÷ integer divisor, `rendering_pipeline.md` §7.8), not native 4K.
- **Light counts and map sizes will grow a lot** ("Unreal maps"). `kinematic-platform.map` is the closest reference look so far, though the owner has not fully found the style: 24 lights, 17 of them static spots with 10°/45° cones, mostly near-white with blue and orange accents, one pulsing animated light, three dynamic; movers throw live shadows through promotion, and its mixed station makes three bright lights compete for two promoted cube slots.
- **Keep quality settings and bake caches** for fast iteration. Bake time is not the top priority, but the bake spends effort only where receivers are.
- **No hardware RT.** GTX 16-series must run very well; laptops matter; 5300M on Metal is the Mac target. SDF shadows as a lightmap replacement are deprecated and leave in a separate change; SDF as a technology stays open only if laptop-cheap.
- **Keep what works.** The owner likes the shadowmask, the shadow pool and the reflection probes, and doubts a redesign will beat the current stack. "Keep X, change only Y" is an acceptable outcome.

## Starting point: SDF-indexed probe (rejected)

Trace the water's reflected ray through the baked SDF, then sample the probe toward the hit, refining against the distance tile. First rejected in an earlier session for costing more than today's box-plus-search probe term. Rejected again here:

- **Low value.** The probe's error where its capture point sees the hit is already ~0.3°. Its failures are appearance, not geometry: undersides, backs of objects, characters. The SDF finds the right point and then samples the wrong colour.
- **No binding room.** The SDF needs a storage buffer; the fluid fragment stage is at 8/8.
- **Not novel.** Tencent WO2026113651A1 (pending) describes tracing the reflected ray to a hit and sampling the probe toward it; its claim 2 names no tracer.

Rule that followed: a probe's partner must supply **appearance from a vantage the capture point lacks**.

## Reflection findings

- **Cell-rate cache** (Reyes / Quake surface cache, shade each pixel-style cell once in compute) was already spiked on 2026-10-17 (`claude/cell-rate-spike-1660`). It pays only above ~2 px per cell; on the 1660 a split draw mattered more; the 5300M has no such penalty.
- **Underside relief tile.** A per-probe orthographic upward capture marched like relief mapping (Miller 1986; Policarpo 2005) shows dock and catwalk undersides that SSR and the water-level capture point cannot. Superseded on the selected plane by planar; still a candidate for probe-only water.
- **Planar water spike** (paused, contract on the owner's `claude/planar-water-spike`): mirror camera over one plane per frame, early-z water mask, probes past distance limits D and R. It is the most literal "appearance from another vantage" and complements everything below. Risks: mirror-only cells must join SH, lightmap and compose residency; one plane per frame drops movers from every other plane; pixel-style tilt mismatch at the planar-to-probe blend; Metal draw cost.

## Diagnosis of today's GI

- **Runtime.** Animated SH compose was the frame on dense content: frame interval 41.93 ms with compose vs 4.11 ms suspended at 1 m spacing (live spawn, Windows 1660 box), now 0.17–1.55 ms on the 1660 after the contributing-row filter. The 5300M pays a fixed ~3.0–3.5 µs per composed row against well under 1 µs on the 1660. The forward pass's SH share has never been isolated (`rendering_pipeline.md` §12).
- **VRAM.** Hallway SH 193.8 MiB across 3,385 clusters, worst mandatory set 49–125 MiB by reach, pool capacity 720 MiB at spawn; campaign composed atlas 55.88 MB for 194,028 grid probes; lightmap pool 136.5 MiB mandatory at L = 16 m, ~295 MiB with animated data.
- **Bake.** `stress-warren-hallway-inspection` at 1 m SH spacing: 11.15 h, 4.82 GiB peak, SH-dominated.
- **What carries the look:** the soft-penumbra directional lightmap, animated lightmaps with real shadows, entity shadows through promotion, an indirect term that follows normals.
- **What is complexity for its own sake:** nearly all of it traces to indirect stored in a dense grid over empty space, re-composed per animated light and streamed because it does not fit. Depth moments exist to stop that grid leaking through walls.

## Baked visibility queries lighter than a dense SDF

| Family | Answers | Edge failure mode | Fit |
|---|---|---|---|
| Soft lightmap visibility (shadowmask, shipped) | Per-light baked visibility | Blur when penumbra ≥ ~2 texels; stair-steps below | Proven |
| Bent normal + aperture cone (Landis 2002; Oat & Sander 2007) | Soft occlusion of any light from one 4 B texel | Blur; leaks where a texel sees two openings | Low tier for dynamic-tier lights |
| Horizon maps (Max 1988) | Shadow from any light elevation | Blur | Height-field-like receivers |
| SH visibility (Sloan 2002) | Soft multi-lobe visibility | Blur, ringing | Low tier |
| Analytic convex-occluder slab test (Kay–Kajiya 1986) | Exact shadow of a convex brush from a point light | None; inflate planes by light size for a penumbra | Movers and doors, ~20 ALU per (light, brush) |
| Object-ID shadow tiles (Hourcade & Nicolas 1985) | Which brush occludes, bias-free | Stair-steps alone; lit cracks at seams with slab resolve | Spike only |
| Baked EVSM tiles (Lauritzen 2008) | Filtered per-light static visibility | Blur plus bounded light bleeding | Candidate lightmap-direct replacement |
| Per-cell distance tile (today's water probe) | Hit distance from a fixed point | — | Glossy reflections |
| Dense SDF | Any ray from any point | — | Poor per fragment; nothing needs it now |

Pattern: cheap queries are receiver-local and answer "how visible is this direction", not "where is the hit".

## Candidate grafts, ranked

Ordered for the owner's priorities, including growing light counts. Confidence is that the graft improves on today.

| # | Graft | Confidence | Gate |
|---|---|---|---|
| 0 | **Penumbra floor.** Effective emitter size ≥ ~2 mask texels at the lit surface; logged. Open owner decision: whether authored `_light_size 0` (hard) is exempt | ~80% | Capture PNGs show no texel step within 2 m at the Auto extent |
| 1 | **SH probe classification and relocation** (Majercik et al. 2021): keep probes that bracket a receiver in their cell; move probes out of walls instead of invalidating them | ~85% | Stored tiles and SH residency fall; no entity or fog lighting regression |
| 2 | **Door and mover slab shadows + portal-gated light reach.** Analytic convex-brush shadows for lights near kinematic movers, beside the pool (which keeps skinned characters); `blocked_portals` gates each runtime-evaluated light's reach | ~75% as a capability | 5300M forward delta at the kinematic-platform stations; promoted-slot pressure falls |
| 3 | **Indirect-light redesign** (below). Form decided by measurement | ~50% look, ~65–70% scaling | Scaling measurements; counsel on the filings in the IP check |
| — | **Per-pixel direct for every static light** (visibility masks instead of baked direct) | Demoted | Cost and mask VRAM grow per light; keep baked direct as default and evaluate a light per pixel only when a door gates it, a mover shadows it, or it animates with shadows |

## Indirect-light redesign options

All replace the dense SH grid; none replaces the shadowmask, the shadow pool or the reflection probes.

- **Door-driven deltas.** Bake each door's open-minus-closed bounce as a delta driven by door state, on the same linearity animated-light deltas (id 27) use. Meets the doors requirement with existing machinery; keeps the empty-space grid.
- **One probe type, two readers** (McGuire et al. I3D 2017). Per-cell radiance + inverse-L1 distance tiles, captured like today's water probes. Bake derives a cosine-prefiltered irradiance mip and Chebyshev moments from the same tiles. Entities, fog and billboards read the irradiance; flagged rough materials read the full tile through today's distance search, hard-bound by cell. Probe count scales with cells, not volume.
- **Patch radiosity re-solved on change** (Goral 1984; George, Sillion & Greenberg 1990; qrad 1997). ~0.5 m patches with baked top-K links and the portals each crosses; Jacobi sweeps on the CPU when a door or light changes; links over sealed portals zero. Animated lights need a hybrid: B = B_door + Σ c_l(t)·B_l with per-light baked bounce, re-solve on door events only.
- **Probe-to-patch links.** Probe diffuse SH = Σ link weight × current patch radiance, so probes follow doors without ray tracing. Reflection tiles are never re-shaded (that variant reads on Tencent WO2026113651A1 claims 11–13).

### Look changes

Same art style: soft baked shadows, neon palette. More reactive: closing a door darkens the next room; bounce follows doors and lights over a frame or two; wet concrete and metal catch neon. Risks: flatter entity and fog gradients in large cells; possible patch blotchiness at 0.5 m; small area-light shading losses where shading moves from bake to analytic.

### Direction and animation

Direction is about the same overall. Gains sit in runtime-evaluated lights and glossy surfaces; patch bounce loses some.

| Term | Today | Redesign | Verdict |
|---|---|---|---|
| Direct, baked lights (default) | One dominant direction per lightmap texel | Unchanged | Same |
| Direct, runtime-evaluated lights (door-gated, mover-shadowed, animated with shadows) | True direction on movers and entities only; world gets baked direct | True direction per pixel on world too | Better |
| Bounce on world | Per pixel from octahedral irradiance probes; full normal response | Patch RGB + one dominant direction | Slightly worse. L1 SH per patch (~12 values instead of 4) closes most of the gap |
| Bounce on entities and fog | Octahedral irradiance, 8 grid probes | Same form, or L2 SH via probe links, from per-cell probes | Same direction; coarser spatial gradient in large cells |
| Glossy surfaces | Water only | Full directional radiance from per-cell tiles | New |

Diffuse bounce is low-frequency; L2 SH holds it nearly as well as the octahedral tile. The patch loss is a format choice, not a design limit.

Animation gains are new kinds of change and lower cost, not more expressive per-light curves:

| Change | Today | Redesign |
|---|---|---|
| Baked-position light pulses or recolours | Per-light grid deltas (id 27), GPU compose per frame; the costly path | Per-light bounce on patches × curve; same flexibility, far fewer samples |
| Door opens or closes | No effect on light | Sealed-portal links zero; bounce re-solves over a frame or two |
| Mover blocks light | Direct only, through fixed pool slots | Direct via slab shadows for every nearby light; bounce still ignores movers |
| Light in reflection probes | Share ratio: top 4 lights × 5 lobes | Unchanged; runtime re-shading of tiles reads on Tencent WO2026113651A1 claims 11–13 |
| Probe diffuse | Composed from grid deltas | Follows the patch solution through probe links, doors included |
| Moving (dynamic-tier) lights add bounce | No | No. Injecting them needs per-patch visibility to a moving light; only a leaky unshadowed estimate fits |

### Scaling

| System | Today scales with | Redesign scales with |
|---|---|---|
| Entity and fog indirect | Volume ÷ spacing³ × animated reach | Cells (1–3 probes each); hallway ~960k valid SH probes → 2.1k–6.2k (*est.*) |
| Indirect bake | Rays × grid probes (11.15 h hallway) | Probe captures + patch links, minutes (*est.*) |
| Mover shadows | Fixed promotion slots (rationed) | Lights near movers × movers, analytic |
| Water probes | Fluid face groups (`stress-env-volumes` overflows the 256 cap) | Cells with water |
| Links | — | Surface area; 144–512 MB on the hallway (*est.*), needs per-cell streaming |

### Composition with planar

Complementary. The mirror view reuses the world pipelines, so door shadows and gated light appear in it for free, and per-cell probes shrink planar's SH residency unions. Per-light runtime cost compounds in the mirror pass. Order: resume the planar spike on today's stack; decide the SH replacement before planar's residency unions ship.

## Measurement plan

Cheapest first; the first four need no renderer change.

1. **`stress-warren-hallway-inspection`:** cells, cell volumes and the large-cell tail, surface area by patch size → real probe counts, link bytes, patch counts.
2. **`stress-env-volumes`:** probes per cell vs per fluid volume.
3. **`stress-warren-lit`, `stress-warren-crates`:** lights within reach of each mover → slab-test cost vs promotion rationing.
4. **`stress-warren-showcase`:** cells per door event and animated reach in patches → door-event CPU and the animated hybrid.
5. **5300M at 4K:** forward pass with world SH on vs off.
6. **Visual A/B:** characters lit from per-cell probes (averaged from today's probes) in a large hall and under lightstyles.
7. **Glossy world-surface cost on the 5300M**, with a per-material pipeline variant and a fix for the forward pass's 16-texture floor.

### Measurement pitfalls

- `stress-warren-hallway-inspection` at 1 m SH spacing bakes in 11.15 h: unattended runs only. The hallway mini at 1 m projected ~46 min.
- On 2026-09-18 the full warren PRL would not render on the 6 GiB 1660, and `POSTRETRO_GPU_TIMING=1` device-lost NVIDIA driver 616.92 on the first frame (`lighting-scale--adaptive-probe-spacing`). Streaming has since landed and the hallway now runs; recheck both before relying on GPU timing there.
- The 5300M can lack timestamp queries; measure with Metal System Trace (`rendering_pipeline.md` §12).
- The forward pass has a fixed 16-texture floor and the fluid fragment stage is at 8/8 storage buffers; any spike adding bindings hits these first.

## Open owner decisions

- Whether the penumbra floor exempts authored `_light_size 0` (hard) lights.
- Whether doors must change bounce, or direct only. Direct-only needs just graft 2; bounce adds door-driven deltas or patch radiosity.
- Tolerance for flatter entity and fog lighting in large cells, or a probes-per-large-cell rule.
- Which materials may read glossy probe reflections, and who flags them.
- Whether patch radiosity stores L1 SH per patch (better normal response, more bytes).
- Allowing patent hosts in the environment's network policy so the IP triage can be re-charted against claim text (hosts listed in [ip-check.md](ip-check.md)), and when to engage counsel.

## Provenance

- **Verified against `context/plans/done/`:** 41.93 / 4.11 ms compose frame interval; 194,028 grid probes and 55.88 MB composed atlas (campaign); 962,636 valid probes and 2,082 cells (hallway); hallway SH 193.8 MiB, 3,385 clusters, mandatory 49–125 MiB; lightmap 136.5 MiB mandatory, 224 MiB at install, ~295 MiB with animated data; 11.15 h hallway bake; 5300M ~3.0–3.5 µs per composed row; 0.17–1.55 ms compose after the row filter; 198 campaign cell blocks; the 16-texture floor in code.
- **Estimates from agent rounds, unmeasured:** every *est.* figure, all patch, link and probe-tile sizes, glossy cost per megapixel, door-event CPU, and every confidence percentage.
- **Elsewhere:** the planar contract exists only on the owner's Windows worktree (`claude/planar-water-spike`, unpushed as of 2026-10-22); cell-rate findings on `claude/cell-rate-spike-1660` under `context/plans/drafts/reflection-probe-fluid-pass-performance--cell-rate-spike/`.

## Rejected or parked

- **Voxel albedo + SH relight at an SDF hit:** needs the SDF and a cell-rate pass; blocky 0.5 m look.
- **Multi-probe blend over one water body:** occlusion-conditioned probe switching sits on Tencent WO2026113651A1 claims 15/17.
- **Mirrored-entity-only draw:** splitting dynamic from static content reads near NetEase CN117496039A; the planar spike draws the static world for that reason.
- **Light propagation volumes:** the empty-space debt by construction.
- **Per-texel hit re-shading in probes:** Tencent claims 11–13.
