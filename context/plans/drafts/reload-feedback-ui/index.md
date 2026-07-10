# Reload Feedback UI

## Goal

Make the UI ready to show a crosshair-anchored reload meter before any timed
reload gameplay exists. Ship the engine-owned `player.reloadProgress` slot
(0..1), a center-anchored reload-meter widget built from the existing `Bar`, and
a dev-only driver that fills the meter so the path is demonstrable end-to-end. A
later timed-reload spec (roadmap `per-shell reload`) points a real reload state
machine at the already-defined slot and drops the dev driver — no UI rework.

## Scope

### In scope

- A new engine-owned readonly `Number` slot `player.reloadProgress`, range
  `[0, 1]`, default `0.0` (rest = not reloading), added to the engine state
  catalog so it surfaces in `getGameState().player.reloadProgress` for both
  TypeScript and Luau. A sibling of the existing `player.weaponCooldownMs`
  weapon-timing slot.
- A dev-only reload-progress driver system that watches the already-bound
  `Action::Reload` edge and ramps `player.reloadProgress` `0 → 1` over a fixed
  dev duration, then resets to `0`. Mirrors `PlayerHudStatePublisher`'s shape.
  Not a reload state machine — no ammo, magazine, or gameplay effect; purely
  drives the slot so the meter visibly works.
- A reload-meter widget in the dev HUD: a separate always-on, center-anchored
  tree holding a `Bar` bound to `player.reloadProgress` (`max: 1.0`), offset
  beside the reticle. Reuses the existing `Bar`; no new UI primitive.
- Tests: catalog entry + SDK type generation for the new slot; slot range clamp
  to `[0, 1]`; the dev driver's ramp/reset behavior.

### Out of scope

- Timed / per-shell reload gameplay, a reload state machine, ammo, or a
  magazine — the `per-shell reload` roadmap bullet owns these and becomes the
  real producer of `player.reloadProgress`.
- Co-op replication of `player.reloadProgress`. It ships `ReplicationScope::None`
  now; the gameplay spec that adds the real producer also adds the per-owner
  projection (as `player.weaponCooldownMs` / `player.health` did in their own
  specs). No projection is built here.
- Whole-widget hide-when-idle. At rest the `Bar` renders empty (zero-width
  fill). Slot-driven `Display::None` visibility is a later refinement the
  gameplay spec owns once it knows reload start/end.
- The radial / ring primitive and the sustained-fire spread ring — a separate
  Epic 13 roadmap item and the Weapon Feel `spread / recoil` bullet. The linear
  reload meter needs neither.
- Any production (non-dev) reload driver. Production content stays free of a
  faked reload until real reload gameplay lands.

## Acceptance criteria

- [ ] `getGameState().player.reloadProgress` resolves in both TypeScript and
  Luau, typed as a readonly number ref; a HUD module can `bindState` it.
- [ ] The slot is engine-owned and readonly: a mod write warns and no-ops; the
  slot clamps to `[0, 1]` and rests at `0.0` when nothing drives it.
- [ ] A `Bar` bound to `player.reloadProgress` with `max: 1.0` fills left-to-right
  in proportion to the slot value (`0.0` → empty, `1.0` → full).
- [ ] The dev HUD shows a reload meter beside the crosshair in its own always-on
  center-anchored tree, distinct from `hud` and `hud.reticle`.
- [ ] With the dev driver active, pressing Reload fills the meter `0 → 100%` over
  the dev duration, then resets to empty; releasing/idle leaves it at `0`.
- [ ] With no driver (production build), the slot stays at `0.0`, the meter
  renders empty, and no per-frame warning is emitted.

## Tasks

### Task 1: Engine slot `player.reloadProgress`

Add a `player.reloadProgress` entry to `BUILTIN_ENGINE_STATE` in
`crates/entities/src/engine_state_catalog.rs`, mirroring the
`player.weaponCooldownMs` entry: `wire_name: "player.reloadProgress"`,
`sdk_path: &["player", "reloadProgress"]`, `EngineStateValueType::Number`,
default `0.0`, range `[0.0, 1.0]`, `persist: false`,
`EngineStateCapability::Readonly`, `network: ReplicationScope::None`. Ensure the
generated SDK state surface (the `getGameState()` reference tree and the
TypeScript/Luau typedefs) picks up the new leaf. Extend whatever catalog test
enumerates the built-in slots (the file's existing coverage that asserts
`player.health` / `player.maxHealth` presence, ownership, and range) to assert
the new slot's presence, readonly ownership, and `[0, 1]` range.

### Task 2: Dev reload-progress driver

