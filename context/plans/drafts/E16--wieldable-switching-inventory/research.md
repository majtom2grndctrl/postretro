# Research — wieldable switching + inventory

Investigation notes behind `index.md`. Not a spec.

---

## 1. The three active-weapon holders, as found

| Holder | Declared | Shape | Role |
|---|---|---|---|
| `App.active_wieldable` | `crates/postretro/src/main.rs:683` | `Option<EntityId>` + sibling `active_wieldable_descriptor: Option<String>` | single-player **and listen-host local pawn** |
| `WeaponOwners` | `crates/postretro/src/netcode/command_queue.rs:78` | `HashMap<EntityId, EntityId>` pawn→weapon + `HashSet<EntityId>` attachment dirty set | host-only, all pawns |
| `ClientWeaponState` | `crates/postretro/src/weapon/mod.rs` (`struct ClientWeaponState`) | seven prediction/tuning fields plus `pawn: EntityId`; **no weapon `EntityId`**. Only `cooldown_remaining_ms`, `cooldown_authority_generation`, and `shoot_press_consumed` are locally predicted — `cooldown_ms`, `fire_mode`, `resolution`, `range` are host-sent tuning | connected client only |

They are not three parallel systems. `App.active_wieldable` is written in exactly four places, all install/teardown (`startup/session.rs:254`, `startup/lifecycle.rs:809`, `:1019`, `:214`), and reaches `WeaponOwners` through one bridge — `host_register_own_pawn` (`netcode/mod.rs`, `host_register_own_pawn`), called from `host_register_own_pawn_after_install` (defined in `main.rs`, its only caller in `crates/postretro/src/startup/lifecycle.rs`). Removing the global therefore removes the bridge, not just a field.

`active_wieldable_descriptor` is **write-only**: no read site exists in the tree despite the doc comment at `main.rs:680` claiming it feeds hot reload. Hot reload actually refreshes weapons through `DescriptorProvenance` — the weapon path in `crates/scripting-core/src/refresh_plan.rs` calls `WeaponComponent::refresh_from_descriptor` (`crates/entities/src/components/weapon.rs`). (The neighbouring `plan_health_replace` is the health equivalent, not a second weapon path.) The field is dead and is deleted rather than migrated.

A connected client owns **no weapon entity**. `ClientWeaponState::from_local_pawn_descriptor` (`weapon/mod.rs:115`) is `#[cfg(test)]`-only, with a comment at `:71` stating a connected client must never consult its local registry for weapon tuning.

## 2. Why the tuning payload is the natural carrier for switch policy

`TuningPayload` (`crates/postretro/src/netcode/tuning_payload.rs:29`) is canonical JSON on `Channel::Control`, epoch-stamped, change-detected against `last_sent_tuning` (`netcode/mod.rs:412`), sent from two call sites (`main.rs:4978` on slot accept, `startup/lifecycle.rs:157` after level install).

It already carries a full `PlayerMovementDescriptor` — and that descriptor's fields are `NumberOrIr` / `BoolOrIr` — see `DashParams` in `crates/foundation/src/data_descriptors/types/movement.rs` — so authored IR already crosses this payload. (The `NumberOrIr`/`BoolOrIr` imports in `tuning_payload.rs` are inside its `#[cfg(test)]` module; the production evidence is the descriptor's own field types.) That finding is what made the rejected guard design affordable; with the guard dropped, the payload carries only per-slot fire values, equip durations, and one resolved boolean, which is strictly less than it already carries for movement. The "replicate rather than hash" property holds either way.

`tuning_payload_for_pawn` (`netcode/mod.rs:2283`) resolves tuning from the pawn class descriptor's `default_weapon` — never from `WeaponOwners`, never from the live `WeaponComponent`. That is why it survives today: exactly one weapon per pawn, fixed at install.

## 3. Why no timing primitive is needed at all

