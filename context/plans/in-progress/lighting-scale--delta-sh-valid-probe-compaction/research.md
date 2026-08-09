# Research — Delta-SH Valid-Probe Compaction

Investigation notes behind `index.md`. Decisions live in the spec; this is the derivation.

## Mechanism, as it stands on main

**Delta storage.** Each of ids 27/41/45 is a sparse CSR over affinity cells (4×4×4 base probes, `AFFINITY_FACTOR = 4`). Per retained `(cell, light)` CSR entry, the section stores a **dense 64-probe sub-block**: `PROBES_PER_CELL(64) × delta_probe_f16_stride × 2` bytes = 64 × 144 halves × 2 = 18,432 B/entry (default tile 6×6 RGBA16F). The 64-probe identity is asserted in two independent places that must move in lockstep:
- `crates/level-format/src/delta_sh_volumes.rs` `to_bytes` (debug_assert) / `from_bytes` (`subblock_count = list_len × PROBES_PER_CELL × probe_f16_stride`), and the id 41/45 sibling modules.
- `crates/level-compiler/src/pack.rs:216-221` `direct_sh_delta_has_valid_csr_shape`: `affinity_lights.len() × PROBES_PER_CELL × delta_probe_f16_stride() == delta_subblocks.len()`. This gate feeds `direct_sh_delta_is_usable_for_selection` → `has_usable_direct_sh_deltas`; if it fails, id 41 (and `EntityShadowLights` + `ShadowmaskAtlas`) silently drop from emission. Under compaction it fails for every payload unless updated — a required touchpoint.

**Compose read.** `read_delta_texel(entry, local_probe, tile_texel)` in each compose shader computes `(entry * PROBES_PER_CELL + local_probe) * delta_probe_f16_stride + texel*4`. `local_probe` (0..63) is the x-fastest in-cell index from `map_probe_to_affinity` (`local = lx + ly*4 + lz*16`). The composed atlas is **dense**: `compose_main` dispatches over `grid.atlas_dimensions × atlas_layer_count`, maps each dense texel → probe → cell + local, reads the delta, writes the dense composed texel. Samplers (forward/fog/billboard/skinned/kinematic) then read the dense composed atlas arithmetically via `probe_tile_origin`.

**Indirect vs direct compose differ in one way that matters for reuse.**
- id 27 indirect (`sh_compose.wgsl`): binds the base **compact** id-34 atlas + `probe_indirection` (binding 26 = per-nominal-probe global compact slot or `INVALID_PROBE_INDIRECTION`). It early-outs invalid probes (`compact_slot == INVALID → store 0, return`) **before** the delta loop, so only valid probes reach `read_delta_texel`.
- id 41/45 direct (`direct_sh_compose.wgsl`, `animated_direct_sh_compose.wgsl`): bind the **dense** id-35 direct base atlas, no `probe_indirection`, no invalid early-out. They read every probe's delta at fixed 64-stride.

So the base indirection word exists only on the indirect path, and even there it is the wrong rank space for the delta (global, not within-cell). See §Reuse.

