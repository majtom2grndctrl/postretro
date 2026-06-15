# Production Gameplay HUD Through the UI SDK

> Prerequisite: `drafts/game-state-sdk-surface` ships first.

## Goal

Replace the development HUD with a production HUD authored through the
TypeScript UI SDK.

Use the HUD as an end-to-end validation of the SDK authoring contract:
`setupMod()` returns all engine-bound data, the short-lived mod-init context
drops, and Rust retains and renders the resulting UI and state references.
The long-lived QuickJS definition context is not subject to this lifecycle
statement.

Keep the engine JSON HUD as a minimal fallback when no mod HUD is registered.

## User Experience

- Normal development launch shows a compact bottom-left health panel.
- A centered text reticle remains independent of the status panel.
- Health text displays current player health.
- A normalized health bar updates and changes style across authored bands.
- Mod theme tokens drive HUD colors, fonts, and spacing.
- HUD anchors and offsets remain literal tree-definition values.
- Demo ammo, intro flash color, and HUD screen-flash swatches are absent.
- Launch without mod HUD registration shows a minimal engine fallback.

## Architecture

### SDK-authored HUD

Add `content/dev/scripts/hud.ts`. Import all authoring vocabulary from
`"postretro"`:

```ts
// Proposed design
import {
  bindState,
  getGameState,
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
// Proposed design
const hudTheme = {
  // Custom HUD color, font, and spacing tokens.
};

export function buildHud() {
  const { player } = getGameState();
  const health = player.health;

  const status = Text({
    content: "HP",
    bind: bindState(health.current, { format: "HP {}" }),
  });

  const bar = Bar({
    bind: bindState(health.fraction, {
      tween: {
        durationMs: 180,
        easing: "easeOut",
      },
    }),
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

  const healthTree = Tree(
    { anchor: "bottomLeft", offset: [24, -24] },
    VStack({ gap: "hud.gap", padding: "hud.padding" }, [
      HStack({ gap: "hud.gap" }, [status]),
      bar,
    ]),
  );

  const reticleTree = Tree(
    { anchor: "center", offset: [0, 0] },
    Text({ content: "+", font: "mono" }),
  );

  return {
    uiTrees: [
      { name: "hud", tree: healthTree, alwaysOn: true },
      { name: "hud.reticle", tree: reticleTree, alwaysOn: true },
    ],
    theme: hudTheme,
  };
}
```

`buildHud()` obtains its own engine-state references. `setupMod()` does not
pass `player` or a state tree into the HUD builder. These references are
authoring vocabulary, not component props or current runtime values.

Luau uses the same tree shape and the same
`bindState(ref, options)` helper. The contract does not rely on TypeScript
object-spread syntax for binding options.

The reticle uses `Text({ content: "+", font: "mono" })`.

`content/dev/start-script.ts` returns the HUD and theme:

