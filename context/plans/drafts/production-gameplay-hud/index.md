# Production Gameplay HUD Through the UI SDK

> Prerequisite: `drafts/game-state-runtime-handles` must ship first. This HUD
> consumes executable `postretro/game-state` handles from that plan.

## Goal

Replace the development HUD with a production HUD authored through the TypeScript UI SDK. Use it to validate TypeScript bundling, mod-init registration, state binding, theme resolution, registry shadowing, retained layout, and draw-data construction.

Keep the engine JSON HUD as a minimal fallback when no mod HUD is registered.

## User Experience

- Normal development launch shows a compact bottom-left health panel and centered text reticle.
- Health text displays current player health.
- A normalized health bar updates and changes style across authored bands.
- Mod theme tokens drive HUD colors, gap, padding, and placement.
- Demo ammo, intro flash color, and HUD screen-flash swatches are absent.
- A launch without mod HUD registration shows a small engine fallback health display.

## Architecture

### SDK-authored HUD

Add `content/dev/scripts/hud.ts`. It imports SDK factories from `"postretro"` and executable typed engine state from `"postretro/game-state"`.

Build two `AnchoredTreeDescriptor` values:

- `hud`: bottom-left health status.
- `hud.reticle`: centered reticle.

Return them as `ModUiTree` envelopes shaped `{ name, tree, alwaysOn }`. `alwaysOn` belongs to this envelope, not `AnchoredTreeDescriptor`.

Two trees are required because each anchored tree has one viewport anchor. Status and reticle can then target bottom-left and center without viewport-sized spacer layout.

The health tree uses:

- `Tree`, `VStack`, `HStack`, `Text`, and `Bar`.
- `player.health.get()` for current-health text.
- `player.healthFraction.get()` for normalized fill and style.
- tween and `styleRanges` feedback.
- custom color, font, spacing, and placement tokens.

The fraction bar uses `max: 1`. Its `styleRanges.max` is also `1`.

The reticle uses:

```ts
Text({ content: "+", font: "mono" })
```

`content/dev/start-script.ts` imports the HUD builder and returns its trees and theme from `setupMod()`:

```ts
return {
  name: "Development Content",
  entities,
  uiTrees: hud.uiTrees,
  theme: hud.theme,
};
```

The active HUD is not loaded from JSON.

### Registry replacement

Keep the engine tree name `hud`. The mod `hud` relies on `UiTreeRegistry` tier precedence to shadow the engine fallback. `hud.reticle` is an additional always-on mod tree.

This validates:

- replacement of an engine tree by name;
- composition of another always-on tree.

### Engine state

Replace `StaticUiProxy` with a player HUD state publisher.

It publishes:

- `player.health`: current health. Preserve its dynamic `[0, max HP]` range attachment during level installation and hot reload.
- `player.healthFraction`: `current / max`, clamped to `[0, 1]`, with static range `[0, 1]`.

Both slots are engine-owned, readonly numbers. If no pawn or health component exists, skip both writes and preserve stale values.

Use the existing `scripting::components::health::pawn_with_health` lookup. It resolves the first entity carrying `PlayerMovement` and its `Health` component.

The publisher ticks after game logic writes settle and before state-crossing detection and UI snapshot construction. Preserve this ordering so crossings and same-frame UI reads observe the published values.

Remove only:

- fake `player.ammo` publication, declaration, generated handles, and tests;
- `intro.flashColor`, its timer, warning state, and obsolete store setup;
- HUD swatches that display flash state.

Preserve the supported `screen.flash` slot, `FlashDecay`, system reaction, screen-effects consumption, and related tests.

Before changing the schema, extract engine-owned slot declarations from `crates/postretro/src/scripting/slot_table.rs` into `crates/postretro/src/scripting/engine_slots.rs`. The new module exposes the declaration collection consumed by `SlotTable::default`. Move all existing engine-owned slots together. Task 2 then deletes `player.ammo` and adds `player.healthFraction`.

Regenerate committed TypeScript and Luau game-state bindings. Runtime namespace
installation remains owned by the prerequisite plan; this plan changes the
catalog by removing `player.ammo` and adding `player.healthFraction`.

### Engine fallback

Reduce `content/base/ui/hud.json` to a minimal health display. Give it a unique fallback-only text primitive. The focused fallback fixture asserts that marker is present when the fallback is built alone. The shadowing test asserts it is absent when the mod `hud` replaces the fallback.

Fallback verification uses:

- a focused fallback-only integration fixture;
- a debug manual launch with mod HUD registration omitted.

Do not require a release launch without a start script. Release mod init rejects that configuration.

### Script lifecycle

The engine keeps one persistent `ScriptRuntime`. Each `run_mod_init` call creates and drops an ephemeral mod-init authoring context. Only owned Rust manifest data survives the call.

