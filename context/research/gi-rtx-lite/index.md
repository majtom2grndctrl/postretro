# RTX-lite GI and Reflections — Exploration

**Date investigated:** 2026-10-22
**Status:** Pre-spec exploration. Not a draft plan. Records an owner-led ideation thread on reflections and the whole GI stack, the owner's stated requirements, a ranked set of grafts, the recommended hybrid stack, bake-only artefacts, and the measurements that gate any redesign. Costs marked *est.* are unmeasured.

> **Read this when:** scoping a lighting or GI spec, reconsidering the SH probe grid, adding door or mover influence on light, generalising reflection probes beyond water, or evaluating an "RTX without RTX" idea against prior conclusions.
> **Key invariant:** today's stack is shipped, measured and tuned; nothing here is. Every graft lands only after its measurement gate. Direction: finish today's stack with grafts (the hybrid below); the full indirect redesign is parked unless its conditions hold.
> **Related:** [IP and cost check](ip-check.md) · [Rendering Pipeline §4, §7.3a](../../lib/rendering_pipeline.md) · [Resource Management §5](../../lib/resource_management.md) · [Dynamic shadow survey](../dynamic-shadow-technique-survey.md) · [Baked promoted depth](../baked-promoted-depth.md) · [Reflection-probe IP check](../../plans/done/reflection-probes/ip-check.md)

---

## Owner requirements

Stated in this thread; treat as design inputs.

- **Doors and movers change how light flows.** Direct light is required. Door-driven bounce is optional: worth it only if cheap.
- **Indirect light in empty space is tech debt,** whatever else changes. Removing it is already planned for today's stack (graft 1); it is not a redesign-only gain.
- **Shadow edges never show their storage lattice.** Soft is fine; chunky texel stair-steps are not. A crisp edge is crisp per scene pixel. Reflections match the geometry's render resolution, except where material roughness justifies blur. "Resolution" means the scene extent (surface ÷ integer divisor, `rendering_pipeline.md` §7.8), not native 4K.
- **Light counts and map sizes will grow a lot** ("Unreal maps"). `kinematic-platform.map` is the closest reference look so far, though the owner has not fully found the style: 24 lights, 17 of them static spots with 10°/45° cones, mostly near-white with blue and orange accents, one pulsing animated light, three dynamic; movers throw live shadows through promotion, and its mixed station makes three bright lights compete for two promoted cube slots.
- **Keep quality settings and bake caches** for fast iteration. Bake time is not the top priority, but the bake spends effort only where receivers are. Bake stays on the CPU; GPU baking is on hold.
- **Bake an artefact, derive, drop it.** BSP → portals is the model: build a rich structure at bake, keep only what it yields.
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

