# sh-compose-row-cost-spike

Spike · compact · reads: `context/lib/rendering_pipeline.md` §4 (Sampled-row compose), §7.1 step 5, §8, §10 (Target hardware), §12 (GPU Pass Timing, Machine-state confounders) · `context/lib/experimental_spikes.md` · `context/lib/development_guide.md` §3.5 · read at de735f026 · evidence: `research.md`

## Problem
On the Mac perf target (Radeon Pro 5300M, Metal), the two streamed SH compose passes cost about 3.2–4.3 µs per composed row (indirect, Streamed SH Compose) and 3.6–4.9 µs per row (Pass B, Streamed Animated Direct SH), linear in rows. In the hallway's large arena they take 14.4 ms of a GPU-bound frame. On a GTX 1660 Super the same passes cost about 0.55 and 0.77 µs/row. The 5300M is under 2× slower on raw throughput and bandwidth, so the 5–8× gap points to a pathology specific to the backend or the architecture (`research.md` §Basis). The developer raised this from measurement during `shadow-fill-cost`. Per-row cost matters whatever the row count.

Source reading gives five unverified hypotheses:
- **H-a:** a per-invocation 36-texel accumulator array.
- **H-b:** redundant per-lane curve evaluation.
- **H-c:** uncoalesced delta reads.
- **H-d:** the coarsened-brick path: idle lanes behind three barriers per entry, plus an 8-slot weighted reconstruction loop per texel.
- **H-e:** Metal's injected bounds checks and loop bounding.

Each row also carries a fixed term no hypothesis names:
- lane 0's serial scan of up to 64 indirection candidates behind a barrier;
- 36 base-atlas samples per lane;
- 36 strided stores per lane.

When this is done, a findings note partitions measured compose cost top-down: fixed per row, per entry, and the unexplained remainder. It then attributes the per-entry and fixed shares to the hypotheses. It recommends which exact levers to build, each labelled Metal-only or all-backends.

## Decisions
- **Cost model, not a row-count precondition.** Per pass, compose time = fixed per dispatch + a per-row term by brick level (L0/L1/L2) + a per-CSR-entry term by brick level, fitted across poses from the `[SH streaming]` rows and the per-level row and entry counts.
  - The model's terms do not change when rows with no entries stop composing, so the spike runs on whatever head is current. It does not wait for the separate contributing-row filter build.
  - Post-filter savings are projected from the model and the filtered counts.
  - All arms in one batch build from one head and one fixture set. A batch never straddles a commit that changes which rows compose.
- **Metric: ms per frame per pass at a fixed pose.** At a fixed pose every arm composes the same rows, so an ablation's delta does not depend on the denominator. Deltas are reported in ms per frame and converted to model terms (µs per row, µs per entry). Pass time is Metal System Trace time per frame. Arms are compared only within one session, build lineage, fixture and pose, and they are interleaved: the same map and rows have read 25% apart across sessions.
- **Partition top-down, then ablate within.**
  - A floor arm skips the entry loop entirely, leaving the indirection scan, base read and stores. It splits fixed per-row cost from per-entry cost.
  - Each hypothesis then gets an ablation arm that removes its suspected cost without regard to output, which bounds its share.
  - Only a hypothesis whose ablation moves pass time beyond the arm's spread gets an exact lever arm.
  - Ablation and floor arms are timing-only: never offered as levers, and byte-compared only as the negative control.
  - Any share of the floor or the entry term that no arm explains is reported as remainder, not spread across the hypotheses.
- **Stacked arm.** The hypotheses interact: loop bounding (H-e) can keep H-a's array from unrolling, and occupancy decides how much of H-b's and H-c's latency is hidden. So one arm stacks every recommended lever. Its measured saving, not a sum of per-lever savings, is the projection the note carries.
- **Exactness: a recommended lever writes byte-identical composed atlases.** It must match the baseline build, streamed and force-full-resident alike, at fixed animation times. This is stricter than §4's invariant, which compares streamed with full-resident inside one build and cannot see a shader change. Both must hold. A capture PNG is RGBA8 and does not prove it; the comparison reads back the `rgba16float` atlases.
- **Every lever carries a backend label (§10).**
  - All-backends needs a 1660 reading that shows the win. Until one arrives the lever is Metal-only. The 1660 run is an owner handoff, never a spike gate.
  - A lever that a returned 1660 reading shows regressing is not recommended.
  - There is no per-backend shader variant system (§8), so a Metal-only lever still ships to the 1660 perf floor. The note's follow-on must require a 1660 no-regression reading before landing.
