# Mod-Defined Frontend Hub

> Prerequisites: `done/runtime-level-lifecycle` (load/unload cycle, Frontend/Loading states), `in-progress/mod-map-catalog` (`defineMod` + the map catalog this plan's level-select and `loadLevel` read), and `in-progress/reaction-composition` (mod-global reaction tier — so death/level-flow reactions are declared once, not per level). Builds on the manifest-return, staged-reload, and tiered-registry contracts established by `in-progress/production-gameplay-hud` and `in-progress/production-pause-menu`.

## Goal

Let a mod declare its frontend through a `frontend` manifest block (on the `defineMod` builder already in source): which menu UI shows at start, an optional background level behind it, and where the camera sits during menus. Add a `--mod` boot flag so a launch selects a mod (not a map) and enters its frontend, a catalog-driven level-select, and wire the bidirectional `Frontend ⇄ Running` transitions (start game, quit to menu, died→menu, level complete) on top of the runtime level lifecycle.

## Scope

### In scope

- **`frontend` block on the `defineMod` manifest** — `defineMod`, `ModManifest`, and the `maps` catalog are already in source (`sdk/lib/data_script.ts:190`); this plan adds the `frontend` field and its types to the manifest the builder already types.
- **`--mod <path|id>` boot flag:** select which mod the engine loads and enter its frontend (no map arg). Replaces the implicit fixed mod root as the boot handle; a bare map-path argument remains the dev raw-path bypass (`LevelSource::Path`, owned by `mod-map-catalog`) that loads straight into a map under the selected mod. Mod-browser UI and persisted last-mod selection stay out of scope.
- **`frontend` manifest block:** `{ menuTree, backgroundLevel?, camera }`. `menuTree` is a UI-registry name; `backgroundLevel` is a **catalog id** (resolved via `mod-map-catalog`). Drained into engine state like `uiTrees`/`theme`; inherits staged-reload semantics.
- **Frontend population:** present the mod's `menuTree` — resolve it through the UI registry and push it as a capturing modal, apply the menu camera pose, and suppress player control. With a declared `backgroundLevel`, the menu is shown over that single loaded level; without one, over the world-less `Frontend` fallback.
- **Menu camera as a declared pose:** position + yaw + pitch applied to the engine camera while the menu is presented. **Static pose only** — a declared/animated camera orbit is out of scope (see below). Not a general runtime camera-scripting API.
- **Engine-default frontend fallback:** a minimal built-in menu when no mod registers one (mirrors the `hud`/`pauseMenu` fallback tier), so debug/no-map boot is usable.
- **Catalog-driven level-select:** the frontend reads the existing map catalog (`in-progress/mod-map-catalog`) to list/filter loadable maps by `name`/`tags` and start one via `loadLevel(map)`. Pre-load discovery — no map is loaded to populate the list.
- **Game-flow vocabulary (the exposed options):** a small closed set of engine-owned system reactions the SDK exposes — `loadLevel(map)` (carries a **catalog id**, resolved via the existing `maps` catalog to a path; queued/drained like `openMenu`/`showDialog`, `scripting.md` §10.4), `restartLevel()` (reload the active map via its retained `LevelSource`), and `returnToFrontend()` (unload → Frontend). Each drives a lifecycle load/unload request. The typedef is the contract, and the set is **designed to grow** (see out of scope).
- **Player-death handling — author's choice from the vocabulary:** the engine fires the existing `playerDied` event (scripting/systems/health.rs:15) and bakes no death policy. The mod binds it to whichever game-flow verb it wants (`restartLevel()` for the simple case) or to `openMenu(tree)` for a death screen whose buttons invoke the verbs. The mod-global death reaction (declared once in `defineMod({ reactions })`, optionally scoped per mode) is **gated on `in-progress/reaction-composition` landing**; until that plan ships, per-level binding via `setupLevel().reactions` is the available path. Level-complete uses the same path via an `onStateCrossing` watcher (there is no built-in `levelComplete` event).
- **Quit-to-menu button:** reserved argument-less `ui.quitToMenu` action (mirrors `ui.exitToDesktop`) — calls `enqueue_level_request(LevelRequest::Unload)` directly (the shared sink), parallel to how `ui.exitToDesktop` triggers shutdown directly, rather than dispatching the `returnToFrontend` reaction. Engine-fallback menus quit without any registered reaction.
- **Background-level behavior:** the frontend can declare a `backgroundLevel` that plays behind the menu — a live scene (animations/particles run) with player control suppressed and the camera at the menu pose. Only one level is ever loaded: the declared backdrop *is* that single loaded level, presented under the menu as a capturing modal (the engine is in `Running`, but the player-facing experience is the frontend). Starting a real map swaps the one loaded level (unload backdrop → load map); quitting back to the menu reloads the backdrop. When no `backgroundLevel` is declared, the world-less `Frontend` state (clear-color) backs the menu as the fallback.

### Out of scope

- **Future game-flow verbs** — `respawnAtCheckpoint()` and `loadLastSave()`. Saving and checkpoints are not implemented; the game-flow vocabulary is the deliberate seam they slot into later (extend the closed set, no redesign). Do not implement them here — but the vocabulary's shape must not foreclose them.
- Options/settings menu and `settings.toml` mutation — deferred to a dedicated settings plan. The menu may carry an "Options" button whose action is wired later; `PlayerOptions` is untouched here.
- Attract-mode rotation / multiple background levels.
- Rich level-select presentation — thumbnails, descriptions, campaign-graph / next-map auto-advance. Basic catalog-driven listing and tag filtering is in scope; richer metadata extends the catalog later.
- A camera path editor or general runtime camera scripting beyond the static pose.
- **Camera orbit / animated menu camera.** Deferred to a later menu-polish / attract-mode plan; this plan ships the static pose only. (Attract-mode rotation is already out of scope above.)
- **Mod-browser UI and persisted mod selection.** The `--mod` flag is the only mod-selection surface here; an in-engine mod browser and remembering the last-played mod are deferred (filed under `boot_sequence.md`'s future mod-selection item).
- The level load/unload *mechanism* itself (`runtime-level-lifecycle`).
- A built-in `levelComplete` event (mods compose it from `onStateCrossing`).

## Acceptance criteria

- [ ] The `frontend` block is accepted on the `defineMod` manifest and `gen-script-types` emits its declarations with the typedef-drift test passing. TS and Luau call-site type correctness is covered by a committed `tsc`-clean fixture (manual gate), not a CI Rust test — matching the `mod-map-catalog` convention.
- [ ] Launched with `--mod <path|id>` and no map arg, the engine loads that mod and presents its `menuTree` with the camera at the declared pose. If a `backgroundLevel` is declared it is the single loaded level rendered behind the menu; if not, the world-less `Frontend` fallback backs the menu.
- [ ] `--mod` selects the loaded mod; a bare map-path argument still loads that map directly (dev bypass) under the selected mod. No mod-browser UI or persisted selection exists (out of scope).
- [ ] Without any mod frontend, the engine-default fallback menu appears in Frontend (debug boot).
- [ ] A menu button bound to `loadLevel(map)` with a catalog id transitions Frontend→Running with that map (the id resolves via the catalog, loads via the lifecycle); the background level is unloaded with no residue.
- [ ] The level-select lists the catalog's maps by `name`, filterable by `tags`, without loading any of them; starting one routes through `loadLevel(map)`.
- [ ] The SDK exposes `loadLevel`, `restartLevel`, and `returnToFrontend` as bindable game-flow verbs. `restartLevel()` reloads the active map by re-enqueuing its retained `LevelSource`; `returnToFrontend()` unloads to the menu; a `ui.quitToMenu` button calls `enqueue_level_request(LevelRequest::Unload)` directly (same shared sink, no registered reaction required).
- [ ] Binding the `playerDied` event to a game-flow verb executes it on death (e.g. bound to `restartLevel()`, death reloads the active map); the engine bakes no default death policy.
- [ ] Frontend suppresses player controls and releases the cursor (capture-mode path), and the static menu camera holds the declared pose — it does not snap to a player spawn.
- [ ] No animated/orbiting menu camera ships; the static declared pose is the sole camera behavior while the menu is presented (orbit deferred to a later plan).
- [ ] Staged reload of `frontend` follows the established boundary: a successful current staged result replaces the frontend block whole; failed/stale results preserve the prior one; omission reverts to the engine fallback.
- [ ] CPU tests cover `defineMod` identity/round-trip, `frontend` drain, staged replace/omit, fallback reveal, and `loadLevel(map)`/`ui.quitToMenu` routing. Manual launch verifies boot→menu→start→play→quit→menu→start-again.
- [ ] No new `unsafe`; no renderer ownership violation; no tracked generated bundle.

## Tasks

### Task 1: `frontend` manifest block + types

Add the optional `frontend` field to the `ModManifest` type in both typedef blocks (`TS_SDK_LIB_BLOCK` typedef.rs:680, `LUAU_SDK_LIB_BLOCK` typedef.rs:1774) and declare the `Frontend` and menu-camera types. `defineMod`, `ModManifest`, and `maps` are already in source; this plan only extends the manifest with the `frontend` field. Add SDK-parity and typedef-drift coverage for the new field.

### Task 2: Drain the `frontend` block into engine state

Add `frontend: Option<Frontend>` to `ModManifestResult` (runtime.rs:56) and `StagedManifest` (staged_manifest.rs:57). Drain it in `run_mod_init_quickjs` (runtime.rs:1485, beside `drain_ui_trees_js`/`drain_theme_js` ~1629–1638) and `run_mod_init_luau` (runtime.rs:1709). Commit it at the same successful-staged boundary as UI trees and theme. A structurally-invalid `frontend` field aborts mod-init like `maps`/`theme`/`reactions` (the drain callers set `out = Err(...); return;`); only sub-field degradation (e.g. a single bad `camera` sub-field) is logged-and-skipped.

### Task 3: Mod boot handle (`--mod` flag)

Add a `--mod <path|id>` CLI argument that selects which mod the engine loads, replacing the implicit fixed mod root as the boot handle; `run_mod_init` runs against the selected mod and (Task 2) drains its `frontend` block. With `--mod` and no map argument, boot proceeds Splash → `Frontend` and presents the mod's menu (Task 4). A bare map-path argument remains the dev raw-path bypass (`LevelSource::Path`) that loads straight into `Running` under the selected (or default) mod. Out of scope: mod-browser UI and persisted last-mod selection.

### Task 4: Frontend population + menu camera

Present the committed `menuTree`: resolve it through the UI registry (`ModalStack`, modal_stack.rs) and push it as a capturing modal; fall back to the engine-default menu when absent (new tier entry alongside `hud`/`pauseMenu` fallbacks). Apply the declared static camera pose by writing `self.camera.{position,yaw,pitch}` (camera.rs:86) — the same fields `install_level_payload` writes at spawn (startup/lifecycle.rs:638; camera writes at ~921-924), so no new camera plumbing. Suppress player control via the capture-mode path (`reconcile_ui_focus` main.rs:3502). Camera orbit is out of scope — static pose only.

Background level — only one level is loaded at a time. If `backgroundLevel` is declared, enqueue its load through the lifecycle request queue: the engine enters `Running` with that single level resident, and the capturing menu modal + suppressed control + menu camera make it the player-facing frontend (the backdrop *is* the loaded level, not a second world). If no `backgroundLevel` is declared, the menu is shown over the world-less `Frontend` state (clear-color, no level installed — lifecycle.rs:151-155). Either way the background-level concept is preserved; it is realized as loaded-level-plus-menu-overlay, never as a level coexisting with `Frontend`.

### Task 5: Game-flow vocabulary and transitions

Add the game-flow verbs as `SystemReactionCommand` variants (`system_commands.rs`, beside `PushTree`/`SetState`), registered in `register_system_reaction_primitives` (main.rs:384) and drained in `dispatch_system_commands` (main.rs:3130, which holds `&mut self`) — the same queue/drain path as the `openMenu`/`showDialog` (`PushTree`) system reactions (`scripting.md` §10.4):

- `loadLevel(map)` → `LoadLevel { map }` → arm calls `self.enqueue_level_request(LevelRequest::Load(LevelSource::Catalog(map)))` (catalog id resolved via the existing `maps` catalog).
- `restartLevel()` → `RestartLevel` → arm re-enqueues the **retained active `LevelSource`**. Retain it at install (mirror `retain_active_level_tags_for_install`, lifecycle.rs:434) so restart works for both catalog and dev raw-path loads without needing a catalog id.
- `returnToFrontend()` → `ReturnToFrontend` → arm calls `self.enqueue_level_request(LevelRequest::Unload)`; if the mod declared a `backgroundLevel`, re-enqueue its load so the menu returns over the backdrop.

Add the reserved `ui.quitToMenu` button action — a constant in `render/ui/actions.rs` (beside `EXIT_TO_DESKTOP_ACTION`), a `UiButtonAction` variant (main.rs:1030), classification in `classify_ui_button_action` (main.rs:1037) and routing in `route_ui_button_action` (main.rs:1069) to the same `enqueue_level_request(LevelRequest::Unload)` sink `returnToFrontend` uses, plus SDK constants (`QUIT_TO_MENU_ACTION`) in `sdk/lib/ui/reactions.{ts,luau}` — so engine-fallback menus quit without a registered reaction. Wire the death path: the existing `playerDied` event (scripting/systems/health.rs:15) binds to any game-flow verb (or to `openMenu` for a death screen) through the existing reaction registration; document the simple-restart and death-screen patterns, plus level-complete via `onStateCrossing`. Keep the game-flow set open for future `respawnAtCheckpoint`/`loadLastSave` — no implementation, no foreclosure.

### Task 6: Tests, docs, manual verification

CPU coverage: `defineMod` round-trip and import-no-FFI; `frontend` drain + staged replace/omit/fallback; `loadLevel`, `restartLevel` (re-enqueues the retained `LevelSource`), and `ui.quitToMenu` routing as pure logic; `--mod` argument parsing/selection. Manual launch checklist: `--mod <campaign>`→menu (camera pose, background level, suppressed controls)→start→play→quit-to-menu→menu→start-again; restart-on-death; no-mod fallback menu. At promotion, update `boot_sequence.md` (Frontend/Loading states, the `--mod` boot handle, the hub flow), `scripting.md` (`defineMod`, `loadLevel`/`restartLevel`/`returnToFrontend`, `frontend` manifest block), and `ui.md` (`ui.quitToMenu`, frontend fallback tier).

## Sequencing

**Dependency note:** The mod-global death-reaction pattern requires `in-progress/reaction-composition` (`defineMod({ reactions })` level-scoping). Until that plan lands, per-level binding via `setupLevel().reactions` is the available path. This plan can ship ahead of `reaction-composition`; the global death-reaction wiring is additive.

**Phase 1 (sequential):** Task 1 — defines the `frontend` manifest shape the rest consumes.
**Phase 2 (sequential):** Task 2 — drains and commits that shape; consumes Task 1's type.
**Phase 3 (sequential):** Task 3 — the `--mod` boot handle; lets a launch reach the selected mod's frontend.
**Phase 4 (sequential):** Task 4 — presents Frontend from the committed block; consumes Task 2 and Task 3.
**Phase 5 (sequential):** Task 5 — transitions in/out of Frontend; consumes Task 4 and the prereq's load/unload requests.
**Phase 6 (sequential):** Task 6 — tests, docs, manual verify.

## Boundary inventory

| Name | Rust | Wire / serde | TS | Luau | Notes |
|---|---|---|---|---|---|
| `defineMod` (already in source) | n/a (SDK only) | n/a | `defineMod()` | `defineMod()` | this plan adds the `frontend` field it types |
| frontend block | `Frontend` (parsed) | `"frontend"` | `frontend` | `frontend` | optional manifest field |
| menu tree ref | registry name lookup | `"menuTree"` | `menuTree` | `menuTree` | resolves through `ModalStack` |
| background level | catalog id → `PathBuf` | `"backgroundLevel"` | `backgroundLevel` | `backgroundLevel` | optional; resolved via the existing `maps` catalog |
| menu camera | `Camera{position,yaw,pitch}` writer | `"camera"` | `camera` | `camera` | static pose (orbit out of scope) |
| load verb | `loadLevel(map)` system reaction | `{name:"loadLevel",args:{map}}` | reaction name | reaction name | `map` is a catalog id (NOT a `ui.*` action) |
| restart verb | `restartLevel` system reaction | `{name:"restartLevel"}` | reaction name | reaction name | reload the active map via its retained `LevelSource` |
| return verb | `returnToFrontend` system reaction | `{name:"returnToFrontend"}` | reaction name | reaction name | unload → Frontend |
| quit button | `UiButtonAction::QuitToMenu` | `"ui.quitToMenu"` | `QUIT_TO_MENU_ACTION` | `QUIT_TO_MENU_ACTION` | reserved, argument-less; calls `enqueue_level_request(LevelRequest::Unload)` directly |
| mod boot handle | `--mod <path\|id>` CLI arg | n/a | n/a | n/a | selects the mod to load; bare map-path = dev bypass |

## Script syntax examples

```ts
// Proposed design
import {
  defineMod, defineReaction,
  loadLevel, restartLevel, returnToFrontend, openMenu,
} from "postretro";
import { buildMainMenu } from "./ui/main-menu";

// Launch into this mod's frontend (no map arg):
//   cargo run -p postretro -- --mod content/mods/my-campaign
export function setupMod() {
  return defineMod({
    name: "My Campaign",
    entities,
    theme,
    // maps: [...] — already on the manifest; classifies levels and feeds level-select
    uiTrees: [
      ...buildMainMenu(),               // registers "mainMenu" tree
      // hud, pauseMenu, ...
    ],
    frontend: {
      menuTree: "mainMenu",
      backgroundLevel: "menu_backdrop",  // a catalog id (see mod-map-catalog)
      camera: { position: [4, 2, 8], yaw: -0.6, pitch: -0.1 },  // static pose (orbit out of scope)
    },
  });
}

// A menu "PLAY" button's onPress names this reaction:
export const startCampaign = defineReaction({
  name: "startCampaign",
  steps: [loadLevel("e1m1")],   // catalog id, resolved via the existing maps catalog
});

// On death, the mod picks from the game-flow vocabulary. Simplest:
export const onDeath = defineReaction({
  name: "playerDied",           // bound to the engine's playerDied event
  steps: [restartLevel()],      // or: [returnToFrontend()]
});
// Or show a death screen whose buttons invoke "restartLevel" /
// "returnToFrontend" (named reactions) instead:
//   steps: [openMenu("deathScreen")]
// Future verbs slot in here: respawnAtCheckpoint(), loadLastSave().
```

## Open questions

- `frontend` in `StagedManifest` — **resolved**: include it. The `fonts`-omitted precedent is real but this plan opts into the staged lane — camera/menu iteration benefits from hot reload. Task 2 and the staged-reload AC are the authority (staged_manifest.rs:57).
- Reaction scope for death/level-flow — **resolved**: handled by `in-progress/reaction-composition` (mod-global reaction tier + per-reaction level-tag scope). The death reaction is declared once in `defineMod`, optionally scoped per mode. This plan consumes that tier.
