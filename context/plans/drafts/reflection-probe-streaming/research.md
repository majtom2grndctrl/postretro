# reflection-probe-streaming — research

Derivations behind the brief's decisions. Read at 949a79411, on top of `feature/reflection-probes` (tasks 1–8 built; 8b, 9, 10 open).

## Why stream
Today one 128² BC6H tile is 16 KiB per probe. At the 256 cap the stress map's probe VRAM is ≈4.14 MiB: array 4 MiB, table 61 456 B, probe-index buffer 81 040 B (`reflection-probes` `plan.md` §Task 9b, stress map). On bytes alone streaming is not worth it, which is why `reflection-probes` ruled it out. Two other limits are what actually bind:

| Limit | Where | Effect today |
|---|---|---|
| Portable array layers (256) | `PORTABLE_MAX_TEXTURE_ARRAY_LAYERS`, one array, layer = probe | Level probe count ≤ 256 |
| One uniform binding (64 KiB) | `FLUID_PROBE_TABLE_BYTES` = 16 + N × 240 B, indexed by level probe id | Level probe count ≤ ~272 even with more layers |

`stress-env-volumes.map` keeps 256 probes and drops 460 derived past the cap (plus 262 inside-another-fluid drops, which are correct and stay). Lifting both limits means indexing the array and the table by something bounded that is not the level probe id. That is a pool slot, and once state is slot-indexed, what fills the slots is a residency policy. Streaming is that policy.

The bytes argument changes with format growth, which can only add to the record: a distance layer for parallax (proposed in the reflection-probes session, outcome open) doubles it; a larger authored tile (additive per `reflection-probes` `research.md` §Resolution) quadruples it at 256²; a roughness chain for glass or stone readers adds a third. Whole-resident, those multiply against a level probe count that is no longer capped. At 2 000 probes with a distance layer, that is ≈62 MiB resident for the whole level against the Low tier's 256 MiB for lightmap-shaped data (`large-map-spatial-residency.md`, owner 2026-09-28). The pool holds the same footprint as today regardless of level size.

## Budget
- Pool: slots × layers-per-unit × tile bytes. One array at the portable limit, one layer per unit: 256 slots, 4 MiB BC6H. With a two-layer unit: 128 slots in the same 4 MiB, or a second array if the per-cell fit drops too much (decided then, with figures).
- Slot table: 16 + 256 × 240 = 61 456 B, unchanged, still one uniform. Per-slot rows upload with tiles: 240 B per install.
- Per-probe map: 4 B per level probe (slot + resident flag), vertex-stage only. A storage buffer is unbounded; a vertex-only uniform would cap the level at 16 384 probes. Fluid vertex storage is 5 of 8 (`fluid_pipeline_vertex_storage_and_sampled_textures_stay_within_limits`); fragment storage is 8 of 8.
- CPU: records 348 B + relation per probe; tiles are not retained in stream mode (today `LevelWorld.reflection_probes` keeps them all).
- Drain: one install is 16 KiB + 240 B; the in-play drain budget is 8 MiB, so probes never contend meaningfully with SH or lightmap installs. The drain rank still orders them, so a mandatory probe is never queued behind optional SH.
- Compile: the stress map's probe stage took 11.49 s for 256 probes; capture cost scales per probe, so keeping several hundred more costs tens of seconds on a 1750 s cold build. The relation bake is a face-bounds against cell-bounds pass.

Per-device sizing is unnecessary at this size: the pool is a fixed 4 MiB, under the noise floor of any tier. It still counts against the tier when the tier planner lands. It is whole-resident from that planner's point of view, because it never grows and cannot usefully yield.

## Rivals
- **Whole-resident, several tiles per layer.** Pack 2×2 or 4×4 tiles per 256² or 512² layer and replace the uniform table with a texture or a bigger binding. That lifts the cap with no residency machinery and is proportionate at today's record size (≈4–31 MiB up to 2 000 probes). But VRAM grows with probe count times record size, and the fluid fragment stage has no free storage binding for a bigger table. The streamed pool cannot usefully yield either, so what separates the two is bounded versus growing VRAM, not reclaimable memory. Rejected by the owner on that growth.
- **Cross-entity grouping.** Suspected of inflating the stress map's count; it does not. Every generated fluid draws its own top height, so merging across entities at one height saves at most a few percent; merging across heights per room would fit the old cap, at a parallax cost on higher planes (`reflection-probe-seams` `research.md` §Grouping evidence). Not a substitute for streaming; real stress content will still outgrow one array.
- **All-resident below the pool.** Streaming only levels over the pool would spare ordinary maps reads, settle waits and fades. Rejected by the owner: every streamed map runs the full path, eviction included, so it stays exercised (`development_guide.md` §1.3).
- **Pool sized by device tier.** Needs the tier planner, which is unbuilt; its Auto heuristic is open (wgpu 29 has no portable VRAM query). Waiting on it buys nothing at 4 MiB.
- **Pool growth like SH and lightmaps.** Those pools grow past their caps for mandatory demand, up to the device layer limit. Here the per-cell fit guarantees mandatory demand fits, so growth would only serve a broken compile guarantee. A refusal-as-miss plus one log line covers that case.

