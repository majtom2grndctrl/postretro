# Light-Bridge Runtime-Light Slot Reclamation

## Goal

Make the light bridge reclaim a despawned runtime (script/gameplay-spawned)
dynamic light's reserve slot, so **high-churn** runtime lights — projectile travel
and impact lights, future muzzle flashes and tracers — are bounded by the
**concurrently-live** count, not the cumulative count spawned this level. Today a
despawned runtime light's `entity_ids` entry is never removed, so cumulative churn
permanently exhausts `RUNTIME_DYNAMIC_LIGHT_RESERVE` and further runtime lights
silently stop rendering. This is the load-bearing prerequisite for
`projectile-weapon-enhancements` (whose per-shot travel + impact lights are the
first high-churn runtime lights) and any later moving/transient gameplay light.

## Scope

### In scope

- Reclamation of the reserve slot held by a despawned runtime light in
  `LightBridge` (`crates/postretro/src/scripting/systems/light_bridge.rs`): a
  despawned runtime entry's slot becomes reusable, so the reserve bound reflects
  live lights.
- The reserve-exhaustion check (`absorb_dynamic_lights`) counts **live** runtime
  lights, not `entity_ids.len() - authored_light_count`.
- The tombstone GPU zeroing behavior is preserved (a vanished light still zeroes
  its forward/compose slot for the frame it disappears); reclamation is additive
  to it.
- A deterministic test: N sequential spawn→despawn cycles of runtime lights keep
  the live runtime count bounded by the concurrent count (not N), and a light
  spawned after the reserve was "cumulatively" exceeded still renders.

### Out of scope (non-goals)

- The **authored map-light prefix** (`[0, authored_light_count)`), which is fixed
  for the level and never reclaimed.
- Any change to how lights are packed, animated, or shadowed beyond slot accounting.
- The follow-Transform contract, radius-animation channel, or any projectile
  feature — those live in `projectile-weapon-enhancements`; this spec only fixes
  slot accounting so those can rely on it.
- Raising `RUNTIME_DYNAMIC_LIGHT_RESERVE` (`renderer_types.rs`) — reclamation, not
  a bigger cap, is the fix.

## Direction

**Problem.** `LightBridge.entity_ids` only ever grows or is cleared wholesale
(field comment: "Despawned entries remain as tombstone slots"; the only mutators
are `clear`, `populate_from_level*`, and the append in `absorb_dynamic_lights`).
`update`'s tombstone path zeroes a vanished light's GPU slot but leaves its
dead-generation `EntityId` in `entity_ids` forever. `absorb_dynamic_lights` bounds
new runtime lights on `runtime_count = entity_ids.len() - authored_light_count`, so
every runtime light ever spawned counts against `RUNTIME_DYNAMIC_LIGHT_RESERVE`
(= 256, `renderer_types.rs`) permanently. This was invisible while runtime lights
were rare and long-lived; projectile lights (up to 2 per hitting shot, both
short-lived) are the first to churn fast enough to exhaust it in seconds.

**Prior commitments.** Preserves the authored-prefix-stable / runtime-appended
layout the bridge relies on (authored indices never move; `snapshots` keyed by id;
the parallel `shape`/`cached_origins_f64`/`cached_influences` arrays and the
`scripted_sample_buf` region indexed by slot). Preserves the no-double-count and
tombstone-zeroing behavior. The reserve is a renderer-owned capacity
(`RUNTIME_DYNAMIC_LIGHT_RESERVE`); this spec changes only CPU-side accounting.

**Alternatives rejected.** (a) Raise the reserve — only delays exhaustion, does not
fix the leak. (b) Compact `entity_ids` on every despawn (shift later runtime
entries down) — churns the parallel arrays and the `scripted_sample_buf` offsets,
and reorders the packed GPU light list every despawn. A **free-list of reusable
runtime slot indices** (recommended) keeps every other slot's index stable, so a
reused slot just receives new data and the authored prefix is untouched; the exact
mechanism is this spec's to pin.

**Foreclosures.** None material. Reclamation reverts cleanly; it adds accounting,
removes no capability.

## Acceptance criteria

- [ ] After N sequential spawn→despawn cycles of runtime dynamic lights (N far
  exceeding `RUNTIME_DYNAMIC_LIGHT_RESERVE`) with at most K < reserve alive at
  once, every cycle's light renders — the reserve bound tracks the live count, not
  cumulative spawns.
- [ ] A despawned runtime light still zeroes its GPU forward/compose slot on the
  frame it disappears (tombstone zeroing preserved); no stale light lingers.
- [ ] The authored map-light prefix is unaffected: authored indices, their
  snapshots, and their packed order are identical before and after the change.
- [ ] Reclaiming and reusing a slot does not mis-attribute animation state: a new
  light in a reused slot starts from its own component state, not the prior
  occupant's snapshot/`scripted_sample_buf` residue.
- [ ] After the churn cycles, the per-frame emitted light buffer and
  effective-brightness lengths — the forward pass's per-fragment `full.light_count`,
  and the byte count uploaded each dirty frame — track **peak-concurrent** runtime
  lights, not cumulative spawns (the bounded-frame-cost payoff; also bounds the
  per-frame `effective_brightness` and `collect_all_as_map_lights` scans).
