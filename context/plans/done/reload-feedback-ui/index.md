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
  TypeScript and Luau. Structurally a sibling of the other `player.*`
  weapon-timing slots (`player.*` `sdk_path`, `Number`, `Readonly`,
  `persist: false`) — its default, range, and network scope are its own, not
  copied from any existing slot.
- A dev-only reload-progress driver system that edge-detects
  `Action::Reload`'s `ButtonState::Pressed` transition — not the held
  `.is_active()` level bit `SimCommand.reload` uses — and ramps
  `player.reloadProgress` `0 → 1` over a fixed dev duration, then resets to
  `0`. Mirrors `PlayerHudStatePublisher`'s shape. Not a reload state machine —
  no ammo, magazine, or gameplay effect; purely drives the slot so the meter
  visibly works.
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
- [ ] (Manual / dev-observation, `dev-tools` build) With the dev driver active,
  pressing Reload fills the meter `0 → 100%` over the dev duration, then
  resets to empty; releasing/idle leaves it at `0`. The automated slice of
  this behavior is Task 2's ramp unit test; the end-to-end driver → slot →
  `Bar` → render path is a visual check.
- [ ] With no driver (production build), the slot stays at `0.0` and the meter
  renders empty. (Review / grep gate — negative-existence claim, not an
  automated test) No per-frame warning is emitted.

## Tasks

### Task 1: Engine slot `player.reloadProgress`

Add a `player.reloadProgress` entry to `BUILTIN_ENGINE_STATE` in
`crates/entities/src/engine_state_catalog.rs`, structured like the other
`player.*` weapon-timing slots (`player.*` `sdk_path`,
`EngineStateValueType::Number`, `EngineStateCapability::Readonly`,
`persist: false`) but with its own values: `wire_name:
"player.reloadProgress"`, `sdk_path: &["player", "reloadProgress"]`, default
`Number(0.0)`, range `[0.0, 1.0]`, `network: ReplicationScope::None`. These
three differ from `player.weaponCooldownMs`
(`crates/entities/src/engine_state_catalog.rs:375-389`), which has `default:
EngineStateDefault::None`, range `[0.0, f32::INFINITY]`, and `network:
ReplicationScope::OwnerPrivatePlayer` — do not copy them. Ensure the generated
SDK state surface (the `getGameState()` reference tree and the TypeScript/Luau
typedefs) picks up the new leaf, then regenerate and commit the typedefs: run
the `gen-script-types` bin (declared in `crates/postretro/Cargo.toml`) and
commit the regenerated `sdk/types/*` files. The committed-drift test
`committed_sdk_types_match_current_registry`
(`crates/postretro/src/scripting/typedef/tests/committed.rs:8`) byte-matches
committed typedefs against the generator and fails if they're stale. Extend
`built_in_catalog_preserves_wire_names_and_capabilities`
(`crates/entities/src/engine_state_catalog.rs:597`) to insert
`"player.reloadProgress"` into the asserted `wire_names` vector (lines
606-618) in sorted position, between `"player.maxHealth"` and
`"player.weaponCooldownMs"`, and to assert the new entry's `sdk_path`,
`value_type`, `default`, `range`, and `capability`.
`player_owner_private_slots_are_replicated`
(`crates/entities/src/engine_state_catalog.rs:663`) asserts every
non-owner-private slot stays `ReplicationScope::None`; our slot ships `None`
and needs no change there.

### Task 2: Dev reload-progress driver

Add a dev-only system that drives `player.reloadProgress`. Model it on
`crates/postretro/src/scripting/systems/ui_proxy.rs`
(`PlayerHudStatePublisher`): a small struct holding a `ScriptCtx` clone, with a
`tick` that reads the frame's `Action::Reload` state and writes the slot via
`write_store_slot(&ctx, "player.reloadProgress", SlotValue::Number(..))`. Edge-
detect the reload press on `ButtonState::Pressed`, not `.is_active()` — the
latter is the held level bit `SimCommand.reload` samples for the sim command,
and is the wrong signal for a one-shot ramp start. On a reload press it starts
a ramp; each tick advances progress by `dt / duration`, clamps to `1.0`, then
resets to `0.0` once full (or on the next press). Keep the ramp's progress
accumulator on the driver struct itself, the way `PlayerHudStatePublisher`
keeps `invalid_max_warned_for` — inspectable and unit-testable via
`read_store_slot` without going through the render path. Gate the driver
behind the `dev-tools` cargo feature (declared on the `postretro` crate), the
same gate the rest of the dev-only tooling uses, so no production build fakes
a reload.