## Fit set
The runtime's mandatory set is visible ∪ lead ≤ L, where L is runtime-tunable (default `DEFAULT_LEAD_METRES` 16; the dev slider reaches the id-51 bake's `MAX_LEAD_METERS` 32). Id 51 holds, per camera cell, every cell within portal-path lead ≤ 32 m plus each such cell's sampled visible set dilated one hop, the camera cell's own visible set included (lead 0). Checking the fit over a camera cell's whole id-51 list, at the section's maximum lead, covers every L the runtime can use and the visible set. It is conservative: an open stress layout may drop more than a default-L check would. The brief keeps the sound choice and asks for the stress-map figures before the build passes the compiler slice.

The per-cell drop loop: while any camera cell's fit set exceeds the slot count, drop the smallest-bound-area derived probe among the probes in over-full cells' sets (ties: fluid entity order, then face order), then recompute the affected cells. A cell still over with only authored probes refuses. It is deterministic because ordering is total and independent of `--jobs`. It runs after every other drop (`reflection-probes` Decisions §Drops and cap), replacing the level-wide cap step.

Face-to-cell relation: the fluid collector draws whole volumes when a volume's surface AABB touches a visible cell (`FluidSurfaceRenderCollector::collect`). Demand keyed per face rather than per volume is tighter. A canal through ten rooms then demands only the probes of the rooms in reach, and the faces in unreached cells are not seen, because the visibility contract makes the visible set a superset of what is seen (`rendering_pipeline.md` §2). A face whose cell is outside demand but drawn through its volume takes the miss policy: no reflection on a face the camera cannot see.

## Miss policy
On interior maps a miss should be rare: the per-cell fit plus settle means a miss needs travel outside the lead in one step (teleport, spawn, level script), a read slower than walking a lead's worth of distance, or a settle timeout. Three options were weighed:
- **Unbound look plus fade (chosen).** It reuses the existing "bound to no probe" path, which is pixel-pinned by the reflection-probes acceptance rows, and needs one per-slot install time and a scene clock in the shader. The SSR draft (`fluid-ssr-reflections`) already fades an off-screen miss to the probe term and an unbound face to nothing, so a non-resident probe reads as unbound there too, with no new state.
- **Hard pop.** Visible on big water at grazing angles, which is where Fresnel makes the reflection strongest.
- **SH in the reflected direction.** Blurry, can itself miss (SH streams), and adds the SH read the reflection-probes light-share design removed (`reflection-probes` `research.md` §Light share). It remains a candidate fill for faces whose probes are *dropped*, keyed by binding (`reflection-probes` `index.md` §Path); that is separate work.

Fade and light share are independent: the ratio multiplies the capture, and the fade multiplies the reflection weight F. A light switched during a fade shows at once.

## Lifecycle
```
install:   records + bindings + relation decoded (no tiles)
           pool, slot table, per-probe map (all non-resident), index buffer created
settling:  demand from presented pose → mandatory + drawn probes read (settling budget)
           → tiles + row uploaded to free slot, map entry set in that submission
           chokepoint: all mandatory + drawn resident → reveal (probes at full weight)
play:      camera cell / L change → demand recomputed
           departed probe: hysteresis → map entry cleared → slot freed
           arriving probe: read → drain admit → upload tiles + row → map entry set → fade from 0
unload:    pool, table, map, index buffer released; issuer route dropped with the level
```

## Pin table
| Id | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| S1 | Slot reuse in one drain | Probe A's map entry cleared; B's tiles and row uploaded to A's old slot; B's entry set | No frame reads A's entry → B's data or B's entry → A's data |
| S2 | Install and evict one probe in one drain | Probe enters and leaves demand before its drain | Probe non-resident, slot free, no map entry set |
| S3 | Reinstall mid-fade | Probe installed at frame N, evicted at N+k before full weight, reinstalled at N+m | Weight restarts at 0 at N+m |
| S4 | Reveal frame | Probes installed during settling; reveal at frame R | Weight 1 at R, no fade |
| S5 | Teleport | Camera jumps to a cell with no resident probes | Faces render unbound; probes install within the drain budget and fade in; never a previous probe's tile |
| S6 | Light switch during fade | Light off at frame N while probe fades | Reflection darkens by the light's share at N; fade continues independently |
| S7 | Pre-relation section | Level built before this brief | Installs all-resident, draws as before, settle answers not streamed |
| S8 | Runtime overflow | Mandatory probes exceed slots (corrupt or hand-built section) | Excess refused as misses, one log line, no crash, no eviction of a mandatory probe |
| S9 | Level change mid-read | A probe read in flight when the level unloads | Delivery dropped; no write into level B's pool |
