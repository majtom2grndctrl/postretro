# Production Gameplay HUD Through the UI SDK

## Goal

Replace the active development HUD with a production-oriented HUD authored through the TypeScript UI SDK, and use that HUD as an end-to-end validation of the SDK path from TypeScript source through compilation, runtime registration, state binding, theme resolution, registry shadowing, layout, and rendering.

The engine-owned JSON HUD remains only as a minimal fallback for launches that do not register a mod HUD. It is not the primary implementation.

## User Experience

- The normal development launch shows a compact bottom-left health panel and a centered text reticle.
- Health text displays the player's current health.
- A health bar displays normalized health and responds to live health changes.
- HUD colors and spacing come from mod-provided theme tokens.
- The previous demo ammo value, intro color flash, and screen flash swatches are removed.
- A launch without a mod-provided HUD still has a small engine fallback health display.

## Architecture

### SDK-authored active HUD

Add `content/dev/scripts/hud.ts` as the production HUD definition. It imports SDK factories from `"postretro"` and typed engine state from `"postretro/game-state"`.

The module builds two `AnchoredTreeDescriptor` values:

- `hud`: bottom-left health status, registered with `alwaysOn: true`.
- `hud.reticle`: centered text reticle, registered with `alwaysOn: true`.

Using two trees is intentional: each anchored tree has one viewport anchor, so status and reticle can independently target bottom-left and center without viewport-sized spacer layout.

The health tree uses:

- `Tree`, `VStack`, `HStack`, `Text`, and `Bar` SDK factories.
- `player.health.get()` for current-health text.
- `player.healthFraction.get()` for the normalized bar.
- SDK tween/style-range features for visible state-driven feedback.
- custom theme tokens for HUD text, accent, panel, bar background, padding, and spacing.

The reticle uses `Text("+")` with the built-in mono font. This avoids introducing a gameplay image-loading dependency while still validating a second independently anchored SDK tree.

`content/dev/start-script.ts` imports the HUD builder and returns its trees and theme from `setupMod()`:

```ts
return {
  name: "Development Content",
  entities,
  uiTrees: hud.uiTrees,
  theme: hud.theme,
};
```

No hand-authored JSON descriptor is used for the active development HUD.

### Registry replacement behavior

Keep the engine tree name `hud`. The mod-provided SDK tree with the same name relies on the existing `UiTreeRegistry` tier precedence to shadow the engine fallback. `hud.reticle` is an additional always-on mod tree.

This deliberately validates both registration outcomes:

- replacement of an engine tree by name;
- composition of an additional always-on tree.

### Engine state surface

Replace the demo-oriented `StaticUiProxy` with a narrowly named player HUD state publisher.

It publishes:

- `player.health`: current health, preserving the existing typed handle.
- `player.healthFraction`: `current / max`, clamped to `[0, 1]`.

Both slots are engine-owned, read-only numbers. `player.healthFraction` declares the range `[0, 1]`. If there is no player pawn or health component, the publisher skips the write and preserves the current stale-value behavior.

Remove:

- constant fake `player.ammo`;
- timer-driven `intro.flashColor`;
- elapsed-time and missing-player warning state used only by those demos.

Before extending the player slot table, extract the engine-owned slot declarations from the oversized `slot_table.rs` into a focused sibling module. This keeps declaration growth out of an already large file without changing slot semantics.

Regenerate the committed TypeScript and Luau game-state bindings so `player.healthFraction` is available through the typed SDK import.

### Engine fallback

Reduce `content/base/ui/hud.json` to a minimal fallback that binds only to supported player health state. Remove all demo ammo and flash content.

The fallback exists for engine-only launches and mods that do not provide a HUD. The normal development launch must prove that the SDK-authored `hud` shadows it.

## End-to-End Validation

Add a focused cold-launch regression around the actual development TypeScript entry point:

1. Bundle `content/dev/start-script.ts` with `postretro_script_compiler::bundle_entry`.
2. Place the bundled JavaScript in a temporary mod root.
3. Create `ScriptRuntime`, install the SDK prelude, and run `setupMod()` through `run_mod_init`.
4. Confirm `run_mod_init` returns before registrations are drained, preserving the no-resident-VM lifecycle.
5. Drain `uiTrees` and `theme` from the parsed manifest.
6. Register an engine fallback `hud`, then register the mod trees at `ScopeTier::Mod`.
7. Merge the mod theme.
8. Publish health and health fraction values.
9. Build gameplay UI draw data and inspect the resulting primitives.

The regression asserts:

- the manifest contains SDK-created `hud` and `hud.reticle` trees;
- both are always-on;
- the mod `hud` shadows the engine fallback marker;
- the additional reticle tree renders;
- health text reflects the bound current-health slot;
- bar fill reflects the normalized-health slot;
- custom theme tokens resolve into draw colors and dimensions;
- no demo ammo or intro-flash content appears;
- the VM does not remain resident after initialization.

Keep lower-level parser, registry, and renderer tests. Update or remove tests whose only purpose is the old raw-JavaScript/demo-HUD behavior. The end-to-end test must begin with TypeScript SDK source, not a hand-built Rust descriptor or raw registration JSON.

## Boundary Inventory

