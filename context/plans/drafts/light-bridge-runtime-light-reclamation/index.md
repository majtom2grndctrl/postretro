# Light-Bridge Runtime-Light Slot Reclamation

## Goal

Make the light bridge reclaim a despawned runtime (script/gameplay-spawned)
dynamic light's reserve slot, so **high-churn** runtime lights — projectile travel
and impact lights, future muzzle flashes and tracers — are bounded by the
**peak-concurrent** count, not the cumulative count spawned this level. Today a
despawned runtime light's `entity_ids` entry is never removed, so cumulative churn
permanently exhausts `RUNTIME_DYNAMIC_LIGHT_RESERVE` and further runtime lights
silently stop rendering. This is the load-bearing prerequisite for
`projectile-weapon-enhancements` (whose per-shot travel + impact lights are the
first high-churn runtime lights) and any later moving/transient gameplay light.

## Scope

### In scope

- Reclamation of the reserve slot held by a despawned runtime light in
  `LightBridge` (`crates/postretro/src/scripting/systems/light_bridge.rs`): a
  despawned runtime slot index becomes reusable, so a later runtime light reuses
  it in place rather than appending a new slot.
- The reserve-exhaustion check (`absorb_dynamic_lights`) counts **live** runtime
  lights (occupied, non-reclaimed, `≥ authored_light_count`), not
  `entity_ids.len().saturating_sub(authored_light_count)`.
- The tombstone GPU zeroing behavior is preserved (a vanished light still zeroes
  its forward/compose slot the frame it disappears, and every dirty frame after,
  until the slot is reused); reclamation is additive to it.
- A deterministic test suite (see Task 1).

### Out of scope (non-goals)

- The **authored map-light prefix** (`[0, authored_light_count)`), which is fixed
  for the level and never reclaimed.
- Any change to how lights are packed, animated, or shadowed beyond slot accounting.
- The follow-Transform contract, radius-animation channel, or any projectile
  feature — those live in `projectile-weapon-enhancements`; this spec only fixes
  slot accounting so those can rely on it.
- Raising `RUNTIME_DYNAMIC_LIGHT_RESERVE` (`renderer_types.rs`) — reclamation, not
  a bigger cap, is the fix.
- **Compaction** (shrinking `entity_ids` below the peak). The forward buffer,
  `effective_brightness`, and the `collect_all_as_map_lights` scan stay bounded by
  the peak-concurrent high-water mark for the level, not the instantaneous live
  count; the high-water resets only on `clear` / `populate_*`. Bounding to the
  live count would require compaction, which reorders the packed GPU list (see
  Alternatives).

## Direction

**Problem.** `LightBridge.entity_ids` only ever grows or is cleared wholesale (the
only mutators are `clear`, `populate_from_level*`, and the append in
`absorb_dynamic_lights`). `update`'s pass 1 (the unconditional diff loop) detects a
vanished light via a failed `get_component::<LightComponent>` and removes its
`snapshots` entry, but leaves its dead-generation `EntityId` in `entity_ids`
forever; `update`'s pass 2 (the dirty-gated pack loop) zeroes that slot's GPU
forward/compose record each dirty frame. `absorb_dynamic_lights` bounds new runtime
lights on `runtime_count = entity_ids.len().saturating_sub(authored_light_count)`,
so every runtime light ever spawned counts against `RUNTIME_DYNAMIC_LIGHT_RESERVE`
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
reused slot receives new data in place and the authored prefix is untouched.

**Chosen mechanism.** Add a `free_slots` list of reclaimable runtime slot indices
and a per-slot **reclaimed** marker — a `reclaimed: bool` on `MapLightShape`. The
marker is the once-only reclaim guard and the "this slot is a tombstone, not a live
light" signal. Keep it on `MapLightShape` (not a separate `HashSet<usize>`): the
index already gives O(1) access, overwriting `shape[i]` on reuse clears the marker
for free, and `clear` / `populate_from_level_with_influences` rebuild `shape`
wholesale so the marker resets across levels with no extra step. A separate set
would be a second source of truth that must be kept in lock-step with `free_slots`
at every mutation site — a desync hazard for no gain.

- **Reclaim (in `update` pass 1, the diff loop):** when a tracked slot at index
  `i ≥ authored_light_count` has a failed `get_component` **and is not already
  reclaimed**, mark it reclaimed, push `i` to `free_slots`, and set `dirty`. Do
  **not** touch `shape[i].is_dynamic`, `shape[i].animated_slot`, or the
  `cached_*` arrays — pass 2 reads `is_dynamic`/`animated_slot` to emit the
  disappear-frame tombstone zero, and the residue those arrays hold is harmless
  because reuse overwrites them (below). The dead-generation `EntityId` stays in
  `entity_ids[i]` so pass 2's `get_component` keeps failing and keeps zeroing the
  slot until it is reused. Marking is snapshot-independent (a light that spawned
  and despawned before any `update` never got a snapshot; its slot must still be
  reclaimed) and fires **exactly once** per despawn (the reclaimed guard stops
  pass 1 re-pushing the same index on later frames).
