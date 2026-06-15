# Production Gameplay HUD Through the UI SDK

> Prerequisite: `drafts/game-state-sdk-surface` ships first.

## Goal

Replace the development HUD with a production HUD authored through the
TypeScript UI SDK.

Use the HUD as an end-to-end validation of the SDK authoring contract:
`setupMod()` returns all engine-bound data, the authoring VM drops, and Rust
retains and renders the resulting UI and state references.

Keep the engine JSON HUD as a minimal fallback when no mod HUD is registered.

## User Experience

- Normal development launch shows a compact bottom-left health panel.
- A centered text reticle remains independent of the status panel.
- Health text displays current player health.
- A normalized health bar updates and changes style across authored bands.
- Mod theme tokens drive HUD colors, spacing, and placement.
- Demo ammo, intro flash color, and HUD screen-flash swatches are absent.
- Launch without mod HUD registration shows a minimal engine fallback.

## Architecture

### SDK-authored HUD

Add `content/dev/scripts/hud.ts`. Import all authoring vocabulary from
`"postretro"`:

```ts
import {
  gameState,
  Tree,
  VStack,
  HStack,
  Text,
  Bar,
} from "postretro";
```

No `"postretro/game-state"` import exists.

Build two `AnchoredTreeDescriptor` values:

- `hud`: bottom-left health status;
- `hud.reticle`: centered reticle.

Return them through `setupMod().uiTrees` as `{ name, tree, alwaysOn }`
envelopes. `alwaysOn` belongs to the envelope, not the anchored tree.

Two trees are required because one anchored tree has one viewport anchor.

The health tree uses direct state references:

```ts
const health = gameState.player.health;

Text({
  content: "HP",
  bind: { ...health.current, format: "HP {}" },
});

Bar({
  bind: health.fraction,
  max: 1,
  fill: "hud.health.ok",
  background: "hud.health.background",
  styleRanges: {
    max: 1,
    entries: [
      { upTo: 0.25, color: "critical" },
      { upTo: 0.5, color: "warning" },
      { color: "ok" },
    ],
  },
});
```

The reticle uses:

```ts
Text({ content: "+", font: "mono" });
```

`content/dev/start-script.ts` returns the HUD and theme:

```ts
return {
  name: "Development Content",
  entities,
  stores: [pauseMenuStore.declaration],
  uiTrees: hud.uiTrees,
  theme: hud.theme,
};
```

Every declaration crossing into Rust is reachable from this return value.
The pause-menu store module is a pure definition imported by this entry.

### Registry replacement

Keep the engine tree name `hud`. The mod tree shadows the engine fallback
through existing registry tier precedence. `hud.reticle` is an additional
always-on mod tree.

This validates replacement by name and composition of another always-on tree.

### Engine state

Replace `StaticUiProxy` with a focused player HUD state publisher.

It publishes:

| SDK path | Stable slot | Value | Range |
| --- | --- | --- | --- |
| `gameState.player.health.current` | `player.health` | current health | dynamic `[0, max HP]` |
| `gameState.player.health.fraction` | `player.healthFraction` | `current / max` | static `[0, 1]` |

The game-state prerequisite owns the catalog and SDK path mapping. This plan
changes catalog entries by removing fake ammo and adding health fraction.

Both health values are engine-owned and readonly. If no pawn or health
component exists, skip both writes and preserve stale values.

Use the existing player-with-health lookup. Read current and maximum health in
one registry borrow. Clamp fraction to `[0, 1]`.

The publisher ticks after game-logic writes settle and before state-crossing
detection and UI snapshot construction. Crossings and same-frame UI reads then
observe the published values.

Remove:

- fake `player.ammo` publication, declaration, generated path, and tests;
- `intro.flashColor`, its timer, warning state, and store setup;
- HUD swatches that display flash state.

Preserve `screen.flash`, its decay system, reaction, screen-effects consumer,
and tests.

### Engine fallback

Reduce `content/base/ui/hud.json` to a minimal health display. Give it a unique
fallback-only text marker.

A focused fixture proves the marker appears when the fallback is built alone.
The shadowing test proves it is absent when the mod `hud` replaces the
fallback.

Use a debug launch with mod HUD registration omitted for manual fallback
verification. Do not require a release launch without a start script.

### Script lifecycle

The engine keeps one persistent `ScriptRuntime`. Each mod-init run creates and
drops an ephemeral authoring context.

The integration test:

1. Bundles `content/dev/start-script.ts`.
2. Runs mod init.
3. Drains owned stores, UI trees, and theme data from the returned manifest.
4. Drops the mod-init context.
5. Uses the retained Rust data to publish state and build draw data.

The test does not claim that `ScriptRuntime` itself is dropped.

## End-to-End Validation

Add a headless cold-launch regression:

1. Bundle the development TypeScript entry into a temporary mod root.
2. Construct scripting context, primitive registry, and runtime.
3. Run mod init and commit only returned manifest data.
4. Register an engine fallback `hud`.
5. Register returned mod trees at mod scope.
6. Merge the returned theme.
7. Publish controlled health snapshots.
8. Compose always-on layers.
9. Build retained draw data for each layer.
10. Inspect the resulting primitives.

