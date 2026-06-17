# Runtime Level Lifecycle

## Goal

Make level load and unload a repeatable runtime operation. Today the engine is single-level-per-process: install runs once, unload coincides with process exit (`boot_sequence.md` §3–§5). This plan lets the engine boot to a no-level **Frontend** state, load a chosen map, unload back to Frontend, and load another — with no residue and no GPU leaks between loads. It is the foundation the mod-defined frontend hub (`drafts/mod-frontend-hub`) sits on.

This deliberately overturns the documented single-level-per-process invariant — `boot_sequence.md` §5 ("level install runs exactly once per process — there is no runtime path to swap levels") and the §6 non-goal "runtime level-to-level transition / level swap mid-process." Reconciling those docs is part of this plan (Task 5).

This plan ships only the engine *mechanism*. No menu UI, no `defineMod`, no authoring surface — those are the feature plan on top.

## Scope

### In scope

- Two new `BootState` states: **Frontend** (renderer + UI run, no world installed) and **Loading** (worker-poll + install for a requested map). Keep `Booting`/`Splash`/`Running`.
- Decouple content-root resolution from the map arg so the engine boots to Frontend with no map. CLI map arg becomes optional: when present, auto-load it (preserves today's dev flow; `boot_sequence.md` §7 records that today the map is a CLI argument and the engine boots straight into it, with a mod-supplied menu/level-selector planned).
- **Level unload**: tear down per-level engine and GPU state; keep renderer, window, script runtime, and engine-global registries. Decoupled from process exit.
- Repeatable install: `install_level_payload` becomes re-runnable with zero residue between loads.
- Reuse the level worker for every load request, not just boot.
- Renderer releases and rebuilds per-level GPU resources (textures, geometry buffers) across the cycle. Renderer owns this.
- A queued **`LevelRequest`** mechanism — `enum LevelRequest { Load(LevelSource), Unload }`, with `enum LevelSource { Path(PathBuf) }` (dev-path arm only; `mod-map-catalog` adds a `Catalog(MapId)` arm behind this same seam) — that the boot state machine drains, driving Frontend↔Loading↔Running.
- Camera reset on unload (no stale pose leaks into the next level).
- A debug-only trigger to exercise the cycle (proves the mechanism without the menu).

### Out of scope

- Menu UI, `defineMod`, the `frontend` manifest block, menu camera, background-level semantics — all `drafts/mod-frontend-hub`.
- Authoring surface for load/unload (the `loadLevel` reaction, `ui.quitToMenu`) — feature plan.
- Multiple simultaneously-resident levels; streaming/async beyond the existing single-worker model.
- Mid-level save-game persistence beyond today's best-effort slot save on exit.
- Networked or mid-level hot-swap.
- Splitting all of `main.rs` — only the level-lifecycle seam is extracted (Task 1).

## Acceptance criteria

> Automated (CPU- or review-gated) unless marked **(manual)** — a runtime/visual gate the harness can't assert.

- [ ] Engine launched with **no map argument** boots to Frontend: no panic, runs the steady loop; **(manual)** it paints a world-less clear-color frame.
- [ ] A load request transitions Frontend→Loading→Running with the requested map installed; **(manual)** the level is playable.
- [ ] An unload request transitions Running→Frontend: the level world, per-level GPU resources, collision world, fog, light bridge, level sounds, sprite collections, active wieldable, per-level reactions and crossings, and progress tracker are gone, and the camera pose is reset; the engine keeps rendering the frontend.
- [ ] Loading a second, different map after an unload installs cleanly — a CPU test asserts no first-map entries/lights/fog remain in the registries; **(manual/harness)** no wgpu validation errors across the cycle (run with WGPU validation on).
- [ ] Slot-table values and the entity-type registry are unchanged before and after an unload (engine-global survival) — proven by a CPU test that snapshots them via their existing read paths (slot-table `get`/`iter`, the `entities` accessor) and asserts equality. (That the unload path holds no reference to either is a code-review assertion, not a runtime metric.)
- [ ] Launching **with** a map argument routes Booting→Splash→Loading→Running (no Frontend frame) — the Loading-hop state trace is headless-observable; **(manual)** the boot is visually equivalent to today (dev flow does not regress).
- [ ] A failed load logs a diagnostic and lands in Frontend — for **both** failure shapes (worker `Err`, and `Ok(LevelPayload{ level: None })` for a missing file); the one exception is a failed **boot CLI map**, which **(manual/integration)** exits non-zero (fail-loud — no menu to fall back to).
- [ ] No new `unsafe`; every GPU release/rebuild lives in the renderer module.

## Tasks

### Task 1: Extract the level-lifecycle state machine from `main.rs`

`main.rs` is 5798 lines and this plan extends its hottest seam. Split first, behavior-preserving: create `crates/postretro/src/startup/lifecycle.rs` (alongside the existing `startup/mod.rs` (`BootState`) and `startup/worker.rs`) and relocate the level-lifecycle seam into it. The exact set to move: `install_level_payload` (main.rs:3461, incl. its head reset block at :3465), the worker-poll currently inside `run_splash_frame` (main.rs:2706; poll block ~2880–2923), the per-level teardown subset of `suspended()` (main.rs:1161-1164 — fog/collision/wieldable clears, NOT the renderer/window drops), the boot-state dispatch `match self.boot_state` (main.rs:1528), the transition assignments (main.rs:1136/1174/2923), and the constructor default (main.rs:530). No behavior change; pure relocation. Later tasks add the `Frontend`/`Loading` `BootState` variants, the `FRONTEND_CLEAR_COLOR` const, and the `LevelRequest` queue into this same module — so Tasks 2-5 edit `startup/lifecycle.rs`, not `main.rs`. Sequence this before everything else.

### Task 2: Boot to a no-level Frontend state

Add the `Frontend` `BootState` variant. Decouple content-root resolution from the map arg: `content_root_from_map` (main.rs:139) and `resolve_map_path` (main.rs:131) gain a no-map path — when no map and no `--content-root`/mods argument is given, default to `content_root_from_map(DEFAULT_MAP_PATH)` (the same root today's dev map resolves, main.rs:103) so sibling texture/script resolution is unchanged; do NOT fall back to `.`/empty. Establish the Splash-exit branch *point*: with no map arg, boot Booting→Splash→**Frontend** and render a world-less frame. (The with-map auto-load — enqueue + enter Loading — is wired in Task 4, once the `LevelRequest` queue exists; this task owns only the no-map→Frontend path and the world-less render guard.) The render path must tolerate `self.level == None`: a guard in the existing frame entry skips the world/visibility/camera-in-world passes **and** the gameplay-tree / slot-bound HUD updates (which read per-level slots absent in Frontend and would panic or read stale), recording only the frontend-safe UI pass + present. The renderer exposes a clear-color parameter that lifecycle code passes `FRONTEND_CLEAR_COLOR` into (a named const this plan defines in `startup/lifecycle.rs`; the renderer does NOT import a main-crate const — renderer-owns-GPU); the menu-camera plan later owns the color's value.

### Task 3: Level unload

Add an unload operation distinct from `suspended()` (which drops the renderer; unload must not). Mirror the per-level subset of `suspended()`'s teardown (main.rs:1161-1164) — fog bridge, collision world, active wieldable (+ descriptor) — and additionally:
- tear down the **light bridge** — add a `LightBridge::clear` or reassign `= LightBridge::new(...)` (it has no clear method today — net-new; `suspended()` does not touch it);
- tear down **sprite collections** in both stores — reuse `particle_render.reset_for_level()` (already exists, main-side) and add a renderer release for smoke collections (`register_smoke_collection` has no release path — net-new);
- clear `self.level` and the per-level progress tracker;
- release level sounds (`audio.release_level_sounds`, already exists);
- reset the camera to `Camera::new(Vec3::ZERO, 0.0, 0.0)` (no `Default` exists; the menu-camera plan later owns the real frontend pose — parallel to `FRONTEND_CLEAR_COLOR`);
- call `data_registry.clear()` (reactions + crossings; scripting/data_registry.rs:134).

Add a renderer entry point that drops per-level GPU resources to no-level state — needed only for the unload-to-Frontend case; the load→load path already overwrites `vertex_buffer`/`index_buffer`/`loaded_textures` via `install_level_geometry`/`install_textures`, with `clear_mesh_pass_for_level_load` (main.rs:3395) as prior art. Keep `script_ctx`, the slot table (no clear method by contract — scripting/slot_table.rs), the entity-type registry (`data_registry.entities`), window, and renderer device/queue. The clear-on-unload audit table **is** the durable contract — produce it inline here:

| Cleared on unload | Kept across unload |
|---|---|
| `self.level` (LevelWorld) | renderer device/queue, window |
| per-level GPU resources (textures, geometry) | `script_ctx`, `ScriptRuntime` |
| light bridge, fog bridge, collision world | slot table (no clear method — engine-global) |
| level sounds, sprite collections | entity-type registry (`data_registry.entities`) |
| `data_registry` reactions + crossings | persisted-state save path |
| progress tracker, active wieldable, camera pose | |

### Task 4: Repeatable load + Loading state + worker reuse

Add the `Loading` `BootState` and the `LevelRequest` queue (`Load(LevelSource)` / `Unload`; `LevelSource::Path(PathBuf)` is the only arm here — `mod-map-catalog` adds `Catalog(MapId)`). Generalize the worker-poll from `run_splash_frame` (main.rs:2706; poll block ~2880–2923) into a reusable Loading-state poll, and spawn `spawn_level_worker` (worker.rs:36) per request. Drain the queue each frame: load from Frontend → Loading → Running; unload from Running → (Task 3) → Frontend; load from Running (level→level) → **explicit unload then Loading** (one teardown path). Complete Task 2's Splash-exit branch: with a boot map arg, enqueue a `Load` and enter Loading (Booting→Splash→Loading→Running). Make `install_level_payload` re-runnable: the head reset block (main.rs:3465-3477) covers decays/trackers/wieldable, but audit the **append-style** state across the whole function — `light_bridge` absorb (~main.rs:3777), smoke/sprite-collection registration (~3637/3807) — so a second run carries no residue; on level→level this relies on Task 3's unload running first.

**Failure handling — two shapes, both route to Frontend.** The worker signals a parse error as `Err(e)` and a *missing file* as `Ok(LevelPayload{ level: None })` (worker.rs ~60-69; today's poll has a separate None branch at main.rs:2932 that stays in Splash). Both log a diagnostic and land in Frontend, except a failed **boot CLI map** exits non-zero (fail-loud — no menu to fall back to). Tag the boot load with a one-shot `boot_load: bool` lifecycle field, set once before the boot enqueue and cleared the moment that load resolves (success *or* either failure shape), so only the boot load is fatal.

### Task 5: Debug trigger, tests, manual verification

Add a debug-only key trigger (behind the `dev-tools` feature, like the codebase's other dev triggers) that enqueues unload then a load of a hardcoded second dev map (e.g. the campaign-test sibling) — exercises the full cycle without the menu. Add CPU tests (note `install_level_payload` early-returns without a renderer, setting `self.level` but skipping GPU, so tests assert CPU-side state only): (a) slot-table + entity-registry survival across unload (snapshot via `get`/`iter` + the `entities` accessor, assert equal); (b) residue-free reinstall — install fixture map A, unload, install fixture map B, assert no A-only entries remain in the CPU registries (`data_registry` reactions, the light-bridge entity list, fog/collision state); name the two fixture maps. GPU residue and "no wgpu validation errors / clean visuals" are a manual/harness gate, not a CPU test. Manual launch checklist: no-map boot to Frontend; trigger load→unload→load-different; confirm no GPU validation errors and clean visuals. At promotion, reconcile `boot_sequence.md` (§3–§6 and the §5 lifetimes table) to describe the repeatable load/unload lifecycle — replacing the single-level-per-process invariant and the level-swap non-goal — and record the clear-on-unload contract there (or as a doc-comment on the unload entry point).

## Sequencing

**Phase 1 (sequential):** Task 1 — extraction unblocks every later edit to the lifecycle seam.
**Phase 2 (sequential):** Task 2 — establishes the Frontend state and world-less render path the rest builds on.
**Phase 3 (sequential):** Task 3, then Task 4 — both rewrite the extracted lifecycle module and share its files; unload (3) is the teardown half, repeatable-load (4) the load half and consumes unload for level→level.
**Phase 4 (sequential):** Task 5 — trigger + tests exercise the completed cycle.

## Rough sketch

- **State model:** `BootState { Booting, Splash, Frontend, Loading, Running }` (startup/mod.rs:16). Transitions currently at main.rs:1136/1174/2923 (plus the boot-state dispatch `match self.boot_state` at main.rs:1528 and the constructor default at main.rs:530) move into the extracted module (Task 1).
- **Unload ≠ suspend.** `suspended()` (main.rs:1147) drops renderer + window and resets to `Booting` for surface rebuild; unload keeps both and lands in `Frontend`. They share the per-level clear subset but differ on renderer ownership and target state.
- **Clear-on-unload contract (the durable artifact):**

  | Cleared on unload | Kept across unload |
  |---|---|
  | `self.level` (LevelWorld) | renderer device/queue, window |
  | per-level GPU resources (textures, geometry) | `script_ctx`, `ScriptRuntime` |
  | light bridge, fog bridge, collision world | slot table (no clear method — engine-global) |
  | level sounds, sprite collections | entity-type registry (`data_registry.entities`) |
  | `data_registry` reactions + crossings | persisted-state save path |
  | progress tracker, active wieldable, camera pose | |

- **Worker:** `spawn_level_worker(map_path, content_root, sender) -> JoinHandle` (worker.rs:36), delivering `LoadOutcome = Result<LevelPayload, Error>` (worker.rs:33); `LevelPayload { level, prm_cache_root, timings }` is `Send` POD (worker.rs:16). Reused per request.
- **Render path** must guard `self.level.is_none()` in Frontend: skip world/visibility/camera-in-world passes, clear to `FRONTEND_CLEAR_COLOR` (named const seam), still record the UI pass and present.

## Open questions

- None open. Level→level routing is resolved: explicit unload-then-load, for a single teardown path (Task 4).
