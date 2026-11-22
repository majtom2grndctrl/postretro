# reflection-probe-fluid-pass-performance

Brief · compact · reads: `context/lib/rendering_pipeline.md` §7.3a (Live lighting, Two pipelines, References), §10; `context/lib/resource_management.md` §5.3; `context/lib/development_guide.md` §2.1 · read at c0dfd54b

## Problem
Developer-raised, from the blend-cost and cell-rate spikes and the `fluid-base-pass-cost` landing runs. Reflecting water costs well over reflection-free on the Radeon Pro 5300M, the Mac perf target (§10) (`research.md` §Evidence). Part of the cause is live lighting. Each reflecting fragment re-evaluates every stored light's curve and sums its lobe shares, though those sums vary only by probe and frame. The cost grows with every consumer that samples a probe: the seams blend, glossy world faces, and a cell path. Done means live lighting is evaluated once per probe per frame on the GPU, every reflecting fragment reads the result, and the image is unchanged.

## Decisions
- **Scope: the probe term's live-lighting ratio.** The reflection-free variant, the search, Fresnel, the composite and both styles' looks are unchanged (§7.3a). The reflecting variant only stops evaluating lights.
- **A GPU compute pass, once per frame with any reflecting-variant draw.**
  - It writes, per probe and per §7.3a lobe, the RGB sum the ratio is linear in: `Σ_l share_{l,k} · (current_l / ref_l − 1)`.
  - The fragment blends those sums by its lobe weights and clamps, as the ratio does today. No fragment entry evaluates a probe's stored lights.
  - GPU, not CPU. The pass reads the descriptor and sample buffers that script writes land in, so scripted lights never read stale. A CPU form needs a mirror of bridge samples the renderer does not keep (`research.md` §Precompute).
  - All backends, not Metal-only (§10): the lever removes fragment work everywhere, and the GTX 1660 floor is measured beside the 5300M.
- **Every installed probe, not the drawn set.** Freshness holds by construction for a face first drawn this frame, with no CPU set build. This diverges from the blend spike's drawn-probes recommendation, which priced a CPU loop, not one dispatch (`research.md` §Precompute).
- **Level-scoped storage, read as a uniform.**
  - The sums live in one buffer at the probe cap, created at fluid install beside the share table (`resource_management.md` §5.3) and released with it.
  - The fluid fragment reads the sums as a uniform in the level-scoped fluid group, because fragment storage is spent (§10).
  - No bind group rebuilds per frame; only the buffer's contents change.
  - Its pipeline builds at full init beside the other fluid pipelines, never during a frame.
- **IP fences (owner, 2026-11-22), from `ip-check.md` §Lever A.** The owner accepts that section's engineering reading for this lever; counsel's answer on the shipped ratio (§For counsel 1–2) does not gate it.
  - The sums live in their own buffer. They never write the probe tile, the share table or any record the capture owns.
  - Every installed probe is evaluated every frame that dispatches; no probe, face or lobe subset is amortised across frames.
  - The coefficient stays `current / ref − 1` per light. No SH basis, transform matrix or virtual-light projection.
  - Only probe-bound faces of geometry whose position and orientation are fixed read the sums. Every probe-bound face is such a face today. A later brief that binds a probe to a moving surface needs owner IP review first.
- **Non-goals:**
  - The cell path, hidden-cell skip and per-draw path choice: `reflection-probe-cell-path`, which reads these sums.
  - Probe blending: `reflection-probe-seams` (draft on branch `claude/probe-streaming-brief-364a5e`) reads the sums per blend neighbour.
  - Slot-indexed probes under streaming: whichever of this brief and `reflection-probe-streaming` lands second re-keys the sums.
  - An added lobe and an overlay dispatch condition: `glossy-world-reflections` extends both.