```ts
// Proposed design
const hud = buildHud();

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
through registry tier precedence. Refactor the registry to retain engine and
mod entries by tier rather than destructively overwriting the lower tier.
`hud.reticle` is an additional always-on mod tree.

This validates replacement by name and composition of another always-on tree.

### Staged reload

Cold and staged mod init use the same returned-manifest contract. Extend
`StagedManifest` to retain parsed UI trees and the complete mod theme override
instead of dropping them after script evaluation.

A successful, current staged result commits stores, UI trees, and theme
together:

- the returned mod UI-tree set replaces the previous mod tier as a whole;
- a tree omitted from the new snapshot is removed from the mod tier, revealing
  an engine fallback with the same name when one exists;
- the returned theme replaces the complete previous mod override and merges
  fresh over engine defaults, so omitted tokens revert to defaults;
- always-on layers resolve the updated registry on the next frame;
- already-pushed modal instances remain stable until closed and pushed again.

Failed and stale staged results preserve the previously committed stores, UI
trees, and theme.

This HUD uses engine-provided body and mono fonts. Staged replacement or
removal of custom font assets is outside this plan because the runtime has no
font replacement/removal contract. Mods that change custom font declarations
must restart.

### Engine state

Replace `StaticUiProxy` with a focused player HUD state publisher.

It publishes:

| SDK path | Stable slot | Value | Range |
| --- | --- | --- | --- |
| `getGameState().player.health.current` | `player.health` | current health | dynamic `[0, max HP]` |
| `getGameState().player.health.fraction` | `player.healthFraction` | `current / max` | static `[0, 1]` |

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

### Tween behavior

The health bar uses a 180 ms `easeOut` controlled tween. The deterministic
fixture begins at fraction `1.0`, publishes `0.2`, verifies that the displayed
value remains strictly between those values around the tween midpoint, and
then verifies that it settles at `0.2`. Style ranges evaluate the displayed
tween value, so the critical band must not activate before the displayed value
crosses its threshold.

### Non-goals

- Theme-tokenized tree anchors or offsets.
- Staged replacement or removal of custom font assets.
- Mutating already-pushed modal instances in place.
- Automated GPU framebuffer comparison.

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

Use at least two health snapshots and controlled time progression that
observes both an in-flight and settled tween value.

Assert:

- returned `hud` and `hud.reticle` envelopes exist and are always-on;
- no import-time state or UI registration is required;
- mod `hud` shadows the fallback-only marker;
- reticle text is present;
- health text changes after the second snapshot;
- the bar has an intermediate displayed fraction around the tween midpoint;
- the bar settles at the destination and reaches the expected style band;
- theme colors and spacing resolve;
- anchored placement resolves;
- ammo, intro flash, and HUD flash swatches are absent;
- retained data remains usable after the mod-init context drops.

After the cold-launch assertions, exercise the staged path:

1. Commit a successful staged result with an updated HUD tree and theme.
2. Assert the always-on draw data and resolved theme update on the next frame.
3. Commit a successful staged result that omits the mod `hud`.
4. Assert the engine fallback marker is revealed.
5. Assert failed and stale staged results preserve the last committed mod
   UI-tree snapshot and theme.

This test covers draw-data construction, not GPU encoding. Verify final GPU
composition through manual launch.

## Boundary Inventory

| Boundary | Producer | Consumer | Contract |
| --- | --- | --- | --- |
| `getGameState().player.health.current` | game-state catalog | HUD SDK | readonly ref to `player.health` |
| `getGameState().player.health.fraction` | game-state catalog | HUD SDK | readonly ref to `player.healthFraction` |
| `player.health` | player HUD publisher | state snapshot | current HP; dynamic range |
| `player.healthFraction` | player HUD publisher | state snapshot | clamped fraction; static range |
| `ModManifest.stores` | `setupMod()` | manifest parser | returned store declarations only |
| `ModManifest.uiTrees` | `setupMod()` | manifest parser | `{ name, tree, alwaysOn }[]` |
| `ModManifest.theme` | `setupMod()` | theme merge | partial token maps |
| `hud` | fallback and mod HUD | UI registry | mod scope shadows engine scope |
| `hud.reticle` | mod HUD | UI registry | additional always-on layer |
| staged UI snapshot | successful current staged result | tiered UI registry | complete mod tree set replaces prior mod tier |
| staged theme snapshot | successful current staged result | theme store | complete mod override replaces prior override |

## Acceptance Criteria

- [ ] Development HUD is authored in TypeScript with UI SDK factories.
- [ ] HUD imports `getGameState` from `"postretro"` and uses no special
  game-state module.
- [ ] `buildHud()` calls `getGameState()` internally. `setupMod()` passes no
  engine-state domain into it.
- [ ] HUD binds `player.health.current` and `player.health.fraction` references
  directly, without `.get()`.
- [ ] Bind formatting and tween options compose through
  `bindState(ref, options)` in TypeScript and Luau.
- [ ] `setupMod()` returns all stores, UI trees, and theme data consumed by the
  engine. Import-time calls perform no engine registration.
- [ ] Reticle uses `Text({ content: "+", font: "mono" })`.
- [ ] Controlled snapshots update health text and prove both an in-flight
  180 ms `easeOut` bar value and the settled critical style band.
- [ ] Fraction bar and style ranges use max `1`.
- [ ] Mod `hud` shadows the fallback marker; `hud.reticle` composes separately.
- [ ] Theme tokens affect colors, fonts, and spacing. Anchors and offsets
  remain literal tree values.
- [ ] Successful current staged results atomically commit stores, replace the
  complete mod UI-tree tier, and replace the complete mod theme override.
- [ ] Omitting `hud` from a staged mod snapshot removes the mod tree and reveals
  the engine fallback. Omitting theme tokens reverts them to engine defaults.
- [ ] Failed and stale staged results preserve the current UI-tree snapshot and
  theme.
- [ ] Always-on layers observe committed staged trees on the next frame;
  already-pushed modal instances remain stable until reopened.
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
- [ ] SDK and modding documentation shows the final `getGameState()` plus
  `bindState()` HUD pattern and documents staged replacement semantics,
  literal placement, modal behavior, and the custom-font limitation.
- [ ] No tracked generated `start-script.js` is added.
- [ ] No new `unsafe` code or renderer ownership violation is introduced.

## Tasks

### Task 1: Publish production HUD state

Refocus the existing UI proxy as the player HUD state publisher. Publish
current and fractional health in the required frame position.

Update the shared engine-state catalog: remove fake ammo, add health fraction,
and map both stable wire slots under `getGameState().player.health`.

Preserve current-health range attachment during level installation and hot
reload. Add focused publisher, range, ordering, and generated-path tests.

Defensive policy: health descriptors reject invalid maximums. If invalid live
data still reaches the publisher, publish current health, skip fraction, and
log once per affected player lifetime. Do not divide by, clamp, or silently
repair an invalid maximum.

### Task 2: Author and register the HUD

Build the health and reticle trees with SDK factories. Return their envelopes
and custom theme from `setupMod()`.

Use `bindState(ref, options)` for health formatting and the explicit 180 ms
`easeOut` bar tween. Keep anchors and offsets literal; use theme tokens for
colors, fonts, and spacing only.

Migrate remaining development stores to the prerequisite's returned
`stores` contract. Remove intro store setup and flash swatches. Simplify the
engine JSON HUD to its fallback role.

Add compiler and manifest parsing coverage.

### Task 3: Complete staged UI and theme commit plumbing

Retain returned UI trees and the complete mod theme override in
`StagedManifest`. Commit them only with a successful current staged result,
alongside returned stores.

Refactor UI registry storage to retain engine and mod tiers. Add
replacement-style commit for the complete mod tree snapshot so omitted mod
trees are removed and engine fallbacks become visible.

Replace the previous mod theme override as a whole and merge the new snapshot
fresh over engine defaults. Preserve current UI trees and theme on failed or
stale results.

Keep already-pushed modal instances stable while making always-on layers
observe committed registry changes on the next frame.

### Task 4: Add the headless cold-launch and staged regression

Exercise the TypeScript-source-to-retained-draw-data path described above.
Prove setup-return publication, state binding, fallback shadowing, reticle
composition, updates, tween progress and settlement, and theme resolution.

Then exercise successful UI/theme replacement, omission of the mod `hud`,
fallback reveal, and preservation on failed and stale staged results.

Keep generated typedef drift in the prerequisite's generation tests. Keep GPU
verification manual.

### Task 5: Document and verify

Update durable UI and scripting docs for the SDK-authored HUD, fallback
shadowing, direct state references, `bindState`, health fraction, literal
placement, staged replacement semantics, modal-instance behavior, and the
custom-font restart limitation.

Run formatting, focused tests, workspace clippy, workspace tests, normal debug
launch, and fallback debug launch.

## Sequencing

**Prerequisite:** `game-state-sdk-surface`.

**Phase 1 (sequential):** Task 1 establishes published health state.

**Phase 2 (sequential):** Task 2 authors against that state contract.

**Phase 3 (sequential):** Task 3 completes staged UI and theme ownership.

**Phase 4 (sequential):** Task 4 validates cold and staged headless paths.

**Phase 5 (sequential):** Task 5 documents and manually verifies rendering.

## Open Questions

None.
