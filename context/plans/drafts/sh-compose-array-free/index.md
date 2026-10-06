# sh-compose-array-free

Brief · resumable · reads: `context/lib/rendering_pipeline.md` §4 (Animated SH delta volumes, Sampled-row compose), §7.1, §8, §10 (Target hardware), §12 (GPU Pass Timing, Machine-state confounders) · `context/lib/build_pipeline.md` §PRL section IDs (ids 27, 41, 45) · `context/lib/testing_guide.md` · read at b9d0a4645 · evidence: `research.md`

## Problem
The `sh-compose-row-cost-spike` experiment found that on the Mac perf target (Radeon Pro 5300M, Metal), 95–99% of streamed SH compose time is a fixed cost per composed row. Most of it is the per-invocation 36-texel accumulator array: removing the array alone removes 43–55% of pass time.

One exact lever fixes it. It fuses the base read, entry sum and store per texel, unrolls the texel loop 36×, and caches light scales per workgroup. On today's row counts it cut both passes (indirect `compose_main`, Pass B `animated_compose_main`) by 27–36% at every measured pose. The composed atlases stayed byte-identical. Once the `sh-compose-contributing-rows` filter lands, fewer rows compose, so the lever's lasting value is smaller: 0.07–0.66 ms per pass per frame, depending on the pose (`research.md` §Targets).

The lever exists only as WGSL rewrites on a throwaway branch. When this is done, both compose shaders carry the measured shape on every path that dispatches them, streamed and legacy alike. Composed atlases stay byte-identical to the pre-change shaders on every brick level the format accepts. The Mac saving holds on the landed code.

## Decisions
- **Land the measured shape, not an equivalent.** On Metal, the indirect kernel's codegen moves ±1–2.7 ms under edits that remove work (`research.md` §Fragility). The kernel follows the exported lever source in statement order, control structure, branch gates and loop shape. Comments, whitespace and identifier names are free.
- **One allowed difference: delete the unreachable old-kernel tail.** The exported lever runs inside a `row_count > 0` wrapper that always returns. After it the whole old kernel survives, including its accumulator array and the shared kept-tile workgroup lattice, and nothing references that code once it is gone. A paired Mac A/B times the deletion against the exported lever before anything else is built. If it holds, the deletion lands and the shape pin re-bases on the landed kernel. If it regresses, the tail stays, and the plan of record says why. Any other shape change needs its own paired re-measure.
- **Port into the shader files; no rewrite layer, no variant.** The lever replaces the kernel body in `sh_compose.wgsl` and `animated_direct_sh_compose.wgsl`. Legacy and streamed pipelines `include_str!` the same files, so both get it. §8 has no variant system, and this adds none.
- **The 36× texel unroll expands at shader assembly (owner, 2026-10-06).** Each shader holds the per-texel body once, between marker comments. The concat that runs at level load, before `create_shader_module`, repeats the body with constant texel coordinates. That concat already joins `curve_eval.wgsl` and `WGSL_DECODE_HELPER`. One routine serves every pipeline that builds these shaders. A WGSL loop can't replace the unroll: naga injects the loop-bound counter that the unroll removes.
- **Keep the L1/L2 reconstruction and prove it (owner, 2026-10-06).** Ids 27/45 are uniform L0 by compiler policy, but their coarsening is a planned door (`build_pipeline.md`, coarsened delta sections). On L0 content, keeping the path costs nothing measured. In 27/45 the lever's L1 path re-reads kept corners per texel instead of loading the shared lattice once per brick. That reverses the access pattern `plans/done/lighting-scale--delta-sh-probe-coarsening` made mandatory, and its cost is unmeasured because no fixture has L1/L2 rows. Id 41's static direct compose keeps the lattice. Enabling 27/45 coarsening must first measure the L1/L2 compose cost.
- **The exactness oracle is the frozen pre-change shader.** The assembled pre-change source of each pass is checked in as a test fixture, matching the spike's exported baseline at claim head. It runs beside the landed shader on identical synthetic sections, because no compiled map has L1/L2 rows. The test is stricter than §4's streamed-vs-full-resident invariant, which compares within one build and cannot see a shader change. Both must hold. The `render-cpu` reference is not the oracle: it cannot see Metal codegen or fast-math contraction.
- **The 1660 gates promotion, not just the merge (owner, 2026-10-06).** The 1660 Super is the §10 perf floor, and with no variants it runs this shader too. The owner runs the spike's 1660 handoff on the probes branch before promotion. A regression in either pass ends this direction. A win relabels the lever all-backends, and otherwise it stays Metal-only. A second reading on the landed code confirms it at merge.
- **The re-measure rule covers the compose kernels, not shared helpers (owner, 2026-10-06).** The shape pin and the durable rule apply to the two compose kernel files. `curve_eval.wgsl` and `WGSL_DECODE_HELPER` stay free to change. The paired method goes into §12 as a method; it is not a landed tool. `gpu-pass-paired-ab` drafts that tool separately.
- **No dependency on the contributing-row filter.** The lever's saving is per row, and it matched across poses where 3% and 48% of rows carry entries. Whichever lands first, the other re-measures on the current head and compares saving per composed row.
- **Durable capture at promotion:**
  - §4 and §7.1 lose "coarsened compose loads the kept lattice" for ids 27/45 and gain the per-texel corner read with its cost unmeasured. They also gain the kernel-shape rule: no per-invocation tile accumulator, and a change to a compose kernel file needs a paired Mac re-measure.
  - `build_pipeline.md`'s coarsened-delta paragraph gains the cost-first condition for 27/45.
  - §8 gains the unroll expansion as a second string-surgery mechanism.
  - §12 gains the per-encoder GPU-time method and a brief paired method, unless `gpu-pass-paired-ab` has already landed its §12 Paired A/B subsection. In that case this brief only references it. `gpu-pass-paired-ab` owns that subsection and folds this text into it when it lands.