- **Probes live on a throwaway branch** as shader and pipeline variants, chosen per build or by an environment variable read at pipeline creation. Main gains the findings note and the measurement records only. No debug sliders, which `experimental_spikes.md` §Tuning levers expects: these are code shapes, not tuning values. No player setting.
- **The H-e arm uses `unsafe`, approved for the throwaway branch only (owner, 2026-10-04).** Turning off wgpu's injected checks takes `create_shader_module_trusted` (`development_guide.md` §3.5). The approval holds only under these guards:
  - one call site, behind a cargo feature that is off by default, covering only the two compose shaders;
  - a `// SAFETY:` comment that cites the load-time validation keeping indices in range (`affinity_lights.len() == offsets.last()`, `validate_storage_levels_against_delta`);
  - the arm is timing-only, and its atlases go through the byte comparison as an in-range check: a mismatch reports an out-of-range bug, not a result;
  - the arm is deleted with the branch. `unsafe` never reaches main, and keeping it would be a new owner decision.
- **Gate H-e statically first.** Use naga's safe API to emit the compose shaders' MSL with checks on and off, and count the checks in the hot loops. Time the trusted arm only if those checks sit inside the per-entry or per-texel loops.
- **Renderer-only; no PRL format change.** A layout lever, such as lane-coalesced deltas for H-c, is probed by repacking at load on the branch. A format change becomes its own follow-on brief.
- **Non-goal: row-count reduction.** A separate direct build owns it: the compose contributing-row filter, which composes only rows carrying entries and lifts the script-mutable zero-drop and cone-cull exemption. This spike neither waits for it nor measures it.
- **Non-goal: rate limiting.** It is parked in `drafts/animated-light-update-rate` and breaks §4 exactness.
- **Non-goal: per-light change scoping.** Every light on the target maps animates continuously, so it saves nothing there.
- **Non-goal: fusing indirect and Pass B.** It is blocked by the binding budget, the pass order and doubled accumulator pressure (`research.md` §Rejected alternative).
- **Non-goal: building the chosen levers on main.** A follow-on owns that.

## Acceptance
### Honesty gates (pass/fail)
- [ ] Every arm in a batch builds from one recorded commit and one fixture set, with the fixtures' SHA-256 prefixes recorded. No batch straddles a change to which rows compose.
- [ ] Every timed run passes the foreground, unlocked and no-screen-saver checks, with an idle `ioreg -c IOAccelerator` snapshot per batch. Invalid runs are discarded and their count reported.
- [ ] Each arm at each pose has at least three traces, and its pass time is reported with spread. A run whose `compose frame` rows change during the run is discarded.
- [ ] For each lever arm offered as a recommendation, and for the stacked arm, the read-back indirect and direct composed atlases are byte-identical to the baseline build at no fewer than two stepped animation times, both streamed and force-full-resident. The sampled-row capture oracle test passes on that arm.
- [ ] Negative control: one ablation arm run through the same atlas comparison shows a difference. This proves the comparison sees compose output.
- [ ] No ablation or floor arm appears in the recommendation. The note marks each arm as floor, ablation, lever or stacked.

### Measured findings (measure-and-report)
- [ ] Per pose and per pass: rows composed by brick level (L0/L1/L2), and CSR entries by brick level.
- [ ] Baseline per-pass ms on the Mac at the hallway arena (`--start-pose=21.13,2.44,30.48,0,0`), kinematic-platform spawn and station (`--start-pose=-6.5,1.22,-27.94,0,0`), and campaign-test spawn. Add the fitted cost model per pass, with each coefficient's spread.
- [ ] Floor arm per pass: the fixed per-row share and the per-entry share of baseline time.
- [ ] For each of H-a to H-e: the ablation arm's change in ms per frame per pass, converted to model terms, and the lever arm's if one was built. H-e is reported unmeasured if the MSL gate finds no checks in the hot loops.
- [ ] Remainder: the part of the floor share and the per-entry share that no ablation arm explains.
- [ ] Stacked arm: measured saving per pass at the arena and kinematic poses, against the sum of the individual lever savings.
- [ ] Shader statistics (registers, spills, occupancy) for the baseline and the H-a arms, from any tool that exposes them. Otherwise report "unavailable", naming the tools tried.
- [ ] 1660 Super per-pass ms (owner handoff) for the baseline, each lever arm and the stacked arm, at campaign-test spawn and the hallway arena, or marked pending.