- [ ] A level reload with a different `authored_light_count` never hands a runtime
  light an authored-prefix slot (the free-list is reset on `clear` /
  `populate_from_level_with_influences`); the authored map lights render unchanged
  after reload.
- [ ] No new `unsafe` (grep gate). No wire-format or GPU-layout change (CPU
  accounting only).

## Tasks

### Task 1: Reclaim runtime-light slots

Give `LightBridge` a free-list of reclaimable runtime slot indices (those `≥
authored_light_count` whose light has despawned). When `update` detects a tracked
runtime id whose `get_component::<LightComponent>` fails (the existing tombstone
branch), in addition to zeroing its GPU slot and dropping its `snapshots` entry,
mark its slot index reclaimable and clear its residue in the parallel arrays
(`shape`, `cached_origins_f64`, `cached_influences`) and its `scripted_sample_buf`
region so a future occupant cannot inherit stale animation samples. In
`absorb_dynamic_lights`, reuse a free-list index for a newly-absorbed runtime light
before appending a new one, and change the reserve-exhaustion check to count
**live** runtime slots (occupied, non-reclaimable, `≥ authored_light_count`)
rather than `entity_ids.len() - authored_light_count`. Keep the authored prefix and
its indices untouched. Preserve the tombstone GPU zeroing for the disappear frame.
**Borrow-split (implementation pin):** collect reclaimed indices *during* the
`entity_ids.iter()` diff loop and apply the `shape`/`cached_*`/`scripted_sample_buf`
residue clears *after* the loop — mirroring the existing `settled: Vec<(EntityId,
LightComponent)>` write-back — since mutating `self.shape[idx]` inside
`self.entity_ids.iter()` will not compile. **Cross-level reset:** clear /
reinitialize the free-list in `clear()` and `populate_from_level_with_influences`
alongside the arrays they already reset — a free index carried into a level with a
different `authored_light_count` would hand a new runtime light an **authored-prefix**
slot and silently corrupt a map light. Add the deterministic churn test (N cycles,
bounded live count, post-exhaustion light still renders) plus: an
authored-prefix-unchanged regression; a reused-slot-no-residue assertion; a
**buffer-length** assertion that after churn `update`'s emitted `lights_bytes.len()`
/ `effective_brightness.len()` (hence the forward pass's per-fragment
`full.light_count`) track peak-concurrent, not cumulative — the bounded-frame-cost
payoff; and a **cross-level-reset** assertion that a reload with a different
authored count never reuses a stale free index into the authored prefix.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| **Reserve bound tracks live, not cumulative** runtime lights | Task 1 (free-list + live-count check) | counting `entity_ids.len()`; a reused slot not decremented from the live count on despawn | AC 1; churn test |
| **Tombstone zeroing preserved** — a vanished light zeroes its slot the frame it disappears | Task 1 (reclamation is additive to the existing zeroing) | reclaiming before the zeroing upload, leaving a stale GPU slot | AC 2; disappear-frame test |
| **Authored prefix untouched** — authored indices, snapshots, packed order stable | Task 1 (free-list confined to `≥ authored_light_count`) | compaction shifting authored or runtime indices | AC 3; authored-prefix regression |
| **Reused slot carries no residue** — a new occupant starts from its own state | Task 1 (clear parallel arrays + `scripted_sample_buf` region on reclaim) | a reused slot inheriting the prior light's snapshot/samples | AC 4; reused-slot test |
| **Bounded frame cost** — emitted buffer + per-fragment `light_count` + per-frame scans track peak-concurrent, not cumulative | Task 1 (live-bounded `entity_ids`) | a compaction/refactor silently regrowing the emitted count; the shrink asserted only implicitly | AC 5; buffer-length assertion |
| **Free-list reset across levels** — no stale index into a new level's authored prefix | Task 1 (reset in `clear`/`populate_from_level_with_influences`) | a free index surviving teardown → a runtime light overwriting an authored map light next level | AC 6; cross-level-reset assertion |

## Rough sketch

Entry points: `LightBridge` (`entity_ids`, `authored_light_count`, `shape`,
`cached_origins_f64`, `cached_influences`, `snapshots`, `scripted_sample_buf`),
`absorb_dynamic_lights` (the append + reserve bound), `update` (the tombstone
branch on a failed `get_component`), all in
`crates/postretro/src/scripting/systems/light_bridge.rs`.
`RUNTIME_DYNAMIC_LIGHT_RESERVE` in `postretro_renderer` (`renderer_types.rs`).
Also audit `collect_all_as_map_lights` (run each frame for fog), which scans
`entity_ids` — a free-list keeps its length bounded by live lights too.

No generational-id hazard to add: `EntityId` carries a generation, and both
`absorb_dynamic_lights`'s `entity_ids.contains(&id)` membership and `update`'s
`get_component` tombstone detection key on the full id, so a reused slot's
dead-generation id never false-matches a live one. `snapshots` (keyed by id) is
already removed on despawn, so it does not leak — only the index-parallel arrays do,
which is exactly what the free-list reclaims.
