# Direct SH Delta Footprint Instrumentation

> **Status:** draft.
> **Track:** Lighting / build pipeline — measurement prerequisite, not a fix.
> **Related:** `context/lib/build_pipeline.md` (PRL sections, prl-build) · `context/lib/rendering_pipeline.md` §4/§7.1 (SH compose) · `context/plans/done/lighting--entity-direct-sh/` (the direct-SH feature this measures) · `context/plans/done/perf-animated-sh-light-culling/` (the sparse-CSR delta form).

## Goal

A stress map compiled with dense settings baked a **1.22 GiB** direct-SH delta storage buffer, overflowing the engine's self-imposed 512 MiB `max_storage_buffer_binding_size` (`crates/renderer/src/render/renderer_init_resources.rs:122`) and failing wgpu bind-group validation at load. Before choosing an allocation strategy, make the direct-SH delta footprint **visible at prl-build time**: which lights dominate the bytes, and how SH's share compares to the rest of the `.prl`. This is a logging-only measurement plan — no behavior, format, device-limit, or buffer-layout change. Its output unblocks a later allocation-strategy decision (footprint-gating vs. adaptive probe density vs. texture-array migration vs. chunking).

## Scope

### In scope

- A per-selection-light delta-byte histogram logged from the direct-delta bake, so map authors see which lights dominate the delta storage.
- A dedicated size log line for the indirect `DeltaShVolumes` section (id 27) in prl-build, matching every other emitted SH section.
- The existing `ComposeStorageFootprint` per-binding storage log wired into the direct compose path at runtime (the indirect path already has it).
- An aggregate "SH sections total vs. non-SH total" summary line in prl-build's section-size report.

### Out of scope (non-goals)

- **Changing the bake output.** No probe is added, dropped, culled, or re-valued. Baked `.prl` bytes are identical before/after this plan.
- **Changing the PRL format.** No new section, no version bump, no field added to any section struct.
- **Changing device limits.** `REQUIRED_STORAGE_BUFFER_BINDING_SIZE` (512 MiB) stays as-is — this plan measures the overflow, it does not resolve it.
- **Changing the buffer layout / bind groups.** No binding added or moved on either compose path.
- **The allocation-strategy decision itself.** Footprint-gating, adaptive density, texture-array migration, and chunking are follow-up plans that consume this plan's measurements.

## Acceptance criteria

- [ ] Building `content/dev/maps/stress-warren-maze-crates.map` with `--lightmap-density 0.2 --sh-probe-spacing 1.33 --soft-shadow-samples 64` at `RUST_LOG=info` prints a per-selection-slot direct-delta byte histogram: one line per selection slot that has at least one CSR entry, sorted descending by byte total, each line naming the selection slot, its global light identity, its CSR-entry count, and its byte total.
- [ ] The histogram's summed per-slot bytes equal the emitted `DirectShDeltaVolumesSection`'s `delta_subblocks` byte length (`affinity_lights.len() × PROBES_PER_CELL × delta_probe_f16_stride(tile_dimension) × 2`). This is the checkable invariant that the accounting is complete and correct.
- [ ] prl-build prints a size line for the indirect `DeltaShVolumes` section (id 27) whenever that section is emitted, mirroring the existing `DirectShDeltaVolumes` (id 41) line — id 27 is no longer the one emitted SH section with no size line.
- [ ] At runtime, loading a map with a direct-delta section logs a per-binding storage footprint for the direct compose path's PRL-baked delta buffers (`delta_subblocks`, `affinity_offsets`, `affinity_lights`) plus their total, and the log is unambiguously attributable to the DIRECT compose path (not confusable with the indirect path's existing footprint line).
- [ ] prl-build prints one "SH sections total vs. non-SH total" summary line after the per-section size logs, accounting for every emitted section.
- [ ] No change to compiled `.prl` output bytes and no change to runtime behavior beyond the added log lines. A byte-for-byte diff of a `.prl` compiled before and after this plan (same inputs) is empty.

## Tasks

### Task 1: Per-selection-light delta-byte histogram (direct-delta bake)