- **Reuse (in `absorb_dynamic_lights`):** before appending, pop an index `i` from
  `free_slots`; **overwrite** `entity_ids[i]`, `shape[i]`, `cached_origins_f64[i]`,
  and `cached_influences[i]` at that index (clear the reclaimed marker), set
  `absorbed_any = true`. The `scripted_sample_buf` region for `i` needs no explicit
  clear: `update` pass 2 calls `scripted_sample_buf.fill(0.0)` before repacking
  every dirty frame, so a reused slot starts from a zeroed sample region. `snapshots`
  needs no clear either: it is keyed by `EntityId`, the prior occupant's entry was
  removed on despawn, and the new occupant carries a new id. Only when `free_slots`
  is empty does absorb `push` a new slot (growing the peak).
- **Live count / reserve check:** derive it —
  `entity_ids.len() - authored_light_count - free_slots.len()` — rather than
  maintaining a counter, so there is nothing to keep in sync across despawn/reuse.
  Reject a new absorb when this count `≥ RUNTIME_DYNAMIC_LIGHT_RESERVE`. Reuse never
  trips the bound (a free slot exists only when the live count is below the peak,
  hence below the reserve), so a single check covers both paths.

**Frame-ordering consequence (load-bearing).** `absorb_dynamic_lights` runs in the
fixed-tick loop (Game logic); `update` runs once per frame in the Render stage,
after all ticks. So reclaim always **lags** absorption within a frame: a slot freed
by this frame's `update` is first reusable by **next** frame's absorb. Same-frame
despawn-then-respawn does not reuse the slot that frame — it appends (or, at exactly
reserve-full, is rejected until the next frame). This is why the reserve headroom in
AC 1 (`K < reserve`) is deliberate, not incidental.

**Foreclosures.** None material. Reclamation reverts cleanly; it adds accounting,
removes no capability.

## Acceptance criteria

- [ ] After N sequential spawn→despawn cycles of runtime dynamic lights (N far
  exceeding `RUNTIME_DYNAMIC_LIGHT_RESERVE`) with at most K < reserve alive at
  once, and at least one frame between a despawn and the next spawn that reuses its
  slot, every cycle's light renders — the reserve bound tracks live, not cumulative,
  spawns.
- [ ] A despawned runtime light still zeroes its GPU forward/compose slot on the
  frame it disappears (tombstone zeroing preserved), and re-zeroes it every dirty
  frame until the slot is reused; no stale light lingers.
- [ ] The authored map-light prefix is unaffected: authored indices, their
  snapshots, and their packed order are identical before and after the change.
- [ ] Reclaiming and reusing a slot does not mis-attribute state: a new light in a
  reused slot renders from its own component (origin, influence, animation samples)
  with no contribution from the prior occupant — guaranteed by overwrite-on-reuse of
  the index-parallel arrays plus the existing per-dirty-frame
  `scripted_sample_buf.fill(0.0)`.
