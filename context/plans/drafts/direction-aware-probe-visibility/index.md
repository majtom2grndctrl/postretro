# direction-aware-probe-visibility

Brief · compact · reads: `context/lib/rendering_pipeline.md` §Indirect, `context/lib/build_pipeline.md` §PRL section IDs, `context/lib/testing_guide.md` §Resource bounds · read at 2968de0ad

## Problem
Owner-raised, from the probe channel's leak diagnosis (`in-progress/mover-light-flow--probe-channel/`): an observed defect. Each SH probe weights by one depth mean and variance averaged over every direction (`sh_corner_depth_visibility`), so a probe beside a surface thinner than the probe spacing trusts its long open-room average and lends light to points behind that surface. The static-door reference closet glows at 0.50 (0–255) where it should read dark; every thin static wall leaks the same way when its sides are lit differently. When done, visibility tests depth in the direction of the shaded point: static thin occluders stop leaking, open doorways read as before, and probe relocation and closed-door depth for movers can later add tiles without a storage redesign.

## Decisions
- **Per-direction moments, continuous weights.** Selected probes carry an octahedral tile of `(E[d], E[d²])`; Chebyshev runs on the texel facing the shaded point (Majercik et al. 2019). IP rule, verbatim from `context/research/gi-rtx-lite/ip-check.md`: "Select diffuse probes by cell adjacency (own cell, then portal-adjacent) with continuous moment weights; store no per-probe zone or cell visibility mask and make no binary visible decision." Tile presence picks which moments a probe weighs with; it never removes a probe. Choosing the texel that faces the shaded point sits near Square Enix '372's zone element (`ip-check.md` Piece 2, needs counsel); the owner accepts building it while counsel question 8, whether a continuous Chebyshev weight escapes that claim, stays open.
- **Sparse, chosen by geometry.** The bake gives a tile only to a probe whose visibility toward a valid lattice neighbour changes materially between its averaged and per-direction moments. Depth alone decides: lighting can change at runtime, geometry cannot. A probe without a tile keeps its averaged moments and weighs exactly as today. This departs from the roadmap entry's tile on every probe, which costs 125–346 MiB.
- **6×6 tiles plus a one-texel border, `Rg16Float`.** Tile size made no cost difference on either GPU; 6×6 reaches 8×8's quality for about half the memory.
- **One new PRL section, id 59, optional.** Absent means no tiles, and the level renders as today. Id 57 stays the probe channel's. Tiles are baked from each probe's bake position, the one irradiance and moments use, so relocation moves all three together.
- **Whole-resident, bounded at bake.** Sparse tiles are small (`research.md` §Memory) and never enter the SH streaming pool. Tile count grows with a map's thin walls, so the bake ranks candidates by visibility change and keeps tiles up to a 16 MiB level cap, logging the dropped count; the build summary reports tile bytes (`development_guide.md` §1.4, rank to a fixed budget and drop the overflow).
- **No binding beyond the tile texture.** Forward fragment textures are at the device's 16. The tile texture joins the shared SH group as vertex and fragment visible; the direct-SH atlas drops fragment visibility there, since no fragment stage of that layout samples it. The mesh superset keeps it for movers and skinned meshes. A probe's tile reference rides in its existing moments texel: the indirection word has no spare bits, but a mean distance is never negative. This spends the slot the direct atlas frees, leaving forward at 16 again, and every level pays the declared texture (about 1 ms at 4K on the Mac) with or without id 59.
- **The tile reference is runtime-writable data.** Streaming already rewrites that texel; a later mover state redirects a probe to another tile through the same write, with no layout change.
- **Cheap lookup.** No normalize before octahedral encoding (the L1 divide already normalizes), no per-corner texture-size query, no runtime integer divide. The Mac's first lookup spent most of its cost on that math, not the fetch (`research.md` §Mac).
- **Floor and bias unchanged.** `SH_DEPTH_MIN_VISIBILITY` stays 0.03 on every path, keeping weights clearly continuous while counsel question 8 is open. The residue it leaves behind a static door (station 3 about 0.05) is accepted; the spike found depth bias moves nothing.
- **No player option.** Off would restore the leak (`player_options.md`, tiers are cost levers). A performance ceiling holds the cost instead.
- **Non-goals.** Fog: it skips depth visibility today and keeps skipping it; per-step tile reads cost far more there, and it gets its own brief if its bleed shows. Closed-door tiles for movers and the mover light channel: `mover-light-flow--probe-channel` owns them, including whether a door's tile switches on dock or blends with travel; probe depth is baked with movers absent, so this brief leaves a closed mover door glowing. Probe relocation: roadmap's own entry. The floor's residue and the spike's unexplained remainder: revisited once counsel answers, or in their own brief if still visible.

