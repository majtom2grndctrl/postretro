# E18-B — Trigger Occupancy Exposure

## Goal

Expose engine-tracked trigger-volume occupancy to scripts as live readonly state, so co-op puzzles — two players each on a plate, or N players each on a plate in a separate room — are authorable entirely in script with no new brush KVP. Grows the shipped trigger substrate (E18 trigger-event-fanout) by surfacing *effective-occupant* counts as replicated Number slots the shipped state-crossing watcher already fires on. Realizes the all-script activation direction of `scripting.md` §12: the prior KVP-driven activation policy is dropped, not ported.

## Scope

### In scope

- **Effective-occupant predicate.** A player occupies a trigger iff spatial overlap ∧ alive. Alive = no health component, or `HealthComponent.current > 0.0`. A corpse on a plate does not count.
- **Per-tag reserved occupancy slots**, engine-owned, readonly-to-scripts, Number, written every host tick from the effective-occupant set: `occupiedCount` (count of matched volumes carrying ≥1 effective occupant) and `occupants` (total effective occupants across matched volumes). Declared automatically at level install for every tag present on a `trigger_volume`.
- **Replication.** The reserved occupancy slots replicate host→client (`SharedGlobal`) so a client's local crossing fires from its own converged value. No new wire section — reuses the shipped shared-global slot replication.
- **SDK surface.** Two pure ref builders from `"postretro"`: `occupiedCount(tag)` and `occupants(tag)`, each returning `ReadonlyStateRef<number>` whose `slot` is the reserved per-tag slot. Both runtimes; typedef template + `gen-script-types` regen + drift check.
- **Authoring pattern + fixtures.** The two-button door and the cross-volume simultaneity puzzle, both authored as `onStateCrossing(occupiedCount(tag), { above: N-1 }, [solve])` on the shipped watcher — an integer count crossing N-1→N. Documented as a reusable co-op pattern.
- **Tests.** Headless AND (solve fires only when the full set is held; releasing one before the crossing suppresses it) and two-endpoint net-QA (occupancy converges on the client; the client's crossing fires once; the host-fired solve mover reaches the client via replicated phase).
- **Dev-tools Triggers tab.** Effective-occupancy column alongside the existing raw occupancy.
- **Split-before-extend.** Relocate occupant tracking + the occupancy accessor out of `trigger_system.rs` (1695 lines) before extending it.

### Out of scope

- **`ContactScope.activators` / "damage whoever pressed"** — reading the firing player inside a reaction needs the §12 dispatch-param mechanism; owned by the **IR-valued reaction args** spec.
- **IR-predicate crossings and `CrossingScope.direction`** — the observer generalization is pure IR work; owned by the IR-valued reaction args spec. E18-B's puzzles need only the shipped `above`/`below` watcher on an integer count.
- **`includeDead`** (counting corpses as occupants) — v1 is alive-only. Deferred; would double the reserved-slot set.
- **`activation_policy` / `host_only` / `count` / `all` brush KVPs, the PRL v3 section, and the `TriggerVolumeComponent` policy fields** — removed entirely (all-script per §12). A map still authoring the old KVP loads with it ignored and warned. `host_only`/`count`/`all` become script predicates over the exposed refs (+ activators, once that spec lands).
- **`incrementState` / `decrementState`** — superseded by the IR-valued reaction args spec (`setState(slot, read(slot) ± 1)`).
- Any new brush KVP, FGD entry, or binary/PRL section.

## Acceptance criteria

- [ ] `occupiedCount(tag)` and `occupants(tag)` return readonly number refs; reading a puzzle's tag reflects the current effective-occupant aggregate for that tag.
- [ ] A dead pawn (HP at 0) standing inside a trigger does not count toward either aggregate; a pawn with no health component does count.
- [ ] `occupiedCount(tag)` = number of distinct matched volumes with ≥1 effective occupant. Two pawns on one plate give `occupiedCount == 1`, `occupants == 2`.
- [ ] Two-button door fixture: the door opens only when both plates hold an effective occupant simultaneously; releasing one before the count crosses suppresses the solve. Verified in a headless `simulate_tick` test with no app loop.
- [ ] Cross-volume fixture: N separated single-occupant plates sharing a tag fire the solve only when all N are held at the same instant (the AND). Verified headless.
- [ ] Two-endpoint test: a connected client's occupancy slots converge to the host's values; the client's crossing watcher fires exactly once when the set completes; the host-fired solve mover reaches the client via replicated mover phase. Late-join: a client joining mid-hold observes the correct count from its baseline.
- [ ] The reserved occupancy slots are readonly to scripts: a `setState`/`updateState` targeting one warns and leaves it unchanged.
- [ ] Determinism: two identical headless runs with scripted inputs produce identical occupancy-slot timelines.
- [ ] No `activation_policy` / `activation_count` / `occupancy_includes_dead` surface exists in the FGD, the PRL trigger section, or `TriggerVolumeComponent` (static-absence guard: the all-script rescope adds no policy field).
- [ ] Typedef drift check passes with `occupiedCount`/`occupants` in `postretro.d.ts` and `.d.luau`; a TS fixture and a Luau fixture author the same puzzle identically.
- [ ] With `--features dev-tools` the Triggers tab shows per-trigger effective occupancy; without the feature no occupancy-exposure UI code compiles.
- [ ] Occupant tracking is relocated out of `trigger_system.rs` with behavior unchanged (existing trigger tests still pass).

## Tasks

### Task 1: Split occupant tracking out of `trigger_system.rs`

Behavior-preserving relocation before the extend. Move the occupant-tracking state and its accessor — the `occupants: BTreeMap<EntityId, BTreeSet<PlayerId>>` field and `occupancy(&self, trigger) -> usize`, plus `paired_enters` if it travels cleanly with them — out of the 1695-line `crates/postretro/src/trigger_system.rs` into a new sibling submodule (e.g. `trigger_occupancy.rs`) owning an `Occupancy` struct that `TriggerSystem` holds. The enter/exit edge mutations call into that struct. No behavior change; the relocated accessor and all existing trigger tests still pass (relocate or re-path the tests per the seam). This isolates the surface Task 2 extends and keeps the feature diff out of the monolith.

### Task 2: Effective occupancy → reserved replicated slots

The engine core. Each host tick, after the occupant map is updated (Task 1's struct) and before state-crossing dispatch, compute per-tag effective occupancy and write it to reserved slots. **Effective filter:** build a set of alive players once per tick — for each occupant `PlayerId`, resolve its pawn `EntityId` (Local wraps the pawn; Remote resolves via the `players: Vec<AuthoritativePlayer>` list already built at the sim tick), read `HealthComponent` via `registry.get_component`, and count the player iff the component is absent or `current > 0.0 && current.is_finite()` (the death predicate from `crates/postretro/src/scripting/systems/health.rs`). **Aggregation:** for every tag carried by any `trigger_volume` in the level, `occupiedCount` = number of matched volumes with ≥1 effective occupant, `occupants` = sum of effective occupants over matched volumes. **Slots:** declare two engine-owned, readonly, `SharedGlobal`-replicated Number slots per tag under a reserved `trigger.` namespace (exact dotted format pinned in the sketch); auto-declare at level install by enumerating tags from the trigger bridge; write through the validated engine-capability path (`write_store_slot` / `apply_store_slot_batch`, which bypasses readonly with validation). Zero when a tag has no effective occupants. The write must land before `dispatch_state_crossings_with_sequences` runs so a completed set crosses in the same frame. Tests cover the alive filter (corpse excluded, health-less pawn included), both aggregates, replication convergence, readonly rejection, and determinism.

### Task 3: SDK occupancy ref builders + typedefs

Author-facing surface. Add `occupiedCount(tag: string): ReadonlyStateRef<number>` and `occupants(tag: string): ReadonlyStateRef<number>` to the SDK library (core module, not UI — sibling to `world`), each pure and returning a frozen `{ slot }` ref whose slot string is the exact reserved format Task 2 pins (mirroring how `defineStore` builds state refs at `sdk/lib/data_script.ts:280`). No FFI. Provide both the TS (`sdk/lib/`) and Luau implementations with runtime parity. Declare the two functions in the SDK typedef template under `crates/scripting-core/src/typedef/templates/` (core template, beside the other `world`/state helpers), regenerate `sdk/types/postretro.d.ts` / `.d.luau` via `gen-script-types`, and update the committed snapshot + drift test. Validate the `tag` argument as tags are validated elsewhere; document that the ref points at engine-owned readonly state.

### Task 4: Puzzle fixtures + tests (headless AND + net-QA)

Prove the pattern end to end. Author two dev fixtures (a level + `setupLevel` script each): (a) a two-button door — two co-located plates sharing a tag, a `kinematic_mover` vault door, and `onStateCrossing(occupiedCount(tag), { above: 1 }, [solve])` firing a solve reaction that starts the door; (b) cross-volume simultaneity — N separated single-occupant plates sharing a tag, `above: N-1`, same solve shape. A headless test drives scripted pawn enters/leaves and asserts the solve fires only on the full simultaneous set and not on a partial-then-release. Two-endpoint net-QA (loopback harness, E18 net-QA precedent): host pawns complete the set; assert the client's occupancy slots converge, the client crossing fires exactly once, and the solve's mover reaches the client via replicated phase; include the late-join case. Extend the determinism harness with an occupancy-firing tick sequence.

### Task 5: Dev-tools Triggers tab — effective occupancy column

Instrumentation. Extend `collect_trigger_diagnostics_rows` (`crates/postretro/src/trigger_diagnostics.rs:21`) and `TriggerDiagnosticsRow` (`crates/renderer/src/render/debug_ui/mod.rs:97`) with an effective-occupancy value beside the existing raw `occupancy`, and widen `draw_triggers_tab`'s column count (`:709`) accordingly. The renderer cannot see trigger types — `postretro` builds the row value from Task 1's occupancy struct plus the alive filter and passes it in, as the existing rows do. Everything compiles out without `--features dev-tools`.

## Sequencing

**Phase 1 (sequential):** Task 1 — behavior-preserving split; blocks Task 2's extend.
**Phase 2 (sequential):** Task 2 — engine occupancy slots; Tasks 3/4/5 all consume them.
**Phase 3 (concurrent):** Task 3 (SDK refs) and Task 5 (dev-tools) — disjoint files, both read Task 2 output.
**Phase 4 (sequential):** Task 4 — fixtures/tests; consume Task 3's ref builders and Task 2's slots.

## Rough sketch

- **Reserved slot format:** `trigger.<tag>.occupiedCount` and `trigger.<tag>.occupants`; engine namespace `trigger.`, one dotted segment for the tag. Same names on the SDK ref builder and the reserved declaration — one source of truth. Confirm the tag charset excludes `.` (or sanitize) so the dotted slot parses unambiguously; flag in Task 2 if tags may contain dots.
- **Write site:** the effective-occupancy aggregation runs in the sim/host tick, after the trigger occupant map settles and before `dispatch_state_crossings_with_sequences` (`crates/postretro/src/scripting/reactions/mod.rs:32`). The crossing reads the authoritative `SlotTable` each frame (`state_crossings.rs:138`), so a completed set crosses the same frame.
- **Alive filter:** dead ⇔ `current <= 0.0 || !current.is_finite()`; absent `HealthComponent` ⇒ alive (`research.md`). Resolve Remote occupants to pawns via `AuthoritativePlayer.pawn` (`sim/mod.rs:156`).
- **Effective vs raw:** `occupancy()` (`trigger_system.rs:108`) is raw overlap. Effective occupancy is a new computation layered over the occupant set, not that accessor; dev-tools shows both.
- **Why no IR:** an integer `occupiedCount` crossing N-1→N fires the shipped `above` watcher (`Watcher::crosses`, `state_crossings.rs:119`). Multi-slot ANDs and direction-sensitivity wait for the IR-valued args spec.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| occupied-count slot | reserved Number slot: engine-written, readonly, `SharedGlobal` | `"trigger.<tag>.occupiedCount"` (shared-global slot replication; no new section) | `occupiedCount(tag) → ReadonlyStateRef<number>` (`{ slot }`) | same | n/a |
| occupants slot | reserved Number slot: engine-written, readonly, `SharedGlobal` | `"trigger.<tag>.occupants"` | `occupants(tag) → ReadonlyStateRef<number>` | same | n/a |

No FGD KVP, no new PRL/binary section — the all-script rescope removes both. Occupancy rides the shipped shared-global slot replication path.

## Script syntax examples

```ts
// Two-button door: two plates tagged "vault-plates", a mover tagged "vault-door".
import { world, defineReaction, onStateCrossing, occupiedCount } from "postretro";

export function setupLevel() {
  const door = world.query({ component: "kinematic_mover", tag: "vault-door" });

  // Sourceless payoff, referenced only in-script — no name; the const is the identity.
  const solveVault = defineReaction({ sequence: door.flatMap((d) => d.start()) });

  // occupiedCount rises 0→1→2 as plates fill; `above: 1` fires when the 2nd lands.
  const crossings = [onStateCrossing(occupiedCount("vault-plates"), { above: 1 }, [solveVault])];

  return { reactions: [solveVault], crossings };
}
```

```ts
// Cross-volume simultaneity: N single-occupant plates in separate rooms, same tag.
// Identical shape — only the threshold changes. occupiedCount counts covered plates,
// not bodies, so one player cannot satisfy it by stepping across two plates in turn.
const N = 3;
const crossings = [onStateCrossing(occupiedCount("ritual-plates"), { above: N - 1 }, [solveVault])];
```

```luau
-- Luau parity.
local Postretro = require("postretro")
local plates = Postretro.occupiedCount("vault-plates")
-- onStateCrossing(plates, { above = 1 }, { solveVault })
```

## Open questions

- **Tag charset in slot names.** Reserved slots embed the tag as a dotted segment. If trigger tags may contain `.`, the format needs sanitization or a length-prefixed encoding — confirm the tag charset during Task 2 and pin the resolution there.
- **Unknown-tag reads.** Reserved slots are declared only for tags present at level install. A script calling `occupiedCount` on a tag with zero triggers reads an undeclared slot; the shipped watcher warns+skips a non-registered slot (`state_crossings.rs:54`), so the puzzle silently never fires. Decide whether that inert-safe degradation is enough or whether unknown tags should warn at author time.