The integration setup constructs `ScriptCtx`, registers primitives in `PrimitiveRegistry`, constructs `ScriptRuntime`, then calls `run_mod_init`. `ScriptRuntime` internally installs and evaluates the SDK prelude.

The test asserts that manifest-owned trees and theme remain usable after `run_mod_init` returns while no mod-init authoring context is retained. It does not claim that `ScriptRuntime` is dropped.

## End-to-End Validation

Add a headless cold-launch regression:

1. Bundle `content/dev/start-script.ts` with `postretro_script_compiler::bundle_entry`.
2. Write the generated JavaScript into a temporary mod root as `start-script.js`.
3. Construct `ScriptCtx`, `PrimitiveRegistry`, and `ScriptRuntime`.
4. Call `run_mod_init`; let the runtime install and evaluate the SDK prelude internally.
5. Drain owned `uiTrees` and theme data from the parsed manifest.
6. Register an engine fallback `hud`, then register mod trees at `ScopeTier::Mod`.
7. Merge the mod theme.
8. Publish controlled health snapshots.
9. Compose layer names with `always_on_layers`.
10. Call `UiTree::build_draw_data_retained` for each composed layer and inspect primitives.

The test covers draw-data construction only. It does not cover GPU encode or rendering. Verify GPU composition through manual launch.

Use at least two controlled slot snapshots and time deltas. Assert:

- SDK-created `hud` and `hud.reticle` envelopes exist and are always-on;
- the mod `hud` shadows the fallback-only marker;
- reticle text is present;
- updated health text appears after the second snapshot;
- bar fraction updates;
- tween progression reaches the eventual authored style band;
- theme colors resolve;
- token-driven gap, padding, and placement resolve;
- no ammo, intro flash, or HUD flash-swatch content appears;
- owned manifest data remains usable after the ephemeral mod-init context drops.

Do not assert exact visual constants beyond authored descriptor values. Bar dimensions are descriptor geometry, not theme-driven geometry.

Keep generated typedef drift as a separate generation test. Keep lower-level parser, registry, retained-tree, and renderer-data tests where they still cover supported behavior.

## Boundary Inventory

| Boundary | Producer | Consumer | Contract |
| --- | --- | --- | --- |
| `player.health` | player HUD state publisher | generated handle and HUD text | readonly number; dynamic `[0, max HP]` range |
| `player.healthFraction` | player HUD state publisher | generated handle and HUD bar | readonly number; static `[0, 1]` range |
| `ModManifest.uiTrees` | `setupMod()` | manifest parser | `{ name, tree, alwaysOn }[]` |
| `ModManifest.theme` | HUD module | theme merge | partial token maps |
| `hud` | engine fallback and mod HUD | `UiTreeRegistry` | mod tier shadows engine tier |
| `hud.reticle` | mod HUD | `UiTreeRegistry` | additional always-on layer |
| bundled development entry | script compiler | `ScriptRuntime` | generated in temporary test root |
| game-state runtime handles | prerequisite plan | HUD factories | `.get()` returns `{ slot }` bind descriptors |

## Tasks

### Task 1: Split engine slot declarations

**Files**

- `crates/postretro/src/scripting/slot_table.rs`
- new `crates/postretro/src/scripting/engine_slots.rs`

**Work**

- Move every existing engine-owned slot declaration into `engine_slots.rs`.
- Expose the declaration collection consumed by `SlotTable::default`.
- Preserve declaration order, schemas, defaults, ownership, and validation.
- Keep `slot_table.rs` focused on table mechanics.

**Tests**

- Existing slot declaration and table tests remain green.
- Add focused extraction coverage only where existing tests do not cover the collection boundary.

### Task 2: Publish production HUD state

**Files**

- `crates/postretro/src/scripting/systems/ui_proxy.rs`
- `crates/postretro/src/scripting/components/health.rs`
- `crates/postretro/src/scripting/runtime.rs`
- `crates/postretro/src/main.rs`
- `crates/postretro/src/scripting/engine_slots.rs`
- generated TypeScript and Luau game-state bindings
- generation tests

**Work**

- Rename and refocus `StaticUiProxy` as the player HUD state publisher.
- Use `pawn_with_health` to read current and maximum health in one lookup.
- Publish `player.health` and clamped `player.healthFraction`.
- Tick after game logic and before crossing detection and UI snapshot construction.
- Delete `player.ammo` and add `player.healthFraction` in the extracted declaration collection.
- Preserve `player.health` range attachment during level installation and `follow_pawn_health_range_after_refresh`.
- Give `player.healthFraction` a static `[0, 1]` range.
- Remove intro flash timer and warning state.
- Preserve `screen.flash` and the flash-decay system unchanged.
- Regenerate typed handles.