The spec first proposed an authored IR guard over a `@wieldable.*` namespace and then dropped it (see `index.md` Direction). Both halves of that investigation are kept here: the first subsection explains why a *general* timing vocabulary was never available, the second why the *scoped* one it would have justified is unnecessary once the dwell moves to the input layer.

### 3a. No general timing vocabulary exists

No mod-authorable timing vocabulary exists:

- `SequenceStep` has three fields and no delay (`crates/entities/src/data_descriptors/types/reactions.rs:42`).
- `onComplete` chains inside one `VecDeque` drain bounded at `MAX_BATCH_DISPATCH_HOPS = 256` (`crates/scripting-core/src/reaction_dispatch.rs:271`) — a same-frame hop, never a later tick.
- `IrNode` is closed at 15 opcodes with no time opcode (`crates/foundation/src/ir/mod.rs:111`).

Time enters IR only as a **named input leaf seeded by a scope**. `@brain.timeInStateMs` (`crates/foundation/src/brain.rs`, accumulated and reset in `scripting/systems/ai/mod.rs`'s `tick`) is the shipped instance, and that `tick` doc comment frames commitment windows as *"an authored `@brain.timeInStateMs` guard rather than an engine rule."* A `@wieldable.*` namespace would have been the same construction — which is what made it look justified, and §3b is why it is not.

Shipped `dt` slot accumulators (`scripting/systems/slot_accumulators.rs:13`) were considered and rejected: they can express *"condition C has held for N ms"* via a self-zeroing `select`, but not *"value unchanged for N ms"* — no scope exposes a previous value, and an accumulator's output is pinned to its own slot (the `output: Some(slot.clone())` line in `slot_accumulators.rs`). The engine owns the cursor, so the engine is the only thing that can publish the dwell.

The IR has no boolean `and`/`or`/`not`. `select` supplies them: `or(a, b)` is `select(a, true, b)` (`crates/foundation/src/brain.rs`, `BRAIN_NO_TARGET_DISTANCE` doc comment).

### 3b. Why the scoped namespace was dropped

Two independent challenges landed on the same finding: the direction review's Q6, and the owner's placement objection. The review's evidence was that both reference policies the spec shipped were expressible in three scalars, so the guard's justification was asserted rather than demonstrated. The owner's was stronger and different in kind — a guard on the player entity descriptor means every character class in a game re-declares an identical rule, and switch behavior is uniform across characters in every comparable title. What varies per-thing is equip *duration*, already per-weapon.

The resolution came from the player-options question rather than from either challenge. Once the dwell is recognized as an **input-layer interpretation preference** — the same category as `PlayerOptions.crouch_mode`, whose resolution the `MovementInput::crouch_intent` doc comment (`crates/postretro/src/movement/mod.rs`) records as never reaching the movement intent — it stops being simulation policy entirely. The simulation never sees a dwell, so it needs no vocabulary to express one, and the only rule left crossing into the tick (may a switch interrupt a reload) is a boolean.

This also dissolves what the direction review flagged as the sharper half of the guard's open question: `@wieldable.selectionDwellMs` and `@wieldable.switchInFlight` would have been peer-local accumulations feeding a replicated guard, so an identical expression over divergent local inputs could commit at different moments on the two peers. With the dwell in the input layer, there is nothing to diverge — each peer decides when *it* wants to switch and sends an intent.

## 4. Typed cross-references between script-declared things

`defineWeapon` **does not exist**. `plans/done/M10--weapon-primitives/index.md:101` records the decision: a `weapon` block on `defineEntity`, "not a standalone `defineWeapon`." The `defineWeapon`/`defineAugment` forms in `context/research/weapon-model.md` are proposals, not surface.

Every shipped `define*` is pure and returns its value; none registers by side effect. `defineEntity` (`sdk/lib/data_script.ts:643`) is an identity builder. `context/lib/scripting.md:49` states the durable rule — entity types, stores, UI trees, themes, map catalogs, and frontend declarations all arrive as manifest data — and `registerEntity` was deliberately removed (`crates/postretro/src/scripting/primitives/mod.rs:645`). `plans/done/game-state-sdk-surface/` migrated `defineStore` from import-time FFI to a returned declaration for staged-init and rollback reasons (`index.md:404-419`).

Phantom brands are an established pattern — state-ref value and write capability (`sdk/lib/ui/widgets.ts:40`), reaction scope (`data_script.ts:160`), impact-policy node types (`:170-174`), theme token category (`sdk/lib/ui/theme.ts:10`). `defineStore<const S>` and `defineUiTree<const Name>` capture author literals with `const` type parameters, purely TS-side.

Three constraints on any handle scheme:

1. **No `tsc` in CI.** `content/dev/scripts/typed-handles-fixture.ts:1-10` says so in its own header, and there is no typecheck job. Handles buy editor-time and rename safety, never an enforced gate.
2. **Value equality only, never identity.** Luau's `require` performs no module caching (`crates/scripting-core/src/luau_require.rs:62`), so requiring a file twice yields distinct objects; and the mod-init bundle and each level data script are separate bundles run in separate VMs. `game-state-sdk-surface/index.md:139-155` documents the same constraint for state refs, which work because `{ slot }` is value-equal.
3. **Cross-file imports are already used.** `content/dev/scripts/arena-lights.ts:16` imports another script. The compiler resolves relative specifiers only (`crates/script-compiler/src/lib.rs:291`), and bare SDK imports must be unaliased named imports (`validate_bare_sdk_imports`).

`context/lib/scripting.md` §12 (Reaction Dispatch Model) gives the rule this decides against: *"An explicit string name is load-bearing only when a referrer cannot hold a reference."* `player.ts` and `reference-shotgun.ts` sit in one bundle, so the referrer can.

Today's failure modes for the string form, both at level install and both warn-and-degrade: an unregistered name gives "defaultWeapon `X` not registered; player spawned unarmed" (`scripting/builtins/data_archetype.rs:879-885`), and a name resolving to a descriptor with no weapon component gives the sibling warning at `:889-895`. The strictest precedent in the tree is the opposite — behavior-graph `to:` targets are rejected at descriptor parse with the whole descriptor refused (`crates/foundation/src/data_descriptors/types/behavior.rs:388`). The worst is reaction dispatch, which silently no-ops on an unknown name (`crates/scripting-core/src/reaction_dispatch.rs:110`, pinned by a test at `:833`).

Codegen cannot help: `crates/postretro/src/bin/gen_script_types.rs` loads no mod and reads no content root, emitting one committed artifact shared by every mod. A union over mod-authored weapon names is impossible through that path.

## 5. Player options precedent

`PlayerOptions` (`crates/postretro/src/options/mod.rs:42`) is TOML at the platform config directory, snake_case, every field `serde(default)`, atomic write, corruption falls back to in-memory defaults. `crouch_mode` (`:63`) is the exact shape the dwell override wants: an input-layer interpretation preference, engine-internal, explicitly no SDK or scripting surface (`player_options.md` §6). `player_options.md` §4 splits the store from the E13 settings menu as distinct deliverables, and §3 records that no save-on-change happens until that menu is wired — so shipping a field with no UI is the established pattern, not a gap.

## 6. Why the wheel is discrete, not analog

`PhysicalInput` has six variants, none of them scroll (`crates/postretro/src/input/types.rs:100`). There is **no `WindowEvent::MouseWheel` arm anywhere in the tree** — the `WindowEvent` match in `main.rs` handles resize, close, keyboard, mouse button, cursor moved/left, focus, and redraw only. Scroll reaches `egui_winit` (`main.rs:1442`) but its `consumed` flag is honored only outside `InputFocus::Gameplay` (`:1443`) and the whole block is `#[cfg(feature = "dev-tools")]`, non-default. Scroll is unclaimed in gameplay.

The analog path (`resolve_mouse_axes` in `input/mod.rs` → `AxisValue`/`AxisSource` in `input/types.rs`) exists but terminates at the camera for look, and only `wish_dir` reaches `MovementInput`. Routing an analog scroll delta to the sim would mean a new analog field on `MovementInput`, `WireMovementInput`, prediction replay, and `sanitize_input_command` — plus `MouseScrollDelta`'s `LineDelta`/`PixelDelta` unit fork, which has no normalization precedent in the tree, and a per-frame clear (`input/mod.rs:381` currently zeroes only look axes and comments that non-look mouse-axis entries are left alone).

Treating a scroll notch as a momentary button sidesteps all of it: `MouseWheelUp`/`MouseWheelDown` bind like any other button, and the sim receives a bounded signed step count. This is also how the genre behaves — wheel-switching is notch-quantised everywhere.

## 7. Presentation is already plumbed

`E21--coop-avatar-weapon-presentation` states at `index.md:33`: *"Weapon-switch input path — no equip/swap mechanic exists. This plan renders whatever weapon the host assigns."*

- Wire: `active_weapon_archetype` on the entity record (`crates/net/src/wire.rs:338`, `:428`, `:442`), valid only on records carrying `PlayerMovementState` (`wire.rs:849`), archetype change dirties a movement baseline (`crates/net/src/replication.rs:294`).
- Host fill reads `WeaponOwners::weapon_of(pawn)` fresh per snapshot (`netcode/replication.rs:246`) — so it follows the holder automatically.
- Socket rewrite: `update_active_weapon_attachment` (`netcode/remote_materialize.rs:82`) at `ACTIVE_WEAPON_SOCKET = "hand_r"` (`:13`), filtered through `hit_zone_store.get_by_name()` (`:100`) so an unloaded model clears rather than leaving a stale prop; callers re-run `resolve_mesh_entity_bindings_for_entities` (`main.rs:299`).
- Client caches separately in `ClientReplication.active_weapon_archetypes` (`netcode/client.rs:196`), applied via `maybe_surface_active_weapon_attachment`, cleared on unload (`:502`) and demotion (`:542`).
- Viewmodel resolves per frame on both roles (`main.rs:3052`), host through `WeaponOwners`, client through `local_active_weapon_archetype` (`netcode/client.rs:593`).
- Models preloaded at install so a switch never triggers a runtime load (`scripting/builtins/data_archetype.rs:427`).

## 8. Session boundary, as found

`networking.md:149` — the session-state ledger has **one entry: the connection**. Nothing weapon-related crosses a level change: the weapon entity is despawned by `registry.clear_for_level_unload()` (`crates/entities/src/registry.rs:1015`), `AmmoReserve` is a component on the despawned pawn re-seeded by `seed_weapon_reserve` (`scripting/builtins/data_archetype.rs`), called once per equipped weapon from the player-start spawn path, and every holder is nulled (`clear_surface_lifetime_level_state` in `startup/lifecycle.rs`; `*weapon_owners = WeaponOwners::new()` in the level-unload reset in `netcode/mod.rs`).

One nuance: the `SlotTable` itself survives unload with no re-default (`startup/lifecycle.rs:227`, pinned by `unload_level_preserves_slot_table_and_entity_type_registry` at `:2826`), so weapon slot values persist as stale numbers until the first publish tick after install.

## 9. Lifecycle — the cross-instance switch handoff

The wieldable machine is hosted on `WeaponComponent` (`crates/entities/src/components/weapon.rs:265`), i.e. per instance. A switch therefore starts its timed step on one component and finishes on another. This is the plan's one genuinely new lifecycle shape.

```mermaid
stateDiagram-v2
    direction LR
    [*] --> Idle_A : install, active = slot A

    state "instance A" as A {
        Idle_A : Idle
        Reloading_A : Reloading / ShellLoading
        Lowering_A : Lowering
        Idle_A --> Reloading_A : BeginReload
        Reloading_A --> Idle_A : Expired
        Idle_A --> Lowering_A : BeginLower(latch=target)
        Reloading_A --> Lowering_A : BeginLower(latch=target)\nreload activity forfeited
        Lowering_A --> Lowering_A : BeginLower(re-latch)\ntimer NOT restarted
    }

    Lowering_A --> Repoint : Expired

    Repoint : engine repoints Inventory.active\nto the latched slot
    note right of Repoint
      The only tick where `player.weapon.current` flips.
      Presentation dirties here, not at commit.
    end note

    state "instance B" as B {
        Raising_B : Raising
        Idle_B : Idle
        Raising_B --> Idle_B : Expired
        Raising_B --> Lowering_B : BeginLower(latch=target)
        Lowering_B : Lowering
    }

    Repoint --> Raising_B
    Idle_B --> [*] : unload / pawn despawn
```

Both `Lowering` and `Raising` deny fire and reload (`WieldableState::allows_fire` / `allows_reload`, `crates/entities/src/components/wieldable_state.rs:20`, `:28`) and are **not** reload activity (`is_reload_activity`, `:36`) — a switch must not read as a reload to `player.reloadActive`.

The existing `Cancel` rows are untouched. `(WieldableState::Reloading, Cancel)` returns `StateTransition::Noop` (`sim/weapon_stage.rs:306`) — atomic reload stays uncancellable *by cancel*. Preemption for a switch is a new `BeginLower` event with its own rows, so shipped reload behavior does not change.

## 10. Observers of active-weapon state

| Vantage | Reads today | After |
|---|---|---|
| single-player | `App.active_wieldable` | `Inventory` on the local pawn |
| listen-host, own pawn | `App.active_wieldable` via `host_register_own_pawn` bridge | `Inventory`, same as any pawn |
| host, remote pawn | `WeaponOwners::weapon_of` | `Inventory` on that pawn |
| connected client, own pawn | `ClientWeaponState` (4 replicated floats) | `Inventory` mirror + per-slot tuning set |
| connected client, remote pawn | `ClientReplication.active_weapon_archetypes` (presentation only) | unchanged |
| headless runner | `App.active_wieldable` (`observability/driver.rs:190`, `:267`) | `Inventory` |

The last row matters: the headless driver is a real read site and its rewire is not covered by any gameplay AC.

## 11. Oversized files this plan touches

| File | Lines | Plan's edit |
|---|---|---|
| `crates/postretro/src/main.rs` | 9,440 | call-site rewires + one `WindowEvent` arm |
| `crates/postretro/src/netcode/mod.rs` | 5,290 | tuning build/send, holder removal |
| `crates/postretro/src/startup/lifecycle.rs` | 4,417 | install/teardown rewires |
| `crates/postretro/src/sim/weapon_stage.rs` | 3,452 | **new states, new event, new rows — new logic mass** |
| `crates/postretro/src/netcode/state_slots.rs` | 2,321 | projection re-source |
| `crates/entities/src/components/weapon.rs` | 892 | unchanged shape |

Among the oversized files in this table, only `weapon_stage.rs` receives genuinely new logic; the rest of *these* are rewires that shrink or hold their line count. Substantial new logic also lands outside the table — the input layer's cursor, dwell, notch counting and last-weapon memory, and the payload's per-slot shape and correction channel. Split-before-extend applies to `weapon_stage.rs` alone. Splitting `main.rs` is Epic 19's charter and is not pulled forward here.

## 12. Prior art — predicted weapon switching cadence

External research, primary sources preferred. Settles the client equip-timer cadence question in `index.md`.

**The decisive argument is not quantization, it is replay.** Reconciliation means "snap to authoritative state, then replay the buffered inputs." Replay requires each buffered input to carry the timestep it was simulated with. Render frame deltas are neither in the input buffer nor reproducible, so weapon state advanced at frame rate cannot participate in reconciliation at all — it can only be snapped. That is a missing correction path, not a tolerance.

**Source engine** (`ValveSoftware/source-sdk-2013`). `CPrediction::RunCommand` reads the switch request off the user command (`ucmd->weaponselect`) and, during prediction, sets `curtime = m_nTickBase * TICK_INTERVAL` and `frametime = TICK_INTERVAL` — the client's render delta is never used. Weapon think runs inside the command via `RunPostThink` → `ItemPostFrame`. `DefaultDeploy` sets the next-attack deadlines from that tick-quantized clock. The weapon state machine is predicted, not just fire timing: `m_iState`, `m_flNextPrimaryAttack`, `m_flNextSecondaryAttack`, `m_flTimeWeaponIdle`, `m_flNextAttack`, and `m_hActiveWeapon` are all declared predicted fields.

**Valve's tolerance for divergence on those timers is 1 ms** — `TD_MSECTOLERANCE = 0.001f` in `src/public/datamap.h`, with the comment that the field "should only be checked to be within 0.001 of the networked info." A frame-rate scheme at 20 fps proposes up to 50 ms, varying with the player's GPU load.

**Quake 3** (`id-Software/Quake-III-Arena`, `bg_pmove.c`). `PM_BeginWeaponChange` sets `weaponstate = WEAPON_DROPPING` and `weaponTime += 200`; `PM_FinishWeaponChange` sets `WEAPON_RAISING` and `weaponTime += 250`. `PM_Weapon` decrements by `pml.msec`, which is `cmd.serverTime - ps->commandTime` — the *command's* delta, not frametime. Weapon selection is a usercmd field. `Pmove` chops long moves with the comment "to prevent framerate dependent behavior," and `pmove_fixed` (default 8 ms) exists because variable-step prediction produced the framerate-dependent physics behind the 125 fps meta. **The lineage engine this project clones shipped a retrofit for exactly the defect frame-rate equip timers would introduce.**

**TF2** (`tf_weaponbase.cpp`, `CTFWeaponBase::Deploy`) supplies two things worth stealing. The viewmodel's `SetPlaybackRate` is *derived* from the logic timer, so fixed-tick logic does not imply choppy visuals. And the switch-to-reset-fire-timer exploit is hardened explicitly: `m_flNextPrimaryAttack = MAX(flOriginalPrimaryAttack, gpGlobals->curtime + flDeployTime)`, with the comment "This prevents people exploiting weapon switches to allow weapons to fire faster." A sibling field preserves the pre-reload fire deadline across an interrupted reload.

**Unreal** is the genuine divergence, and it diverges by *not predicting the switch*, not by predicting it at frame rate. Lyra's `ULyraQuickBarComponent::SetActiveSlotIndex` is a `UFUNCTION(Server, Reliable)` with `ActiveSlotIndex` replicated and an `OnRep` — a full round trip, no prediction key, no rollback. Epic's own remedy for predicting anything beyond movement, the Network Prediction plugin, introduces a *fixed* network tick decoupled from the Unreal frame tick.

**Overwatch** runs fixed 16 ms command frames (7 ms in tournament config), and of its client-side systems only three carry gameplay netcode: movement, **weapon**, and state script — weapon is a first-class command-frame predicted system, peer to movement. (Second-hand from the GDC 2017 talk; the vault video was not directly fetchable.)

**Titanfall / Apex** run a licensed fork of the Portal 2 Source branch, so their prediction architecture is Source's by inheritance. No Respawn statement specific to weapon-switch prediction was found. **Halo and Destiny: no data** — the well-known GDC talks cover other layers, and nothing about equip-timer cadence was found; not inferred.

**Table-stakes feature set** (netcode-independent, ordered by universality). Near-universal: direct slot select; next/previous cycle; **last-weapon toggle** (Half-Life onward, exposed in Source as `SelectLastItem` / `lastinv`, and the basis of the competitive quick-switch bind); sub-half-second equip (Q3 200 ms + 250 ms; TF2 0.5 s base, attribute-modifiable); switch cancels a reload (Source `AbortReload`; TF2 resets reload mode in both `Holster` and `Deploy`; Infinity Ward removed reload-cancel in MW2 2022 and restored it in MW3 after backlash); switching interruptible mid-switch (Q3 early-outs so a second request does not re-add the drop time). Genre-specific: weapon wheel and whether it dilates time — **under authoritative co-op, dilation is a server-side gameplay decision and a client-local slowdown desyncs on contact**. Emergent rather than authored: fast-swap damage optimization (Doom Eternal's Ballista/SSG swap, documented techniques in Apex, Hunt, Sea of Thieves) — meaning switch timing gets exercised at its frame-exact boundary by real players, an argument for determinism over tolerance.

**Artifacts players report**, each downstream of the missing replay path: switch "rubberbanding" back to the previous weapon after a correction (CS2 reports at ~65 ping); viewmodel/actual-weapon desync (`ValveSoftware/halflife` issue #3819 — switching while `+attack` is held leaves the wrong viewmodel locally and the wrong weapon on the player model for others); switches that never register and need a full resync (Fortnite Grappler); inconsistent switch-during-reload behavior and its associated fire-timer exploit.

Sourcing note: Q1 and Q2 above rest on shipped engine source, which is stronger than commentary. The table-stakes and artifact sections lean on community and press material and are correspondingly lower confidence, though the mechanics themselves are verifiable in shipped code and patch notes.


## 13. Authority model — why switching follows fire, not movement

Settled by owner decision after the movement-prediction design was drafted and rejected.

Movement is predicted and reconciled because both peers simulate continuously from the same buffered inputs and **drift** — there is a divergence every tick that replay corrects. A switch is a discrete declaration; nothing drifts. The host does not need to re-derive it, only to accept or refuse it. That is the shape `E16--client-authoritative-combat` already shipped for per-shot geometry: client-detected, host-validated, no server rewind.

Choosing the movement shape cost the earlier draft a correction channel, a snapshot-recency gate, and tick-exact equip agreement. The recency gate was an unsolved blocker: authority metadata refreshes on every ingest **without bumping the baseline** (`crates/net/src/replication.rs`), deliberately, so an unchanged pawn is never resent — a stationary client's refused switch had no comparand and could never converge. The declaration model removes the mechanism rather than repairing it.

**Possession-based fire validation** removes the remaining coupling. The host resolves a firing weapon today from its own active pointer — `prepare_remote_pawn_command` calls `weapon_owners.weapon_of(pawn)` — and `HitDeclaration` carries `{ shot_id, records }` with no weapon identity. Resolving from the client instead, and validating that the pawn *possesses* that weapon rather than that it is *selected*, means the two peers never have to agree on the active slot for a shot to be correct. The equip-boundary false-rejection case disappears, and a refused switch degrades to a presentational difference.

The firing slot rides the per-tick input command as a **level**, the shape `reload` already uses on the wire (the host re-derives the edge). A level repeats harmlessly across a gap-hold and needs no edge machinery. The switch declaration itself rides the reliable path, since it is an event rather than a per-tick value.

Ammo and fire rate were already host-authoritative and per-instance before this plan (`E16--ammo-resource`, `E16--client-authoritative-combat`); the only change is which instance is resolved.

**Priced cheat surface.** Possession validation lets a client declare a shot from any owned weapon at any time — including while visibly holding another, or alternating to obtain two weapons' fire rates. Accepted: `context/lib/index.md` §4 non-goals anti-cheat and competitive PvP, this is co-op, and each shot still debits a real per-instance magazine.