- [ ] After the churn cycles, the per-frame emitted light buffer and
  effective-brightness lengths — the forward pass's per-fragment `full.light_count`,
  and the byte count uploaded each dirty frame — track **peak-concurrent** runtime
  lights (the level's high-water mark), not cumulative spawns; and the
  `collect_all_as_map_lights` scan length is likewise bounded by the peak. (Reused
  slots hold the peak steady; they do not shrink it — see Scope non-goals.)
- [ ] A level reload with a different `authored_light_count` never hands a runtime
  light an authored-prefix slot: `free_slots` is reset on `clear` /
  `populate_from_level_with_influences`, so no stale index survives teardown; the
  authored map lights render unchanged after reload.
- [ ] No new `unsafe` (standing CI grep gate; no task step). No wire-format or
  GPU-layout change (CPU accounting only).

## Tasks

### Task 1: Reclaim runtime-light slots

Implement the **Chosen mechanism** above in
`crates/postretro/src/scripting/systems/light_bridge.rs`:

- Add `free_slots` and the per-slot reclaimed marker to `LightBridge` /
  `MapLightShape`.
- In `update`'s **pass 1** (the `entity_ids.iter().enumerate()` diff loop),
  extend the existing failed-`get_component` branch: for a slot
  `i ≥ authored_light_count` not already reclaimed, mark it reclaimed, push `i` to
  `free_slots`, set `dirty`. The reclaim push is a **sibling** of the existing
  `snapshots.remove(&id).is_some()` dirty check — unconditional on its result, not
  nested inside its `is_some()` arm — so a zero-duration light that never got a
  snapshot is still reclaimed (P4). Leave `shape[i].is_dynamic` / `animated_slot`
  intact.
  (No borrow-split is needed: `free_slots` and `shape` are fields disjoint from
  `entity_ids`, so mutating them inside `entity_ids.iter()` compiles — the loop
  already mutates disjoint fields inline, e.g. `snapshots.remove`, `dirty`. The
  `settled` write-back exists to defer *registry* mutation, not `self`-field
  mutation, and does not apply here.)
- Leave `update`'s **pass 2** (the dirty-gated pack loop) unchanged: it already
  emits the forward/compose tombstone zero for any is-dynamic slot whose
  `get_component` fails, which now includes reclaimed-but-not-yet-reused slots.
- In `absorb_dynamic_lights`, before the `push` path, pop a `free_slots` index and
  overwrite all index-parallel arrays at that index; change the reserve-exhaustion
  check to the derived live-count formula.
- **Cross-level reset:** clear `free_slots` in `clear()` and
  `populate_from_level_with_influences` alongside the arrays they already reset — a
  free index carried into a level with a different `authored_light_count` would hand
  a new runtime light an **authored-prefix** slot and silently corrupt a map light.
  The `reclaimed` markers need no separate reset: they ride on `MapLightShape`,
  which both seams rebuild wholesale.
- **`collect_all_as_map_lights`:** confirm (no change expected) that its existing
  `get_component(id).ok()?` filter already skips reclaimed slots (their dead id
  fails the lookup), so a tombstone never surfaces as a fog map-light.

Add the deterministic tests, one per row of the **Pin table** below. At minimum:
the churn test (AC 1), a **disappear-frame** tombstone assertion (AC 2 — a
despawned runtime light zeroes its forward/compose slot the frame it vanishes),
an authored-prefix-unchanged regression (AC 3), a reused-slot-no-residue assertion
(AC 4), a **buffer-length** assertion that after churn `update`'s emitted
`lights_bytes.len()` / `effective_brightness.len()` (hence the forward pass's
per-fragment `full.light_count`) track peak-concurrent, not cumulative (AC 5), and
a **cross-level-reset** assertion that a reload with a different authored count
never reuses a stale free index into the authored prefix (AC 6).

## Pin table

The mechanics each row pins are the defect class this spec exists to close
("invariant stated, mechanics unpinned"). Task 1's tests assert these rows; do not
restate them in prose.

| # | Scenario | Ordering (concrete event sequence) | Expected outcome |
|---|----------|-----------------------------------|------------------|
| P1 | Tombstone not reused for several frames | F: despawn L (reclaim slot `i`). F+1, F+2, F+3: no absorb reuses `i`. | `i` appears in `free_slots` **exactly once**; pass 1 skips the already-reclaimed slot on F+1..F+3. `free_slots` never becomes `[i, i, …]`. |
| P2 | Reclaim once, then reuse once | F: despawn L (reclaim `i`). F+1: one spawn. | Exactly one absorb pops `i`; `entity_ids[i]` = new id; no second consumer can pop `i`. |
| P3 | Disappear-frame zero survives reclaim | F: a dynamic runtime light despawns; `update` runs pass 1 (reclaim) then pass 2. | Pass 2 still emits the forward `GpuLight` zero (and compose zero if slot-bearing) for slot `i` **this frame**; `is_dynamic`/`animated_slot` are intact when pass 2 reads them. |
| P4 | Zero-duration light (never snapshotted) | Spawn tick 3, despawn tick 4 of the same frame; single `update` at frame end. | Slot is reclaimed despite `snapshots.remove` returning `None`; `free_slots` gains `i` once; no leak. (The light itself is never rendered.) |
| P5 | Reclaim→reuse frame boundary | F tick 1: despawn A. F tick 2: spawn B. | B does **not** reuse A's slot in frame F (A is reclaimed only at F's `update`); A's slot is reusable from F+1. |
| P6 | Reserve-full same-frame churn | Reserve runtime lights live; F tick 1 despawn A; F tick 2 spawn B. | B is rejected in frame F (live count still includes not-yet-reclaimed A); A's slot is reusable F+1. Test asserts next-frame reuse; AC 1 keeps `K < reserve`. |
| P7 | Live-count formula, N despawn / M absorb | F: N runtime despawn (reclaimed at F's `update`). F+1: M absorb. | Live count = `entity_ids.len() - authored_light_count - free_slots.len()`; correct for every (N, M) including (0, 0), M ≤ N, M > N. Reserve check fires only when live count ≥ reserve. |
| P8 | Parallel-array alignment on reuse | Reuse slot `i` for new light B; then `collect_all_as_map_lights` reads it. | `entity_ids[i]`, `shape[i]`, `cached_origins_f64[i]`, `cached_influences[i]` all reflect **B**; fog reads B's origin/influence, not the prior occupant's. |
| P9 | Batched reuse ordering | F: despawn 3 (`free_slots = [a, b, c]`). F+1: absorb 5. | 3 reuse a/b/c + 2 append; buffer emits 5 live entries; new peak = prior peak + 2 appended; no index reused twice; `absorbed_any = true`. |
| P10 | Cross-level reset | Level A leaves `free_slots` populated; reload to level B with smaller `authored_light_count`. | `clear()` and `populate_from_level_with_influences` both empty `free_slots`; no stale index lands a level-B runtime light in level B's authored prefix. |
| P11 | Buffer length after peak-then-drop | Spawn 10 concurrent, despawn 5, churn. | Emitted forward buffer / `full.light_count` **and** the `collect_all_as_map_lights` scan length = **peak (10)**, held by retained tombstone zeros, not the current live count (5) and not cumulative — pin peak semantics and assert it. |
| P12 | Reuse then re-despawn (marker reset) | F: despawn L (reclaim `i`). F+1: reuse `i` for B. F+1 later tick: despawn B. F+1 `update`: pass 1. | Slot `i` is **re-reclaimed** and re-appears in `free_slots` exactly once (the reclaimed marker was reset when `shape[i]` was overwritten on reuse); the derived live count does not inflate. |

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| **Reserve bound tracks live, not cumulative** runtime lights | Task 1 (free-list + derived live-count check) | counting `entity_ids.len()`; a stale free index; a maintained counter drifting on despawn/reuse | AC 1; P1, P2, P7 |
| **Tombstone zeroing preserved** — a vanished light zeroes its slot the frame it disappears, and every dirty frame until reused | Task 1 (reclaim leaves `is_dynamic`/`animated_slot` intact; pass 2 unchanged) | reclaim wiping `is_dynamic` before pass 2, leaving a stale GPU slot | AC 2; P3 |
| **Authored prefix untouched** — authored indices, snapshots, packed order stable | Task 1 (free-list confined to `≥ authored_light_count`; reset across levels) | reclaiming or reusing an index `< authored_light_count`; a stale free index after reload | AC 3, AC 6; P10 |
| **Reused slot carries no residue** — a new occupant renders only its own state | Task 1 (overwrite-on-reuse of index-parallel arrays + existing `scripted_sample_buf.fill(0.0)`; `snapshots` keyed by id) | mixing `push` with in-place write, desyncing the parallel arrays | AC 4; P8 |
| **Reclaim fires exactly once per despawn** — snapshot-independent; marker resets on reuse | Task 1 (reclaimed marker on `shape`; not gated on `snapshots.remove`; cleared by overwrite-on-reuse) | gating reclaim on snapshot presence (misses zero-duration lights); re-pushing an index each frame; a marker not reset on reuse (never re-reclaimed → live-count inflation) | AC 1; P1, P4, P12 |
| **Bounded frame cost** — emitted buffer + per-fragment `light_count` + per-frame scans track **peak-concurrent**, not cumulative | Task 1 (reused slots do not grow `entity_ids`) | a compaction/refactor silently regrowing the emitted count; the peak asserted only implicitly | AC 5; P11 |
| **Free-list reset across levels** — no stale index into a new level's authored prefix | Task 1 (reset in `clear`/`populate_from_level_with_influences`) | a free index surviving teardown → a runtime light overwriting an authored map light next level | AC 6; P10 |

## Rough sketch

Entry points: `LightBridge` (`entity_ids`, `authored_light_count`, `shape`,
`cached_origins_f64`, `cached_influences`, `snapshots`, `scripted_sample_buf`, new
`free_slots`), `absorb_dynamic_lights` (the append/reuse + reserve bound), `update`
(pass 1 diff-loop tombstone branch on a failed `get_component`; pass 2 dirty-gated
pack loop with the existing GPU zeroing), `clear` / `populate_from_level_with_influences`
(reset seams), all in `crates/postretro/src/scripting/systems/light_bridge.rs`.
`RUNTIME_DYNAMIC_LIGHT_RESERVE` in `postretro_renderer` (`renderer_types.rs`).
`collect_all_as_map_lights` (run each frame for fog) scans `entity_ids` and already
skips tombstones via `get_component(...).ok()?`; the free-list keeps its scan length
bounded by the peak.

No generational-id hazard to add: `EntityId` carries a generation, and both
`absorb_dynamic_lights`'s `entity_ids.contains(&id)` membership and `update`'s
`get_component` tombstone detection key on the full id, so a reused slot's
dead-generation id (retained in `entity_ids` until overwrite) never false-matches a
live one. `snapshots` (keyed by id) is already removed on despawn, so it does not
leak; and `scripted_sample_buf` is zeroed by the existing per-dirty-frame
`fill(0.0)` before repack — so the only state the free-list must actively manage is
the reclaimed marker and the overwrite-on-reuse of the index-parallel arrays.
