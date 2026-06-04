# Research notes — movement--cross-cutting-policies

Code anchors confirmed against current source while drafting. Ephemeral — for the implementer/reviewer to verify fast; not durable architecture.

## Tick dispatch + transition seam
`crates/postretro/src/movement/mod.rs`:
- `tick(component, input, collision_world, gravity, dt, position) -> (Vec3, MovementEvents)` at `:990`.
- State-intent dispatch (the `Option<MovementState>` seam): `:1005–1010`. `Normal => normal_intent(...)`; `Dash { elapsed_ms, boost } => dash_intent(component, input, gravity, dt, elapsed_ms, boost)` — payload destructured here and threaded as params (the copy-out D7 removes).
- Substrate call: `integrate_collision(...)` at `:1015–1022`.
- Landing-refresh: `:1028–1030` `if substrate.hit_floor { component.refresh_on_landing(); }`.
- **Transition application (Task 2 extends this):** `:1040–1042` `if let Some(next_state) = transition { component.movement_state = next_state; }`. No carry-rule today.
- Cooldown decrement, unconditional, outside dispatch: `:1048–1050`.

## State payload threading (D7 target)
- `MovementState` enum: `player_movement.rs:25–42`. `Normal` (default), `Dash { elapsed_ms: f32, boost: Vec3 }` (`:41`). Derives `Copy`.
- `dash_intent(component, input, gravity, dt, elapsed_ms, boost) -> Option<MovementState>` at `mod.rs:811`. Payload passed by value, mutated locally (boost reconcile `:881–890`, elapsed accumulate `:937`), re-packed `MovementState::Dash { elapsed_ms, boost }` at `:950`. Copy-out/copy-back is forced because `movement_state` lives on `component` and the intent also takes `&mut component` — the borrow D7 resolves once.
- `Normal`→`Dash` decided in `try_enter_dash` (`:737`, returns `Some(Dash{...})` `:795–798`). `Dash`→`Normal` in `dash_intent` exit test `:946–948`.

## Input forgiveness raw material (Task 3 / D5)
- `MovementInput` at `mod.rs:76–89`: `wish_dir: Vec2`, `jump_pressed: bool` (`:78`), `dash_pressed: bool` (`:83`), `running: bool`, `facing_yaw: f32`. No buffering/coyote anywhere.
- `jump_pressed` consumed RAW: grounded jump `:606`, air-jump `:617` — on the tick it's true. These steps move to consuming the *derived* edge.
- `air_ticks` (the airborne-duration signal coyote keys off): updated in `integrate_collision` `:547–554` — `0` when grounded, `saturating_add(1)` when airborne; `landed` gated on `prev_air_ticks >= 3`. Counts up from ground-loss, resets on contact. **No "recently grounded" grace flag today** — coyote needs a window check against this plus a "ground-jump spent" flag.
- `is_grounded` write (step 8): `:533–539`. Jump branch clears `is_grounded` in `normal_intent` `:608`.
- Input derivation upstream — `main.rs`: `jump_pressed = snapshot.button(Action::Jump).is_active()` (`:791`, level signal); `dash_pressed = matches!(snapshot.button(Action::Dash), ButtonState::Pressed)` (`:796`, rising edge). `MovementInput` constructed in `run_movement_tick` `:1836–1842`; event mapping `:1860–1865`.

## Component + landing refresh
`player_movement.rs`:
- `PlayerMovementComponent` `:44–100`: `capsule/ground/air/fall` (`:46–49`), `cos_walkable` (`:54`), `dash: Option<DashParams>` (`:58`), `is_grounded`, `velocity`, `air_jumps_remaining`, `air_dashes_remaining` (`:66`), `dash_cooldown_ms` (`:71`), `air_ticks` (`:77`), stuck-stop fields, `movement_state` (`:99`). Forgiveness timers/flags add here.
- `from_descriptor` `:106–129` (precompute site for materialized windows). `refresh_on_landing` `:136–143` (resets `air_jumps_remaining`, `air_dashes_remaining`).

## Substrate result (D8 — deferred)
- `SubstrateResult` `mod.rs:104–114`: only `hit_floor` (`:110`), `landed` (`:113`). **No contact normal.**
- `cast_capsule` (`collision/mod.rs:130–156`) returns parry `ShapeCastHit` with `normal2` (world-space surface normal) + `time_of_impact`. Call sites in `mod.rs`: `:169,:180,:204` (step-up), `:315` (slide), `:476` (ground-stick). Normals computed and discarded internally (`last_wall_normal` `:292` is tick-local). D8 = surface these forward in the result, no ad-hoc intent casts.

## Descriptor → SDK emission (Task 3 surface)
- Register: `scripting/primitives/mod.rs` `register_shared_types` (`:17`); movement types `PlayerMovementDescriptor` `:208`, `GroundParams` `:238`, `AirParams` `:252`, `FallParams` `:263`, `DashParams` `:272`. Add the forgiveness type alongside + an optional field on `PlayerMovementDescriptor`.
- Type-name maps: `scripting/typedef.rs` `:118–124` (TS) / `:211–217` (Luau). Generated files `sdk/types/postretro.d.ts` / `.d.luau`. Drift test `typedef.rs:1886+` (byte-equality vs registry; regen via `gen-script-types`).
- Parsers: JS + Luau in `data_descriptors.rs`, symmetric validation (`validate_non_negative_finite` etc.). Optional-with-defaults precedent: `stuck_stop_enabled`/`stuck_stop_threshold` (`:222–223`).

## Governing design intent
- `movement.md` §6: momentum conservation + input forgiveness are both foundation-level, settle before the states; "settle the policy and seam, not the full breadth."
- `movement.md` §2: closed carry-rule / trigger vocabularies; engine owns the evaluator; author surface (transition graph) firms up across the series.
- Roadmap M11: next plan after `movement--state-machine` is "Cross-cutting movement policies" (momentum + forgiveness); slide "owns and consumes" the momentum policy; wall-run is "first environment-probe state."