- **Runtime.** Animated SH compose is the Mac's costliest lighting term. After the contributing-row filter the 5300M at the kinematic-platform station still pays 2.89 ms indirect + 0.61 ms Pass A + 0.89 ms Pass B per frame, ~4.4 ms (`sh-compose-contributing-rows`); the 1660 pays 0.17–1.55 ms. The 5300M costs ~3.0–3.5 µs per composed row against well under 1 µs on the 1660. The older 41.93 vs 4.11 ms frame interval (1 m spacing, live spawn, 1660) predates the dispatch-collapse fix and measured mostly dispatch count (~1,590 dispatches a frame), so it no longer motivates anything. The forward pass's SH share has never been isolated (`rendering_pipeline.md` §12).
- **VRAM.** Hallway SH 193.8 MiB across 3,385 clusters, worst mandatory set 49–125 MiB by reach, pool capacity 720 MiB at spawn; campaign composed atlas 55.88 MB for 194,028 grid probes; lightmap pool 136.5 MiB mandatory at L = 16 m, ~295 MiB with animated data.
- **Bake.** `stress-warren-hallway-inspection` at 1 m SH spacing: 11.15 h, 4.82 GiB peak, SH-dominated. One run, 2026-09-18; the cold SH falloff early-out (~20.9× on a synthetic fixture) may postdate it. Re-measure before treating it as a cost driver. Each probe casts 256 rays and, at every hit, re-shadows each light in range with area samples: most SH time is re-shadowing direct light. Bounce is one bounce off a constant grey albedo (0.45), black sky, emissive ignored, so today's indirect has no colour bleeding. Probe validity is BSP solidity plus an exterior-leaf test, not a ray test. SH and delta stages finish before atlas preparation, so they cannot read the lightmap.
- **Doors.** Bake auto-detects which portals each mover's closed brush fully seals (`sealed_portal_ids`); a docked-closed door blocks the camera flood. Movers shadow every dynamic light and each static light they promote, onto world through the shadowmask union. Limits: 8 spot + 2 cube promoted slots, ≤ 4 mask channels per texel. Unpromoted baked light passes through a closed door, since the bake has no movers. Bounce ignores doors.
- **What carries the look:** the soft-penumbra directional lightmap, animated lightmaps with real shadows, entity shadows through promotion, an indirect term that follows normals.
- **Grid costs that cleanup fixes:** what air probes cost, not their existence. Entities and fog need indirect in the air, so every design stores some. Culling per-light entries by receiver reach is ruled out (`lighting-scale--sh-delta-receiver-reach-cull`, routed to no work): it would double-count a light on receivers in culled cells, and capsule + jump + dilation already spans about one 4 m layer above every floor, so little is left to cull. Valid-only storage and L1/L2 coarsening already cut campaign stored tiles 194,028 → 57,128. What remains is the octahedral tile format and world surfaces reading the grid.
- **Grid costs that cleanup cannot fix:** the grid ignores the portal graph, so door effects must be baked, and door bounce deltas sum only approximately for doors in series. Per-light deltas (ids 27/41/45) grow with lights × volume reached, the real exposure for larger maps. World indirect stays tied to probe spacing while world surfaces read the grid. Depth moments exist to stop the grid leaking through walls.

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
| 0 | **Penumbra floor.** Each texel's visibility filtered over a ≥ ~2-texel receiver footprint, for every static light including authored `_light_size 0` (owner ruling). An emitter-size floor cannot widen a contact edge. Done: `context/plans/done/lightmap-penumbra-floor/` | ~80% | Texel-step detector (§Bake-only artefacts) and capture PNGs show no texel step within 2 m at the Auto extent |
| 1 | **SH probe classification and relocation** (Majercik et al. 2021): keep probes that bracket a receiver in their cell; move probes out of walls instead of invalidating them | ~85% | Stored tiles and SH residency fall; no entity or fog lighting regression |
| 2 | **Door and mover slab shadows + portal-gated light reach.** Analytic convex-brush shadows for lights near kinematic movers, beside the pool (which keeps skinned characters); baked light → cell reach, filtered by `blocked_portals`, gates each runtime-evaluated light | ~75% as a capability | 5300M forward delta at the kinematic-platform stations; promoted-slot pressure falls |
| 3 | **Door channel.** For each baked light whose reach crosses a door's sealed portal, bake visibility masks for the door's closed and open poses (the bake has no movers, so "door absent" is not "door open"). The animated-lightmap compose blends open toward closed by how closed the door is, on door events only. Doors combine as a product of visibilities, exact for hard occluders and point lights; owner specular takes the same factor. A sparse per-probe factor carries it to entity direct SH. Graft 2 supplies the crisp edge while the door moves | Unrated; unmeasured | Masks ≤ ~10% of the lightmap mandatory set; compose ≤ 0.2 ms per door event on the 5300M |
| 4 | **Bounce in the lightmap** (Quake 2 / Source vrad). One summed static bounce layer, gathered from the surface radiance cache; the grid serves entities, fog and billboards and can coarsen. Reuses cell-block streaming and BC6H; ~0.25–0.5 B/texel at ÷4 (*est.*). Catch: world surfaces read the composed atlas, so animated bounce kept on probes means the world keeps the 8-probe stencil or gains a delta-only atlas, and "one indirect path per surface" changes. Never per-light bounce per texel ('451 claim 3) | Unrated; unmeasured | Lightmap VRAM gain vs SH residency drop; wall-vs-entity A/B (they stop reading one field); normal-mapped bounce response holds |
| 5 | **Glossy world reflections.** Flagged rough materials read per-cell or per-face radiance + distance tiles through today's distance search. Separable from any indirect change | Unrated | Measurement plan item 9 |
| 6 | **Indirect-light redesign** (below). Parked | ~50% look, ~65–70% scaling | §When the redesign earns its place |
| — | **Per-pixel direct for every static light** (visibility masks instead of baked direct) | Demoted | Cost and mask VRAM grow per light; keep baked direct as default and evaluate a light per pixel only when a door gates it, a mover shadows it, or it animates with shadows |

## Hybrid stack

Recommended direction after an independent review (2026-10-23). Today's stack, finished, plus grafts and bake-only artefacts. With doors required to change direct light only, the indirect redesign's one unique gain, bounce that combines correctly across several doors, is not needed.

| Layer | Keep | Add | Drop |
|---|---|---|---|
| World direct | Directional lightmap, shadowmask, union subtraction, owner specular | Penumbra floor (0); door channel (3); slab shadows for moving doors and movers past the slot budget (2) | SDF shadows; per-pixel direct for every static light |
| World indirect | SH grid with Chebyshev weighting | Base and deltas gathered from the surface radiance cache; classification and relocation (1); optionally bounce in the lightmap (4) | Patch radiosity, probe-to-patch links |
| Entity, mover, billboard direct | Direct SH (ids 35/41/45), billboard scatter (47/48), promotion crossfade | Door factor in the direct-SH compose; slab shadows on entity receivers | — |
| Entity indirect, fog | SH grid, shrunk by graft 1 | Navmesh-guided coarsening; portal-gated reach for fog; a coarser grid only if graft 4 lands and an A/B passes | Per-cell probes (leaf seams, Square Enix '372) |
| Reflections | Water probes, SSR, planar spike | Glossy world surfaces (5), after counsel on Tencent WO claim 1 | — |
| Doors, movers | `sealed_portal_ids`, `blocked_portals` flood, promotion | Door classification and influence graph; light → cell reach for runtime lights | — |
| Streaming | Shared read issuer, lightmap cell blocks, SH clusters | Door masks page as per-cell blocks beside lightmap and shadowmask; seam hints derived from door and choke portals | No third streaming system |

### Build order

| # | Step | Gate | Reversible | IP |
|---|---|---|---|---|
| 1 | Texel-step detector + penumbra floor | No texel step within 2 m at the Auto extent | Compile check; bake flag | None |
| 2 | Door classification, door influence graph, light → cell reach + compile report (doors, gated lights, texels, KiB) | None; sizes step 5 | Compile-only | None |
| 3 | Portal-gated reach for runtime lights and fog, from step 2's reach | No regression with doors open | Runtime | Low (Doom 3 prior art) |
| 4 | Surface radiance cache + surface gather for SH base and deltas | Grey single-bounce mode reproduces id 34 within BC6H tolerance on the campaign map; hallway-mini bake time falls; owner A/B on colour bleeding and multi-bounce | Bake path flag | Bake-only; uniform or cosine rays; no shipped patch lists |
| 5 | Door channel, world direct (additive PRL section) | Step 2 sizes; masks ≤ ~10% of lightmap mandatory set; ≤ 0.2 ms per door event on the 5300M | Section droppable | Visibility masks, not a swapped ambient layer (Take-Two, claims unread) |
| 6 | Door factor in direct-SH compose | Entity behind a closed door goes dark, no promotion pop | Yes | As step 5 |
| 7 | Slab shadows | 5300M forward ≤ ~0.5 ms at the kinematic-platform stations; promoted-slot pressure falls | Runtime | Kay–Kajiya 1986; not charted |
| 8 | Probe classification and relocation, fed by step 4's leak counts and navmesh-guided coarsening | Stored tiles and residency fall; no entity or fog regression | Small format change | Appears clear |
| 9 | Compose scoping by residency and visibility | Mac compose at the kinematic-platform station falls | Exact | Frame as a residency gate, never a per-light change trigger ('451 element a) |
| 10 | Optional: bounce in lightmap + coarser grid | 5300M world-SH share (plan item 7) ≥ ~1.5 ms; animated-bounce path chosen; wall-vs-entity and normal-mapped A/Bs | While both paths sit behind a bake flag | Low (Quake 2 / Source) if summed |
| 11 | Optional: shadowmask-mode owners | Step 1 detector; 5300M forward ≤ ~0.3 ms; mask VRAM inside the mandatory set; no-entity soft-penumbra gate re-proved | Bake flag | Valve 2007 prior art; combination not triaged |
| 12 | Optional: emissive bounce, mover rest-pose channel, specular occlusion cones, door bounce deltas | Owner rulings | Yes | Door bounce shares the '451 question with shipped id-27 deltas |

Steps 1–2 are compile-time only.

### Bake-only artefacts

Patent risk sits in runtime update triggers and runtime data layouts, not in bake intermediates. Two source artefacts feed the outputs below; both drop before packing. The portal graph and `sealed_portal_ids` feed the door influence graph.

| Artefact | Built | Derives |
|---|---|---|
| Surface radiance cache: fixed-size patches (face + grid coordinate, stable ids), outgoing radiance (`.prm` albedo × direct irradiance, plus opt-in emissive), optional iteration for multi-bounce; per-light unit caches for animated lights | Before atlas preparation, so stages stay independent of lightmap density; ~24–100 MB bake RAM (*est.*) | SH base and deltas; bounce layer; emissive bounce; specular occlusion cones; leak counts |
| Visibility pass | Today's lightmap visibility samples, supersampled where needed | Texel-step report; distance-to-edge masks; per-face density hints; light → cell reach |

| Output | Verdict | What it does | IP |
|---|---|---|---|
| Surface gather | Keep; first | Probes gather 256 rays from the cache instead of re-shadowing every light at every hit. Hallway shadow rays ~47 G → ~0.3 G, plus 246 M gather rays (*est.*, ~6 lights per hit). Adds colour bleeding and optional multi-bounce, so the look changes | Uniform or cosine rays (ARM '177); never ship patch lists (MS '270 cl. 7) |
| Door influence graph | Keep | Per door: cells, lights, compose rows, texel bounds. Feeds the door channel's light set, compose scoping, seam hints across door and choke portals, a mapper report. A few KB per door | Scopes work; never drops probes at read time ('372) |
| Light → cell reach | Keep | Bits for the cells whose texels each light lit, ~110 KB for 157 lights × 5,700 cells (*est.*). Filtered by blocked portals at runtime, it replaces sphere-vs-reachable-cell tests for graft 2, the ranker pre-cull and fog | Low (Doom 3) |
| Leak counts | Keep | Backface hits among the radiance rays, plus texel samples inside solids → relocation, validity, mapper report | NVIDIA app, Unity Virtual Offset prior art |
| Navmesh-guided coarsening | Keep | Protects probes near navmesh and walkable air from coarsening. Never culls: fog, projectiles and jump arcs need air probes | — |
| Texel-step detector | Keep | Supersamples a sample of texels against the shipped lightmap; warns and suggests `_lightmap_scale`. Makes "edges never show their storage lattice" a compile check | — |
| Shadowmask-mode owners | Improve; later | Each face's ≤ 4 owner lights leave the irradiance bake. Runtime evaluates their diffuse × visibility rebuilt from a distance-to-edge mask (Green 2007) at scene-pixel resolution, one BC5-friendly channel per light, penumbra width per (face, light). Restores per-pixel N·L. Two edges in one texel (railings, cables) fall back to raw visibility | Valve 2007 prior art |
| Edge-driven density | Only if shadowmask-mode owners fall short | Automates `_lightmap_scale` per face; grows post-atlas cache keys and the mandatory set | Heckbert 1990, q3map2 |
| Bounce-layer direction | Improve | Rg8 dominant direction + directionality byte at bounce resolution, as the direct atlas does | — |
| Specular occlusion cones | Optional | Bent normal + aperture from gather rays darken owner specular and probe reflections in crevices; ~2 B/texel coarse | — |
| Emissive bounce | Owner ruling | Opt-in per material. Reverses the recorded rule that emissive surfaces light nothing | q3map2 surface lights |
| Mover rest-pose channel | Later | Door channel for every mover terminus (docked lifts, platforms) | Contact-AO variant needs counsel (Take-Two) |

Killed: link-based probe culling (distance-to-surface and navmesh say the same, and air probes are needed); runtime irradiance gradients (bilinear already reconstructs; as a bake cache only on a fixed grid, to keep cold bakes byte-identical); light trees for entity direct (no per-receiver static occlusion, so they leak through walls; the falloff early-out already took the many-light win); bake-time patch links (ray gathers from the cache do the same job).

### Door classification

Doors are plain `kinematic_mover`s. Bake already auto-detects seals: a portal is sealed when the union of the mover's convex brushes at waypoint 0 fully covers it. The owner chose auto-detect over an opt-in KVP for camera occlusion (`E17--doors-as-occluders`), with a per-mover override named as an additive extension.

Light-door rule: seals ≥ 1 portal at waypoint 0, has a terminus (`once` or `ping_pong` over a finite chain), not a pure rotator. Open state is the last waypoint. A path with more waypoints fades by path parameter, with a warning. Override KVP, e.g. `_light_door` = `auto` / `on` / `off`: `off` for movers that never rest closed; `on` for partial blockers that seal no portal (shutters, grates), gating lights whose reach overlaps the mover's bounds.

| Per door | Baked |
|---|---|
| Lights | Baked-tier static and animated-baked lights whose portal-reachable region crosses a sealed portal. Dynamic-tier lights excluded: pool and slab shadows own them |
| States | Closed and open masks; open mask dropped when the open pose occludes nothing (pocket doors) |
| Surfaces | Only texels whose visibility differs between poses, sparse, per cell block; a per-probe factor for those lights' direct SH |
| Cache key | Door brushes, waypoint-0 and terminus poses, light set. Moving one door re-bakes only its masks |
| Budget | Warn above a per-door byte budget; stop baking that door's lights deterministically once hit |

Auto-detect failures:

- **Missed door** (light leaks, as today): hairline gap breaks coverage; door authored open, so waypoint 0 is not closed; no portal at the door plane, or the door covers one portal fragment; swing doors built as rotators.
- **Over-detect** (correct but paid for): a lift docked over a shaft portal. A mover that never rests at waypoint 0 costs bytes but never engages; the factor applies only when docked closed.
- **Blowup:** a door near many lights multiplies masks; the per-door budget bounds it.

Mapper cost: none for standard doors. Feedback extends the seal log and door debug view with "door X gates lights […], N texels, M KiB".

### Composition with planar

Complementary. The mirror view reuses the world pipelines, so door masks, slab shadows and gated light appear in it for free; graft 1 shrinks planar's SH residency unions. Per-light runtime cost compounds in the mirror pass. Resume the planar spike on today's stack; the hybrid keeps the SH grid, so planar's residency contract stands.

### When the redesign earns its place

Cheapest check first. All must hold.

1. Owner requires bounce that combines correctly across several doors. Today's ruling: not required.
2. Cleanup does not shrink the grid: after graft 1 plus navmesh-guided coarsening, stored tiles stay ≥ 40% of today, or the mandatory set ≥ 60 MiB. Plan item 2.
3. Patches and links fit: hallway links ≤ 256 MB on disk, ≤ 64 MB resident, ≤ 5% energy lost to top-K truncation. Plan item 4.
4. 5300M compose plus world-SH sampling stays ≥ 2 ms after build step 9 and a graft 4 prototype.
5. Per-cell probes show no pop where characters cross leaf boundaries in a large hall. Plan item 8.
6. A carrier for entity direct from baked lights exists: 30+ nearby lights at today's cost, no double count.
7. Counsel clears Square Enix '372 and Tencent '451 against claim text.

## Indirect-light redesign options

Parked behind §When the redesign earns its place. Kept for the record and for grafts it donated (glossy reflections, door gating). All aim to replace the dense SH grid for indirect; none replaces the shadowmask, the shadow pool or the reflection probes. None carries direct light from baked lights onto entities and billboards (ids 35/41/45/47/48, which also hold the promotion crossfade's subtraction). That term lives on the same grid, so the grid stays until it has another carrier, e.g. per-pixel direct for entities.

- **One probe type, two readers** (McGuire et al. I3D 2017). Per-cell radiance + inverse-L1 distance tiles, captured like today's water probes. Bake derives a cosine-prefiltered irradiance mip and Chebyshev moments from the same tiles. Entities, fog and billboards read the irradiance; flagged rough materials read the full tile through today's distance search, hard-bound by cell. Probe count scales with cells. Cells are BSP leaves, not rooms: leaf boundaries cut through rooms, and the large-cell tail is unmeasured.
- **Patch radiosity re-solved on change** (Goral 1984; George, Sillion & Greenberg 1990; qrad 1997). ~0.5 m patches with baked top-K links and the portals each crosses; Jacobi sweeps on the CPU when a door or light changes; links over sealed portals zero. Hallway ≈1.0–2.0M patches (*est.*) against 962,636 valid probes: samples do not fall; the saving is bytes per sample, which graft 4 also gets. Animated lights need a hybrid: B = B_door + Σ c_l(t)·B_l with per-light baked bounce, re-solve on door events only.
- **Probe-to-patch links.** Probe diffuse SH = Σ link weight × current patch radiance, so probes follow doors without ray tracing. Reflection tiles are never re-shaded (that variant reads on Tencent WO2026113651A1 claims 11–13). A lighter variant re-gathers from the lightmap; it scales with probes × links, not lights, but raises '451 element (a) if triggered per light change and Crytek US20140327673A1 if granted.

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
| Glossy surfaces | Water only | Full directional radiance from per-cell tiles | New; also available as graft 5 |

Diffuse bounce is low-frequency; L2 SH holds it nearly as well as the octahedral tile. The patch loss is a format choice, not a design limit.

Animation gains are new kinds of change and lower cost, not more expressive per-light curves:

| Change | Today | Redesign |
|---|---|---|
| Baked-position light pulses or recolours | Per-light grid deltas (id 27), GPU compose per frame; the costly path | Per-light bounce on patches × curve; same flexibility, no GPU compose; patch count at or above today's valid probes (*est.*) |
| Door opens or closes | Promoted and dynamic lights shadowed; unpromoted baked light and bounce pass through (graft 3 closes the first; door bounce is optional) | Sealed-portal links zero; bounce re-solves over a frame or two |
| Mover blocks light | Direct only, through fixed pool slots | Direct via slab shadows for every nearby light; bounce still ignores movers |
| Light in reflection probes | Share ratio: top 4 lights × 5 lobes | Unchanged; runtime re-shading of tiles reads on Tencent WO2026113651A1 claims 11–13 |
| Probe diffuse | Composed from grid deltas | Follows the patch solution through probe links, doors included |
| Moving (dynamic-tier) lights add bounce | No | No. Injecting them needs per-patch visibility to a moving light; only a leaky unshadowed estimate fits |

### Scaling

| System | Today scales with | Redesign scales with |
|---|---|---|
| Entity and fog indirect | Volume ÷ spacing³ × animated reach | Cells (1–3 probes each; BSP leaves, not rooms); hallway ~960k valid SH probes → 2.1k–6.2k (*est.*) |
| Indirect bake | Grid probes × rays × lights in range (11.15 h hallway; may be stale) | Probe captures + patch links, minutes (*est.*) |
| Mover shadows | Fixed promotion slots (rationed) | Lights near movers × movers, analytic |
| Water probes | Fluid face groups (`stress-env-volumes` overflows the 256 cap) | Cells with water |
| Links | — | Surface area; 144–512 MB on the hallway (*est.*), needs per-cell streaming |

## Measurement plan

Items 1–3 and 7 serve the hybrid (the roadmap's door item and hybrid GI measurements); the rest test the redesign conditions. Cheapest first; the first six need no renderer change.

1. **Door classification report** on `stress-warren-showcase` and `kinematic-platform`: doors, gated lights, changed texels, KiB, influence graph → sizes build step 5.
2. **Grid shrink** on the hallway and campaign after graft 1 plus navmesh-guided coarsening: stored tiles, mandatory set → redesign condition 2.
3. **`stress-warren-lit`, `stress-warren-crates`:** lights within reach of each mover → slab-test cost vs promotion rationing.
4. **`stress-warren-hallway-inspection`:** cells, cell volumes and the large-cell tail, surface area by patch size → real probe counts, link bytes, patch counts.
5. **`stress-env-volumes`:** probes per cell vs per fluid volume.
6. **`stress-warren-showcase`:** cells per door event and animated reach in patches → door-event CPU and the animated hybrid.
7. **5300M at 4K:** forward pass with world SH on vs off → build step 10, redesign condition 4.
8. **Visual A/B:** characters lit from per-cell probes (averaged from today's probes) in a large hall and under lightstyles.
9. **Glossy world-surface cost on the 5300M**, with a per-material pipeline variant and a fix for the forward pass's 16-texture floor.

### Measurement pitfalls

- `stress-warren-hallway-inspection` at 1 m SH spacing bakes in 11.15 h: unattended runs only. The hallway mini at 1 m projected ~46 min.
- On 2026-09-18 the full warren PRL would not render on the 6 GiB 1660, and `POSTRETRO_GPU_TIMING=1` device-lost NVIDIA driver 616.92 on the first frame (`lighting-scale--adaptive-probe-spacing`). Streaming has since landed and the hallway now runs; recheck both before relying on GPU timing there.
- The 5300M can lack timestamp queries; measure with Metal System Trace (`rendering_pipeline.md` §12).
- The forward pass has a fixed 16-texture floor and the fluid fragment stage is at 8/8 storage buffers; any spike adding bindings hits these first.

## Open owner decisions

- Light-door override KVP: name, and whether `auto` is the default.
- Whether door bounce (build step 12) is worth its bytes once step 5 lands.
- Coloured, multi-bounce indirect from the radiance cache: a look change, decided by A/B (build step 4).
- Emissive bounce: reverses the recorded rule that emissive surfaces light nothing.
- Shadowmask-mode owners: moves owner lights' diffuse from bake to runtime.
- Which materials may read glossy probe reflections, and who flags them.
- Redesign only, if unparked: tolerance for flatter entity and fog lighting in large cells (or a probes-per-large-cell rule); whether patches store L1 SH (better normal response, more bytes).
- Allowing patent hosts in the environment's network policy so the newer filings can be charted against claim text (hosts and counsel questions in [ip-check.md](ip-check.md)), and when to engage counsel.

## Provenance

- **Verified against `context/plans/done/`:** 41.93 / 4.11 ms compose frame interval; 194,028 grid probes and 55.88 MB composed atlas (campaign); 962,636 valid probes and 2,082 cells (hallway); hallway SH 193.8 MiB, 3,385 clusters, mandatory 49–125 MiB; lightmap 136.5 MiB mandatory, 224 MiB at install, ~295 MiB with animated data; 11.15 h hallway bake; 5300M ~3.0–3.5 µs per composed row; 0.17–1.55 ms compose after the row filter; 198 campaign cell blocks; the 16-texture floor in code.
- **Estimates from agent rounds, unmeasured:** every *est.* figure, all patch, link and probe-tile sizes, glossy cost per megapixel, door-event CPU, every confidence percentage, and every threshold in the hybrid build order and the redesign conditions.
- **Independent review (2026-10-23):** the hybrid stack, door channel, bounce in the lightmap and light-door rule are that reviewer's reasoning, unmeasured. Verified in source: seal auto-detection at waypoint 0, door as plain `kinematic_mover`, the 5300M post-filter compose totals, campaign stored tiles 194,028 → 57,128.
- **Bake-only round (2026-10-23):** artefact verdicts and costs are the reviewer's, unmeasured. Verified in source: 256 rays per probe, per-hit area-sampled re-shadowing, grey 0.45 bounce albedo, black sky, 0.04 m lightmap texels, 1 m default probe spacing, the recorded emissive boundary.
- **Patent re-chart (2026-10-23):** see [ip-check.md](ip-check.md) for source quality per filing.
- **Elsewhere:** the planar contract exists only on the owner's Windows worktree (`claude/planar-water-spike`, unpushed as of 2026-10-22); cell-rate findings on `claude/cell-rate-spike-1660` under `context/plans/drafts/reflection-probe-fluid-pass-performance--cell-rate-spike/`.

## Rejected or parked

- **Voxel albedo + SH relight at an SDF hit:** needs the SDF and a cell-rate pass; blocky 0.5 m look.
- **Multi-probe blend over one water body:** occlusion-conditioned probe switching sits on Tencent WO2026113651A1 claims 15/17.
- **Mirrored-entity-only draw:** splitting dynamic from static content reads near NetEase CN117496039A; the planar spike draws the static world for that reason.
- **Light propagation volumes:** the empty-space debt by construction.
- **GPU baking:** on hold. Would need an exception to "renderer owns GPU", fixed-order reductions for byte-identical caches across Metal and DX12, and a CPU fallback for CI.
- **Per-texel hit re-shading in probes:** Tencent claims 11–13.
