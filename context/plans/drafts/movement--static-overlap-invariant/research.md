# movement--static-overlap-invariant — research

Read at b0a5c0f. Read-only audit; symbols cite crates/physics unless noted.

## The vault does not set position

- `vault_intent` (movement/intents/vault.rs) writes only `velocity = (next - carried) / dt`. It rebuilds the path from the payload every tick, and `integrate_collision` sweeps the result. The exit is velocity-only too.
- `vault_entry` (movement/vault.rs) sets `start_offset` so the path starts at the current feet. It does not move the pawn.
- `settle_broken_vault` (crates/foundation/src/movement/player_movement.rs) resets state and velocity only. An off-path abort past `BLOCKED_TOLERANCE` (0.1 m) leaves the pawn at its last swept pose, often hugging the lip at the skin band.
- `probe_ledge`: one down ray reads the top height. Clearance comes from two full-capsule sweeps: up to rest height plus skin, then forward to the landing point. `lip_allowance` keeps path ticks inside the probed volume (test `the_lip_allowance_keeps_the_capsule_in_the_probed_volume_and_a_skin_off_the_lip`).
- Reconcile (crates/netcode/src/reconcile.rs) restores the host Transform and Vault payload as a consistent pair and replays through swept motion. It cannot create an embed, only carry one that the host already has.

## Bypasses in `integrate_collision` and friends (movement/substrate.rs)

Ranked by likelihood of the observed vault embed:

1. **Free advance on a second TOI-0 walkable contact.** Once the per-tick floor push budget is spent, the code does `current_pos += component.velocity * remaining_dt; break;`.
   - How it is reached: projection zeroes `velocity.y`. Against a tilted walkable normal (a convex lip edge with n.y between `cos_walkable` and 1, or a ramp), horizontal motion closes on the surface again. That gives TOI 0 (push), then TOI > 0, then TOI 0 (free advance).
   - Depth: about `r − (r + s)·n.y` per occurrence, scaling with horizontal speed. That is a few cm at r 0.2, about 13 cm at r 0.4.
   - Persistence: ground-stick from the embedded pose hits at TOI 0 with a walkable normal (drop 0, grounded). Flat motion across the top face has rate 0, so no hit. Only separating motion exits, which matches "walked out of it".
   - This breaks the skin-cast contract's 0.5 mm band (`entity_model.md` §7).
2. **Ground-stick centre-ray fallback** (`floor_below`): when the swept down-cast's first hit is non-walkable, a thin ray from the capsule centre finds the floor. The pawn drops by the ray distance (up to `max_down`) without a sweep, ignoring geometry beside it. Near a ledge of about step height this can sink the capsule's side a few cm into the riser. It can fire on a vault tick that keeps the grounded flag.
3. **`step_up_lift`**: grounded with a wall-like hit ahead, it lifts `step_up_limit + 2·SKIN_DISTANCE` with no overhead sweep. The follow-up probes start inside geometry and report no hit for non-closing motion, so the lift commits into an overhang. That puts the head in a ceiling.
4. **Mover carry** (`apply_mover_carry`, top of `integrate_collision`): not checked against static geometry. Matters only when the ledge is a mover. Mover policy, out of scope.
5. **Floor push** (one skin plus nudge, budgeted once per tick): negligible (~0.1 mm into a ceiling at worst).

6. **Stuck-stop rollback** (`integrate_collision`, corner-wedge deadzone): restores the start X/Z but keeps the new Y, which can embed on a ramp (from `/validate-plan`).

## Agent controller copies

`crates/sim/src/agent/mod.rs` is a separate kinematic controller (`collide_and_slide`) with its own `step_up_lift`, which has no overhead sweep, and its own `ground_stick`, which uses a centre-ray `cast_ray` drop by ray distance. These are two of the player-side bypasses. `movement.md` §7 records that enemies use their own controller.

## Reusable checks

- `capsule_fits_combined` (collision/mod.rs): exact `intersection_test` of the capsule against the static trimesh, mover penetration, and core points not in solid cells. Only teleport arrival uses it (movement/teleport.rs).
- `capsule_static_placement_center`, `sphere_fits_world`.
- There is no static penetration-depth query. `deepest_mover_penetration` covers movers only.
- There is no start-solid flag. The skin cast blocks closing motion at TOI 0, allows tangent or separating motion, and falls back to parry's cast when the capsule core pierces a triangle. That is why a player can walk out but is never pushed out.

## Existing tests

- No test asserts that a tick never ends overlapping static geometry.
- `the_eye_and_capsule_stay_clear_of_the_lip_and_the_headroom_along_the_path` (vault_tests.rs) is an analytic distance check on the nominal path only.
- The "…without_persistent_overlap" tests in the movement module cover movers only.