Use at least two health snapshots and enough time progression to settle the
authored tween.

Assert:

- returned `hud` and `hud.reticle` envelopes exist and are always-on;
- no import-time state or UI registration is required;
- mod `hud` shadows the fallback-only marker;
- reticle text is present;
- health text changes after the second snapshot;
- bar fraction changes;
- tween progression reaches the expected style band;
- theme colors and spacing resolve;
- anchored placement resolves;
- ammo, intro flash, and HUD flash swatches are absent;
- retained data remains usable after the mod-init context drops.

This test covers draw-data construction, not GPU encoding. Verify final GPU
composition through manual launch.

## Boundary Inventory

| Boundary | Producer | Consumer | Contract |
| --- | --- | --- | --- |
| `gameState.player.health.current` | game-state catalog | HUD SDK | readonly ref to `player.health` |
| `gameState.player.health.fraction` | game-state catalog | HUD SDK | readonly ref to `player.healthFraction` |
| `player.health` | player HUD publisher | state snapshot | current HP; dynamic range |
| `player.healthFraction` | player HUD publisher | state snapshot | clamped fraction; static range |
| `ModManifest.stores` | `setupMod()` | manifest parser | returned store declarations only |
| `ModManifest.uiTrees` | `setupMod()` | manifest parser | `{ name, tree, alwaysOn }[]` |
| `ModManifest.theme` | `setupMod()` | theme merge | partial token maps |
| `hud` | fallback and mod HUD | UI registry | mod scope shadows engine scope |
| `hud.reticle` | mod HUD | UI registry | additional always-on layer |

## Acceptance Criteria

- [ ] Development HUD is authored in TypeScript with UI SDK factories.
- [ ] HUD imports `gameState` from `"postretro"` and uses no special
  game-state module.
- [ ] HUD binds `gameState.player.health.current` and
  `gameState.player.health.fraction` directly, without `.get()`.
- [ ] Bind formatting composes by spreading the current-health reference.
- [ ] `setupMod()` returns all stores, UI trees, and theme data consumed by the
  engine. Import-time calls perform no engine registration.
- [ ] Reticle uses `Text({ content: "+", font: "mono" })`.
- [ ] Controlled snapshots update health text, bar fraction, and eventual style
  band.
- [ ] Fraction bar and style ranges use max `1`.
- [ ] Mod `hud` shadows the fallback marker; `hud.reticle` composes separately.
- [ ] Theme tokens affect colors, spacing, and placement.
- [ ] Headless coverage bundles TypeScript, runs mod init, drops its authoring
  context, composes always-on layers, and builds retained draw data.
- [ ] Fake ammo, intro flash state, obsolete intro stores, and HUD flash
  swatches are removed.
- [ ] `screen.flash` and its decay path remain supported.
- [ ] `player.health` keeps dynamic range attachment during install and hot
  reload.
- [ ] `player.healthFraction` is clamped and carries static range `[0, 1]`.
- [ ] Minimal fallback loads in a focused fixture and debug manual launch.
- [ ] GPU composition is verified manually.
- [ ] No tracked generated `start-script.js` is added.
- [ ] No new `unsafe` code or renderer ownership violation is introduced.

## Tasks

### Task 1: Publish production HUD state

Refocus the existing UI proxy as the player HUD state publisher. Publish
current and fractional health in the required frame position.

Update the shared engine-state catalog: remove fake ammo, add health fraction,
and map both stable wire slots under `gameState.player.health`.

Preserve current-health range attachment during level installation and hot
reload. Add focused publisher, range, ordering, and generated-path tests.

Defensive policy: health descriptors reject invalid maximums. If invalid live
data still reaches the publisher, publish current health, skip fraction, and
log once per affected player lifetime. Do not divide by, clamp, or silently
repair an invalid maximum.

### Task 2: Author and register the HUD

Build the health and reticle trees with SDK factories. Return their envelopes
and custom theme from `setupMod()`.

Migrate remaining development stores to the prerequisite's returned
`stores` contract. Remove intro store setup and flash swatches. Simplify the
engine JSON HUD to its fallback role.

Add compiler and manifest parsing coverage.

### Task 3: Add the headless cold-launch regression

Exercise the TypeScript-source-to-retained-draw-data path described above.
Prove setup-return publication, state binding, fallback shadowing, reticle
composition, updates, tween settlement, and theme resolution.

Keep generated typedef drift in the prerequisite's generation tests. Keep GPU
verification manual.

### Task 4: Document and verify

Update durable UI and scripting docs for the SDK-authored HUD, fallback
shadowing, direct state references, and health fraction.

Run formatting, focused tests, workspace clippy, workspace tests, normal debug
launch, and fallback debug launch.

## Sequencing

**Prerequisite:** `game-state-sdk-surface`.

**Phase 1 (sequential):** Task 1 establishes published health state.

**Phase 2 (sequential):** Task 2 authors against that state contract.

**Phase 3 (sequential):** Task 3 validates the complete headless path.

**Phase 4 (sequential):** Task 4 documents and manually verifies rendering.

## Open Questions

None.