Tick it from `session.scripting`, the same home as `player_hud_state`, at the
seam where the HUD publisher itself ticks:
`session.scripting.player_hud_state.tick_for_role(...)` around
`crates/postretro/src/main.rs:2056`. Both `frame_dt` and
`gameplay_snapshot.as_ref()` are in scope there; store the driver alongside
`player_hud_state` on `session.scripting` and tick it at the same point,
reading the reload button from `gameplay_snapshot` and the frame time from
`frame_dt`. This is the same `Action::Reload` signal `build_sim_command`
samples for `SimCommand.reload` (`crates/postretro/src/main.rs:767`,
`snapshot.button(Action::Reload)` at line 777) — but `build_sim_command` is a
pure per-sim-tick helper with no `self`, `session`, `ScriptCtx`, or `dt`, so
the driver cannot tick there. The slot write must land before the UI
read-snapshot is built, same ordering as `PlayerHudStatePublisher`. Unit-test
the ramp: a press starts progress climbing, it reaches and clamps at `1.0`,
and resets to `0.0`.

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
  `sdk_path`, so no hand-authored SDK edit is needed — but the generated
  `sdk/types/*` typedefs are committed artifacts and must be regenerated via
  `gen-script-types` and committed alongside the catalog change.
  `ReplicationScope::None` keeps it out of the net fingerprint and the
  `StateSlotId` space.
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
only adds a one-line driver-tick call site in `main.rs` next to the
`player_hud_state.tick_for_role(...)` call — a call site, not an extension —
so no split is warranted here. The driver logic itself lives in a new module.

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

---

## Polish addendum: centered, lifecycle-visible reload meter

This addendum supersedes the earlier dev-HUD placement / always-on-empty-bar
details. It is deliberately a small `Bar` capability, not a general animation
or scripting runtime: authored UI declares the policy; the retained UI owns the
presentation clock and never writes state back to gameplay.

### Contract and ownership

- Add engine-owned readonly Boolean `player.reloadActive`, default `false`,
  `persist: false`, `ReplicationScope::None`, SDK path
  `getGameState().player.reloadActive`. It means the authoritative reload
  lifecycle is active, not "the meter happens to have a nonzero fraction". The
  eventual reload gameplay producer writes both this Boolean and
  `player.reloadProgress`; the dev driver remains the temporary producer.
- `player.reloadProgress` continues to be the authoritative numeric target.
  At completion the producer publishes `reloadProgress = 1.0` and
  `reloadActive = false` in the same snapshot, then may return progress to its
  resting `0.0` on the following tick. This gives the UI the terminal value to
  retain without putting a post-completion timer in game logic.
- The retained UI samples that one snapshot, then owns the exit presentation:
  it captures the displayed bar value and denominator on the active→inactive
  transition, retains its layout/draw node, and fades both background and fill
  alpha over the authored duration. State changes while it is exiting do not
  alter that captured image. At the end it becomes `Display::None` and emits no
  quads. A false first resolution is immediately `Display::None`; there is no
  fade-in. A false→true transition (including a retrigger during exit) cancels
  the exit, restores authored display, and renders at full alpha in that frame.
- UI time remains the existing dt-accumulated `UiReadSnapshot::time_seconds`;
  it pauses with game logic and is never wall-clock time. This remains
  renderer-local presentation state in `postretro-ui`, not renderer GPU state
  and not a Rust-side gameplay opacity slot.

### Exact additive public API

Extend only the passive `Bar` primitive (the concrete consumer), preserving all
old defaults and wire forms when omitted:

```ts
type BarExitFade = { durationMs: number };

type BarProps = {
  // existing props …
  width?: number;       // finite, > 0 logical-reference px; default 120
  height?: number;      // finite, > 0 logical-reference px; default 12
  visibleWhen?: Predicate;
  exitFade?: BarExitFade; // legal only with visibleWhen; finite durationMs > 0
};
```

The descriptor wire fields are camelCase `width`, `height`, and `exitFade`,
with `BarExitFade { duration_ms: f32 }` in Rust. The fade is intentionally
linear and has no `from`, target, or easing option: adding those would create a
general visibility-animation vocabulary without a second consumer. The SDK
factories for both TypeScript and Luau reject non-finite / non-positive sizes
and durations, and reject `exitFade` without `visibleWhen` at authoring time.
Direct descriptor inputs still use serde's normal malformed-registration
containment.

The dev HUD authors the requested treatment directly:

```ts
const reloadMeter = Bar({
  bind: bindState(player.reloadProgress),
  max: 1.0,
  width: 120.0,
  height: 24.0,
  visibleWhen: stateEquals(player.reloadActive, true),
  exitFade: { durationMs: 500.0 },
  fill: color.ok,
  background: color.hud.health.background,
});

Tree({ anchor: "center", offset: [0.0, 36.0] }, reloadMeter)
```

