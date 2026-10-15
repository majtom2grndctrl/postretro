# reflection-probe-seams

Brief · compact · reads: `rendering_pipeline.md` §7.3a, §10 · `build_pipeline.md` §Compiler pipeline, §PRL section IDs · `resource_management.md` §5 · read at 949a79411 · builds after `reflection-probes` lands

## Problem
Owner-requested capability. Water reflections step where the probe changes: wherever two adjacent pieces of water at one height bind different probes, the reflection jumps at the shared edge. There are two causes. The compiler groups derived probes within one fluid entity, so water beside sludge in one basin, or two abutting pools at one height, get separate probes with different capture points. And a canal running through a doorway binds one probe per room, by design, because a projection box is right only when it matches its room (`reflection-probes` Decisions; `research.md` §Seams, which accepted the step and named blending as the follow-up). When this is done, water at one height reads as one continuous reflection: touching pieces in one room share a probe, and across a room boundary the reflection crossfades between the two rooms' probes over a short band, with no step at the edge.

## Decisions
- **Bake: derived groups span touching fluid entities.** Faces of one fluid entity group by box match as before; faces of different entities, whatever their fluid, also join when their boxes match and their water shares an edge at one plane height. A group is a connected component of that relation. Requiring contact fixes the seam where two fluids meet without pulling a separate pool in the same room onto an off-centre capture point, since the merge buys no probe count (`research.md` §Grouping evidence). Heights still never merge: a capture point serves only planes at or above it, and merging heights trades parallax on the higher plane for a probe the stress map does not need (`research.md` §Grouping evidence). A merged probe's record names the earliest member fluid entity in map order; warnings name every member entity. It supersedes the per-entity grouping rule in `reflection-probes` Decisions and `resource_management.md` §5.1.
- **Runtime: region-distance blend.** A probe's region is the water bound to it at one plane height. Each probe whose region lies within the blend width W gets the weight max(0, 1 − d / W), where d is the fragment's horizontal distance to that region, so its own region weighs 1. The weights are normalised to sum to 1. Every face near a point then sees the same probes at the same distances, so the mix is continuous across every binding boundary, a doorway piece narrower than W included. A probe whose region lies beyond W of every point on a face takes no part on that face, which is unchanged there (`research.md` §Blend math). Rejected rivals: pairwise ramps per shared edge, which step along the edge of any piece narrower than W, and one probe stretched over both rooms, which projects every far wall wrong (`research.md` §Rivals).
- **Bounded cost.** A face carries at most two nearby probes besides its own, the two whose regions come nearest the face (ties to the lower probe index), so a fragment takes at most three probe samples, and one wherever no other region is within W. Continuity holds wherever at most three probes' regions lie within W of a point; a fourth is the one accepted break. Each sample applies its own probe's live-lighting ratio before the mix; Fresnel weights the mixed reflection once. This relaxes the parent's fixed per-pixel cost to a bounded one inside bands; whether the extra samples need a cost lever is the spike's to settle (Open questions).
- **W is engine-owned**, one constant shared by the compiler's nearby-probe selection and the shader, pinned by a parity test. It hides a seam rather than styling the reflection, so it is an exception to "reflection look is the author's" (`rendering_pipeline.md` §7.3a, Style).
- **Baked in section 54.** The compiler finds each bound face's nearby probes and, for each, the boundary segment of its region nearest the face. Baking keeps one relation for every consumer: `reflection-probe-streaming`'s per-cell fit counts nearby probes, so compiler and runtime cannot disagree on what a face may sample (`index.md` §2). Nearby probes are part of each binding, so they extend section 54 at its next internal version rather than a section of their own; whichever of this brief and `reflection-probe-streaming` lands second takes the version after.
- **Prior art and IP.** Distance-weighted blending of local probes is Lagarde & Zanuttini 2012 (open). The parent's avoid list still holds: no technique choice by direction, no reprojection, no precomputed intersection table, and every probe index a fragment samples comes from the face's binding data alone, never from a lookup direction (`reflection-probes` `research.md` §Prior art and IP).
- **Existing contracts hold.** Pixel style evaluates the blend weight once per water texel cell, so every fragment of a cell still reads one reflection. A face with no nearby probe, a face bound to no probe, the underside, the lighting-mask gate and the zero-opacity draw rules render exactly as `reflection-probes` defines them. No fluid fragment binding is added; nearby-probe data rides the vertex stage.
- **Faces at different heights never blend.** A step in the water surface is a visible edge already.
- **Non-goals.** Blending across heights or into unbound water. Influence volumes that overlap rooms (Lagarde's authored influence boxes): the region band needs no new authoring. Blending for non-fluid readers.

## Acceptance
### Automated
Compiler
- [ ] Two fluid entities (water and sludge) whose water touches at one height in one room share one derived probe; the same two apart, not touching, keep a probe each; at two heights they get two; at one height in two rooms joined by a doorway they get one probe per room, as today. Two separate pools of one entity in one room still share, as today.
- [ ] A match chain across three entities is one group whatever the entity and face order; the merged record names the earliest member entity; a drop warning names every member entity.
Pin ids refer to `research.md` §Pin table.
- [ ] Nearby probes: faces of one canal binding two rooms' probes each record the other with their shared edge as its segment; with a doorway piece narrower than W, each room's faces near it record both the piece and the far room, the far room's segment being the piece's far edge; B3; B6; a face beyond W of every other region records none.
- [ ] `--jobs 1` and `--jobs 4` emit byte-identical section 54; the stage's cache key misses when a region boundary moves.
Format
- [ ] Section 54 at its next internal version round-trips nearby probes; one naming a probe past the count, or the face's own probe, fails the load naming the section; an earlier version loads and blends nothing.
Runtime, CPU references pinned to the shader
- [ ] Continuity: fragments a hair either side of a binding boundary differ by no more than a pinned epsilon, for smooth and pixel style, with one light away from its reference in only one probe: two rooms (B1); along both edges of a doorway piece narrower than W (B2); B5.
- [ ] Beyond W of every other probe's region, and on every face with no nearby probe, output equals today's single-probe output exactly.
- [ ] Weights stay in [0, 1] and sum to 1 everywhere, a point within W of three regions included.
- [ ] Pixel style: every fragment of one water texel cell reads one blended reflection.
- [ ] B4: a light switched off that only probe Q stores darkens P's water only inside the band, by Q's weight there.
- [ ] Underside, lighting-mask gate and zero-opacity draw rows from `reflection-probes` still pass unchanged.
- [ ] Source scan: every probe index the fluid shader samples derives from the face's binding inputs alone; no lookup-direction value reaches one.
- [ ] Budgets: fluid fragment binding counts unchanged; vertex attributes and inter-stage variables within the device limits. DX12 FXC compiles the fluid shader (ignored-gated, Windows).
Adapter-backed: each row's run log shows the GPU case ran, not skipped
- [ ] Two faces sharing an edge, bound to probes with uniform captures R1 and R2: at the edge both read the encoding of the mean; past the band each reads its own colour.
### Manual
On `content/dev/maps/reflection-probes-test.map`'s two-room canal, plus a canal brush that crosses a wall unsplit (so a doorway piece binds its own probe) and a basin holding water beside sludge at one height:
- [ ] Neither canal's room boundary shows a step, the doorway-piece crossing included; the band reads as a soft crossfade, not a doubled image that draws the eye at play speed.
- [ ] Water beside sludge in one basin reflects as one continuous image.
- [ ] Fluid-pass GPU-time delta against the build before this brief, at a canal-doorway pose and at a pool pose away from any edge: 1660 Super with `POSTRETRO_GPU_TIMING=1`; 5300M via Metal System Trace (owner). Probe count on `stress-env-volumes.map` before and after cross-entity grouping. Report in the plan of record.

## Path
- **Grouping.** `derive_probes` partitions by `faces[..].volume` before `box_match_components`; lift the partition to plane height across volumes and add the cross-entity contact test to the union. Contact is a shared edge within the compiler's flush tolerance, the same edge test the nearby-probe bake needs. `box_match_components` is all-pairs; per plane across a level, bucket by box bounds if a stress map makes it slow. Cap tie order keys on the group's first face, already in fluid entity order.
- **Nearby-probe bake.** Id-52 faces carry vertices but no adjacency. Per plane height, a probe's region is the union of its bound faces; for each face, the regions within W and each region's boundary segment nearest the face come from polygon-to-polygon distance. Interior edges between two faces of one probe are not boundary. The 1 cm plane merge in `plane_heights` is the precedent for heights. Store each as its probe index and segment endpoints, against the reserved no-probe value. A single segment stands in for a region whose nearest boundary bends (an L-shaped region); the error is a weight slightly low near the bend, delegated below.
- **Runtime.** The probe index rides vertex buffer location 4 and reaches the fragment flat (`vs_main`); nearby probes ride a further vertex buffer written once at install, after `create_probe_index_buffer`. `fluid_reflection_sample` is the per-probe body; call it per contributing probe and mix before Fresnel. The pixel-style cell centre already exists for the single sample.
- **Cost watch.** Each extra sample repeats up to K descriptor evaluations (`reflection-probes` `plan.md`, live-lighting cost). The reflecting variant of the two fluid pipelines (task 8b) carries the blend; the e24 codegen sensitivity recorded in `plan.md` §Task 9b is the risk to measure first.
- **Streaming.** `reflection-probe-streaming` treats nearby probes as demand; until it lands every probe is resident.
- **First slice**, after the spike: nearby probes for the test map's canal and an unsplit doorway crossing, smooth style, real share tables, adapter row: proves continuity and the band, at the cost the spike accepted, before grouping or pixel style.
- **Large files.** `fluid_surface.wgsl` and `fluid_surface.rs` are near or past 800 lines; split first, behaviour-preserving, own commit, if this extends them.
- **Docs at promotion.** `resource_management.md` §5.1 (grouping, nearby probes); `rendering_pipeline.md` §7.3a (blend band); `build_pipeline.md` id 54.

## Wire format
Section id 54, next internal version. Bindings gain, per (volume, face, probe) triple, two nearby-probe entries, each a probe index (the reserved no-probe value when absent) and the boundary segment of that probe's region nearest the face, as two endpoints in engine X and Z, `f32`. Entries are ordered nearer region first, ties to the lower probe index. Everything else is unchanged; an earlier version loads with no nearby probes.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Nearby probes | section 54 binding field | id 54 | n/a | n/a | n/a |

## Open questions
- Cost of up to three samples, each with its own live-lighting evaluation, and which lever applies if it is over budget (per-probe ratio precompute, a stochastic per-cell probe pick, or a third pipeline variant) — owner — **blocks build**: settled by a spike per `experimental_spikes.md` before this brief is finalized. The spike measures a three-probe band with real share tables at a view-filling pose, on the GTX 1660 Super (`POSTRETRO_GPU_TIMING=1`) and on the owner's MacBook Pro (Radeon Pro 5300M; Metal System Trace where timestamps are unsupported, owner-run). It runs after the parent's timed work frees the machine.
- W's value, and whether the falloff is linear or smooth (any continuous falloff keeps continuity) — **delegated**: pin in the CPU reference; the owner may tune W at the manual canal rows
- Whether a bent nearest boundary needs a second segment per nearby probe — **delegated**: measure the weight error on an L-shaped fixture and report