## Acceptance
Closet stations, capture method and spike figures: `research.md` §Fixture and §Spike. Brightness is post-tonemap sRGB 0–255.

### Automated
- [ ] Today's build still glows at the static-door reference (station 3 mean ≥ 0.4), so the fixture reproduces the defect.
- [ ] With tiles, static-door station 3 mean ≤ 0.06 and station 2 mean ≤ 0.02.
- [ ] Open doorway: station 2 and 3 frame means within 0.02 of today's, and no more than 0.1% of pixels move by more than 2 levels.
- [ ] Hallway Q2 views: no pixel darkens by more than 2 levels against today.
- [ ] Mover-door closet reads as today (regression guard recording scope).
- [ ] A PRL without id 59 loads and renders byte-identical to today's build.
- [ ] A probe without a tile computes the same visibility as today, bit for bit, beside a tiled neighbour.
- [ ] Rewriting one probe's tile reference switches its visibility to the new tile on the next frame, with no new binding or layout.
- [ ] Bake: tiles are deterministic across two builds; selection on a fixture with no thin occluder emits no tiles; every closet probe with a blocked valid neighbour gets one.
- [ ] Forward fragment textures stay at 16 with cube arrays and 15 without; every pipeline sharing the SH group stays within its stage limits; DX12 FXC compiles all affected shaders.
- [ ] Tile bytes appear in SH allocation accounting while the level is installed and return to zero after unload.
- [ ] Hallway tile memory ≤ 5.3 MiB (spike count at 6×6, plus 25%).
- [ ] A fixture whose candidates exceed the cap emits exactly the cap's worth, highest visibility change first, and logs how many it dropped.

### Manual
- [ ] Forward-pass cost over today's build at the hallway Q2 views stays within the ceiling: +1.5 ms per view at 4K on the Radeon Pro 5300M, +0.45 ms per view at 1080p on the GTX 1660. Over the ceiling stops the build for the owner; it never starts a tuning loop.
- [ ] Owner in-engine: closet from inside with the door shut; thin walls and doors on the hallway; open doorways unchanged.
- [ ] Observation only: fog beside a lit closed door. Report whether bleed shows.

## Path
- Bake: bin the gather hits `bake_probe_rgb_with_moments` already traces (`sh_bake/probe.rs`) into the tile; no second trace. Selection reuses the spike's 26-neighbour comparison.
- Runtime: tile texture beside the dense moments in `sh_bind_group_layout_entries`; `append_shared_bind_group_layout_entries` owns binding 15's visibility. Streaming rebuilds bindings 14 and 15 in `sh_streaming/gpu/setup.rs` and `bindings.rs`, and `pack_moment_upload_spans` rewrites moments texels: the tile reference must survive it. `group3_shader_bindings_are_represented_by_rust_layout` pins the binding set.
- Lookup: a power-of-two tile row lets the shader find a tile with shifts and masks.
- Accounting: a new `ShAllocationKind` beside `DepthMoments`.
- Fixture: port `mover-closet` and its static-door and no-door variants from `spike/per-direction-depth` into `content/dev/maps`.
- Rivals: dense tiles streamed through the SH pool (125–346 MiB, and +2.7–3.2 ms at 4K on the Mac with every probe tiled); weighting by cell adjacency alone (a partial wall's ends are adjacent, and dropping a probe is a binary decision the IP rule bars); one occluder plane per probe (reads on '372's second independent claim).
- First slice: bake closet tiles, hook the lookup, capture station 3. It tests both the quality claim and the binding plan.

## Open questions
- Record closed-pose depth tiles, and the switch-or-blend choice, in `mover-light-flow--probe-channel`'s Decisions; its paused plan expects this brief to fix its closet. — owner: drafter — **blocks build**
- Pin the Mac ceiling and 720p reference from the finished timing report; current figures come from the interim run log. — owner: drafter — **blocks build**
- Hallway Q2 view poses for the Windows ceiling run: reuse the spike's capture scenes. — **delegated**

## Boundary inventory

| Name | Rust | Wire | WGSL |
|---|---|---|---|
| Probe depth tiles | `SectionId::ProbeDepthTiles` | id 59 | `sh_depth_tiles` |

## Wire format
Id 59 mirrors id 34's conventions: little-endian, f16 as raw `u16` bits. Header: `u32` version (1), `u32` tile edge (6), `u32` border (1), `u32` tile count. Then `tile count` × `u32` dense lattice probe index, ascending, unique. Then `tile count` tiles, in index order, each `(edge + 2·border)²` texels of `[f16 E[d], f16 E[d²]]`, row-major. Zero tiles: header only. A section that fails these rules refuses the level by name.
