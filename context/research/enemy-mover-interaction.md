# Research — enemies vs. kinematic movers (block, crush, ride, sweep)

Code read at worktree `codex/teleporters` @ `75d91fbbb`. Every file cited below is identical on `main` unless marked **[branch]**. Prior-art claims are from id/Valve source on GitHub; I re-checked Doom `p_map.c:1298` and Q2 `g_func.c:65,412,1083–1092` myself. Unreal/Unity are doc-level only.

## 1. Current state, end to end

### Host tick order (`crates/sim/src/sim/mod.rs`)

| # | Stage | Mover relevance |
|---|---|---|
| 1 | `run_kinematic_mover_tick` (:563) | Advances every mover, then publishes `MoverTickStateTable`: end-of-tick transform, `tick_delta`, `tick_rotation_delta`, velocities and translation legs (`physics/src/kinematic_mover.rs:160–235`). It publishes movers at rest too, so a closed door has a pose. |
| 2 | Player movement (:595 `CombinedCollisionWorld::new(collision_world, mover_colliders, mover_tick_states)`, :608) | Players move against static and mover geometry. |
| 3–5 | Triggers, touch, projectiles | All of these see static geometry only. |
| 6 | `run_ai` (:842) | Gets `collision_world: Some(collision_world)`, which is static only. |
| 8 | `agent_steering::tick(registry, collision_world, nav_graph, env, dt)` (:871) | Static only, **although `mover_colliders` and `mover_tick_states` are in scope three lines later (:885–886)**. |
| 8b | `run_mover_blocking_pass` (:903) | Host-only policy decisions for players **and agents**. |

### Player vs. movers (predicted and reconciled)

`physics/src/movement/substrate.rs::integrate_collision` handles a mover in this order:
1. **Carry** (:186 → `mover_carry.rs:22 apply_mover_carry`). It revolves the body by the mover's tick rotation about the mover pivot, then adds `tick_delta`. It keys off the previous tick's `GroundRef::Mover(id)`.
2. **Displace** (:187 → `mover_carry.rs:55`). It pushes the body out along the deepest mover push-penetration and settles for up to 4 iterations. It refuses when `mover_push_is_blocked_by_static` (:130) says static geometry is in the way, and leaves the body overlapping. That is the `displace` policy: the mover never yields.
3. **Sweep** against combined static+mover geometry with `cast_capsule_query`, plus step-up and ground-stick. Ground is set from the hit source (`ground_ref_from_hit`, :272).
4. **Release velocity** when the body leaves a mover (:452).

Crush is not in the substrate. It is decided in the blocking pass.

### Blocking pass (`physics/src/kinematic_mover/blocking.rs`)

- **Already iterates agents** (`agent_capsules` :589, agent loop :285–325).
- **Stop/Reverse:** any leading-sweep contact with an agent stops or reverses the mover. There is no rider exemption for agents. The player rider exemption and its 150 ms grace are player-only (:223–238, `RIDER_GRACE_MS` :27).
- **Crush:** for an agent, *any* push-penetration counts as pinned. The comment at :305 says "Agents never take the player-only mover-displacement path." So today a crush mover moving through an enemy in open air damages it. A player in the same spot is displaced, and is only crushed when pinned against static geometry.
- **Displace** (default, `BlockPolicy::Displace` `#[default]`, `entities/src/components/kinematic_mover.rs:22`) skips the pass entirely, so enemies are ignored.
- **Damage:** goes through `apply_damage_with_context(... "mover.crush", InTick)` with a per-(mover, victim) cadence, and continues past death (an E17 decision).

### Enemies today

- Enemies are custom-kinematic capsules, not rigid bodies. They are `AgentComponent` (`entities/src/components/agent.rs`) with `radius`, `height`, `step_height`, `velocity`, `knockback_velocity` and `is_grounded: bool` (:78). There is no ground reference.
- `agent_steering.rs::tick_impl`:
  - Takes a start-of-tick separation snapshot, skipping quiescent (zero-HP) agents (:355–359).
  - Admits replans under a budget of 4 per tick with 30-tick staleness.
  - Computes steering and adds stuck-recovery tangent bias after 20 stuck ticks.
  - Calls `collide_and_slide(collision_world, …)` (:587, idle path :438).
