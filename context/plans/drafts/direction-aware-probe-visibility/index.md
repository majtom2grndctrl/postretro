# direction-aware-probe-visibility

Brief · compact · reads: `context/lib/rendering_pipeline.md` §Indirect, `context/lib/build_pipeline.md` §PRL section IDs, `context/lib/testing_guide.md` §Resource bounds · read at 2968de0ad

## Problem
Owner-raised, from the probe channel's leak diagnosis (`in-progress/mover-light-flow--probe-channel/`): an observed defect. Each SH probe weights by one depth mean and variance averaged over every direction (`sh_corner_depth_visibility`), so a probe beside a surface thinner than the probe spacing trusts its long open-room average and lends light to points behind that surface. The static-door reference closet glows at 0.50 (0–255) where it should read dark; every thin static wall leaks the same way when its sides are lit differently. When done, visibility tests depth in the direction of the shaded point: static thin occluders stop leaking, open doorways read as before, and probe relocation and closed-door depth for movers can later add tiles without a storage redesign.

## Decisions
- **Per-direction moments, continuous weights.** Selected probes carry an octahedral tile of `(E[d], E[d²])`; Chebyshev runs on the texel facing the shaded point (Majercik et al. 2019). IP rule, verbatim from `context/research/gi-rtx-lite/ip-check.md`: "Select diffuse probes by cell adjacency (own cell, then portal-adjacent) with continuous moment weights; store no per-probe zone or cell visibility mask and make no binary visible decision." Tile presence picks which moments a probe weighs with; it never removes a probe.
- **Open counsel question accepted.** Choosing the texel that faces the shaded point sits near Square Enix '372's zone element (`ip-check.md` Piece 2, needs counsel). The owner accepts building it while counsel question 8, whether a continuous Chebyshev weight escapes that claim, stays open.
- **Sparse, chosen by geometry.** The bake gives a tile only to a probe whose visibility toward a valid lattice neighbour changes materially between its averaged and per-direction moments. Depth alone decides: lighting can change at runtime, geometry cannot. A probe without a tile weighs exactly as today. This departs from the roadmap entry's tile on every probe, which costs 125–346 MiB.
- **8×8 tiles plus a one-texel border, `Rg16Float`.** Cost matched at every size on both GPUs; 8×8 reads darkest behind the static door and flags fewer probes than 6×6; 4×4 leaves station 3 at 0.13.
- **One new PRL section, id 59, optional.** Absent means no tiles, and the level renders as today. Id 57 stays the probe channel's. Tiles are baked from each probe's bake position, the one irradiance and moments use, so relocation moves all three together.
- **Whole-resident, bounded at bake.** Sparse tiles never enter the SH streaming pool. Tile count grows with a map's thin walls, so the bake ranks candidates by visibility change and keeps at most 32,768 tiles per level, the 15-bit reference's limit, logging the dropped count; the build summary reports tile bytes (`development_guide.md` §1.4, rank to a fixed budget and drop the overflow).
- **No binding beyond the tile texture.** Forward fragment textures are at the device's limit. The tile texture joins the shared SH group with the moments texture's visibility (vertex, fragment, compute); fog declares it and never reads it. The direct-SH atlas drops fragment visibility in that layout, since none of its fragment stages samples it; the mesh superset keeps it for movers and skinned meshes. This spends the freed slot, and every level pays the declared texture on the Mac with or without id 59.
- **Tile reference in the moments texel's second channel.** R keeps `E[d]` for every probe, so the SDF open-space skip and every other R reader are unchanged. G holds `E[d²]` for an untiled probe and a sign-flagged tile index for a tiled one, which needs no averaged variance. One shared decode reads G; every texel writer, whole-load and streamed, packs from one CPU reference table built from id 59. The encoding is runtime-only, so widening it later needs no rebake.
- **The reference is runtime-writable.** A later mover state redirects or clears a probe's reference through that table, with no layout change; clearing restores the probe's `E[d²]` from id 34.
- **Reflection capture keeps parity.** The bake-time capture's CPU mirror of the SH sample reads id 59 through the same decode, and its stage key hashes id 59, so reflections lose the leak with the live view (`resource_management.md` §5.2).
- **Cheap lookup.** No normalize before octahedral encoding (the L1 divide already normalizes), no per-corner texture-size query, no runtime integer divide (`research.md` §Mac).
- **Floor and bias unchanged.** `SH_DEPTH_MIN_VISIBILITY` stays 0.03 on every path, keeping weights clearly continuous while counsel question 8 is open. The residue it leaves behind a static door is accepted; the spike found depth bias moves nothing.
- **No player option.** Off would restore the leak (`player_options.md`, tiers are cost levers). A performance ceiling holds the cost instead.
- **Non-goals.** Fog: it skips depth visibility today and keeps skipping it; per-step tile reads cost far more there, and it gets its own brief if its bleed shows. Closed-door tiles for movers and the mover light channel: `mover-light-flow--probe-channel` owns them, including whether a door's tile switches on dock or blends with travel; probe depth is baked with movers absent, so this brief leaves a closed mover door glowing. Probe relocation: roadmap's own entry. The floor's residue and the spike's unexplained remainder: revisited once counsel answers, or in their own brief if still visible.

