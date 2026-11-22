# reflection-probe-fluid-pass-performance — research

Derivation behind the brief's decisions. Read at `c0dfd54b`; re-checked at `e36ea2d92` after `area-lights` merged, which changes no fluid shader, frame order, budget or share layout. The cell path's derivation, early-out and spike tables live in `../../drafts/reflection-probe-cell-path/research.md`. IP: `ip-check.md` here, §Lever A.

## Evidence

**After `fluid-base-pass-cost` landed.** `measurements/fluid-base-pass-cost/results-landing2.md`, 5300M, Metal System Trace, 2 interleaved runs per arm, two-binary A/B. Mask `0x13F` selects reflection-free; `0x17F` (all terms) selects the reflecting variant at the capture's reflection tier. Fluid Surface Pass, ms per frame at 4K:

| View | reflection-free | reflecting | reflection's share |
|---|---|---|---|
| near (e24 pool, straight down) | 2.16 | 12.33 | 10.2 |
| test (probes map, straight down) | 2.14 | 14.82 | 12.7 |
| mid (e24, pitch −30) | 1.25 | 6.52 | 5.3 |

The reflection is now most of the reflecting pass. Before the landing the reflection-free pass dominated: near 52.3 against 64.2 reflecting.

**Live lighting's part, measured before the landing:**
- **Cell-rate spike, ratio forced to 1:** the ratio cost 1.2 ms per fragment at e24 near 4K and 5.2 ms on the probes map at 4K. The probes map stores more animated lights per probe.
- **Blend spike, CPU-side precompute:** one sample 17% cheaper on the 5300M at the typical pose, 21% on the 1660. A forced three-sample blend dropped from +70% to +14% on the 5300M.
- Neither has been re-measured on the post-landing build. The brief's manual rows do that. Register pressure in the reflecting entry changed with the landing, so the old figures bound the saving's order, not its size.

**Evidence lost.** The 5300M cell-rate spike branch (`claude/cell-rate-spike`, `3f4cabed5`) is gone from origin with its data. Its figures survive only as recorded in the sibling research. The blend spike branch (`claude/blend-cost-spike`) survives.

## Precompute

**Algebra.** The ratio is `max(1 + Σ_k w_k(d) · S_k, 0)` with `S_k = Σ_l share_{l,k} · (current_l / ref_l − 1)`. `S_k` depends on the probe and the frame only, so one RGB sum per lobe per probe reproduces the per-fragment ratio. The blend spike measured agreement within 1e-4 over more than 10k cases.

**Inputs on HEAD.**
- `fluid_reflection_light_value` reads `anim_descriptors` and `uniforms.time`, through the shared `curve_eval.wgsl` helpers, which read `anim_samples`.
- Script-driven samples reach `anim_samples` through `upload_light_bridge_snapshot`, a queue write before recording.
- Descriptors flush once per frame in `update_per_frame_uniforms` (`upload_descriptors_if_dirty`), before `record_scene_passes`.
- Group 3's descriptor and sample bindings are fragment- and compute-visible. The light-value function is duplicated across the SH composes, the scatter compose and the fluid shader, each pinned by text against `animated_light_scale`. `area-lights` adds a forward copy, `area_glow_factor`, which evaluates an area light's descriptor per fragment for its glass glow.

**Why GPU.**
- The renderer mirrors descriptors and activity on the CPU. It keeps no copy of bridge samples: the light bridge stages them and the renderer uploads them without retaining one. A CPU precompute therefore needs a samples mirror.
- The blend spike's mirror skipped bridge writes, so scripted lights read stale there.
- A compute dispatch reads the same buffers and curve code the fragment reads. That gives parity by construction and adds no CPU work on the Mac, which is CPU-bound on the hallway map.

**Every installed probe.** The blend spike recommended limiting the precompute to probes of drawn volumes. Its reason was CPU cost: about 0.6 µs per probe, about 150 µs at the cap. On the GPU the whole cap is one small dispatch over at most four lights per probe. Evaluating every installed probe drops a CPU set build and keeps an undrawn or newly drawn probe current. The pass's own cost is a manual row.