| Boundary | Producer | Consumer | Contract |
| --- | --- | --- | --- |
| `player.health` | player HUD state publisher | generated SDK handle and HUD text | read-only number |
| `player.healthFraction` | player HUD state publisher | generated SDK handle and HUD bar | read-only number in `[0, 1]` |
| `ModManifest.uiTrees` | `setupMod()` in TypeScript | `run_mod_init` manifest parsing | array of named anchored tree descriptors |
| `ModManifest.theme` | SDK HUD module | runtime theme merge | partial theme token maps |
| `hud` tree name | engine fallback and mod SDK HUD | `UiTreeRegistry` | mod tier shadows engine tier |
| `hud.reticle` tree name | mod SDK HUD | `UiTreeRegistry` | additional always-on overlay |
| bundled development entry | script compiler | `ScriptRuntime` | SDK imports lower to runtime globals |

## Tasks

### Task 1: Split engine slot declarations

**Files**

- `crates/postretro/src/ui/state/slot_table.rs`
- new sibling module under `crates/postretro/src/ui/state/`

**Work**

- Move engine-owned slot declaration construction into a focused module.
- Preserve current declarations and validation behavior exactly.
- Keep `slot_table.rs` responsible for table mechanics rather than the growing engine schema.

**Tests**

- Existing slot declaration and table tests remain green.
- Add a focused declaration test only if the extraction exposes behavior not already covered.

### Task 2: Publish production HUD state

**Files**

- `crates/postretro/src/scripting/systems/ui_proxy.rs`
- `crates/postretro/src/main.rs`
- engine slot declaration module from Task 1
- generated TypeScript and Luau game-state bindings
- binding-generation tests or snapshots

**Work**

- Rename/refocus `StaticUiProxy` as a player HUD state publisher.
- Continue publishing `player.health`.
- Add clamped `player.healthFraction`.
- Remove fake ammo, intro-flash timing, and demo-only warning state.
- Add the new engine slot declaration and regenerate typed SDK handles.
- Keep `main.rs` changes mechanical: field/type/call-site updates only.

**Tests**

- Current and fractional health publish correctly.
- Zero or invalid maximum health cannot produce a non-finite fraction.
- Missing player/health preserves the established skipped-write behavior.
- Generated TypeScript and Luau bindings expose `player.healthFraction`.

### Task 3: Author and register the HUD through the UI SDK

**Files**

- new `content/dev/scripts/hud.ts`
- `content/dev/start-script.ts`
- committed generated `content/dev/start-script.js`
- `content/base/ui/hud.json`
- remove obsolete intro-store TypeScript/Luau files

**Work**

- Build health and reticle trees exclusively with SDK factories.
- Bind them through generated typed player handles.
- Define and return custom HUD theme tokens.
- Register both trees and the theme from `setupMod()`.
- Simplify the engine JSON HUD to a supported minimal fallback.
- Remove obsolete intro-store setup and generated output.

**Tests**

- Script compiler coverage confirms the production entry bundles.
- Manifest parsing confirms both tree registrations and the theme payload.
- Existing JSON loader coverage confirms the fallback still loads.

### Task 4: Add the SDK cold-launch rendering regression

**Files**

- scripting/runtime integration tests
- UI lifecycle/render integration tests, or a focused cross-module test fixture
- `crates/postretro/Cargo.toml` if the script compiler must be added as a dev dependency

**Work**

- Exercise the full TypeScript-source-to-draw-data path described above.
- Use the real development entry point so the regression covers the shipped SDK HUD definition.
- Prove mod-tier shadowing and additional-tree composition.
- Prove theme and typed state bindings affect rendered primitives.
- Retain lower-level tests while deleting obsolete demo-only assertions.

**Tests**

- The new regression is the primary test for the acceptance criteria.
- Run focused scripting and UI test suites during implementation.

### Task 5: Document and verify the production path

**Files**

- `context/lib/ui.md`
- any generated SDK reference that lists engine game-state handles

**Work**

- Document that the development HUD is the canonical SDK-authored example.
- Document the engine fallback and tier-shadowing behavior.
- Document `player.healthFraction` and its normalized range.
- Verify normal development launch and engine-fallback launch manually.

**Verification**

- `cargo fmt --all --check`
- focused scripting, UI state, registry, and render tests
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace`
- launch `cargo run -p postretro -- content/dev/maps/campaign-test.prl` and inspect the SDK HUD
- launch without development mod HUD registration and inspect the engine fallback

## Sequencing

1. Task 1: split declarations before extending the engine schema.
2. Task 2: establish and generate the typed health state contract.
3. Task 3: author the SDK HUD against that contract.
4. Task 4: validate the complete cold-launch path.
5. Task 5: document the proven behavior and run final verification.

## Acceptance Criteria

- The active development HUD is defined in TypeScript with UI SDK factories.
- `setupMod()` returns SDK-created `uiTrees` and a mod theme; it does not register the active HUD from JSON.
- The HUD reads health through generated typed game-state handles.
- Current health and normalized health update the rendered text and bar.
- The mod `hud` shadows the engine JSON fallback, while `hud.reticle` composes as a second always-on tree.
- Custom theme tokens visibly affect rendered HUD primitives.
- The TypeScript compiler, SDK runtime, manifest parser, VM lifecycle, registry, theme merge, state bindings, layout, and renderer are covered by one cold-launch regression.
- Fake ammo and intro-flash demo state and content are removed.
- The minimal engine JSON fallback still loads and renders without mod HUD registration.
- No new `unsafe` code or renderer ownership violation is introduced.

## Open Questions

None.
