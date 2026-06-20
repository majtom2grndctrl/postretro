# Phase 0 — Grounded seam map

> Investigation behind the spec (`index.md`). Line-numbered against current source —
> drifts as the files change; the spec captures the durable decisions. Companion to the
> milestone seam map in `context/research/netcode/research.md` §6 (this is the
> Phase-0-specific extraction detail).

## The tick loop (`main.rs` 1905–2025)

`for _ in 0..ticks { ... }` inside the `WindowEvent::RedrawRequested` handler. In order:

| Line | Call | Role | Headless? |
|---|---|---|---|
| 1913 | `registry.snapshot_transforms()` | order 0: copy current→previous transform | pure |
| 1915–1975 | inline input-intent resolution | axes/sprint + jump/dash/crouch edges → `MovementInput` args; reads `player_options.crouch_mode`, `crouch_toggle_active` | pure (input-layer) |
| 1942–1955 | fly-cam branch (no pawn) | writes `camera.position` directly | render-side |
| 1977–1985 | `run_movement_tick(...)` | movement state machine | pure |
| 1991–2008 | camera-follow (has pawn) | writes `camera.position = pawn + eye` | **render-side leak** |
| 2011 | `run_weapon_fire_tick(snapshot, dt)` | weapon fire + hitscan | pure |
| 2020 | `run_death_sweep()` | zero-HP sweep | pure |
| 2023–2024 | `frame_timing.push_state(...)` | interpolation ring | **render-side leak** |
| 2028–2063 | event drains + `dispatch_system_commands()` | fire named events → enqueue system commands → dispatch | pure tick; **dispatch tail leaks** |

## The per-tick wrappers (`impl App`, main.rs)

- **`run_movement_tick`** (3545): `fn(&mut self, forward_axis, right_axis, jump_pressed, dash_pressed, crouch_intent, running, tick_dt) -> Vec<&'static str>`. Reads `script_ctx.registry` (RW), `script_ctx.gravity` (R), `collision_world` (R), `camera.yaw` (R, pure look). Iterates `PlayerMovementComponent` entities, calls `movement::tick(...)`.
- **`run_weapon_fire_tick`** (3616): `fn(&mut self, snapshot: &ActionSnapshot, tick_dt) -> Vec<&'static str>`. Reads `registry` (RW), `active_wieldable` (R), `&camera` (R — for `aim_ray()`), `collision_world` (R), `hit_zone_store` (R), `anim_time` (R). Calls `weapon::tick(...)`.
- **`run_death_sweep`** (3685): `fn(&mut self) -> Vec<String>`. Reads `registry` (RW), writes `progress_tracker`. Calls `scripting_systems::health::sweep_deaths(...)`.

## Key finding: `Camera` is GPU-free

`Camera` (`camera.rs:86`) = `{ position: Vec3, yaw, pitch, aspect }`. `aim_ray()` (137), `forward()`, `right()` are pure math, **no wgpu**. So the camera couples the tick only as *data*. The seam takes the per-tick **aim** (origin + yaw/pitch) and **facing_yaw** as resolved input — sampled from the local camera on the host, supplied by the wire command on the server. Camera-*follow* (writing `camera.position` from the pawn) and `aspect`/`update_aspect` (window size) are render-side, after the seam.

## The leaks to sever (the only non-pure reaches)

1. **`frame_timing.push_state`** (2024) — render interpolation; shifts current→previous **every tick**. Stays **per-tick in the caller loop** (not the seam), or interpolation breaks on multi-tick catch-up frames.
2. **Camera-follow** (2003) + **fly-cam** (1954) — render-side, **per tick** in the caller; camera-follow feeds the position `push_state` snapshots.
3. **`dispatch_system_commands`** (3391–**3524**): audio/gamepad/UI reaches — `PlaySound → audio` (3395), `Rumble → gamepad` (3415), `PushTree/PopTree/ReturnToFrontend → modal_stack/frontend` (3453/3466/3469). The pure (game-state) arms are `FlashScreen`, `Vignette`, `ScreenShake`, `SetState`, `CellWrite`, `AppendText` (3497), `BackspaceText` (3508), `ClearText` (3516), `LoadLevel`, `RestartLevel`. **Phase-0 decision:** the seam neither fires events nor dispatches commands — it **returns the tick's event names**; the caller accumulates them across ticks and, post-loop, fires them and runs the existing `dispatch_system_commands` **unchanged** (the audio/UI gating partition for a headless server is a later phase).

Everything else the tick touches is pure: `script_ctx.{registry, gravity, system_commands, data_registry}`, `collision_world`, `hit_zone_store`, `anim_time`, `active_wieldable`, `progress_tracker`, the reaction/sequence/system registries, `player_options.crouch_mode`, `crouch_toggle_active`. (`registry` is `Rc<RefCell<EntityRegistry>>` per `ctx.rs:24`.)

## `movement/mod.rs` split seams (logic ≈ 1,875 lines; tests 1910–6055)

`movement/` is **already a directory** (`mod.rs` 6,055 ln, `carry.rs`, `scope.rs`); the
split adds `substrate.rs`/`intents.rs`/`dispatch.rs` from `mod.rs`'s content and leaves
`carry.rs`/`scope.rs` alone. Maps onto the `movement.md` §4 substrate / intent / dispatch seam:

| Lines | Section | Split target |
|---|---|---|
| 1–80 | header + tuning constants | stays / substrate config |
| 86–145 | `MovementInput`, `MovementEvents`, `SubstrateResult`, `Transition` | **public API** (`mod.rs`) |
| 151–270 | `pm_accelerate`, `wish_dir_from_input`, `step_up_lift` | **substrate** |
| 271–646 | `integrate_collision` (376 ln, collide-and-slide core) | **substrate** |
| 647–718 | `resize_capsule`, `standup_clearance_probe` | **substrate** |
| 719–828 | `air_jump_ready`, `derive_jump_edges`, `advance_forgiveness` | substrate/utility |
| 829–1022 | `normal_intent` (194 ln) | **intents** |
| 1023–1073 | `resolve_number`/`resolve_bool` (IR eval) | utility (intents) |
| 1075–1206 | `try_enter_dash` | **intents/dispatch** |
| 1207–1416 | `dash_intent` (210 ln) | **intents** |
| 1417–1624 | `crouching_intent` (208 ln) | **intents** |
| 1625–1709 | standup/decay/boost-carry helpers | **dispatch** |
| 1746–1795 | `dispatch_state_intent` (50 ln) | **dispatch** |
| 1797–1875 | `pub(crate) fn tick` (79 ln, orchestrator) | **public API** |
| 1910–6055 | `#[cfg(test)] mod tests` (~72 cases) | co-locate or sibling (exempt from size rule) |

`movement::tick`'s public signature stays unchanged across the split (behavior-preserving).

## Existing harness pattern (reuse for the determinism test)

No headless server entry; `crates/postretro/tests/` holds only `fixtures/` (gltf assets), no integration `.rs` harness. `bin/` holds only `gen_script_types.rs`. The movement test block (1910+) already ticks game logic with **no App/window/GPU**: build a `PlayerMovementDescriptor` → `PlayerMovementComponent::from_descriptor` → `CollisionWorld` from a parry3d `TriMesh` → call `movement::tick(...)` directly with `DT = 1.0/60.0`. The determinism harness extends this to the full seam: registry + spawned player/weapon entities + `HitZoneStore` + a recorded input-command stream, ticked through `simulate_tick`. Tests run with no GPU (`testing_guide.md` §3); sandbox uses `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test`.