In `bake_direct_sh_delta_volumes` (`crates/level-compiler/src/direct_sh_bake.rs:263`), insert a logging block immediately before the `Some(DirectShDeltaVolumesSection { … })` return at `direct_sh_bake.rs:320`. At that point the function holds `affinity_lights: Vec<u32>` (`:304`) and `selected: Vec<SelectedDirectLight>` (`:280`). Each entry in `affinity_lights` is a **selection index** into `selected` — a CSR (cell, selection-slot) entry, NOT an AlphaLights or source-light id (pinned by the test `direct_sh_delta_affinity_lights_are_selection_indices`, `direct_sh_bake.rs:1396`). Compute per-selection-slot CSR-entry counts by tallying occurrences of each value in `affinity_lights`; multiply each count by the per-entry payload `PROBES_PER_CELL × delta_probe_f16_stride(TILE_DIMENSION) × 2` bytes (= 64 × 144 × 2 = 18,432 for the default 6-texel tile — do not hard-code; derive from the format constants `postretro_level_format::delta_sh_volumes::{PROBES_PER_CELL, delta_probe_f16_stride}` already imported into this module as `FORMAT_PROBES_PER_CELL` / via `TILE_DIMENSION`). Log the slots sorted descending by byte total (log all, or a top-N if slot count is large — state the cap in the message). Label each slot with its global light identity: `selected[slot]` carries `.static_index` (the light's position in the `StaticBakedLights` set — the stable global identifier) and `.light: &MapLight`; log `static_index` and, if cheap, the light's `origin`. **Plumbing note:** `selected` is built by `selected_direct_lights` (`direct_sh_bake.rs:337`) via a `filter_map` over `entity_shadow_lights.light_indices`, which discards the originating `light_indices` (AlphaLights) index for each surviving slot. If the label should also name the source `EntityShadowLights.light_indices` alpha value, extend `SelectedDirectLight` (`:331`) to carry the originating alpha index and populate it in `selected_direct_lights`; otherwise label by `static_index` alone, which `selected[]` already carries. The summed per-slot bytes must equal `affinity_lights.len() × per_entry_payload` (the AC-2 invariant) — log that total alongside the histogram.

### Task 2: Size log for the indirect DeltaShVolumes section (id 27)

In `crates/level-compiler/src/pack.rs`, add a size log line for the indirect `DeltaShVolumes` section, mirroring the existing `DirectShDeltaVolumes` (id 41) block at `pack.rs:848-855`. The section is already threaded and emitted: the param `delta_sh_volumes: Option<&DeltaShVolumesSection>` (`pack.rs:567`), its bytes `delta_sh_volumes_bytes` (`pack.rs:634`), and its emit at `SectionId::DeltaShVolumes` (`pack.rs:741`). It is the one emitted SH section with no line in the section-size report (`pack.rs:789-928`). Add a guarded `log::info!` (matching the `if let (Some(section), Some(bytes)) = …` pattern used by the neighboring optional-section logs) reporting the byte length and a useful count (e.g. `affinity_lights.len()` CSR entries and/or `animation_descriptor_indices.len()` animated lights), placed adjacent to the other SH-section logs.

### Task 3: Wire ComposeStorageFootprint into the direct compose path

In `DirectShComposeResources::new` (`crates/renderer/src/render/direct_sh_compose.rs:64`), after the three PRL-delta byte vectors are built at `direct_sh_compose.rs:82-84` (`subblock_bytes`, `offsets_bytes`, `lights_bytes`), construct a `ComposeStorageFootprint` (`postretro_render_cpu::sh_compose::ComposeStorageFootprint`, `crates/render-cpu/src/sh_compose.rs:24`) and log it, mirroring the indirect path (`crates/renderer/src/render/sh_compose.rs:105-115`). Two direct-path differences the wiring must account for: (a) the direct path has **no** `animation_descriptor_indices` buffer (`DirectDeltaComposeBuffers` carries none), so set `animation_descriptor_indices_bytes: 0`; (b) `selection_weights` (bound at `BIND_SELECTION_WEIGHTS = 26`) is a **runtime** buffer passed in as `weights_buffer` — it is NOT part of the PRL-baked delta payload that overflows, so it is excluded from the footprint (the footprint measures the baked delta storage, matching the struct's four fields). **Label plumbing:** `ComposeStorageFootprint::log()` (`sh_compose.rs:39`) hard-codes the string `"SH compose @group(1) storage footprint"`, which would mislabel the direct path (distinct pass, group 0, and the indirect path already prints that exact line). Make the direct log unambiguously the direct path — either parameterize `log()` with a caller-supplied label/path prefix (updating the indirect call site to pass its current label) or emit a sibling direct-specific log line — so AC-4's "unambiguously attributable to the DIRECT compose path" holds. The existing direct-path line at `direct_sh_compose.rs:208-213` ("Direct SH compose: N selected-light CSR entr…") stays; the footprint is additive.

### Task 4: SH-total vs. non-SH-total summary line

In `crates/level-compiler/src/pack.rs`, after the per-section size logs (`pack.rs:789-928`, before the `Ok(())` at `:930`), add one summary line partitioning the emitted sections into an SH group and a non-SH group and reporting each group's total bytes (and their sum, which should equal the on-disk section-payload total). The SH group is the SH-lighting sections: `OctahedralShVolume`, `DirectShVolume`, `DeltaShVolumes`, `DirectShDeltaVolumes`, `EntityShadowLights` (and any present `sh_*` sibling). Sum from the same `*_bytes` locals the per-section logs already use (each is `Vec<u8>` or `Option<Vec<u8>>`), so no new byte computation is introduced — reuse the byte lengths already in scope. State in the message which sections count as SH so the partition is auditable against the per-section lines directly above it.

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 2, Task 3, Task 4 — four independent, additive log sites in three files (`direct_sh_bake.rs`, `pack.rs` ×2, `direct_sh_compose.rs`). Tasks 2 and 4 touch the same `pack.rs` section-report region but at distinct, non-overlapping insertion points; land them in either order (a trivial merge if concurrent). No task depends on another's output.

## Rough sketch

- **Per-entry payload is a constant of the tile geometry.** One CSR entry = one 4×4×4 affinity cell = `PROBES_PER_CELL` (64) probes × `delta_probe_f16_stride(tile_dimension)` (144 halves for the default 6-texel tile) × 2 bytes = 18,432 bytes. The whole delta payload is `affinity_lights.len()` such entries. This identity is the histogram's accounting basis and the AC-2 cross-check. Confirmed constants: `AFFINITY_FACTOR = 4`, `PROBES_PER_CELL = 64`, `DEFAULT_DELTA_PROBE_F16_STRIDE = 144` (`crates/level-format/src/delta_sh_volumes.rs:25,28,35`).
- **Selection index ≠ global light id.** `affinity_lights[i]` indexes `selected[]` (the post-filter selected-direct-lights vec); `selected[slot]` resolves to a `MapLight` and its `static_index`. The bake already does exactly this resolution when it builds `delta_subblocks` (`direct_sh_bake.rs:314-316`: `let entry = selected[selection_index as usize];`) — the histogram mirrors that lookup for labeling.
- **Direct compose bindings (for the footprint).** PRL-baked delta storage on the direct path: `delta_subblocks` (`BIND_DELTA_SUBBLOCKS = 20`), `affinity_offsets` (`BIND_AFFINITY_OFFSETS = 21`), `affinity_lights` (`BIND_AFFINITY_LIGHTS = 24`). Runtime-only (excluded): `selection_weights` (26), `debug_override` (27). No `animation_descriptor_indices` exists on this path. (`direct_sh_compose.rs:14-20`.)
- **Ballpark for the observed overflow.** 1.22 GiB ÷ 18,432 B ≈ 71k CSR entries — the histogram attributes those entries to their dominating lights, which is the number the allocation-strategy follow-up needs.
- **`.prl` byte-identity is the guardrail.** Every change is a `log::*` call; none touches a byte vector, section list, or bind group. The AC-6 before/after `.prl` diff is the objective check that scope held.