## Acceptance
Orderings cite `research.md` §Pins.
### Automated
- [ ] The ratio blended from the sums matches today's per-fragment ratio within 1e-4 of the larger of 1 and the summed terms' magnitude, across random probes, lights, lobes and directions. Cases include a slot holding no light, a light past the descriptor table, a zero reference channel, and a ratio that clamps at zero.
- [ ] Within a frame, in the game and in capture alike: script light writes, the descriptor flush, the sums pass, then the fluid pass. The game's frame loop commits the light bridge before the uniform upload, and both before the frame render. Capture's forced descriptors precede its first uniform upload. The sums pass records into the frame's encoder, after the timing prefill and before the fluid pass. No command recorded between them writes a descriptor or sample buffer. Queue writes need no gate, since the frame's upload batch copies ahead of its whole submission. Source-order gates in the app and the renderer pin it (P1).
- [ ] Entry globals, pinned the way the reflection-free entry's are, by global use rather than by function name: neither fluid fragment entry uses the animated-light descriptor buffer or reaches the per-fragment ratio; the reflecting entry declares the sums; the reflection-free entry declares none of them.
- [ ] The dispatch condition takes the frame's one mask snapshot and tier value, the ones the fluid pass's draws take. A mask set between a frame's uniform upload and its render moves neither the variants nor the dispatch until the next upload. A source gate and a GPU frame pin it (P6).
- [ ] IP fences, grep gates:
  - the sums pass's layout holds one writable binding, the sums;
  - the probe colour and distance arrays gain no storage or render-attachment usage, and the share table stays uniform-only;
  - only the fluid surface layout binds the sums: never the screen-space, planar, mover, model or world layouts;
  - the fluid vertex buffer stays vertex-only, written once at install.
- [ ] The sums pass's light evaluation keeps the per-fragment evaluation's source gates, retargeted to its shader: no promotion weight, crossfade, lighting mask, debug override or SH data; descriptors indexed by roster index; the frame's animation time.
- [ ] The sums pass and the fragment blend each have a GPU-free reference, and a test pins each shader text against it (§7.3a References).
- [ ] The no-history gate's binding inventory covers the sums pass shader and lists the sums by name; none is named for history or intersections.
- [ ] Budget pins pass with the new binding, including the compute stage's counts; no shared group gains a stage its shader does not read.
- [ ] The sums pass records a labelled GPU timing pair inside the frame's prefilled query set.
- [ ] No sums pipeline is created outside full init. No sums buffer is created outside fluid install or released outside unload. A source gate refuses both.
### Automated: GPU gates (a GPU machine, under `POSTRETRO_REQUIRE_GPU`; a skip for want of an adapter is not a pass)
- [ ] Dispatches: no fluid level, reflection-free fluid only, or a lighting mask shutting every reflecting draw dispatch no sums pass. A frame with reflecting draws dispatches it exactly once, whatever the draw count (P2).
- [ ] The dispatch follows the fluid pass's reflecting draws, not the screen-space pass's (P6):
  - a reflecting frame at the low tier, and a very-high frame whose reflecting draws all lie on the selected plane, each dispatch exactly once and match the build before within 1/255 with a stored light forced away from its reference;
  - a compose-only frame with a level held dispatches none.