**Drop + cap (the honesty core).**
- `apply_exact_zero_drop_policy` (`delta_sections.rs:69`) → `drop_*_zero_entries` (`delta_drop_policy.rs`): removes a CSR entry iff its whole 64-probe payload decodes to **exactly zero** f16 RGB. Lossless (a zero entry contributes exactly zero through every compose path including id 41's signed subtraction + clamp). Script-mutable animated slots are retained conservatively. This is the "landed conservative delta-entry-dropping" the task references — it is exact-zero, not near-zero, and not cap-driven.
- `enforce_payload_cap` (`delta_sections.rs:125`): sums compacted `delta_subblocks` bytes across ids 27+41+45; if `total > max_payload_bytes` (default `DEFAULT_MAX_PAYLOAD_BYTES = 256 MiB`, `delta_sections.rs:15`; CLI `--sh-delta-max-size`) it returns a **hard anyhow error** naming all three ids and the overage. It performs **no** selection and **no** drop-to-fit. Over-cap = failed build.

This directly contradicts the task's paraphrase ("a 64 MB cap that fits them by dropping whole lights"). Verified false against source: default is 256 MiB, and the cap errors rather than fits. The spec's Direction and Open Questions record the correction rather than propagating the premise (spec-session habits #1, #5).

## Measured basis

From the just-completed `--sh-analyze` spike (`crates/level-compiler/src/sh_analyze.rs`, JSON + log summary). The instrument already emits, per section, three independent byte lines: `(a)compacted` (valid-probes-only — this spec's lever), `(b)exact-zero-drop`, and `(c)all-L1/all-L2` coarsening. Attribution is deliberately separate — compaction, exact-zero drop, and density coarsening are distinct levers, which is itself evidence for separability.

- `stress-warren-showcase.map` @ 2 m SH spacing: 115,200 probes, 67,838 valid (59%). Measured on the **emitted/shipped delta set** (post static-light selection + exact-zero drop; the `--sh-analyze` tool was corrected from a ~4.6× inflated pre-filter superset — 22,608 → 4,940 entries): id 41 47.9 → 26.4 MB (0.55×); id 27 22.3 → 11.8 MB (0.53×); id 45 20.9 → 11.3 MB (0.54×); total deltas 126.4 → 69.0 MB (0.55×). All well under the 256 MiB cap, so the cut is real bandwidth/VRAM.
- Base atlas (id 34) is already valid-probe-compacted at rest (the shipped precedent). The delta sections are the remaining uncompacted probe payload.

These are the anchors Task 5 re-derives before trusting the before→after ratios. Confirm the anchor map: `content/dev/maps/stress-warren-showcase.map` exists; the stopped successor cited a different stress map (`stress-warren-maze-crates.map`, also present) — the spec uses `showcase` per the task's evidence.

## Reuse — why a sibling per-cell descriptor, not the base word

The task constraint: reuse the base-atlas indirection-word infra; do not build a parallel delta-only indirection unless justified.

`build_probe_indirection_words` (`crates/renderer/src/render/sh_compose.rs:465`) returns, per nominal probe, its **global** valid-rank (rank among all valid probes, whole-grid x-fastest) or `INVALID_PROBE_INDIRECTION`. The delta compaction needs, per probe, its **within-cell** valid-rank (rank among the valid probes of its own 4×4×4 cell) and, per entry, the payload base offset. A cell's 64 probes are non-contiguous in whole-grid x-fastest order (the grid walks whole rows, not cells), so within-cell rank is not derivable from global rank without re-scanning the cell — the shared word answers a different question.

**Reused:** the validity **source** (id-34 metadata validity — "the sole source of truth", `build_pipeline.md` §OctahedralShVolume), the load-time **derivation pattern** (`build_probe_indirection_words`-style, derived from id-34, not new serialized runtime bytes), the compose-only **carrier** shape, and the fail-loud/no-shim posture.

**New (justified):** a per-cell valid-probe descriptor. Recommended: one `u64` mask per affinity cell.
- `is_valid(local) = (mask >> local) & 1u`
- `within_cell_rank(local) = countOneBits(mask & ((1u64 << local) - 1u))` (WGSL `countOneBits` over the two u32 halves)
- `valid_count(cell) = countOneBits(mask)`
- entry payload base = prefix-sum of `valid_count(cell(e)) × stride` over entries `< e` (entries within one cell share `valid_count`, so the prefix is a pure function of the CSR + descriptor). Materialized at LOAD (CPU) into a per-entry offset array the shader indexes as `offset[entry]` — not summed in-shader; derived from the final **post-drop** CSR in entry order (== payload write order). See index.md Direction (per-entry-offset Alternative).

The mask is one artifact serving three roles: section self-validation (popcount fixes payload length), compose-time resolver, and loader cross-check target against id-34. That is strictly less machinery than a per-entry offset table plus a separate validity source, and it is a sibling built by the reused infrastructure rather than a parallel invention.

## Separability — the direction verdict, in full

**Verdict: cleanly separable. Compaction is a lossless prerequisite that pre-builds density coarsening's variable-stride delta substrate without paying coarsening's sampler-rework or quality-risk cost.**

Both compaction and coarsening express the same abstract shape: *a variable stored-set per brick/cell, resolved via a per-probe membership predicate plus within-set indexing.* Compaction's predicate is `valid`. Coarsening's is `valid ∧ on-level-lattice(level)`. This shared shape is what tempts a merge. Three axes make separation correct:

1. **Validation posture (decisive).** Compaction is lossless — it drops only probes whose stored tile is never read on the composed path (invalid probes), so f16-bit-identity to the uncompacted result is achievable and is the honesty gate. Coarsening is lossy — it drops *valid* probes and reconstructs at sample time, so its gate is a measured composed-error and shared-face-seam budget, a go/no-go spike, and manual A/B. Merging forces a shippable lossless refactor to wait on an unresolved lossy quality question. The successor plan is **stopped/reopened** pending a research spike (`archived-plans/lighting-scale--adaptive-base-probe-density`, header) — coarsening is months out and may reshape. Blocking compaction behind it is wrong sequencing.

2. **Blast radius.** Compaction touches only the delta storage and the three compose passes that read it. The composed atlases stay dense; the five SH samplers, `probe_tile_origin`, and `sh_sample.wgsl`'s corner walk are untouched. Coarsening additionally **compacts the composed atlases**, which forces every sampler through a new resolve — the successor's Task 4/5, its load-bearing and risky part. Compaction is compose-internal; coarsening is a whole-SH-reader rewrite. Different, far larger surface.

3. **The seam is real and reusable.** What compaction builds and coarsening reuses wholesale:
   - Per-section version bump + self-describing per-cell stored-set descriptor + parse validation.
   - Per-entry payload = `stored_count(cell) × stride`, entry-order prefix-summed (variable stride).
   - Loader cross-check of the delta descriptor against id-34 metadata, fail-loud all-or-nothing.
   - Compose-time resolution `(cell, local_probe) → stored slot | absent` via the descriptor.

   Coarsening generalizes exactly one thing: the stored-set predicate (`valid` → `valid ∧ on-level-lattice`) and the id-34 metadata it reads (the reserved-zero `density_level` field, already present in v9). Everything else in the list is compaction's deliverable, unchanged. The delta format bumps again when coarsening lands, but the bump is cheap (no compat shim by house posture); the expensive parts (sampler rework, lossy validation, gate spike) are coarsening-only and must not gate the lossless win.

**Counter-case considered.** "Version-bump the delta format once, do both together." Rejected: the format bump is the cheap part; coupling forces the cheap-and-lossless onto the expensive-and-lossy-and-stopped timeline. The successor's own Direction already treats a delta-only refinement as a distinct, later question (its Task 6 decision 4) — this compaction is the floor beneath even that.

**What compaction must NOT do to stay the clean prerequisite.** It must not compact the composed atlases, must not touch any sampler, and must not introduce a per-probe *level* field (that is coarsening's, and premature here). Keeping the composed atlas dense is what confines the change to compose internals and keeps the parity gate exact. These are pinned as non-goals and as invariant C4.

## Touchpoint inventory (grounding for the tasks)

| Concern | Site |
|---|---|
| id 27 format + 64-identity | `crates/level-format/src/delta_sh_volumes.rs` (`to_bytes`, `from_bytes`, `DELTA_SH_VOLUMES_VERSION`) |
| id 41 format | `crates/level-format/src/direct_sh_delta_volumes.rs` |
| id 45 format | `crates/level-format/src/animated_direct_sh_delta_volumes.rs` |
| id 41 CSR-shape 64-identity | `crates/level-compiler/src/pack.rs:216-221` `direct_sh_delta_has_valid_csr_shape` |
| id 27 bake | `crates/level-compiler/src/delta_sh_bake.rs` (`:199,585,636`) |
| id 41 bake | `crates/level-compiler/src/direct_sh_bake.rs` (`:388` `per_entry_payload_bytes`) |
| id 45 bake | `crates/level-compiler/src/animated_direct_sh_bake.rs` (`:136,226,375`) |
| drop + cap | `crates/level-compiler/src/delta_sections.rs`, `delta_drop_policy.rs` |
| id 27 compose | `crates/renderer/src/shaders/sh_compose.wgsl` `read_delta_texel` (`:169-177`) |
| id 41 compose | `crates/renderer/src/shaders/direct_sh_compose.wgsl` `read_delta_texel` (`:108-116`) |
| id 45 compose | `crates/renderer/src/shaders/animated_direct_sh_compose.wgsl` `read_delta_texel` (`:130`) |
| render-cpu delta buffers + footprint | `crates/render-cpu/src/sh_compose.rs` (`build_direct_delta_buffers`, `DirectDeltaComposeBuffers`, `ComposeStorageFootprint`) |
| validity source | id-34 probe metadata, `crates/level-format/src/sh_volume.rs` (`OctahedralShProbe.validity`, `SH_VOLUME_VERSION = 9`) |
| base word (reuse pattern, not value) | `crates/renderer/src/render/sh_compose.rs:465` `build_probe_indirection_words` |
| measurement instrument | `crates/level-compiler/src/sh_analyze.rs` (`--sh-analyze`, `(a)compacted` line) |
| out-of-scope (no probe axis) | `crates/level-compiler/src/shadowmask_bake.rs` → id 42 `crates/level-format/src/shadowmask_atlas.rs` |
</content>
