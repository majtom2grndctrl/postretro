# lighting-scale--sh-delta-receiver-reach-cull

Decision record · route: **no work** unless two owner rulings below change it · Epic: compile-time peak RAM · reads: `context/lib/rendering_pipeline.md` §4, `context/lib/build_pipeline.md` §PRL section IDs, `context/lib/index.md` §2 · read at e8eda7f

Not a brief. This folder records why the receiver-reachability cull on id 41 was not drafted, what would have to be ruled to revive it, and the spike shape it takes if both rulings land. Delete the folder or promote the spike; nothing here is queued.

## Problem as raised
Developer-raised, following Phase 1 (`context/plans/ready/lighting-scale--sh-delta-cone-reach-cull`). id 41's byte total is Σ over promotable static lights of (affinity cells reached × tile bytes). Coarsening lowers bytes per tile, never the (cell, light) count; merging lights is closed by the per-light subtraction the promotion crossfade needs. The proposed lever: drop id-41 entries for affinity cells no dynamic receiver can ever sample, on the grounds that id 41's only consumers are dynamic receivers sampling the composed direct-SH atlas. Unlike every landed cull, the dropped cells bake **nonzero** — safety rests entirely on the completeness of a baked "receiver-reachable" volume, which does not exist today and would have to be constructed.

## Why it does not clear the bar

**The invariant fixes the envelope.** `context/lib/index.md` §2: a physical light's contribution must never be double-counted on a given receiver — an architectural invariant, not a quality knob. A receiver sampling a cell whose id-41 entry was culled composes `base` (light L included) with no `w·delta_L` subtraction, then adds `w × runtime term` (`selection_weight`, `direct_sh_compose.wgsl`): L is counted twice on that receiver for the whole promotion, not only during the crossfade. So the envelope must cover every receiver class, or the cull ships the double-count by design.

**The consumer set is wider than movers and skinned characters.** `sh_direct_atlas` is sampled by `kinematic_brush.wgsl` (movers), `skinned_mesh.wgsl` (every mesh visual — skinned characters, rigid props, model-body projectiles, dropped wieldables), and `billboard.wgsl` (the legacy path when id 47 is missing, invalid, over-cap, or device-limit-rejected — a runtime fallback, not a bake-time choice). Projectiles fly any open line; particles have authored buoyancy and rise (`scripting.md` §10.1 Emitter and Particles). Their reachable set is every open, portal-connected cell — the same set the existing reach predicate already keeps (`cells_for_light` ∩ `reachable_leaves`, `affinity_grid.rs`).

**The sampler footprint dilates it further.** Composed-atlas reads are trilinear over the eight lattice neighbours (`sh_trilinear_weight`, `sh_sample.wgsl`; `sh_reconstruct::trilinear_weight`), so a receiver reads probes one spacing away in every axis; L1/L2 reconstruction stays intra-brick and adds nothing. Any envelope is dilated by one probe on every face.

**Even the narrow envelope is content-bounded, not engine-bounded.** With projectiles and particles excluded, receiver height above a floor is authored: jump/air-jump/air-dash budgets live in the declarative movement descriptor (`movement.md` §2 Author surface, §4 State-machine seam), knockback with upward blend and explicit rocket jumping in weapon and character descriptors (`entity_model.md` §Knockback, `movement.md` §6 Knockback), mover rides in id-43 waypoints. No compiler-visible constant bounds a receiver's altitude. A safe bound is a new declared surface — a worldspawn KVP or mod-wide constant — i.e. a modder-facing contract, and every map that under-declares it ships the double-count silently. Spawn positions are not the problem the grounding feared: E18 spawners place enemies at the authored spawner origin plus a fixed per-index offset (`context/plans/done/E18--spawner-and-closet-containment`), bake-knowable.

**What is left to cull is small and quantised away.** An affinity cell is `AFFINITY_FACTOR × probe_spacing` = 4 m at the shipping default (`sh_bake::DEFAULT_PROBE_SPACING`). Capsule (1.8 m, `nav_agent_height`) + jump + one-probe dilation spans roughly one cell layer above every floor and every mover pose. The candidate set is therefore open cells two or more layers above any floor and outside every mover sweep: the upper half of tall rooms. On the warren the rooms are 13.0 m interiors and corridors 6.5 m (`tools/gen_stress_map.py`, `STORY_H`, `ROOM_STORIES`), so only room layers 3–4 qualify — and only under the projectile/particle exemption the invariant forbids. Unmeasured; the per-selection-light histogram (`DirectDeltaBakeStats`) attributes bytes to lights, not to cells, so measuring it needs new per-cell attribution either way.

**The adjacent byte-identical lever is already empty on the motivating map.** Cells whose centroid lies in a solid leaf are kept by the reach predicate's bypass; their invalid probes bake the zero tile (`bake_direct_delta_subblock`, `probe_is_valid`) and `drop_direct_zero_entries` removes them after the dense payload exists. Culling zero-valid cells at decomposition would be byte-identical — but `context/plans/done/lighting-scale--compile-peak-ram/research.md` records the warren's compaction count equal to its dense count (full validity across every CSR cell), so that lever removes nothing there. Not pursued; Phase 1 owns the decomposition seam.

