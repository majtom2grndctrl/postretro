# E10--npc-response-to-downed-targets

Brief · compact · draft, not yet validated · Epic 10 · reads: `context/lib/entity_model.md` §7c · `context/lib/scripting.md` §2 (Impact-policy evaluation), §11, §12 (Player life states, Assist) · `ready/E10--ai-tick-scheduling/` (notice meter, respawn drop) · read at e98097b4a

## Problem
Owner-requested capability (2026-11-18): an author decides what an NPC does once its target is downed and once it dies — guard the downed player, keep attacking them, or drop them and take the next target. Today guards and candidacy read the target's death latch (`brain.targetDied`, `candidate.died`) but not its life state, so an NPC cannot tell a downed player from a dead one. In the dev mod (`content/dev/scripts/player-death.ts`) a player goes down at zero HP, so a downed player reads `targetDied`; only restored health (a revive, a second wind, a respawn) re-arms the latch, and a bleed-out is a respawn on the same pawn id. When done, a behavior graph reads its selected target's life state and each candidate's, so guarding, attacking and dropping a downed player are each a graph choice, and a revived target reads as standing again.

## Decisions
- **Engine supplies the facts; the graph owns the policy.** This brief adds a life-state read to the brain scope (selected target) and the candidate scope. Selection, retention, ranking and the attack gate stay as they are, keeping `entity_model.md` §7c "Target selection holds no aliveness policy" and `done/E18--player-life-states/`'s non-goal of excluding a life state from AI selection. Today's levers stay: a retained target is never checked against candidacy (`ai_tests::candidate_filter_is_never_evaluated_for_a_retained_target`), a non-engaging activity drops it (`compute.rs`, `graph_eval::engages_active`), and the attack gate reads the target state's `damageable` capability (`targeting::selected_target_alive` → `health::is_damage_target_eligible`).
- **Read semantics.** True while the selected target, or the candidate, is a player pawn in the named state; false with no target and, unwarned, for a non-player, as `source.inLifeState` reads (`scripting.md` §2, Impact-policy evaluation). A state change reaches an NPC's guards and candidacy at its next evaluation after the change. Host-only, no wire: graph evaluation is host-only (§7c) and life state is host-authoritative.
- **Names resolve at load.** A name absent from the mod-wide life-state table rejects load, naming the state and the archetype, as player-event `inLifeState` rejects binding (`scripting.md` §12). A declared name that the target's descriptor lacks reads false (`life_state_inputs::resolve_life_state_id`). A hot reload that rebuilds the name table keeps every read bound by name.
- **Candidacy stays boolean.** `candidate.inLifeState` narrows fresh acquisition only. A rule such as "prefer standing players, take a downed one only when none is left" ranks candidates, and ranking stays the engine's (§7c). So does a preference for a teammate reviving the guarded target: a guard reaches a reviver through today's retaliation and switch hysteresis.
- **Non-goals.**
  - Dropping targets across a respawn, which keeps the pawn id: `ready/E10--ai-tick-scheduling/`'s respawn drop owns it.
  - Re-acquisition pacing after a drop. Today the NPC acquires at its next acquisition check; under `ready/E10--ai-tick-scheduling/` it notices through its meter, which fills only for candidates that pass candidacy, so a candidacy clause on life state also keeps a downed player's meter from filling.
  - Downed NPC targets. An NPC's downed state is modder-owned per-entity state (`done/E18--player-life-states/`); a guard reading another entity's `@state.*` waits on the cross-entity write path (`scripting.md` §11).
  - Time in the target's life state. `brain.timeInActivityMs` on the guard activity times a guard; a fact can join the append-only table later.
- **Docs amended at promotion:** `entity_model.md` §7c (target-side life-state reads beside the death latch), `scripting.md` §11 (brain and candidate scope listings). Human-facing: the death-guard recipe and the brain and candidate fact tables in `docs/scripting-reference.md`, and the type docs in `sdk/lib/brain.ts` and `brain.luau`.

### Scripting surface
Proposed names, pending the owner (Open questions). Normative once settled.
```ts
import { brain, candidate, defineAttack, defineEntity } from "postretro";

const jab = defineAttack({ damage: 8, maxRange: 2, cooldownMs: 1200 });

export const warden = defineEntity({
  canonicalName: "warden",
  components: {
    // mesh, health and faction as for any NPC
    behavior: {
      initial: "idle",
      moveSpeed: 3,
      engagementRadius: 2, // combat-slot radius: the guard rests about this far from the body
      // Fresh acquisition skips downed players.
      candidateFilter: candidate.inLifeState("down").not(),
      activities: {
        idle: { animation: "idle", motion: "hold" },
        attack: { animation: "attack", motion: "chaseTarget", action: { attack: jab } },
        // Engaged motion with no action: keeps the downed player as its target, never strikes.
        guard: { animation: "walk", motion: "chaseTarget" },
      },
      transitions: {
        "*": [{ to: "idle", when: brain.hasTarget.not() }],
        idle: [{ to: "attack", when: brain.acquisitionDue.and(brain.hasTarget) }],
        // A drop-and-retarget archetype goes to "idle" here instead.
        attack: [{ to: "guard", when: brain.targetInLifeState("down") }],
        // A revive stands the target up.
        guard: [{ to: "attack", when: brain.targetInLifeState("down").not() }],
      },
    },
  },
});
```
### Boundary inventory
Both runtimes ship. Names follow the owner's answer to Open questions.