- **Non-goals:**
  - Row-count reduction: `sh-compose-contributing-rows` owns it.
  - The H-e trusted shader module: `unsafe` never reaches main, and a separate session may return to it.
  - Enabling 27/45 coarsening.
  - The no-build arms (`research.md` §No-builds).
  - Pass A promotion cost at the kinematic poses.

## Acceptance
### Automated
**Exactness (both passes, landed vs frozen pre-change shader, identical inputs, rgba16float readback byte-equal):**
- [ ] A row of L0 cells with entries, and a row with no entries.
- [ ] L1 cells with dropped-valid probes reconstructed from kept corners, and L2 cells with the representative probe kept and dropped.
- [ ] A row mixing L0, L1 and L2 cells.
- [ ] Entry counts on both sides of the per-workgroup scale cache: exactly its capacity, and one more (the fallback).
- [ ] Two animation times at which every animated light's scale differs, so a stale or swapped scale cannot pass.
- [ ] Indirect with the static base on and off; Pass B with a non-zero intermediate atlas.
- [ ] Negative control: a shader that skips the entry loop, run through the same comparison, differs.
- [ ] No adapter: the parity test fails under `POSTRETRO_REQUIRE_GPU` and skips without it.
**Shape:**
- [ ] Each pass's kernel, as assembled, equals the checked-in measured kernel after normalizing comments, whitespace and a declared identifier-rename table. Any other difference fails. Edits to the shared helpers do not trip it.
- [ ] Streamed and legacy pipelines assemble byte-identical source for the same pass.
- [ ] The expansion emits one block per tile texel, each texel exactly once, and fails loudly if its markers are missing or duplicated.
- [ ] Neither landed kernel declares the 36-entry accumulator array or the shared kept-tile lattice, unless the tail-deletion re-measure regressed.
**Regression guards:**
- [ ] Existing compose naga-validation tests and `sampled_row_gate_capture_matches_full_resident_at_stepped_times` (`--features capture`, `--ignored`) pass on the landed head.

### Manual
- [ ] Before promotion (owner, Windows): the 1660 Super spike handoff on probes `1a052cfed`, per the spike findings' handoff section. `baseline` vs `array-free,unroll36`, `sh_compose` and `animated_direct_sh_compose` from `[GpuTiming]`, at campaign-test spawn and the hallway arena, 3 interleaved launches each. No regression beyond spread in either pass.
- [ ] Mac, first slice: paired A/B of the tail-deleted kernel against the exported lever, at the hallway arena (`--start-pose=21.13,2.44,30.48,0,0`) and the kinematic station (`--start-pose=-6.5,1.22,-27.94,0,0`), 3 launches each. The result decides the tail, per Decisions.
- [ ] Mac, landed: paired A/B of the landed shaders against the frozen baseline, alternating per frame in one launch, at the same poses, 3 launches each. An A/A null reads within ±0.02 ms. Neither pass regresses at either pose, and each saves at least 75% of the spike's per-row saving at that pose (`research.md` §Targets). Time is per compose encoder.
- [ ] Mac legacy path: a capture with `POSTRETRO_SH_STREAMING=off` is PNG byte-equal between the pre-change and landed binaries.
- [ ] Level-load cost: pipeline creation time for both compose pipelines, pre-change vs landed, on the Mac and on the 1660 machine's backend, reported.
- [ ] At merge (owner, Windows): the 1660 reading repeated on the landed code, with no regression beyond spread. The result sets the backend label.

## Path
Non-binding.
- **Measured source:** `measurements/sh-compose-row-cost-spike/lever-wgsl/` holds both passes' baseline and lever source, the diffs and SHA-256s. Its generator is `compose_spike::build_source` with the `array-free` arm in `probes.patch`, and that arm's unroll is the reference for the assembly step. Deleting the tail removes the `row_count > 0` wrapper, the old body after it, `shared_kept_*`, `reconstruct_l1_shared_texel` and `l1_shared_slot`. Check whether the preamble barriers survive only for the scale cache.
- **First slice:** the tail-deletion A/B can run on the probes branch as a new arm before any main-side code exists. Then the expansion routine and the indirect shape pin.
- **Assembly seams:** the source concat in `sh_streaming/gpu/indirect.rs` (`new`), `sh_streaming/direct_compose/passes.rs`, legacy `sh_compose.rs` and `animated_direct_sh_compose.rs`. Their naga tests replicate the join, so route them through the same routine.
- **Parity harness:** the closest precedent is `animated_atlas_parity_test.rs`: its own device, a real compose pass, readback, and the per-file `POSTRETRO_REQUIRE_GPU` gate. `gpu_test_harness.rs` readers are RGBA8 only, so rgba16float needs its own copy-and-map. `render-cpu::sh_compose` `build_*delta_buffers` and `build_compose_grid_bytes` turn hand-built level-format sections (public fields) into GPU words. Keep test-only readback under `#[cfg(test)]`; the upload drift scanner skips it.
- **Level ≥3:** the loader rejects it. The baseline gates L1 on `level == 1u`. The lever's `else` sends any level other than 0 and 2 to L1, so the two differ only on levels that never load. Restoring the explicit gate is a shape change.
- **File size:** `sh_compose.rs` is past 800 lines. Put the parity test and the expansion routine in their own files.
- **Rival weighed:** keeping the rewrite layer as a permanent assembly step. It keeps the old kernel text alive as an anchor nobody reads.

## Open questions
- The 1660 reading on probes `1a052cfed` — owner — **blocks build** (promotion precondition)
- Where the frozen baseline and measured kernel fixtures live (crate test data vs `measurements/` via `include_str!`) — **delegated**