Add a dev-only system that drives `player.reloadProgress`. Model it on
`crates/postretro/src/scripting/systems/ui_proxy.rs`
(`PlayerHudStatePublisher`): a small struct holding a `ScriptCtx` clone, with a
`tick` that reads the frame's `Action::Reload` state and writes the slot via
`write_store_slot(&ctx, "player.reloadProgress", SlotValue::Number(..))`. On a
reload press it starts a ramp; each tick advances progress by `dt / duration`,
clamps to `1.0`, then resets to `0.0` once full (or on the next press). Gate it
behind `dev-tools` (or an equivalent dev-only path) so no production build fakes
a reload. Wire its `tick` into the frame loop next to the existing
`Action::Reload` read in `crates/postretro/src/main.rs` (`let reload =
snapshot.button(Action::Reload)` feeding `SimCommand.reload`) — pass the reload
button state and `dt` in; the slot write must land before the UI read-snapshot
is built, same ordering as `PlayerHudStatePublisher`. Unit-test the ramp: a
press starts progress climbing, it reaches and clamps at `1.0`, and resets to
`0.0`.

### Task 3: Dev HUD reload-meter widget

Add a reload-meter tree to `content/dev/scripts/hud.ts`. Define a new
`defineUiTree({ name: "hud.reloadMeter", alwaysOn: true, tree: Tree({ anchor:
"center", offset: [...] }, Bar({ bind: bindState(player.reloadProgress, ...),
max: 1.0, fill, background })) })`, exported alongside `hud` and `reticle`. It is
its own tree because one anchored tree has one viewport anchor (the status HUD is
`bottomLeft`, the reticle is `center`). Offset the meter a small fixed distance
from screen center so it sits beside the crosshair without overlapping the `+`.
Style via the existing `hudTheme` tokens. A short tween on the bind is optional
and presentational.

## Sequencing

**Phase 1 (sequential):** Task 1 — the slot and its generated SDK surface block
both the dev driver's write target and the HUD bind.

**Phase 2 (concurrent):** Task 2, Task 3 — independent (Rust driver vs. TS HUD
content, different files). Both consume the Task 1 slot. Task 3 renders the
meter; Task 2 makes it visibly move — neither blocks the other.

## Rough sketch

The whole feature reuses shipped seams:

- **Slot** — `EngineStateCatalogEntry` in `engine_state_catalog.rs`. The catalog
  auto-generates the `getGameState()` tree and both runtimes' typedefs from
  `sdk_path`, so no hand-authored SDK edit is needed. `ReplicationScope::None`
  keeps it out of the net fingerprint and the `StateSlotId` space.
- **Driver** — a new `scripting/systems/` module beside `ui_proxy.rs`. It writes
  through `write_store_slot`; the readonly engine slot accepts engine-side writes
  (writability, not ownership, gates the slot). The slot-staleness contract means
  a skipped frame just holds the last value.
- **Widget** — the `Bar` collector (`crates/ui/src/tree/ui_tree_collect.rs`)
  already normalizes `value / max` to a `[0, 1]` fill fraction and draws a
  horizontal fill quad; binding `player.reloadProgress` with `max: 1.0` yields a
  0..1 meter directly.

**File-size flag (soft):** `crates/postretro/src/main.rs` and
`crates/postretro/src/startup/lifecycle.rs` are both well past ~800 lines. Task 2
only adds a one-line driver-tick call site in the `main.rs` frame loop next to
the existing reload read — a call site, not an extension — so no split is
warranted here. The driver logic itself lives in a new module.

## Boundary inventory

| Name | Rust (`wire_name`) | serde / wire | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| reload progress slot | `"player.reloadProgress"` | `"player.reloadProgress"` | `getGameState().player.reloadProgress` | `getGameState().player.reloadProgress` | n/a |

camelCase leaf (`reloadProgress`) matches the wire casing of the other
`player.*` slots; the catalog derives every runtime name from the single entry.

## Script syntax examples

HUD author consuming the new slot (dev HUD, `content/dev/scripts/hud.ts`):

```ts
const { player } = getGameState();

const reloadMeter = Bar({
  bind: bindState(player.reloadProgress, {
    tween: { durationMs: 90.0, easing: "easeOut" },
  }),
  max: 1.0,
  fill: color.ok,
  background: color.hud.health.background,
});

export const reloadMeterTree = defineUiTree({
  name: "hud.reloadMeter",
  alwaysOn: true,
  tree: Tree(
    { anchor: "center", offset: [36.0, 0.0] },
    reloadMeter,
  ),
});
```

## Open questions

- Meter placement and size beside the reticle (offset, width) are dev-content
  tuning, resolved during implementation — not a blocking decision.