| Name | Rust | Wire / serde (IR input) | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| target life-state read | brain scope, name-keyed leaf | `@brain.targetInLifeState:<name>` | `brain.targetInLifeState(name)` | `brain.targetInLifeState(name)` | n/a |
| candidate life-state read | candidate scope, name-keyed leaf | `@candidate.inLifeState:<name>` | `candidate.inLifeState(name)` | `candidate.inLifeState(name)` | n/a |

## Acceptance
### Automated
- [ ] With the dev mod's life states, the target read is true from the NPC's first guard evaluation after its target enters `down`, and false from the first after a revive. It reads false with no target and when the target is an NPC peer. The candidate read follows the same rules per candidate.
- [ ] A guard or candidacy read naming a state no descriptor declares rejects load, naming the state and the archetype. A name declared only by another player descriptor reads false. After a hot reload that adds a state ahead of `down` in the name table, the reads still track `down`.
- [ ] The Scripting surface example, as a fixture, with player A targeted and player B standing in view: A goes down. The warden enters `guard` at its next guard evaluation, keeps A as its target, and fires no attack while A is down. Revived, A is attacked again without a fresh acquisition.
- [ ] The same fixture with a retaliation tolerance authored toward players, B damaging the warden past it while it guards A: the warden switches to B by retaliation, and does not acquire A again while A is down.
- [ ] The drop variant, otherwise identical: A goes down. The NPC leaves A at its next guard evaluation, acquires B at its next acquisition check, and never acquires A while A is down. After A is revived, A can be acquired again.
- [ ] Regression guard: a graph that reads no life state behaves as today toward a downed player. The reference enemy drops them through `brain.targetDied`. A graph with neither a death guard nor a candidacy clause keeps attacking a downed player whose state leaves `damageable` on.
- [ ] Both SDKs lower every new read to the same IR.
### Manual
- [ ] Two-client co-op playtest at 150 ms RTT (`POSTRETRO_NET_LAG`) on the dev mod: a warden stands over a downed client and attacks again once a teammate revives them; the drop variant turns to the standing player.

## Path
- Seams: `crates/ai/src/brain_scope.rs`, whose name-keyed `@state.*` leaves are the precedent for a name-keyed read and whose `BRAIN_TARGET_DIED_INPUT` refresh is the selected-target precedent; `crates/ai/src/candidate_scope.rs`; `life_state_inputs::resolve_life_state_id` and `player_life_state` in `scripting-core`. That module is `pub(crate)`, and `crates/ai` reaches `scripting-core` only through `postretro-sim`: widen it or mirror the reads it needs.
- Fixture: the example fits as a `content/dev` archetype, which the automated rows load and the playtest places on a dev map.
- SDK: `sdk/lib/brain.ts` and `brain.luau` carry a sync obligation with `BRAIN_INPUTS` and `CANDIDATE_INPUTS`; the drift tests in `crates/scripting-core/src/data_descriptors/tests/behavior.rs` guard it. Name-keyed reads sit outside the fixed tables, as `state(name)` does. Lowering-parity precedent: `addressing_sdk_tests::life_state_verbs_and_predicates_lower_identically_in_both_sdks`.
- Hot reload: the name table is rebuilt by `rebuild_life_state_table`; check that bound guard programs rebind when only a player descriptor changes.
- Shape chosen: a boolean read per name, matching every existing life-state read. Rival: a target life-state string fact compared by name, which needs string comparison in the brain IR.
- First slice: the candidate read and one guard read in a `crates/ai` test with two pawns, one downed.

## Open questions
- Names: `brain.targetInLifeState(name)` and `candidate.inLifeState(name)`, or rivals such as `brain.target.inLifeState(name)` (after `assist.target.inLifeState`) — owner — **blocks build**
- What the shipped reference enemy (`sdk/behaviors/reference/entities.ts`) and the docs death-guard recipe do with a downed player: keep dropping through `brain.targetDied` (today), drop by life state, or guard — owner — **blocks build**
- Finishing: does this brief give a hit on a downed player an authored consequence (an impact policy reading or setting the target's life state), or does finishing stay "keep attacking while `damageable`"? Today such a hit drains defense layers and changes no health, death latch or life state — owner — **blocks build**
- Sequence against `ready/E10--ai-tick-scheduling/`: landing first means a guarding NPC whose target bleeds out into a respawn sees `down` end and attacks the respawned pawn wherever it spawned, until that brief's respawn drop lands — owner — **blocks build**