### Findings note (last deliverable)
- [ ] A findings note in this folder. It carries:
  - the top-down partition: fixed per row, per entry and remainder, per pass;
  - an attribution table: hypothesis, arm, change per pass, Metal and 1660;
  - per lever: byte-identity result and backend label;
  - the stacked arm's saving, projected through the model to post-filter row and entry counts at the arena and kinematic poses;
  - a build or no-build call per lever, naming whether the follow-on is a direct build or a brief.

## Path
Non-binding.
- **Seams:**
  - `compose_main` and `animated_compose_main`.
  - Their `animated_light_scale`, `read_delta_texel` and `reconstruct_l1_shared_texel`.
  - `curve_eval.wgsl`.
  - Pipeline creation in `sh_streaming/gpu/indirect.rs` and `sh_streaming/direct_compose/passes.rs`. The legacy `sh_compose.rs` and `animated_direct_sh_compose.rs` share the shaders.
- **First slice:**
  - The floor arm, to get the top-down split.
  - Then switch Pass B's accumulator to vec3. Its alpha is dead, because the store writes validity, so the change is exact by construction (`research.md` §Premise notes). It is the cheapest falsifier of H-a.
  - Then try texels outer and entries inner on the L0 path, which leaves a scalar accumulator.
- **H-b:**
  - Pass B's binding-26 `AnimatedLightScale` uniform already carries a per-light table, `compose_weights`, for 256 lights. It is precedent for per-light data that costs no storage binding.
  - A small GPU pre-pass that reuses `curve_eval.wgsl` is the exact-safe rival to evaluating scales on the CPU. `postretro_render_cpu::sh_compose::animated_light_scale` is tested only for closeness.
- **Ablation ideas:**
  - Floor: an empty entry loop.
  - H-b: a constant scale.
  - H-c: every lane reads `probe_rank` 0.
  - H-d: skip the L1/L2 branch. Separately, a single-slot reconstruction, which splits idle lanes from arithmetic.
  - H-e: the trusted module, after the MSL gate.
  - Fixed term: precompute the brick indirection word instead of lane 0's scan.
- **Atlas readback:** `ShProbeReadback::encode_copy` already copies the total atlas. The capture scene field `force_full_resident_sh_compose` and the `sampled_row_gate_capture_matches_full_resident_at_stepped_times` fixtures pin camera and stepped time.
- **Measurement:**
  - Copy `measurements/shadow-fill-cost/` (`clean.sh`, `run.py`, `gpu_time.py`, and its README's settings pins) into this spike's own measurements folder.
  - Take traces and deletions per §12, one trace at a time. Disk is tight.
- **Compiler statistics, both unverified:**
  - Xcode's Metal frame capture pipeline statistics on AMD.
  - Radeon GPU Analyzer, offline on naga-emitted SPIR-V for gfx1010, on the owner's Windows machine. It shows AMD's Vulkan compiler, not Metal's, so it is indicative only.
- **Optional backend split:** running Windows (Boot Camp) on the same 5300M would separate backend from architecture. It is an owner's-call extra, not a gate.
- **Shape chosen and rivals:** a top-down partition first (floor arm), then ablation within each share, then exact levers measured stacked. Two rivals were weighed:
  - **Bottom-up hypothesis ablation only.** It leaves unnamed costs, such as lane 0's scan and the base reads and stores, unattributed, and it sums interacting savings.
  - **Waiting for the row filter and measuring µs/row after it.** It couples the spike to unlanded work for no gain, since the per-row and per-entry model already carries the projection.

## Open questions
- A lever that moves pass time but differs from the baseline by rounding only, for example through FMA contraction — **delegated**: report its maximum absolute deviation and mark the recommendation conditional. The owner rules at findings review.
- Pass B's L1/L2 entry counts may be too small at some poses to fit their per-entry terms — **delegated**: add a pose with more such entries, or report the term as unfitted, and name it in the note.