- `sim/src/agent/mod.rs::collide_and_slide` (:96) handles gravity (downward only), step-up, a 4-iteration slide and ground-stick. **Every cast is `cast_capsule(world, …)` against the static trimesh.**
- The navmesh excludes movers: "Mover brushes are not present in … navmesh" (`build_pipeline.md` §KinematicGeometry). `find_path(graph, start, goal)` (`sim/src/nav/path.rs:83`) has no filter or cost hook. `NavGraph.region_components` (`nav/mod.rs:87`) is built once at load.
- **Confirmed:** enemies walk through closed doors, fall through or stand inside lifts, and are never carried. A `reverse` or `stop` door already reacts to an enemy standing in its sweep. A `crush` mover damages any enemy it overlaps.
- **Adjacent gap, same root:** AI LOS (`crates/ai` gets static `CollisionWorld`), hitscan, projectiles and splash (`docs/scripting-reference.md:549`) all ignore movers. Enemies see and shoot through closed doors.
- **Teleport:** fires inside the player movement step only (`movement/teleport.rs` **[branch]**). Agents never teleport, which confirms the premise.

### What is reachable at the steering call site

At the steering call site there are:
- `mover_colliders: &[MoverCollider]`
- `mover_tick_states` (`MoverPoseSource`, this tick's end pose and delta)
- the static world

`cast_capsule_combined` and `cast_ray_combined` are public (`collision/moving.rs:185, 266`). They include a per-mover bounding-sphere broadphase (a linear scan with an exact cull).

**Visibility wall:** `CombinedCastHit.source` is `pub(crate)` (`moving.rs:41`). So are `deepest_mover_push_penetration`, `MoverPenetration` and `mover_push_is_blocked_by_static`. `apply_mover_carry` and `displace_from_movers` are `pub(super)` to `movement`. `development_guide.md` puts "movement, kinematic movers, and … mover collision" in `postretro-physics`. **The agent body substrate should move down into physics rather than physics widening its API.**

**[branch] `point_inside_mover`** (`collision/moving.rs`, used at `collision/mod.rs:554`) exists because contact queries see triangles only: "a shape wholly inside a hull reports none." This applies directly to a fast or thick crusher that swallows an enemy in one tick. Today that case yields no displace and no crush.

## 2. Prior art

| Behaviour | Quake 1/2/3 | Doom | Source | Recast/Unreal/Unity |
|---|---|---|---|---|
| Door blocks NPC | Door is `SOLID_BSP`; `SV_movestep` traces include it; monsters bump (Q1 `sv_move.c:138`) | Closed door means ceiling=floor; the line blocks | Solid door. NPCs open `prop_door` only; `OnDoorBlocked` is an empty FIXME (`ai_basenpc.cpp:12334`) | Detour toggles poly flags (`setPolyFlags`) plus a filter, and `checkPathValidity` replans. Unity carves an obstacle. Unreal uses `NavArea_Null` / smart links |
| Crush | Pusher moves, pushes each overlapping or riding entity, and if the result is stuck it unwinds and calls `blocked` → `T_Damage(dmg)`, then reverses unless crusher (Q2 `DOOR_CRUSHER`, `g_func.c:1092`). Q2 gives non-monsters 100000 damage and gibs them. Q1 `SV_PushMove` sv_phys.c:439–555 | `PIT_ChangeSector`: 10 dmg every 4 tics (`p_map.c:1298`); corpses gib; normal doors reopen, `close` doors keep pressing | `CBaseDoor::Blocked`: `dmg` → `DMG_CRUSH`; `forceclosed` never reverses | — |
| Ride | `groundentity` link; riders always pushed (Q1 L487). Q2 "if ok to leave in old position, do it" | Sector floor re-clip (`P_ThingHeightClip`) | Ground entity plus `NotifyPushMove` to the navigator | Based movement (Unreal `MovementBase`) |
| Nav across lift | Monsters can't call lifts; they only ride one the player summoned | Lift is just a moving sector | Scripted; L4D `ConnectElevators` (`#ifdef TERROR`) | Off-mesh link with a wait state; Detour leaves the traversal to the game |
| Horizontal sweep | Same push path (Q2 `train_blocked`) | None | Trace-based push | — |

**What fits PostRetro.** The player already follows the Quake model:
- Persistent ground ref, carry by delta, push out of overlap, host-side `blocked` policy, damage through a chokepoint.
- Q2's mover-side push with an undo stack doesn't fit, because carry is body-side and *predicted* for players.
- The right move is **one body-side mover-contact substrate (carry → displace → combined sweep → ground ref) shared by player and agent**, with the existing host blocking pass as the `blocked` callback.

**For nav,** every engine pairs physical blocking with a nav-layer gate (Detour flags, `func_nav_blocker`, `NavArea_Null`). The Source FIXME loop is what happens without one. Lifts are always off-mesh links with board/ride/exit, and never a moving navmesh.

## 3. Navigation options

| Need | Options | Recommendation |
|---|---|---|
| Closed door | (a) local avoidance only, (b) runtime gate of nav portals the mover overlaps, (c) bake-split regions at mover footprints plus a per-region gate attachment, (d) costed door link with a wait-at-door state | **First slice (b+region):** load-time, map each mover to the candidate portals and regions inside its swept-path AABB (inflated by agent radius). Per tick, a region or portal is gated when the mover's current-pose AABB overlaps its band `[floor+step_height, floor+agent_height]`. That rule makes a lift top resting at floor level walkable and a full-height door blocking. `find_path` takes a `&NavGate` filter. A gate generation bump admits replans as drift-priority under the existing budget. **Later (c)** if the measurement in §7 shows door footprints landing inside large regions, which would over-block. **(d)** is a feel upgrade. |
| Lift / ferry | (a) none: the upper landing stays disconnected, so `blocked` and the enemy holds; (b) "mover links" derived at load from each terminus top face to the adjacent regions within step height; (c) baked link records (an additive NavMesh portal kind, per `build_pipeline.md` §Navigation bake "future portal kinds") | (a) in slice 1. That is safe because riders still physically ride. (b) in Phase 3, promoted to (c) only if load cost shows up. |

**Off-mesh link state machine.** The link is traversable to board only while the mover is docked at the boarding terminus. While it is moving, the agent is in Ride: steering is suspended, the agent keeps its footing, and stuck detection is paused. On docking at the far terminus, the agent exits and replans.

Also required:
- `region_components` become a static *superset* once gates exist. The blocked-destination admission clause must consult the gate, or it will burn budget retrying.
- Agents riding off-mesh fail `region_at`. Keep the existing stale-but-moving and `blocked` semantics, and suppress stuck recovery while `ground == Mover`.

## 4. Networking and determinism

- **Host only.** Agent carry, displace and blocking stay host-authoritative. Clients have no `Agent` and never simulate enemies (`networking.md:1081`). There is no change to mover prediction and no change to the static-content digest: the nav gate derives from data already hashed.
- **Presentation (the real wire question).** Remote enemies are interpolated in the **past** (`render_server_tick`). Movers are **predicted in the present** (`networking.md:1071`). An enemy riding a 3 m/s lift therefore renders about RTT/2 + interp delay off the lift, which is 30–50 cm at a 150 ms RTT.
  - The draft `context/plans/drafts/E17--mover-relative-rider-presentation/` solves this for remote **players** by rebasing delayed riders onto the render-time mover, using `WirePlayerMovementState.ground`.
  - Enemies replicate only `Transform` + `MeshAnimationState` (`E10--networked-enemy-authority-baseline`), so they need **one new field: the enemy's ground mover** (reuse `WireGroundRef`). That means a `WIRE_VERSION`/`SNAPSHOT_VERSION` bump.
  - The draft's rebase is then generalized from "remote player" to "remote ground-attached entity". This is the same idea as Unreal's replicated based movement (unverified) and Source's ground entity.
- **Hit validation is unaffected.** It is world-LOS-only (`networking.md:919`), so a client aiming at the rebased pose is safe.
- **Crush death on clients:** already handled by replicated despawn plus the impact animation.
- **Ordering pin:**
  - Mover tick (1) → … → agent steering (8): carry → displace → combined sweep → ground ref.
  - Then the blocking pass (8b), with the agent rider exemption.
  - AI (6) reads pre-carry agent poses, one tick behind the mover. This is acceptable and should be documented, not fixed.
  - The carry must use the stage-1 table. Trigger commands only mutate phase for the *next* tick, so the table stays valid through stage 8.
- **Determinism risk:** `pinned_crush_contacts` is a std `HashSet` iterated to apply damage and push `Crushed` events (`blocking.rs:203, 360`). With several crushed victims, the order of damage, events and `on_impact` follows `RandomState`. Fix it in slice 1: sort, or use an ordered collection. Add an agent-on-mover case to `sim/determinism_tests`.

## 5. Performance

| Item | Figure |
|---|---|
| Fixtures | `stress-env-volumes`: 12 movers (11 `reverse`) and 64 agents. `stress-warren-showcase`: 8 movers and 12 enemies. `closet-reveal`: 1 door and 2 enemies. `campaign-test`: 1 `crush` and 2 `reverse`. |
| Current cost | `perf-ai-tick-cost` draft: `sim_steer_agents` 2.5 ms at 64 agents with about 10k particles in the registry; target ≤ 0.8 ms. The cost is registry iteration plus a linear `region_at`, not collision. |
| Added cost per agent per tick | About 8–10 combined casts (slide ×4, step-up ×3, ground-stick ×1–2) plus one displace penetration query, each with a sphere cull over N movers. 64 × 10 × 12 is about 7.7k sphere tests per tick. Narrow phase runs only near movers. Expect well under 0.1 ms; measure with the existing `sim_steer_agents` and `sim_steer_movers` scopes. |
| Allocations | New code must not allocate per tick: reusable gate bitset, precomputed per-mover candidate lists. Existing per-tick allocations stay out of scope: `AgentComponent` clone with `path: Vec` (draft `movement-tick-component-clone-alloc`), and the blocking pass's `Vec`/`HashSet`. |

## 6. Authoring surface

**Already exists:**
- FGD keys `block_policy` (`displace`|`reverse`|`stop`|`crush`, default `displace`), `crush_damage`, `crush_interval_ms`, plus events and sounds (`sdk/TrenchBroom/postretro.fgd:369–386`).
- Runtime `setBlockPolicy` (`sdk/types/postretro.d.ts:1763`).

The semantics carry over to enemies unchanged. **No new key is needed for slices 1–2.**

**Possible later keys:**
- `nav_gate` (`auto`|`never`), for decorative movers that should not gate nav.
- `nav_link` (`auto`|`none`), for lifts. Phase 3.

**Docs gap:** `docs/` has no mover or block-policy page at all (only the FGD hints). Add a `docs/level_design.md` movers section covering: what each policy does to players **and enemies**, that `displace` pins leave the body inside the mover (so use `crush`/`reverse`/`stop` for anything that can trap), that enemies don't call lifts, and how nav gating behaves.

## 7. Risks and edge cases

| Case | Today | Proposed rule |
|---|---|---|
| **Enemy rides a `reverse`/`stop` lift** | It already triggers the policy, because there is no agent rider exemption. As soon as carry lands, the lift will bounce off its own passenger. | Extend the ground-keyed exemption and grace to agents. This revises the `E17--rider-not-a-blocking-contact` scoping and its AC8. |
| **`crush` lift carrying an enemy through open air** | Damaged, because any overlap counts as pinned (`:305`). | Use the player's pin predicate, `mover_push_is_blocked_by_static`. |
| Door closing on an enemy mid-doorway | `reverse`/`stop` already react; `displace` ignores the enemy. | Matches the player. Under `displace` the enemy is now pushed out, or left inside if pinned. |
| Enemy wedged between two movers | — | `mover_push_is_blocked_by_static` ignores *other* movers, so two closing movers never "pin". This is the same gap the player has. Note it; don't fix it in slice 1. |
| Thick or fast crusher swallows the capsule | Triangle contact misses it | Fall back to **[branch]** `point_inside_mover` on the capsule axis points, treated as pinned. |
| Rotating mover | — | Position revolves like the player's. Yaw is not carried, because AI facing re-derives from steering (open: honour `carry_yaw`?). Nav-gate AABB comes from the transformed local AABB; add a local AABB to `MoverCollider`, since its bound sphere is too loose for a long door. |
| Lift leaves mid-step | — | Ground-stick reach (about 0.6 m per tick) keeps footing on a descender. A riser displaces the body upward and it boards. On an edge-teeter the enemy falls under gravity. |
| Corpses | Steering skips quiescent agents (`:359`), so a corpse is never carried. `agent_capsules` does *not* skip it, so a corpse in a `reverse` doorway holds the door open until despawn. | Owner decision (Q1 shrinks corpses, Doom gibs them). |
| Crushed enemy presentation | Mod impact policy (death animation or despawn). The engine adds nothing (E17). | Unchanged. |
| `closet-reveal` / teleporters fixtures | The sliding door uses `displace`; residents start passive (`75d91fbbb` **[branch]**). | Containment holds and gets stronger, since the door now physically blocks. One risk: a resident standing in the door's slide path is displaced on reveal. Keep passive residents; optionally add a test that an *aggro* resident is also held. |
| Doors in large nav regions | — | **Unknown:** whether the rectangular decomposition gives a doorway its own region or portal. Measure at the start of slice 2 on `closet-reveal` and `stress-warren-mini`. |
| Enemies seeing and shooting through closed doors | Yes | Out of scope here. Flag to the owner, because it undermines "wait at a closed door". |

## 8. Proposed approach

**Slice 1 — riskiest premise: agents can share the player's mover-contact substrate without steering or stuck-recovery fighting it** (door jitter, lift self-bounce, displace pin, stuck loop).

- Move `collide_and_slide` into `crates/physics/src/agent_motion/` (`mod.rs` slide, `mover_contact.rs`). Generalize `movement/mover_carry.rs` carry, displace and release into a body-agnostic `collision/mover_contact.rs` used by both callers. The player path stays behaviour-identical, gated by the existing movement and predict/reconcile harnesses.
- `AgentComponent.is_grounded: bool` becomes `ground: GroundRef` with an `is_grounded()` projection, renaming every consumer.
- Release velocity goes into the agent's knockback layer, so it decays with its own drag and isn't wiped by steering.
- Thread `CombinedCollisionWorld` into `agent_steering::tick` (`sim/mod.rs:871`).
- Blocking pass: agent tuples carry `GroundRef`, the rider exemption and grace generalize to agents, crush pinning uses the player predicate plus the inside-hull fallback, and crush iteration order becomes deterministic.
- Suppress stuck recovery while `ground == Mover(_)` and the mover is moving.
- `agent_steering.rs` (1233 lines) and `sim/mod.rs` (3963 lines) get only threading changes. New logic goes in sibling modules.

**Slice 2 — door nav gate.**
- Add `sim/src/nav/gate.rs`: load-time per-mover candidates, a per-tick reusable gate set, and a generation counter.
- `find_path` takes the gate filter.
- Admission treats a gate change as drift-priority, and `region_components` defers to the gate.
- Blocked agents hold position.

**Slice 3 — lift links** (load-time derived, plus the board/ride/exit state in `sim/src/agent_steering/mover_link.rs`).

**Slice 4 — networked enemy rider presentation.** Add the enemy ground field on the wire and generalize the rider-presentation draft. Sequence it after or with that draft.

**Slice 5 — docs and SDK page; optional `nav_gate`/`nav_link` keys.**

### Tests (named, focused)

- **Physics `agent_motion`:**
  - `agent_on_rising_lift_is_carried_by_tick_delta_and_grounded_on_mover`
  - `agent_on_translating_platform_keeps_mover_local_offset`
  - `agent_on_rotator_keeps_radius`
  - `sliding_mover_displaces_agent_out_of_sweep`
  - `pinned_agent_is_not_displaced_into_static`
  - `agent_swept_by_closed_door_stops_at_skin`
- **Blocking:**
  - `agent_rider_does_not_reverse_its_lift`
  - `agent_rider_airborne_grace`
  - `crush_lift_carrying_agent_in_open_air_deals_no_damage`
  - `crush_pins_agent_against_ceiling`
  - `crush_engulfed_agent_counts_as_pinned`
  - `multi_victim_crush_order_is_deterministic`
- **Nav:**
  - `closed_door_gates_route`
  - `open_door_restores_route`
  - `lift_top_at_floor_is_walkable_in_gate_band`
  - `gate_change_admits_replans_within_budget`
- **Steering integration:**
  - `chaser_behind_closed_door_holds_without_stuck_cycling`, which bounds positional variance over 300 ticks and the number of recovery firings
  - `door_opens_chaser_passes_within_n_ticks`
- **Sim:** a determinism scenario with an agent on a lift.
- **Netcode harness (slice 4):** a riding enemy's rendered mover-local offset stays constant within 1e-3 m under the conditioned link.

### Draft acceptance criteria

**Automated:**
- A closed solid mover stops agent motion at the skin. The open state restores passage.
- An agent grounded on a moving mover reports `ground == Mover(id)` every tick, and its displacement equals the carry transform within 1e-4 m.
- A `displace` mover moves an unpinned agent clear. A `crush` mover damages an agent only when pinned against static geometry or engulfed.
- Riders never stop or reverse their own mover.
- With the door closed and no alternative route, a chasing agent latches `blocked` and holds. Its position spread is ≤ 0.05 m over 5 s, and stuck recovery fires ≤ 1×.
- Multi-victim crush produces identical event order across runs.
- `sim_steer_agents` on `stress-env-volumes` grows ≤ 0.1 ms. The new code adds zero per-tick heap allocations, per `alloc_probe`.
- The player movement and predict/reconcile suites are unchanged.

**Manual-visual:**
- In the new `content/dev/maps/enemy-movers.map` (door, lift, crusher, slider, rotator, spawner): enemies wait at the door without jitter, ride the lift without foot-sliding, are swept by the slider, and die under the crusher.
- In co-op loopback at 150 ms RTT, a riding enemy sits on the lift without visible float or sink (slice 4).

## 9. Open owner decisions

1. **Crush default for enemies.** Keep the per-mover policy (recommended, matching Q2's per-mover flag) rather than an enemy-specific rule. Also: should an enemy *in a `displace` mover's path* be pushed (recommended, matching the player) or keep being ignored as today?
2. **Corpses.** Carried? Excluded from Stop/Reverse obstruction (recommended: yes, matching Q1/Doom)? Kept as crush victims past death (the existing E17 decision says yes)?
3. **Nav door semantics.** Treat a closed door as impassable, so enemies detour or hold (slice 2), or as costed with wait-at-door (later)? And do enemies ever *open* doors (Doom yes, Q1 by trigger field)? Today triggers are player-only.
4. **Gate fidelity.** Is the load-time portal/region overlap enough, or should a bake-time region split plus a NavMesh version bump be the first cut? Decide after the §7 measurement.
5. **Lifts.** Do enemies only ride lifts that happen to be docked (Q1 behaviour), or can they summon one?
6. **Wire.** Accept a `WIRE_VERSION` bump for the enemy ground ref, and sequence slice 4 behind the `E17--mover-relative-rider-presentation` draft, or merge the two?
7. **Scope creep.** Fold "enemies see and shoot through closed doors" (AI LOS, hitscan, projectiles and splash ignore movers) into this epic, or leave it as its own item?
8. **API break.** Replace `AgentComponent.is_grounded` with `GroundRef` (recommended, given "move fast"), and move the agent substrate from `sim` into `physics`.