**Tests**

- Current and fractional health publish from controlled health components.
- Missing pawn or health skips both writes.
- Publisher runs before crossing detection and snapshot consumption.
- Level install and hot reload still attach `player.health` range from live max health.
- `player.healthFraction` retains static `[0, 1]`.
- Generated bindings expose `player.healthFraction` and omit `player.ammo`.

### Task 3: Author and register the HUD

**Files**

- new `content/dev/scripts/hud.ts`
- `content/dev/start-script.ts`
- `content/base/ui/hud.json`
- remove `content/dev/scripts/intro-store.ts`
- remove `content/dev/scripts/intro-store.luau`

**Work**

- Build health and reticle trees with SDK factories.
- Use `{ name, tree, alwaysOn }` registration envelopes.
- Bind through generated player handles.
- Pin fraction bar and style ranges to max `1`.
- Return custom HUD theme tokens from `setupMod()`.
- Simplify the engine JSON HUD to the fallback-only fixture shape.
- Remove intro store setup and HUD flash swatches.
- Do not track `content/dev/start-script.js`. Generated JavaScript stays ignored.

**Tests**

- Compiler coverage confirms the production TypeScript entry bundles.
- Manifest parsing confirms both tree envelopes and theme payload.
- Fallback JSON loads and contains its unique marker.

### Task 4: Add the headless cold-launch regression

**Files**

- focused scripting/UI integration fixture

**Work**

- Use the prerequisite plan's `postretro-script-compiler` dev-dependency.
- Generate `start-script.js` into the temporary test root with `bundle_entry`.
- Exercise the TypeScript-source-to-retained-draw-data path above.
- Compose names through `always_on_layers`.
- Call `UiTree::build_draw_data_retained` for each layer.
- Use two or more controlled health snapshots and deltas.
- Prove fallback shadowing, reticle composition, live text/fraction updates, eventual style band, and token resolution.
- Do not claim GPU encode or render coverage.

**Tests**

- The focused regression covers the cross-subsystem path.
- Generated typedef drift remains a separate generation test.
- GPU composition remains manual verification.

### Task 5: Document and verify

**Files**

- `context/lib/ui.md`
- generated SDK reference listing engine game-state handles

**Work**

- Document the SDK-authored development HUD.
- Document fallback shadowing.
- Document `player.healthFraction` and its static range.
- Document that `player.health` keeps dynamic range attachment.
- Document removal scope without weakening `screen.flash`.

**Verification**

- `cargo fmt --all --check`
- focused scripting, slot, registry, retained UI, and integration tests
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace`
- normal debug launch with mod HUD registration
- debug launch with mod HUD registration omitted to inspect fallback

## Sequencing

**Prerequisite:** `game-state-runtime-handles` ships before Task 1.

1. Task 1: extract declarations before schema changes.
2. Task 2: establish the typed health contract.
3. Task 3: author the HUD against that contract.
4. Task 4: validate the headless cross-subsystem path.
5. Task 5: document and manually verify GPU composition.

## Acceptance Criteria

- Development HUD is authored in TypeScript with UI SDK factories.
- `setupMod()` returns `{ name, tree, alwaysOn }` UI envelopes and a mod theme.
- Reticle uses `Text({ content: "+", font: "mono" })`.
- HUD reads `player.health` and `player.healthFraction` through generated handles.
- Controlled snapshots update health text, bar fraction, and eventual style band.
- Fraction bar and style ranges use max `1`.
- Mod `hud` shadows the fallback-only marker; `hud.reticle` composes separately.
- Theme tokens affect colors, gap, padding, and placement. Bar dimensions are not claimed as theme-driven.
- Headless coverage bundles TypeScript into a temporary mod root, runs mod init, composes `always_on_layers`, and builds retained draw data per layer.
- Persistent `ScriptRuntime` remains valid; only the ephemeral mod-init context drops.
- Generated typedef drift is covered by its separate generation test.
- GPU encode and render composition are verified manually.
- Fake ammo, intro flash state, obsolete intro stores, and HUD flash swatches are removed.
- `screen.flash` and `FlashDecay` remain supported.
- `player.health` keeps dynamic range attachment during install and hot reload.
- Minimal fallback loads and renders in a focused fixture and debug manual launch without mod HUD registration.
- No tracked `content/dev/start-script.js` deliverable is added.
- No new `unsafe` code or renderer ownership violation is introduced.

## Open Questions

- Invalid health maximum policy remains unresolved. Production health descriptors reject non-finite or non-positive `max`, but the publisher still needs a defensive policy if invalid runtime data reaches it. Decide that policy before implementation; do not infer it from clamping.