**Delivery.**
- Fluid fragment storage is 8 of 8 (`fluid_pipeline_fragment_storage_request_matches_bgl_definitions`). Fragment uniforms are 7 of 12 under `wgpu::Limits::default()`.
- At the cap the sums are 256 × 5 × 16 B = 20 480 B, inside one 64 KiB uniform binding. They cannot join the share table, which is already 61 456 B and written once at install.
- Group 4 is level-scoped: built at fluid install, holding the params window and the probe resources. Group 5 rebuilds with the scene targets and exists twice, for SSR and for the planar layer. The sums belong in group 4.
- The blend spike bound its uniform at group 5 binding 2. That slot now holds the reflection target.

**Doors.**
- `reflection-probe-seams` assumes each blend sample applies its own probe's ratio. With the sums, a neighbour costs a lobe blend, not a light loop.
- `glossy-world-reflections` reads the sums for world faces. It appends a sixth lobe and extends the dispatch condition to overlay draws.
- `reflection-probe-streaming` indexes probe data by pool slot. Whichever lands second re-keys the sums.
- `dynamic-reflection-layer` research names the sums as where an aggregated glow term would join.
- `area-lights` stores an area light like any other animated light: its glass glow is in that light's share, and its value comes from its compose descriptor. One as-built limitation (owner call, its `plan.md` row 47): a `light 0` area light's reflected glass holds its reference glow even while dark. The sums match the per-fragment ratio, so they keep that behaviour; a fix belongs to a later brief.
- Forward's area glow evaluates a per-light, per-frame value per fragment. The per-light rival (Path) would serve it too; the per-probe sums do not.

## Measurement

Kit: `measurements/fluid-base-pass-cost/metal/` (two-binary A/B, `CELL_BIN_BEFORE` / `CELL_BIN_AFTER`; poses `near`, `test`, `mid` and their `-auto` forms at 1536×960). Arms run under mask `0x17F` to select the reflecting variant. Report Fluid Surface Pass plus the sums pass, and whole-frame GPU time beside it. Auto clocks float, so report each run's mean clock. Build `before` from the merge-base in a disposable worktree (`development_guide.md` §Worktree builds). The binary must be named `postretro`.

## Pins

| Id | Scenario | Ordering | Expected |
|---|---|---|---|
| P1 | A script switches a stored light off on frame N | bridge write, descriptor flush, sums pass, fluid pass, all on N; queue writes copy ahead of the frame's whole submission, so only an encoder command between the two passes can break it | The reflection drops that light on N |
| P2 | Zero reflecting-variant draws (no fluid level, reflection-free only, or a mask shutting every reflecting draw); N reflecting draws | — | No dispatch; exactly one |
| P3 | A face first drawn on N; an undrawn probe; the first frame after a newly built level set is published | the sums pass evaluates every installed probe on N, before the fluid pass | No fragment reads a stale or unwritten sum |
| P4 | Stored light on an animated curve at time t, after a prior frame at another time | the sums pass reads N's animation time | Matches per-fragment at t |
| P5 | Level B built off the main thread while A draws, then published in A's place; frame N is the first after the publish | B's sums buffer and the sums pass's bind group are built in B's set, touch no pass state, and become the ones the dispatch binds at the publish, before N; frames before the publish dispatch into A's | N's sums are B's; no frame before N reads or writes B's |
| P6 | A reflecting frame at the low tier; a very-high or ultra frame whose reflecting draws all lie on the selected plane; a compose-only frame; a mask set after the frame's uniform upload | the dispatch condition is the fluid pass's own: its draw gate, the frame's one mask snapshot and one tier value, every reflecting draw on or off the selected plane, never the screen-space pass's gate | A dispatch exactly when the fluid pass records a reflecting draw this frame; no fragment reads sums written on an earlier frame |
| P7 | Level A kept resident, B published, A published again on frame N, a stored light of A changed while unpublished | A's sums buffer and group were built with A's set; publish builds nothing; the sums pass writes A's probes on N, before the fluid pass | N's sums are A's, with A's current lights |