## Acceptance
Stations, capture method, measure and spike figures: `research.md` §Fixture, §Measure and §Spike; orderings O1–O10: `research.md` §Orderings. "Today" is this build rendering the same map baked without id 59, except the cross-build row under Manual.

### Automated
- [ ] Without tiles the static-door reference still glows (station 3 mean ≥ 0.4), so the fixture reproduces the defect.
- [ ] With tiles, static-door station 3 mean ≤ 0.05 and station 2 mean ≤ 0.02.
- [ ] A sprite and a kinematic brush placed at station 3's floor point behind the static door each read at most a set fraction of their own no-tile brightness, with tiles; the fraction comes from the first-slice pilot and is recorded in the plan of record.
- [ ] The closet pool's reflection capture of the static door matches the tiled runtime's view behind it, and a changed id 59 re-keys the capture stage.
- [ ] Open doorway: station 2 and 3 frame means within 0.02 of today's, and no more than 0.1% of pixels move by more than 2 levels.
- [ ] Hallway Q2 views at 1920×1080 (ignore-gated, bakes stress-warren-hallway-inspection): no pixel darkens by more than 2 levels against today.
- [ ] Mover-door closet: station 2 and 3 frame means no lower than today's minus 0.02 (regression guard recording scope; the door glows by design, so only darkening flags a dropped probe).
- [ ] A PRL without id 59 and one with a zero-tile id 59 render byte-identical to each other.
- [ ] A probe without a tile computes the same visibility as today, bit for bit, beside a tiled neighbour.
- [ ] A valid tiled probe's visibility never falls below 0.03 on any depth-aware path (per-corner, L1 whole-cell, L2 whole-cell weight, direct-and-indirect pair): a tile texel with mean 0 and a shaded point past it weighs exactly 0.03.
- [ ] Every texel writer (whole-load pack, streamed promote, evict, session clear then promote) leaves a tiled probe's R equal to its id-34 `E[d]` bits, so the SDF open-space skip reads today's distance (O8).
- [ ] The shader decode reads the flag from raw bits: a probe referencing tile 0 and one referencing tile 32,767 each sample their own tile, and an untiled probe whose `E[d²]` is +0 stays untiled.
- [ ] Rewriting one probe's tile reference switches its visibility to the new tile on the next frame, with no new binding or layout, including a probe that had no tile; clearing it restores today's averaged visibility bit for bit; a rewrite and a streaming install of the same probe in one drain keep the rewrite in either order, and it survives its cluster's eviction and reinstall (O5–O7).
- [ ] Both moments writers carry a tiled probe's reference: the whole-load packer's texel decodes to the probe's tile, and so does the streamed texel after first install, after its cluster's eviction and reinstall, and after a session clear and reinstall (O3).
- [ ] After streamed dense-pool growth rebinds the sample groups, a tiled probe still reads its tile (O4).
- [ ] A reference rewritten on one load is absent from the next load of the same map, and from a set built while it is published: every texel decodes to that load's id 59 (O10).
- [ ] Bake: tiles are deterministic across two builds, and a warm build of unchanged inputs emits the same id 59 bytes and dropped-count log as a cold one; a cache entry written before this change does not satisfy a tiled build (O1, O2). Selection on a single convex room whose walls are thicker than the probe spacing (new dev fixture) emits no tiles; every closet probe with a blocked valid neighbour gets one.
- [ ] With the cap lowered for the test, a fixture whose candidates exceed it emits exactly the cap's worth of tiles, highest visibility change first with ties broken by ascending probe index, in ascending index order on the wire, and logs how many it dropped; the shipped cap is 32,768 tiles (O9).
- [ ] A valid probe whose id-34 moments carry a set sign bit refuses the level by name.
- [ ] A malformed id 59 refuses the level by name: wrong version, tile edge other than 8, border other than 1, more than 32,768 tiles, an index outside the lattice or naming an invalid probe, unsorted or duplicate indices, or a payload that is truncated or carries trailing bytes.
- [ ] Forward fragment textures stay at 16 with cube arrays and 15 without; every pipeline sharing the SH group stays within its stage limits, and an offscreen renderer builds every full-init pipeline with `POSTRETRO_REQUIRE_GPU` set; DX12 FXC compiles all affected shaders.
- [ ] The mesh superset keeps binding 15 fragment-visible, and the kinematic and skinned fragment texture budgets are counted from the superset layout they bind.
- [ ] Tile bytes appear in SH allocation accounting while the level is installed and return to zero after unload.
- [ ] The build summary reports tile bytes, and they equal the tile bytes the SH allocation ledger records when that level installs.
- [ ] Hallway tile memory, as the SH allocation ledger's tile-texture bytes (ignore-gated, same bake), ≤ 6.4 MiB (spike count at 8×8, plus 25%).