`offset: [0.0, 36.0]` centers the meter horizontally and puts its center below
the reticle; `height: 24.0` is exactly twice the existing 12 logical-pixel bar
height. Width remains the existing 120 logical pixels.

### Scope changes

In scope now includes the above `reloadActive` lifecycle slot, per-bar logical
dimensions, and a `Bar` exit fade. The earlier "whole-widget hide-when-idle" and
"beside the reticle" exclusions/criteria are replaced by this addendum. The
following remain out of scope: reload/ammo gameplay, production fake drivers,
replication, generic widget opacity, generic enter/exit animation systems, and
nonlinear visibility choreography.

### Tasks and sequencing

**Task 4 — Lifecycle state and dev producer (after existing Tasks 1–2).**
Add `player.reloadActive` to `crates/entities/src/engine_state_catalog.rs` with
the Boolean/default/network contract above; regenerate and commit both SDK
typedef files; extend the catalog and UI-snapshot expectations. Update
`crates/postretro/src/scripting/systems/reload_progress.rs` so press sets
active true and starts progress at zero, the half-second ramp writes progress,
completion writes `(1.0, false)` together, and the next tick restores only
progress to zero. A fresh press restarts an active ramp and also starts a new
lifecycle if it arrives while the UI is fading. The dev-only gate and its
pre-UI-snapshot tick location stay unchanged.

**Task 5 — Sized, exit-fading `Bar` (after Task 4).** Add the descriptor fields
in `crates/scripting-core/src/ui/descriptor/widgets.rs`; TypeScript/Luau
factory validation and public declarations in `sdk/lib/ui/widgets.{ts,luau}`;
then regenerate `sdk/types/postretro.d.{ts,luau}`. Teach
`crates/ui/src/tree/build.rs` to use authored width/height or the existing
120×12 default. Extend the retained visibility state / node draw context in
`crates/ui/src/tree/{widget_meta.rs,ui_tree.rs,node_context.rs,ui_tree_collect.rs}`
to implement the lifecycle above, multiplying both `Bar` quad alphas and
freezing the terminal presented value while exiting. Keep the renderer API and
GPU code unchanged: it already consumes alpha-bearing draw-list colors. Update
`context/lib/ui.md` §3 with the durable `visibleWhen`/`Bar.exitFade` ownership
and retrigger contract.

**Task 6 — Dev HUD polish (after Tasks 4–5).** Update
`content/dev/scripts/hud.ts` to import `stateEquals`, use the exact `Bar` and
tree values above, and keep `hud.reloadMeter` as its own always-on anchored
tree. `alwaysOn` means the retained tree participates in composition; the
bar itself is non-rendering at idle through `visibleWhen`.

### Acceptance and verification

- [ ] The generated TS and Luau game-state trees expose readonly
  `player.reloadActive` as Boolean and retain the existing numeric
  `player.reloadProgress`; catalog tests pin both entries and their defaults.
- [ ] The dev driver test observes press → `(active=true, progress rising)` →
  `(active=false, progress=1.0)` → `(active=false, progress=0.0)`, using
  deterministic deltas and approximate float assertions.
- [ ] Descriptor/SDK tests prove `Bar` round-trips `width`, `height`, and
  `exitFade` in camelCase; omitted fields preserve old descriptor wire output;
  TypeScript and Luau factories reject invalid dimensions/durations and an
  orphan `exitFade`.
- [ ] Retained UI tests prove: initially false emits zero bar quads; true shows
  full-alpha quads immediately; true→false retains the terminal 100% fill and
  fades background and fill alpha over exactly 500 ms; after expiry it emits no
  bar quads; and a true retrigger mid-fade cancels the old exit without stale
  opacity or frozen value. These are CPU draw-list tests, not GPU tests.
- [ ] Manual `dev-tools` check using
  `cargo run -p xtask -- run --features dev-tools -- content/dev/maps/campaign-test.prl`:
  R shows a 120×24 meter centered below the crosshair, fills over the dev
  ramp, holds its terminal image at completion, and fades away over 500 ms;
  pressing R during the fade starts a fresh, fully opaque meter.

**Phase P1 (sequential):** Task 4 establishes the authoritative lifecycle
signal used by both the HUD expression and the dev driver test.

**Phase P2 (sequential):** Task 5 consumes that semantic contract and adds the
small renderer-local presentation capability. It is intentionally before HUD
authoring so the final content uses real typed API, not a temporary shape.

**Phase P3:** Task 6, focused checks, then visual dev verification.