- [ ] A script switches a stored light off on frame N: the reflection drops that light on N (P1).
- [ ] A reflecting face first drawn this frame, after a camera turn or from behind a wall, with a stored light of its probe forced away from its reference, shows that light on that frame. On a frame whose drawn faces all bind one probe, an undrawn probe's sums, read back, carry its forced light too (P3).
- [ ] Every installed probe's sums, read back on two consecutive dispatching frames with a stored light changed between them, match the CPU reference on both frames. Probe counts are 1, one more than a workgroup's width, and the cap (P3).
- [ ] A frame with reflecting draws creates no buffer, bind group or pipeline beyond a reflection-free frame's, by the GPU creation counts, across consecutive frames with a stored light changed between them.
- [ ] Against the build before, within 1/255, at a view where a stored light differs from its reference, in pixel and smooth style. Also a stored light on an animated curve at time 0, and at a later time reached after a prior frame at a different time (P4).
- [ ] Sums pre-filled with a poison value render identically to cleared ones on the first frame after a level is published (P3).
- [ ] Level B, with a different probe count and different stored lights, is built while A draws and then published in A's place. The first frame after the publish reads B's probes and matches the build before within 1/255. A frame drawn between B's build and its publish still matches A's (P5).
- [ ] Level A is kept resident while level B is published, then A is published again. A stored light of A is forced away from its reference while A is unpublished. A's first frame shows that light and matches the build before within 1/255, and publishing A creates no sums buffer, pipeline or bind group (P7).
- [ ] The reflection-free variant and every §7.3a Off case render identically to the build before.
- [ ] Every fluid pipeline and the sums pipeline compile under the DX12 FXC test on a Windows DX12 adapter, its log showing it ran, not skipped.
### Manual
- [ ] **5300M, near, probes and mid at 4K and Auto (1536×960):** reflecting fluid pass plus sums pass against the build before, with whole-frame GPU time beside it. No view is slower beyond run-to-run spread. Saving and the sums pass's own cost measured and reported (`research.md` §Measurement).
- [ ] **GTX 1660, Vulkan:** the same views against the build before. Measured and reported.

## Path
- **Seams:**
  - `fluid_reflection_ratio`, `fluid_reflection_light_value` and the lobe and share helpers in `fluid_reflection.wgsl`.
  - `reflection_ratio` and `reflection_light_value` in render-cpu `fluid_surface/reflection.rs`.
  - `FluidProbeResources` and `fluid_probe_bind_group_layout_entries` for the level-scoped group.
  - `record_pre_scene_compute` / `record_direct_sh_pre_scene_compute` for compute placement.
  - The tests `fluid_reflection_live_lighting_wgsl_bodies_match_the_cpu_reference`, `the_reflection_free_entry_declares_none_of_the_reflection_globals` and `fluid_pass_declares_no_history_or_precomputed_intersection_binding`, and `pipeline_budget_tests.rs`.
- **Shape:** a compute shader over installed probes, reusing the ratio's light-value and share helpers, writing one RGB sum per lobe per probe.
- **Rivals:**
  - A CPU precompute uploaded each frame (the blend spike's). It needs a bridge-sample mirror.
  - Per-light values once per frame, Quake's lightstyle shape: each light's `current / ref − 1` in a buffer, with the share × value sum kept per fragment. No per-probe, per-direction state exists, so it is the IP fallback if counsel reads '451 claim 6 onto the sums (`ip-check.md` §Lever A). It loses on cost: fragments still read shares and loop lights × lobes, and each seams neighbour repeats that loop.
- **Keep the per-fragment ratio:** `fluid_reflection_ratio` and its render-cpu reference stay, unreachable from any entry, as the sums' parity oracle and as the fallback for a reflector the IP fences send back to per-fragment (`glossy-world-reflections` Sequencing).
- **Buffer usage:** one buffer bound as storage to the sums pass and as a uniform to the fluid fragment, or a copy into a uniform. Delegated. Check the usage combination on Metal, Vulkan and DX12.
- **Compute group:** the share table is fragment-visible today. The sums pass needs it with compute visibility, through its own layout over the same buffer, or a widened one.
- **First slice:** the sums pass and blend behind the parity test, then one 5300M A/B at probes 4K. The ratio cost there was largest pre-landing.
- **Register screen:** the reflecting entry loses its light loop, which may move it to a different VGPR tier on the 5300M (§7.3, Mover register cliff). Screen `fs_main_reflecting` before and after with `tools/metal_register_screen/`'s method, retargeted from the mover pipelines, so a timing change has a named cause.
- **Large files:** `fluid_surface.rs` and `renderer_render_frame.rs` are past §2.1's split line. Split the touched responsibility out first, in its own behaviour-preserving commit.

## Open questions
- Placement of the dispatch within the frame (pre-scene compute or the fluid block) — **delegated**.