### Manual
- [ ] Cross-build: a PRL without id 59 renders byte-identical to 2968de0ad at the closet stations on one adapter (two-worktree capture `cmp`, as `capture_frame.rs` documents).
- [ ] Forward-pass cost over today at the hallway Q2 views stays within the ceiling: +1.5 ms per view at 4K on the Radeon Pro 5300M (Metal System Trace forward encoder), +0.45 ms per view at 1080p on the GTX 1660 (`POSTRETRO_GPU_TIMING` forward, dev profile, features `capture,dev-tools`, 240 warm-up and 600 sample frames, configurations interleaved, as spike Q2), each view's mean against today's at the same pose. Over the ceiling stops the build for the owner; it never starts a tuning loop.
- [ ] Owner in-engine: closet from inside with the door shut; thin walls and doors on the hallway; open doorways unchanged.
- [ ] Observation only: fog beside a lit closed door. Report whether bleed shows.

## Path
- Bake: production runs `bake_probe_with_source` → `bake_probe_rgb_along` (`sh_bake/probe.rs`); bin those gather hits into the tile, with no second trace. The SH base stage is one cached entry, so tiles must ride its cache entry and its stage version must move. Selection reuses the spike's 26-neighbour comparison.
- Runtime: tile texture beside the dense moments in `sh_bind_group_layout_entries`; `append_shared_bind_group_layout_entries` owns binding 15's visibility, and `mesh_bind_group_layout_entries` derives from the same entries. Texel writers: `pack_probe_depth_moments` (whole load), `sh_streaming/rows.rs` (promote, evict) and `sh_streaming/lifecycle.rs` (session clear), uploaded through `pack_moment_upload_spans`. Pool growth rebuilds the sample groups (`sh_streaming/gpu/growth.rs`). `sdf_shadow.wgsl` reads R raw. `group3_shader_bindings_are_represented_by_rust_layout` pins the binding set.
- Lookup: a power-of-two tile count per row lets the shader find a tile with shifts, masks and one multiply by the bordered stride.
- Accounting: a new `ShAllocationKind` beside `DepthMoments`.
- Reflection capture: the CPU mirror is `reflection_probes/sh_sample.rs`; its key is built in `reflection_probes/stage_key.rs`.
- Fixture: port `mover-closet` and its static-door and no-door variants from `spike/per-direction-depth` into `content/dev/maps`.
- Rivals: dense tiles streamed through the SH pool (125–346 MiB, and +2.7–3.2 ms at 4K on the Mac with every probe tiled); weighting by cell adjacency alone (a partial wall's ends are adjacent, and dropping a probe is a binary decision the IP rule bars); one occluder plane per probe (reads on '372's second independent claim); a 32-bit moments texel carrying the reference in its own channel (doubles the dense texture, which scales with lattice volume).
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
Id 59 mirrors id 34's conventions: little-endian, f16 as raw `u16` bits. Header: `u32` version (1), `u32` tile edge (8), `u32` border (1), `u32` tile count. Tile count is at most 32,768. Then `tile count` × `u32` dense lattice probe index, ascending, unique, each naming a valid probe inside the lattice. Then `tile count` tiles, in index order, each `(edge + 2·border)²` texels of `[f16 E[d], f16 E[d²]]`, row-major. Zero tiles: header only. A section that fails these rules refuses the level by name.
