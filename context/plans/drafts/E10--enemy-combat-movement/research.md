# E10--enemy-combat-movement — research

Read at 5932287f. Findings that inform the brief but do not decide it.

## Motion and facing today
- `MotionVerb` (foundation `data_descriptors/types/behavior.rs`) has six parameterless variants. `steering_for` (`crates/ai/src/graph_eval.rs`) maps `ChaseTarget` to `Chase`, `Freeze` to `Hold`, and the rest to `Clear`. `position_goal_steering` (`crates/ai/src/steering.rs`) overrides the position goals with `MoveTo`.
- `action_for_path` returns no action under a position-goal verb, so position goals cannot fire.
- `facing_direction` (`crates/ai/src/compute.rs`):
  - `MoveTo` faces velocity.
  - With `committed_aim`, faces the aim.
  - When engaged, faces velocity first, then the aim.
  - The attack gate needs post-slew yaw within `ATTACK_FACING_TOLERANCE_RADIANS` (π/12).
- The line-of-sight brief named a strafe or kiting verb as out of scope.

## Knockback
- `AgentComponent::add_knockback` adds the impulse to both `velocity` and `knockback_velocity`, and sets `GroundRef::Airborne` on an upward push.
- `tick_impl` (`crates/sim/src/agent_steering.rs`) stores steering plus knockback in `agent.velocity`. `path_state` copies it into `AgentPathState::velocity`, which facing and `locomotion_intent` both read.
- No AI code checks ground state. `KnockbackResponse` defaults are `air_drag: 0` and `control: 1`.

## Combat slots
- A tick has three passes: snapshot, then `compute::evaluate` per enemy, then the batch `resolve_combat_slots`, then serial `apply_outcomes`. All of them walk brains in slot-index order.
- `select_combat_positions_batch` sorts claimants by `EntityId::to_raw`. Proposals compete by score with a global `min_spacing` and are accepted greedily.
- The held slot is `BrainComponent::combat_slot` plus `combat_slot_hold_ticks` (`COMBAT_SLOT_HOLD_TICKS` = 8).
  - While the incumbent is retained, challengers are not scanned.
  - Once the hold expires, the previous slot is not passed and carries no penalty. So `apply_hysteresis` never chooses anything in this caller.
- Think stride gates acquisition only. Every enemy is evaluated every tick.

## Attack firing
- Step 4 of `evaluate` builds an immutable `PendingAttack`. `apply_outcomes` is the first place enemies are arbitrated against each other.
- A projectile weapon keeps advancing through `AiWeaponControllers::advance_actor` while `weapon_aim` is set. A shot can therefore land ticks after the request.
- No per-target shared state exists. `AiRuntime` holds programs, line-of-sight grace, weapons and warn latches.
- Untested: enemy multi-shot bursts. The executor appears to continue the sequence, but no AI test covers it. The brief adds a row for it.

## Precedents
- **Mod-wide scalar:** `factionSentimentDecay`.
  - Parsed by `drain_faction_sentiment_decay_js` and `drain_faction_sentiment_decay_lua`.
  - Stored via `FactionRegistry::with_sentiment_decay`.
  - Hashed in `mod_compatibility_digest`.
- **Player locomotion naming:** `MeshDescriptor.animations` typedoc. It names `walk_forward`/`walk`, `walk_backward`, and `jump_loop` on a launch without a jump.
- **Directional clips:** `content/dev/models/clip_libraries/mixamo_rifle/library.gltf` ships `walk_left`, `walk_right`, `walk_backward`, and their `run_*` siblings. `rifle_trooper` and `rifle_warrior` bind it.

## Industry reference (owner conversation)
- Doom 2016/Eternal grant a few attack tokens per player, and enemies without one reposition. The brief takes that shape.
- "Longest waiting wins" keeps slot-index order from starving the same enemy every time.

## Encounter-budget precedent
- `AiEnemy.enabled_on_spawn` in `sdk/TrenchBroom/postretro.fgd` is the per-placed-enemy aggro key.
- `entity_spawner.spawned_tags` passes tags to each spawn (`crates/sim/src/spawner.rs`).
- `npcs({ tag }).update({ aggro })` lowers to `updateNpcState` (`UpdateNpcStateArgs`, a deny-unknown additive partial). It applies to members at fire time.

## Pin table

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | Turn arbitration within one tick | Every request and release on a target is known before any grant. Grants go by longest wait; equal waits go to the earlier slot. | The longest waiter wins even when a fresher requester is earlier in slot order. The grantee may start on the grant tick. |
| P2 | Attack refused for want of a turn | Compute proposes an attack; arbitration refuses it the same tick. | Nothing commits: no cooldown, count, event or weapon request. The once-per-entry fire stays open. |
| P3 | Unmarked grant with no attack | Turn granted; an earlier outcome in the pass kills the target, or the weapon is still executing. | The turn is released that tick. Nothing commits. |
| P4 | Unmarked weapon attack ends | Turn granted, shots land over later ticks, the execution clears inside an apply walk. | The turn holds through the execution and the cooldown. The freed turn is grantable from the next tick. No other start lands between shots. |
| P5 | Holder leaves the AI set | Quiescence, queued despawn, inventory ownership, killed earlier in the pass, aggro closing, graph reseat. | Stops counting from that tick. A waiter is granted no later than the next tick. A returning enemy re-requests. |
| P6 | Holder switches target | Tick N: compute selects B while holding a turn on A. | A's turn frees on tick N. The enemy requests on B behind B's existing waiters. |
| P7 | Guards read turn facts | Compute refreshes facts before arbitration. | Facts show the previous tick's turns. `hasAttackTurn` first reads 1 the tick after the grant. Several waiters may see `attackTurnFree`; exactly one is granted. |
| P8 | Cost-0 enemy beside an oversized requester | Budget 2; a cost-0 enemy in a marked activity; a cost-3 requester waits. | Cost 0 attacks without counting and reads `hasAttackTurn` 1 in a marked activity. Cost 3 is granted when no counted holder remains. |
| P9 | Strafe with no usable neighbor | Both neighbors unusable, or the batch gives the next slot to another strafer. | A batch loser reverses once. With neither neighbor usable it holds its slot, facing and firing, and does not flip again until a neighbor frees. |
| P10 | Launch and landing | Upward push on tick N, landing on tick M; activity and steering unchanged. | `jump_loop` after the fall delay, or the current clip when none is declared. Tick M switches back to the resolved clip. |
| P11 | Brain restored from serialized state | A holder and a waiter are serialized and restored. | No turn held, wait clocks at zero; the first tick arbitrates from scratch. |