**Against Phase 2.** `context/plans/drafts/lighting-scale--sh-delta-cell-major-two-pass-bake` bounds peak RAM by max-lights-per-cell regardless of Σ and stays byte-identical. Emitted bytes and per-frame compose traffic are governed by the 256 MiB cap, the 128 MiB loader floor, and the runtime-safe coarsening envelope (`sh_runtime_envelope.rs`), which already carries a mapper-authored receiver-relevance signal in the other direction (`sh_protect_volume` forces L0). A cull whose safety story is a constructed volume with authored bounds does not beat a structural, byte-identical lever plus an error-bounded coarsener.

## Owner decisions this raises
1. **Invariant scope.** Is "never double-counted on a given receiver" allowed to exclude projectile bodies and billboard particles under promotion — a designed over-brightening on small fast receivers? Recommendation: **no**. It is the one lighting invariant in `index.md` §2; carving a receiver class out of it is a one-way door for every later consumer of binding 15.
2. **Receiver altitude bound.** If (1) is opened: who declares the maximum receiver height above a walkable floor / mover pose, and at what layer — worldspawn KVP, mod-wide constant, or engine default? Recommendation: do not add the surface; an under-declared bound fails silently, and the cull's whole win sits above it.
3. **Legacy billboard fallback.** If (1) is opened: is the id-47-absent billboard path (a runtime device/cap fallback) in or out of the safety contract? Recommendation: in — it is not authorable per map.
4. **Where the RAM problem goes instead.** Confirm Phase 1 → Phase 2 as the peak-RAM line; treat this cull as closed unless (1) and (2) both open.

## If both doors open: spike, not a brief
Build-to-learn per `context/lib/experimental_spikes.md`; deliverable is a measured cullable fraction, not a cull.
- **Honesty gate (automated).** The envelope is built from baked signals only — navmesh regions (id 36) lifted by the declared altitude bound, id-43 mover sweeps lifted the same way, authored spawner and `player_spawn` origins — then dilated by one probe on every face and intersected with `probe_is_valid` space; every affinity cell containing a lifted point is inside it. An entry is counted cullable only when the cell is outside the envelope **and** its baked payload is nonzero after `drop_direct_zero_entries` (a zero entry is Phase 1's, not this spike's).
- **Measured finding (measure-and-report).** On `campaign-test`, `kinematic-platform`, and `stress-warren-hallway-inspection` at the shipping density: cullable entries and bytes as a fraction of post-drop Σ, per selection light and in total, at two altitude bounds (capsule + authored jump; capsule + authored jump + authored rocket-jump apex). No pass threshold.
- **Recommendation rule.** Promote to a brief only if the fraction at the *larger* bound is material after Phase 1 and Phase 2 land; otherwise close this folder.

## Handoff
```markdown
Problem: id-41 Σ is a count floor coarsening cannot reach; proposed to drop nonzero entries in cells no dynamic receiver samples.
Outcome: a smaller id-41 CSR with no receiver ever sampling a cell missing its light's subtraction term.
Read at: e8eda7f
Verified facts: (by symbol)
  - Consumers of the composed direct atlas: kinematic_brush.wgsl, skinned_mesh.wgsl, billboard.wgsl (legacy path) — `sh_direct_atlas`.
  - Promotion subtraction is per entry, weighted by `selection_weight` (direct_sh_compose.wgsl); a culled entry double-counts its light on any receiver in that cell for the whole promotion.
  - Sampling is trilinear over eight lattice neighbours (`sh_trilinear_weight`, sh_sample.wgsl); reconstruction is intra-brick.
  - Reach predicate today: `cells_for_light` = light AABB ∩ `reachable_leaves`, solid/exterior-centroid cells kept (affinity_grid.rs); no validity or receiver input.
  - Invalid probes bake the zero tile (`bake_direct_delta_subblock`); zero entries are dropped post-bake (`drop_direct_zero_entries`).
  - Warren: compaction count == dense count (full validity) — compile-peak-ram research.md.
  - Cell pitch = `AFFINITY_FACTOR` × `DEFAULT_PROBE_SPACING` = 4 m; warren rooms 13.0 m, corridors 6.5 m (gen_stress_map.py).
  - Spawner placement is authored (E18); no script-computed spawn position surface found in scripting.md.
  - Receiver altitude is descriptor-authored (movement ability budgets, weapon knockback/rocket jump); no engine constant.
  - `AffinityReachPolicy` does not exist yet — Phase 1 is in ready/, not landed.
Not verified: the cullable fraction under any envelope (no per-cell attribution exists; histogram is per light).
Decisions: none taken — every candidate decision is an owner ruling on the invariant or a new authored bound.
Non-goals: zero-valid-cell cull at decomposition (byte-identical, Phase 1's seam, empty on the warren); any change to id-40 selection or the crossfade.
Proof: n/a for no work. Spike proof shape above if revived.
Open owner questions: 1–4 above.
Route: no work
Why this route: under the double-count invariant the safe envelope equals the existing reach predicate; the only cullable remainder needs an invariant carve-out plus an authored altitude bound, and Phase 2 bounds the same peak byte-identically.
Next action: discuss
```
