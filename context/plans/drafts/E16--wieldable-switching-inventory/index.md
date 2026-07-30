# Wieldable Switching + Inventory

## Goal

Give a pawn an ordered inventory of wieldable instances and one active reference that repoints between them, preserving each instance's own state. Converge the three divergent active-weapon holders onto that inventory so single-player, listen host, and connected client read one source of truth. The engine owns the inventory, the tick-simulated lower/raise, and whether a switch is permitted; the input layer owns how a switch intent is produced, and the mod declares both once per game.

## Scope

### In scope

- An `Inventory` component on the pawn holding an ordered set of wieldable slots plus the active index. Its length is an **engine capacity constant**, not the authored loadout length — a shorter loadout leaves trailing slots empty.
- Separation of **selection** (moving a local cursor — changes nothing held, never leaves the machine it runs on) from **commitment** (a tick-stamped intent that repoints the active reference).
- `Lowering` and `Raising` equip states on the shipped wieldable machine, with per-archetype durations, advanced by a new timed-state predicate that is **not** the reload-activity predicate.
- A **client-side fixed-tick weapon simulation**, so equip timers advance on the same discrete grid on both peers and participate in reconciliation replay.
- Retirement of `App.active_wieldable` and `App.active_wieldable_descriptor`, and of `WeaponOwners`' pawn→weapon map. All read sites rewired, including the headless observability driver and the level-install products that carry the value.
- Retirement of `ClientWeaponState` **as a holder**: a connected client runs the same `Inventory` component. It keeps a per-slot *fire-prediction carrier* — see the note below.
- Per-slot fire tuning, equip durations, archetype identity, and the resolved reload-block rule in the host→client tuning payload.
- A replicated **active-slot index** as the correction channel, so a mispredicted switch converges.
- A mod-global `switching` block on the mod manifest, with a per-weapon override for its one simulation-side rule.
- A player-options field overriding the cycle-commit dwell, persisted and resolved in the input layer, with no settings UI.
- Discrete mouse-wheel physical inputs; direct-select, cycle, and **last-weapon-toggle** actions; a bounded, edge-gated commit intent on the command path.
- A **deploy clamp** so switching cannot be used to reset a fire cooldown.
- `player.weapon.current`, `player.weapon.pending`, `player.weapon.switching` engine state slots, all local projections of the owning machine's `Inventory`.
- A `loadout` array on the player descriptor's inventory block, holding **descriptor references** rather than name strings, replacing the single `defaultWeapon` string.

### The two-layer split, and why it is the load-bearing decision

Switch behavior divides cleanly along a line the codebase already draws:

| Layer | Owns | Cadence | Replicated |
|---|---|---|---|
| **Input** (local to the machine the player sits at) | pending cursor, dwell timer, direct-versus-cycle, last-weapon memory, the player's dwell override | frame rate | never — it produces a commit intent, nothing more |
| **Simulation** (authoritative, and predicted on the client) | whether a commit intent is honored, the repoint, the lower/raise timers | fixed tick | yes, through the existing command and snapshot paths |

`PlayerOptions.crouch_mode` is the precedent for the input half and it is exact: toggle-versus-hold is resolved in the input layer, and the `MovementInput::crouch_intent` doc comment (`crates/postretro/src/movement/mod.rs`) records that the movement intent *"NEVER sees the raw button or the mode."* A wheel dwell is the same kind of value. Keeping it out of the simulation means the dwell never has to agree across peers — the client decides *when* it wants to switch, the host decides *whether* it may.

### Why the simulation half must be fixed-tick

The client's weapon prediction today is a per-frame post-loop pass advanced by `frame_dt`. Advancing equip timers there would not merely quantize differently from the host — **it would foreclose reconciliation entirely.** Reconciliation is "snap to authoritative state, then replay the buffered inputs"; replay requires each buffered input to carry the timestep it was simulated with, and frame deltas are neither in the input buffer nor reproducible. Weapon state could then only be snapped, never corrected-and-replayed, which is the mechanism behind the switch-rubberbanding and viewmodel-desync artifacts players report in shipped games.

Both reference implementations put the switch request in the buffered input command and quantize its clock to the tick grid — Source's `ucmd->weaponselect` with `curtime = m_nTickBase * TICK_INTERVAL` and `frametime = TICK_INTERVAL` during prediction, and Quake 3's `pm->cmd.weapon` with `weaponTime -= pml.msec` where `msec` is the *command's* delta. Valve's tolerance for divergence on predicted weapon timers is **1 ms** (`TD_MSECTOLERANCE`). Quake 3 shipped `pmove_fixed` specifically because variable-step prediction produced framerate-dependent behavior. Details and sourcing in `research.md` §12.

Fixed-tick logic does not imply frame-rate-locked visuals: the viewmodel's playback rate is derived from the authoritative timer, as TF2 does in `Deploy`. The client already runs a per-tick loop carrying `tick_dt` (`main.rs`, the connected-client branch alongside `client_predict_movement_tick`), so this is a new call in an existing loop rather than a new loop.

### Out of scope

- **Pickup and drop.** The map-placement spawn path never attaches a weapon component, even to a descriptor that is otherwise placeable — pinned by `map_sweep_skips_weapon_component_on_otherwise_placeable_descriptor` in `scripting/builtins/data_archetype.rs`. So no weapon instance can exist in the world to be picked up. (`is_directly_map_placeable` alone does **not** foreclose this: it returns true for any descriptor carrying `light`, `emitter`, `movement`, `mesh`, or `health`, so a mesh-bearing weapon descriptor *is* placeable. The refusal to attach the component is the foreclosure.) Roadmap `E16 › Weapon Systems › pickup` owns the feature; its prompt affordance is owned by the unbuilt **combat presentation substrate** (`context/plans/roadmap.md:229`).
- **A settings UI for the dwell override.** The options store ships the field and its persistence; the menu that edits it is a separate deliverable by standing policy — `context/lib/player_options.md` §4 splits the store from the E13 settings menu, and §3 records that no save-on-change occurs at runtime until that menu is wired.
- **Weapon-wheel time dilation.** Common in the genre, and deliberately not built: under authoritative co-op, slowing time is a server-side gameplay decision, and a client-local slowdown desyncs prediction on contact. The wheel does not alter time. Revisit only with a server-authoritative dilation mechanism, which nothing in the engine has today.
- **Dual-wield.** This plan's `Inventory` carries a single active index (Task 2), and no authored surface can request a second — the `switching` block declares only commit rules and the loadout is an ordered list with no off-hand position. Nothing an author can write reaches the case. It is *reachable by extension* rather than unrepresentable-forever: the roadmap's `E16 › Weapon Systems › dual-wield` entry reads "generalize the single active reference to a primary/off-hand pair… Depends on switching," so this plan is its prerequisite, not its blocker. (Do not warrant this on `EntityTypeDescriptor::weapon` — that field is what makes a descriptor *be* a weapon archetype, not what a pawn holds.)
- **Augments, rolls, and non-passthrough stat resolution.** `WeaponComponent::effective` takes `&self` only and is a pure projection over stored component fields; the component stores no modifier or roll data, so no composed stat can exist.
- **Heat and cell resources.** `WeaponResource` is a tagged union with exactly one arm, `{ kind: "ammo" }`.
- **Secondary activation / alt-fire.** `Action::AltFire` is bound (`crates/postretro/src/input/defaults.rs`) with zero consumers outside `input/`; roadmap `E16 › Weapon Systems › secondary activation` owns it.
- **Mod-authored input bindings.** The SDK exposes no `Action` type and no action or axis read surface, so a mod cannot name a physical input to bind.
- **Radial / ring weapon selector.** `UiInstance` (`crates/ui/src/output.rs`) and `ui_quad.wgsl` draw axis-aligned quads only; a ring needs the radial primitive deferred at `context/plans/roadmap.md:149`. This plan publishes the slots a selector binds to; a list-shaped selector is authorable on shipped widgets.
- **Compile-time enforcement of the loadout's descriptor references.** There is no `tsc` step in CI — `content/dev/scripts/typed-handles-fixture.ts` states this in its own header. Handles buy editor-time and rename safety; the enforced gate stays the descriptor-parse validation this plan adds.

### The client's prediction carrier is not a fourth answer

The three holders answer "which wieldable is active." All three lose that job: single-player, listen host, host-simulated remote pawn, and connected client all read the pawn's `Inventory`. What survives on the client is narrower and differently shaped — the locally predicted fire scalars that Epic 15 Phase 3 requires any predicted action to carry. Of `ClientWeaponState`'s eight fields, only three are predicted (`cooldown_remaining_ms`, `cooldown_authority_generation`, `shoot_press_consumed`); the rest are host-sent tuning that Task 4 makes per-slot, plus a `pawn: EntityId`.

State the claim precisely: this plan gives every session role **one implementation and one place to ask**, not one answer. The client's `Inventory` is predicted, so it differs from the host's during a lower/raise and after a refused commit. Convergence is AC 9's job and depends on the replicated slot-index correction channel (Task 4), not on this section.

### Ships knowingly broken — owner decision

**Inventory and ammo reserve do not survive a level transition.** Nothing in source forecloses carrying them; this is a choice. The durable per-player key that carry needs is the host-minted **seat**, unbuilt in E15 Phase 3.75 (`context/plans/roadmap.md:202`), which `drafts/E16--per-player-currency` is already parked on. Building carry now means either blocking on that spec or standing up a single-player-only carry path — a fourth divergent holder, the exact disease this plan cures. Consequence shipped: a campaign cannot carry weapons or ammo across levels; every level re-equips from the player descriptor and re-seeds the reserve. Owner decision, 2026-07.

## Direction

**Problem.** A pawn's active weapon is stored three different ways, and none of them can change at runtime. `App.active_wieldable` is written only at level install and teardown; `WeaponOwners` is host-only; a connected client owns no weapon entity at all and models its weapon as four floats resolved from the pawn class's `default_weapon`. There is no place to put a second wieldable, and no path by which the active one could change. The cause is that "the weapon" was modeled as a property of the *session role* rather than of the *pawn*.

**Prior commitments.** `context/research/weapon-model.md` §6/§7 pins the shape: switching repoints an active reference, per-instance state survives because instances own it, and the container plus its equip/switch machinery are named for **wieldables**, not weapons (invariant 7), with inventory a peer of the pawn's `Health` and `AmmoReserve` rather than a parent (§1). `crates/entities/src/components/wieldable_state.rs` states outright that equip states join that enum when switching owns their behavior, and `E16--weapon-state-machine` shipped its preemption seam for this, recording that "the switching spec is the first to exercise preemption from *every* state."

`E21--coop-avatar-weapon-presentation` deferred the switch input path to this plan explicitly (`index.md:33`: *"This plan renders whatever weapon the host assigns"*) and shipped the machinery that assignment needs — the replicated active-archetype field, the client-side change detection, and the hand-socket rewrite. Remote-avatar presentation is close to free as a result. Two paths this plan moves are **not** free and are named as work in Task 3: the host snapshot fill reads the map Task 3 deletes, and the client's own viewmodel resolves through a replicated archetype value that must yield to the predicted `Inventory`.

The mod-global block follows the shipped manifest rule rather than inventing a home: `context/lib/scripting.md:49` records that store declarations, UI trees, theme data, map catalog entries, and frontend declarations all arrive as **manifest data, not import-time side effects**, and `game-state-sdk-surface` (`plans/done/`) migrated `defineStore` from import-time FFI to a pure returned declaration for exactly this reason.

E15's session-lifecycle spec set the rule that a client's predicted values are **replicated, not hashed** — the four weapon fire fields are deliberately absent from the content-parity digest because the host sends them. This plan honors that by growing the payload rather than the digest.

Where this diverges, twice, both stated rather than slipped in. The player descriptor's `defaultWeapon` string is **replaced** by a `loadout` array rather than kept as sugar, because leaving a second one-weapon path alive would preserve exactly the divergence this plan exists to remove. And that loadout moves from the descriptor's **top level** into `components.inventory`, reversing M10's deliberate placement ("equip is a different concern at the same level") — justified because equip now carries per-pawn runtime state rather than a single name. The tree is pre-stable and `content/dev/scripts/player.ts` is the sole consumer, so the call sites move in the same change.

**Placement.** Four placements, each on a different axis. The *inventory and the repoint* sit in the engine: authoritative state a host owns and a client predicts. The *equip timers* sit in the fixed-tick simulation on both peers, because prediction and reconciliation are one mechanism and replay needs a reproducible timestep. The *commit rule's simulation half* — may a switch interrupt a reload — sits in mod-declared data, per-weapon-overridable because abandoning a per-shell reload and abandoning an atomic load are genuinely different design calls. The *commit rule's input half* — dwell, direct-versus-cycle, last-weapon memory, and the player's override — sits in the input layer beside `crouch_mode`, because it describes how one person's hardware is interpreted.

**Alternatives rejected.** *Host-authoritative switching with no client prediction* is markedly cheaper — no payload change, no epoch bump, no client weapon tick. It is what Unreal's Lyra actually does: a server RPC plus a replicated `ActiveSlotIndex`, costing a full round trip. Rejected because switching is an input-driven state change on the local pawn, the category Epic 15 Phase 3 built prediction for, and because retrofitting prediction later touches the same payload, the same machine, and the same reconciliation path a second time. It is, however, the honest third option: not "predict at frame rate," but "don't predict, and pay the RTT."

*Frame-rate client equip timers* were the plan's own earlier shape and are rejected on the reconciliation argument above, not on quantization aesthetics. Sourcing in `research.md` §12.

*An authored IR guard over a `@wieldable.*` namespace* was drafted and dropped. **Placement:** the guard would hang off the player entity descriptor, so every character class would re-declare an identical rule. Switch commit behavior is uniform across characters in every comparable game. **Structure versus thresholds:** a behavior-graph author invents states and edges; a switch-commit author sets numbers against engine-fixed structure. The cost accepted — adding a guard later means deprecating authored fields — is priced against a case nobody can name.

*A side-table keyed by pawn*, generalizing `WeaponOwners` in place, is rejected because a side-table is what made the state divergent: it lives on `NetEndpoint::Host` and cannot exist single-player, which is why `App.active_wieldable` exists at all.

**Foreclosures and one-way doors.** The tuning payload epoch bump and the wire-version bump are one-way in that older peers are rejected; that is what those constants are for. Replacing `defaultWeapon` with a handle-bearing `loadout` is a breaking descriptor change, bounded but not free. Declaring switch policy as manifest fields forecloses a later move to expression-authored policy without a deprecation path. Nothing here forecloses pickup, dual-wield, or augments: all three extend the slot array or the instance, not the active reference.

## Acceptance criteria

- [ ] A pawn spawns holding every wieldable its loadout references, with the first slot active; each slot holds a distinct instance, and two slots referencing the same descriptor hold two independent instances.
- [ ] Pressing a direct-select input for an occupied slot other than the active one plays the outgoing weapon's lower, then the incoming weapon's raise, and the incoming weapon becomes active exactly once.
- [ ] Scrolling the wheel moves the pending selection without changing what is held; the switch begins only after the resolved dwell elapses with no further scroll, and a scroll during the dwell restarts it.
- [ ] A last-weapon-toggle input returns to the previously active slot, and pressing it twice in succession returns to where the player started.
- [ ] A weapon switched away from and back to retains its own magazine count, cooldown remaining, and reload progress state — it is not re-created and does not inherit the other weapon's values.
- [ ] Switching cannot shorten a fire cooldown: a weapon switched away from and back to is no readier to fire than if the player had held it throughout.
- [ ] Firing and reloading are both refused for the whole lower and raise, and the reload indicator reads false from the first tick of the lower — including when the switch preempted a reload that had already published feedback.
- [ ] With the mod's block-during-reload rule off, switching away during a per-shell reload keeps the shells already loaded and during an atomic reload loads none; with it on, the switch does not begin until the reload resolves. A per-weapon override wins over the mod-global value for that weapon only.
- [ ] Ammo reserve is seeded once per ammo type regardless of how many loadout slots draw on it, and two weapons sharing a type draw from one pool; switching between them does not move, duplicate, or reset reserve rounds.
- [ ] A connected client's switch is visible locally on the input frame — the lower begins without waiting for a host round trip — and the client and host resolve the same switch on the same tick index, not merely within a tolerance.
- [ ] Client and host converge on the same active slot after a refused or dropped commit, with 150 ms RTT, 5% loss, and jitter, and the correction does not replay as a second switch.
- [ ] A remote player's avatar shows the weapon they switched to, and the local player's viewmodel shows theirs, on both host and client; the client's viewmodel follows its predicted switch rather than waiting for the host.
- [ ] The HUD's ammo, reserve, reload, and cooldown values follow the active weapon across a switch, on both host and connected client.
- [ ] Two players in one session with different dwell overrides each switch on their own timing, and neither player's timing affects the other or produces a reconciliation correction.
- [ ] A loadout referencing a descriptor that declares no weapon is rejected when the mod's declarations are validated, naming the offending entry — not warned about at level install with the player spawned unarmed.
- [ ] Every ordering in the Orderings table resolves to its stated outcome.
- [ ] A client whose tuning payload or wire version predates this change is rejected with a diagnostic rather than predicting with stale weapon values.
- [ ] Selecting an empty slot, or a slot index beyond the loadout, leaves the active weapon unchanged and logs no error at gameplay severity.
- [ ] The engine reports the same active wieldable in single-player, on a listen host, for a host-simulated remote pawn, and on a connected client — no session role has its own answer, and the client's answer comes from the same place the host's does.
- [ ] Reload behavior is unchanged from before this plan: atomic reload still cannot be cancelled by the existing cancel path, per-shell reload still cancels with its loaded shells credited, and both still transfer the same rounds for the same inputs.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Exactly one live wieldable machine per pawn | Task 2 (`Inventory.active`) | Task 3 removes the global that could name a second; a slot holding a despawned instance must clear, not linger | AC 1, 19 |
| A repoint preserves the instance's own state | Task 2 (repoint touches indices only) | Task 4 must not rebuild a component from tuning on switch; Task 3's rewires must not re-seed | AC 5, 9 |
| Equip timers advance on the fixed tick on every role | Task 2 (timed-state predicate + client weapon tick) | Task 6 must not advance them from the frame-rate cursor; the client's existing per-frame fire pass must not acquire them | AC 10 |
| The lower→raise handoff crosses instances exactly once per commit | Task 2 (latched target on `Inventory`, repoint on lower expiry) | Task 6 can emit a second commit during `Lowering`; re-latching must not emit a second repoint | AC 2, O5, O6 |
| Fire and reload are refused for the whole switch, and a switch is not reload activity | Task 2 (state predicates + feedback clearing) | the reload-feedback stream reports active from a queued endpoint regardless of state, so a preempting row must clear it | AC 7 |
| Ammo reserve is pooled on the pawn, seeded once per type | Task 2 (seeding rule) | Task 8's multi-slot loadout is the first case with more than one seeding source | AC 9 |
| No dwell, cursor, or player preference reaches the simulation | Task 6 (input layer owns all three) | Task 4 must not carry them in the payload; Task 7's slots are local, not replicated | AC 14 |
| Host and client converge on the same active slot | Task 4 (replicated slot index + snap) | Task 6's dropped or neutralized intents; a per-weapon override resolved differently on the two peers | AC 11 |
| Presentation reflects the committed active, not the pending cursor | Task 2 (dirty at repoint) | Task 6's cycle moves the cursor every notch — presentation must not follow | AC 3, 12 |

## Orderings

Scenario, ordering, expected outcome. Task 9 cites these rows; other tasks must not restate them.

| # | Scenario | Ordering | Expected |
|---|---|---|---|
| O1 | Commit intent's step index within one tick | one tick carrying reload intent, commit intent, a reload expiry, and a fire intent | the tick runs: reload intent → timer advance → expiry → **commit intent** → fire intent. The reload completes and credits, the lower begins on the same tick, fire is refused |
| O2 | Equip timer advance | `Lowering` entered at tick N with `lowerMs = 220`; ticks N+1… run with no further input | the remaining duration decrements every tick and expiry fires at zero. Asserted against the timed-state predicate directly, **not** through reload-activity |
| O3 | Direct-select and cycle on one input frame | both present | direct-select wins and commits; the cycle is discarded, not applied after; the cursor snaps to the direct-selected slot |
| O4 | One frame, three catch-up ticks, one commit intent | `ticks == 3`, dwell elapsed once before the frame | exactly one latch and one repoint; the intent is delivered at `tick_index == 0` only |
| O5 | Cycle arrives while `Lowering` | commit latched slot B, cursor moves to C and its dwell elapses | the commit re-latches to C; the lower timer is **not** restarted; exactly one repoint occurs |
| O6 | Commit intent on the tick the lower expires | expiry and intent in the same tick | expiry runs first — the repoint completes before a new latch is taken |
| O7 | Cursor returns to the active slot during `Lowering` | commit latched B, `Lowering` in flight on A, player cycles back to A and dwells | the switch to B completes; returning to A emits nothing. Cancelling an in-flight lower is not supported |
| O8 | Commit target equals active | cursor cycles back to the held weapon | no commit intent emitted — no lower, no presentation dirty |
| O9 | Zero `lowerMs` | commit with `lowerMs = 0` | `Lowering` is entered and expires on the same tick; the repoint happens exactly once |
| O10 | Zero `raiseMs` | commit with `lowerMs = 100`, `raiseMs = 0` | `Raising` is entered at the repoint and expires on the **next** tick the incoming instance is ticked, because only the active instance ticks and the incoming becomes active at the repoint |
| O11 | Timer overshoot at the handoff | a tick overshoots the lower's expiry by 12 ms, `raiseMs = 350` | the overshoot is **discarded** at the handoff; `Raising` starts at its full authored duration. Overshoot carry applies only within one instance's restarted step |
| O12 | Outgoing instance's terminal state | repoint at tick N; the player re-selects the outgoing slot much later | the outgoing instance was returned to idle with all timed-state fields cleared at the repoint; its re-equip runs a full raise from a clean start |
| O13 | Target slot is empty | loadout shorter than the selected index, or a slot whose instance despawned | cursor does not move; no commit intent; active unchanged |
| O14 | Slot emptied between cursor move and commit consumption | cursor moves to slot 2 at frame F; slot 2's instance despawns at tick T; commit consumed at T+1 | no repoint; active unchanged; cursor resets to the active slot; no error at gameplay severity |
| O15 | Active weapon despawns mid-switch, pawn survives | `Lowering` in flight on A, A despawned | the latch resolves or is dropped deterministically; the active index never points at a dead entity; no second repoint later |
| O16 | Commit intent on the tick of level unload | intent latched, then unload | no repoint after teardown; no dangling instance reference survives |
| O17 | Pawn despawns mid-switch | `Lowering` in flight, pawn removed | every slot's instance despawns with the pawn; no orphan weapon entity |
| O18 | Commit held across a host input gap | client commits at tick T; T+1..T+3 lost; host applies the gap-hold policy | the held command carries **no** commit intent — neutralized like the fire and use edges; exactly one repoint, at T |
| O19 | Commit dropped by catch-up fast-forward | the client's pending queue exceeds the buffer bound with a commit in the discarded prefix | the intent is lost, and the client's predicted active slot is corrected from the replicated slot index within one snapshot interval. No recovery lane |
| O20 | Client reconciliation with an unacked commit outstanding | commit sent at tick T; a correction at T+2 clears history without replay | the client converges to the host's active slot within one snapshot interval; no permanent divergence, no second repoint |
| O21 | Sim refuses a commit the input layer emitted | block-during-reload on, reload running | the commit is dropped, the cursor resets to the active slot, and the client converges via the replicated slot index — **not** by both peers evaluating the rule identically, which they cannot, since the client does not predict reload state |
| O22 | Two slots holding the same descriptor, client correction | loadout `[pistol, pistol]`, client predicts 0 → 1, host refuses | the client returns to slot 0, resolved against the replicated slot **index**; the archetype name is identical for both slots and cannot express the correction |
| O23 | Client and host equip resolution | connected client at 20 fps, host at 60 Hz, `lowerMs = 220` | both resolve the lower on the **same tick index**. The client's equip timers advance in its per-tick prediction pass, never from frame delta |
| O24 | Held fire across a switch | FIRE held continuously through commit, lower, repoint, raise, and expiry | stated per fire mode: an auto weapon fires on the first idle tick; a semi weapon does not until release and re-press. Both pinned |
| O25 | Held reload across a switch | RELOAD held continuously through the whole switch; incoming weapon has a non-full magazine and live reserve | stated: the incoming weapon does not auto-reload on the first idle tick; the player must release and re-press |
| O26 | Switch does not reset a fire cooldown | weapon A has 400 ms of cooldown remaining; switch away and back with a total equip time under 400 ms | A's remaining cooldown is preserved — the deploy clamp takes the later of the pre-existing deadline and the deploy deadline |
| O27 | Reload feedback at a preemption | per-shell reload with 3 shells credited and an unacknowledged start endpoint; commit arrives | the reload indicator reads false on the first tick of the lower; the 3 shells stay in the magazine; a cancellation carrying 3 is delivered exactly once |
| O28 | Feedback stranded by the repoint | an endpoint published on outgoing weapon A on the repoint tick; the frame's publisher samples B | A's endpoint does not replay when A is re-equipped; the reload indicator is false on the re-equip tick |
| O29 | Dwell elapses on a zero-tick frame | `ticks == 0`, dwell expires, the next frame produces one tick | the intent survives to the next tick-producing frame and is consumed exactly once; a second dwell expiry before consumption replaces the target rather than queueing a second commit |
| O30 | Wheel input clear | one scroll notch at frame F, no further input for 60 frames | the cursor moves exactly once. The scroll physical input is cleared each frame; a wheel emits no OS release event, so without a clear it would latch active forever |
| O31 | Multiple notches in one frame | N notches inside one frame | the cursor moves N steps against an explicit notch counter, and the dwell restarts once. A plain pressed/held button cannot express N |
| O32 | Last-weapon toggle with no history | toggle pressed before any switch has occurred, or after the previous slot emptied | no commit intent; active unchanged |
| O33 | Dwell override read cadence | the dwell is resolved when the cursor first moves | the dwell in force is the one resolved at cursor-move, not re-read each frame while running |
| O34 | Switch shorter than one snapshot interval | `lowerMs = 8`, `raiseMs = 8`, connected client, 30 Hz snapshots | accepted lossy: the client may never observe the switching flag true, and the current-weapon slot transitions in one step. Both slots are local projections, so no wire coalescing is involved |
| O35 | Switch contained in one multi-tick frame | `ticks == 3`, zero equip durations | the HUD publisher runs once per frame and publishes the final active weapon only; intermediate weapons are not published |
| O36 | Tuning payload arrives mid-switch | a valid new per-slot payload installed while `Raising` | the in-flight timer and the active index survive unchanged; the payload merges into the live inventory rather than re-materializing the pawn |
| O37 | Tuning payload decode failure mid-switch | a truncated or wrong-epoch payload arrives during `Lowering` | the live inventory and in-flight switch are unaffected; the previous tuning survives or the client is rejected with a diagnostic — never left with no tuning and a live inventory |
| O38 | Descriptor hot reload of equip durations mid-switch | `lowerMs` edited 220 → 800 while `Lowering` has 100 ms remaining | the in-flight step completes on the old total; the next switch uses the new one. The live total and the authored value never disagree in any published progress value |
| O39 | Loadout with one entry | single-entry loadout | cycle, direct-select, and last-weapon emit no commit intent; behavior matches today's single-weapon pawn |
| O40 | Two slots referencing the same descriptor | loadout `[pistol, pistol]` | two independent instances, each with its own magazine; the shared ammo type is seeded once |
| O41 | Two peers commit on one host tick | clients A and B commit to different slots on the same tick, same pawn class | each pawn repoints independently and exactly once; neither client's timing perturbs the other; no reconciliation correction for either |

## Tasks

### Task 1: Split `sim/weapon_stage.rs`

`crates/postretro/src/sim/weapon_stage.rs` is 3,452 lines and this plan adds two states, a new event variant, and its transition rows to the machine it hosts. Split it first, behavior-preserving, along the seams already visible in the file: the ordered per-tick machine driver, the state/event dispatch and its transition helpers, the fire authorization path, and the local/remote command entry points. No behavior, signature, or test assertion changes — this is a move, and the existing tests must pass unmodified. Carry the driver's numbered-step doc comment forward verbatim; it records that reload entry deliberately runs before expiry and fire, and later tasks depend on that order being legible. Leave the single-dispatch-point property intact: after the split there is still exactly one function matching state against event, with no wildcard arm.

### Task 2: Thin slice — inventory, equip states, tick-simulated switch, end to end

Build the narrow real version of the whole path and integrate it before anything fans out.

**Component.** Add an `Inventory` component on the pawn in `crates/entities` — an ordered array of optional wieldable entity ids, an active index, and the latched pending target of an in-flight switch. **The array's length is an engine capacity constant, not the authored loadout length**; name it and give it a value. A shorter loadout leaves trailing slots empty, and every bound in this plan — component indices, the wire sanitize clamp, the payload's per-slot cardinality, and the direct-select action count — binds to that constant, never to what a descriptor authored. The latch lives here rather than on the weapon component, because a latch on the outgoing instance dies if that instance despawns while its pawn lives. The component holds no cursor and no dwell: selection is the input layer's (Task 6). Name it for wieldables, not weapons, and keep it a peer of the pawn's `Health` and `AmmoReserve` — the reserve is not moved inside. `ComponentKind` uses explicit discriminants, so append rather than renumber; the component is never replicated, so it needs no snapshot record.

**States.** Add `Lowering` and `Raising` to `WieldableState`. Both deny fire and reload, and neither is reload activity. **This alone would freeze the timer**: the driver's timer-advance step and its expiry loop are both gated on the reload-activity predicate, so a state that is timed but not reload activity never decrements and never expires. Add a fourth `const fn` predicate meaning "is a timed state" — true for both reload states and both equip states — and re-gate those two sites on it, leaving the reload-activity predicate for the reload-indicator fallback. Note that the exhaustive-match argument does **not** surface these: they are calls, not matches. Audit equality comparisons against `WieldableState` by hand for the same reason; at least one gates a fire-cancels-shell-reload path.

**Dispatch.** The dispatch matches `(state, event)` with no wildcard arm, so adding two states and one event makes the domain twenty pairs and **all twenty must be written**. Name the load-bearing ones: `BeginLower` from idle, from atomic reload (the in-flight reload is forfeited — nothing has transferred yet, since the atomic path takes from the reserve only in its expiry arm), from per-shell reload (already-credited shells stay, mirroring the existing cancel row), from `Lowering` (re-latch, do not restart the timer), and from `Raising`; plus `(Lowering, Expired)`, which is the row the repoint hangs off, and `(Raising, Expired)`. Do not modify the existing cancel rows. Both `BeginLower` rows out of a reload state must **clear the outgoing weapon's pending reload feedback** for both consumers, the way the shell-loading cancel row already does — the feedback sample reports active from a queued unacknowledged endpoint regardless of state, so without this the reload indicator stays true through the whole lower.

**Repoint.** A commit intent latches the target; when `Lowering` expires the engine repoints the active index and starts `Raising` on the incoming instance. In the same operation, return the outgoing instance to idle with its timed-state fields cleared, and discard the lower's overshoot rather than carrying it into the raise. Only the active instance is ticked, so the incoming instance's raise first advances on the tick after the repoint. Apply a **deploy clamp**: the incoming weapon's next-fire deadline becomes the later of its existing deadline and the deploy deadline, so switching cannot shorten a cooldown.

**Client tick.** A connected client must advance its equip timers in the **per-tick prediction pass**, not the per-frame fire pass — reconciliation replays buffered inputs, and a frame-delta timer has no replayable timestep. The client branch of the tick loop already runs with `tick_dt` alongside movement prediction; add the weapon machine tick there.

**Spawn and seeding.** Spawn instances from a `loadout` on a new inventory block of the player descriptor, replacing the top-level `defaultWeapon` string — update the descriptor type, its Rust mirror, both typedef surfaces, and the dev mod's player script. There are **two** pawn spawn sites, and both must build the inventory: the player-start path and the network-slot path that spawns a host-simulated remote pawn's weapon. Seed the ammo reserve **once per distinct ammo type**, not once per slot: the seeding helper credits additively, so a per-slot loop would double a shared pool and quadruple it for a duplicate loadout. Weapons are declared as a `weapon` block on `defineEntity`; there is no `defineWeapon` and this plan must not add one (`plans/done/M10--weapon-primitives/index.md:101`). Add `lowerMs` and `raiseMs` to that block, and to the weapon component, its constructor, and its effective-stats projection; the descriptor-refresh path must copy them, and per O38 an in-flight step keeps its old total.

**Input, one path only.** Direct-select actions for slots — no wheel, no dwell, no last-weapon. The commit intent is **edge-shaped**: gate it on the first catch-up tick the way the dash, shoot, and use edges are, or a multi-tick frame will replay one press into several switches. Carry it on the command struct, mirrored onto the movement input for the client-prediction boundary as the use edge already is, with the wire mirror, a sanitize clamp against the slot capacity, and a **wire-version bump** — adding a field to a shipped message requires it. The gap-hold policy must neutralize the intent rather than carry it, alongside the fire and use edges it already neutralizes.

Presentation dirties at the repoint, not at the commit intent. Until Task 4 lands, a client predicting a switch to a slot whose tuning it lacks reuses the first slot's fire values.

### Task 3: Retire the global holder and converge every reader

Delete `App.active_wieldable` and `App.active_wieldable_descriptor`. The descriptor field is write-only — no read site exists in the tree, and hot reload refreshes weapons through descriptor provenance instead — so delete it rather than migrating it. The retirement reaches further than those two fields: the value is produced by the player-spawn result struct and carried through the level-install products struct, and both carriers must lose it too, along with the `default_weapon` spawn branch that fills them. The headless observability driver reads the *install-products* field, not `App`, and is a real consumer with no gameplay acceptance criterion covering it.

`WeaponOwners` cannot simply lose its map: the map **is** its change-detection mechanism and its payload — `set` marks dirty by comparing against the stored entry, `remove_pawn` marks dirty only when an entry existed, and the drain returns the weapon id read back out of it. Reduce the type to an explicitly-marked dirty set whose drain resolves the weapon id from the pawn's `Inventory`, with the repoint as the only site that marks dirty.

Every reader of the two holders resolves through the pawn's `Inventory`: the fire path, the HUD publisher, the per-frame viewmodel resolution (host, client, and the single-player fallback that synthesizes a throwaway owner map each frame), the level-install attachment sync, the snapshot's active-archetype fill, the owner-private ammo and cooldown projections, the remote-pawn command preparation, and the despawn cleanup that treats pawn and weapon as one ownership unit — extended to every slot, since a pawn now owns several. On a connected client the viewmodel must resolve from the **predicted** `Inventory` rather than the replicated archetype string, or it will lag the prediction by a round trip.

Remove the listen-host bridge that copied the global into the owner map after install: the host's own pawn now carries an inventory like any other pawn. Level teardown clears the component with the pawn and leaves no dangling instance reference.

Retire the single-weapon client prediction struct **as a holder**: which wieldable is active comes from the client's own `Inventory`. What remains client-side is a per-slot carrier of the three genuinely predicted fire scalars. Only the active slot reconciles against the host's cooldown fact, which describes the active weapon only; inactive slots advance locally and re-sync on becoming active. Keep the rule that a connected client never resolves weapon *tuning* from its local registry.

### Task 4: Per-slot tuning payload and the slot-index correction channel

Grow the host→client tuning payload from one weapon's fire values to the set the pawn holds, and bump its epoch so a client on the old shape is rejected with a diagnostic. It becomes an ordered per-slot set sized by the engine slot capacity, each occupied entry carrying that weapon's **canonical archetype name**, its fire values, its lower and raise durations, and its resolved block-during-reload rule, alongside the same movement descriptor. The archetype name is required, not optional: without it the client cannot spawn per-slot instances, cannot pick the incoming viewmodel at its predicted repoint, and cannot publish a weapon name. Models resolve locally from that name — render-only tuning stays local — while fire tuning still never resolves locally.

Resolve the payload from the pawn's live `Inventory`, not the authored descriptor array, so pickup needs no re-source later. Be honest about the limit: the send is change-detected but reachable only from slot lifecycle and post-install, so nothing polls it and a mid-level inventory change is not detected today. That is acceptable here because the loadout is fixed at spawn; pickup must add a send trigger.

Carry no cursor, dwell, or player preference — those are input-layer values, and putting them on the wire would reintroduce the divergence the layering prevents. Keep the canonical-JSON control-channel transport, the per-client change detection, the two send sites, and the rule that the movement descriptor's view-feel field is cleared. Update the committed fixture.

Add the **correction channel**: a replicated owner-private active-slot **index**. The existing replicated archetype name cannot express a correction when two slots hold the same archetype, and weapon state does not participate in movement replay. On disagreement the client snaps its active index to the host's and returns any in-flight equip state to idle — a snap, not a replayed switch. Install must merge per-slot tuning into the live inventory rather than re-materializing the pawn, and a decode failure must not clear the client's tuning while leaving a live inventory.

### Task 5: Mod-global switch policy and its per-weapon override

Add a `switching` block to the mod manifest carrying the game's switch rules, and a per-weapon override for the one rule the simulation evaluates. The manifest already carries stores, UI trees, themes, map catalogs, and frontend declarations as data rather than import-time side effects, so this needs no new registration primitive — but note the edit sites: `defineMod` is an identity builder needing no change and the typedef surface is generated, so the real work is the canonical Rust manifest struct, the registered SDK type a parity test holds in lockstep with it, and the manifest drain and validation path.

The block declares whether a direct-select commits immediately, the cycle-commit dwell in milliseconds, and whether a commit may interrupt reload activity. Only the last is a simulation rule; the first two are read by the input layer and never leave the machine they run on. The block-during-reload rule takes an optional per-weapon override on the weapon block — resolve it through the weapon's effective-stats accessor rather than the authored field, so a future augment altering fixed classifiers has somewhere to land. That accessor projects only stored component fields, so this requires a field on the weapon component, a line in its constructor, a line in its descriptor-refresh path, and a field on the effective-stats struct.

Validate the block where the mod's declarations are validated, and pin the consequence rather than saying "rejected": a negative or non-finite dwell **aborts mod init** with a diagnostic naming the field, matching how map catalog, theme, and frontend declarations already fail. Both descriptor parsers must degrade identically. The resolved per-weapon value is what Task 4 puts in the payload.

### Task 6: Input layer — wheel, cursor, dwell, last-weapon, and the player override

Own everything between the player's hardware and a commit intent, and let nothing below it see the parts that are local.

Add discrete scroll physical inputs and a window-event arm producing them; a scroll notch is a momentary button, not an analog axis, because routing an analog delta to the sim would mean a new analog field on the movement input, the wire mirror, prediction replay, and sanitization. Normalize the two scroll-delta units the windowing layer reports, accumulating the pixel-based one to a threshold; there is no precedent for either in the tree, so cover both in tests and put the threshold on the player-options surface beside the other device-feel values rather than hardcoding it. Two consequences the shipped input path forces: a wheel emits **no OS release event** and the per-frame reset clears only the mouse accumulators, so the scroll inputs must be cleared explicitly each frame or they latch active forever; and a pressed/held button cannot express two notches in one frame, so carry an explicit notch count if the cursor is to move more than one step per frame.

Bind the scroll inputs to cycle-next and cycle-previous, add direct-select actions up to the engine slot capacity with their default number-key bindings, and add a **last-weapon-toggle** action returning to the previously active slot — near-universal in the genre and the basis of the quick-switch binds competitive players build. Hold its memory here, updated at each committed repoint.

Hold the pending cursor and its dwell timer here, not in any component. A cycle moves the cursor and restarts the dwell; a direct-select moves it and, if the mod declared immediate commit, emits at once; the dwell elapsing with no further movement emits. A cursor movement onto the active slot or an empty one emits nothing and does not move. Resolve the dwell as the player's override when set, else the mod's declared value, resolved once at cursor-move; add that override to the persisted player-options store as an optional field with `serde(default)`, following the crouch-mode precedent for an input-layer preference with no SDK surface and, per standing policy, no settings menu.

The dwell runs at frame rate and can elapse on a frame producing **zero ticks**, which the shipped input latch drops. Name an input-layer holder that persists the emitted intent until a tick consumes it, name the clear point after consumption, and state that a second emission before consumption replaces the target rather than queueing. The fire path's zero-tick special case and manual clear are the precedent.

The only thing crossing into the simulation is a bounded commit intent naming a slot, edge-gated to the first catch-up tick, clamped by sanitization against the slot capacity. Scroll must remain unclaimed by the debug overlay during gameplay focus, which is already the case.

### Task 7: `player.weapon.*` state slots

Publish the active wieldable's identity as engine-owned state so a HUD can show what is held and what is pending. Add three built-in engine state slots — the current weapon's canonical archetype name, the pending selection's archetype name, and whether a switch is in flight — declared in the engine state catalog beside the existing weapon-adjacent slots; two string-typed, one boolean, both supported, with nested SDK paths expressed as explicit segment arrays as the catalog already allows.

**All three are local scope, not replicated.** Each is a projection of whichever `Inventory` the machine owns — the host's authoritative one, the client's predicted one — so a client's HUD follows its own prediction rather than lagging by a round trip, and the two converge because Task 4's correction channel converges the inventories. This is a deliberate departure from the owner-private ammo, reserve, reload, and cooldown slots, which stay host-authoritative and replicated; the difference is that those carry values only the host knows, while weapon identity is already predicted locally. Feed all three from the same per-frame publisher on every role, dropping the connected-client short-circuit for these three slots only — it must remain for the host-authoritative ones. Note the consequence for authors: because these are local, a mod crossing on them fires per-machine rather than authoritatively; they are display values.

The current-weapon slot names the committed active instance, so it flips at the repoint and not at the commit intent — during the lower it still names the outgoing weapon.

### Task 8: Typed loadout references and the dev-mod reference

Make the loadout hold descriptor references rather than name strings, and author the dev mod against them. Today a weapon is named by a string resolved at level install, where an unregistered name and a name resolving to a weapon-less descriptor both degrade to a warning and an unarmed player. Change the loadout's element type to the descriptor value `defineEntity` already returns — it is a pure identity builder, mod scripts already import each other across files, and the referring script sits in the same bundle as the referenced one, which is the condition the durable naming rule gives for preferring a reference over a string. Constrain the accepted type to descriptors declaring a weapon block so a weapon-less reference is an editor-time error, and reject it again at declaration validation, naming the offending entry and aborting mod init — the type constraint is not an enforced gate, because the repo runs no typecheck step in CI.

Name the lowering step: the manifest drain rewrites each loadout entry to its canonical name before it reaches Rust, in **both** runtimes, so nothing new crosses the FFI. A referenced descriptor whose canonical name is absent is a validation error naming the entry, since the field is optional on the descriptor type. **Depend only on value equality, never object identity**: the Luau require implementation performs no module caching, so requiring one file twice yields distinct objects, and mod-init and level data scripts are separate bundles in separate VMs.

Update the dev mod's player script to import both reference weapon descriptors into its loadout, and declare the mod's switching block in the start script with the wheel dwell and the direct-select rule, so number-key, wheel, and last-weapon behaviors are all exercisable in-game.

### Task 9: Ordering and edge coverage

Cover every row of the Orderings table with a test that names its scenario, and cover the state-preservation, reserve-seeding, and deploy-clamp invariants directly rather than only through the switch path. The rows span three layers — input-layer cursor, dwell and notch handling; the state machine's transition rows and tick order; and host/client convergence — so place each test at the layer that owns its outcome rather than driving everything through a full-session harness. The zero-duration, empty-slot, single-entry, duplicate-descriptor, overshoot, and terminal-state rows belong at the machine and component level. The convergence, refusal, dropped-intent, and two-peer rows need the two-peer path; use the existing latency-simulation harness for the loss-and-jitter criterion rather than standing up a new one. The tick-order row deserves a direct assertion on the step sequence, since two shipped behaviors depend on reload entry preceding expiry.

## Sequencing

**Phase 1 (sequential):** Task 1 — behavior-preserving split; every later task edits this file.
**Phase 2 (sequential):** Task 2 — thin slice, falsifies the boundary assumptions across input, sim, component, presentation, and the client's tick-rate prediction path before anything fans out.
**Phase 3 (sequential):** Task 5 — Task 4 carries its resolved reload-block rule, so the rule must exist first.
**Phase 4 (concurrent):** Task 3, Task 4, Task 7 — independent surfaces on the slice's and Task 5's contracts. Task 7 ships its pending slot defaulted empty; Task 6 attaches the producer.
**Phase 5 (sequential):** Task 6 — consumes Task 5's declared dwell and rules, Task 2's commit-intent path, and Task 7's pending slot.
**Phase 6 (concurrent):** Task 8, Task 9.

## Rough sketch

`Inventory` lands in `crates/entities/src/components/inventory.rs` with a new `ComponentKind` and `ComponentValue` arm, following `AmmoReserve` (`crates/entities/src/components/ammo_reserve.rs`) as the nearest shape precedent — private storage, small accessor surface. `ComponentKind` uses explicit discriminants (`crates/entities/src/registry.rs`), so `Inventory` appends at the next free value and no existing wire discriminant moves.

Machine work is confined to the dispatch function Task 1 splits out of `sim/weapon_stage.rs`. `WieldableState` gains `Lowering`/`Raising` and a timed-state predicate in `crates/entities/src/components/wieldable_state.rs`. The new `BeginLower` event sits beside `BeginReload`/`Expired`/`Cancel`; the existing reload-cancel rows are untouched. Reload feedback clearing reuses the helper the shell-loading cancel row already calls.

The client weapon tick goes beside `client_predict_movement_tick` in the connected-client branch of the tick loop in `main.rs`, which already carries `tick_dt`. The commit intent's edge gate follows the `tick_index == 0` pattern used for the dash, shoot, and use edges in the same loop; the gap-hold neutralization follows `held_gap_sim_command` in `netcode/command_queue.rs`.

The cursor, dwell, notch counting, and last-weapon memory live beside the crouch-mode resolution in `crates/postretro/src/input/`; the dwell and notch-threshold overrides go beside `crouch_mode` in `crates/postretro/src/options/mod.rs`.

Manifest changes are **not** in `sdk/lib/data_script.ts` — `defineMod` needs no edit for a new field, and `sdk/types/postretro.d.ts` is generated. The real sites are `ModManifestResult` in `crates/scripting-core/src/runtime/types.rs`, the registered `ModManifest` shape in `crates/postretro/src/scripting/primitives/manifest.rs` (a parity test holds those in lockstep), the manifest drain and validation path, and regenerated typedefs. The weapon block's new fields are on `WeaponDescriptor` in `crates/foundation/src/data_descriptors/types/combat.rs`, on `WeaponComponent` and `EffectiveStats` in `crates/entities/src/components/weapon.rs`, and in that component's constructor and descriptor-refresh path.

Tuning payload changes are local to `crates/postretro/src/netcode/tuning_payload.rs` plus its build/send path in `netcode/mod.rs`; the committed fixture is `crates/postretro/src/netcode/tests/fixtures/tuning_payload.expected.json`. The correction slot is an entry in the owner-private projection in `netcode/state_slots.rs`.

Input additions touch `crates/postretro/src/input/types.rs`, `input/defaults.rs`, the `WindowEvent` match in `main.rs`, the command build, and `netcode/wire_convert.rs` for the wire mirror and sanitize clamp. The wire-version constant lives in `crates/net/src/handshake.rs`.

State slots are entries in `BUILTIN_ENGINE_STATE` (`crates/entities/src/engine_state_catalog.rs`), fed by `scripting/systems/ui_proxy.rs`.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Inventory component | `ComponentValue::Inventory` | `"inventory"` — **serde tag only, never a replicated payload** | `inventory` (descriptor block) | `inventory` | n/a |
| Loadout list | `InventoryDescriptor::loadout` | `"loadout"` (array of canonical-name strings) | `loadout` (array of descriptor refs) | `loadout` | n/a |
| Slot capacity | `WIELDABLE_SLOT_CAPACITY` | n/a (bounds the wire array and the sanitize clamp) | n/a | n/a | n/a |
| Commit intent | `SimCommand::commit_wieldable_slot` / `MovementInput` mirror | `commit_wieldable_slot` on the wire movement input; **wire version bumps** | n/a | n/a | n/a |
| Active slot (correction) | owner-private projection | `"player.weapon.activeSlot"` | not author-facing | n/a | n/a |
| Lower duration | `WeaponDescriptor::lower_ms`, `WeaponComponent::lower_ms`, `EffectiveStats::lower_ms` | `"lowerMs"` | `lowerMs` | `lowerMs` | n/a |
| Raise duration | `WeaponDescriptor::raise_ms`, `WeaponComponent::raise_ms`, `EffectiveStats::raise_ms` | `"raiseMs"` | `raiseMs` | `raiseMs` | n/a |
| Per-weapon reload-block override | `WeaponDescriptor::block_switch_during_reload`, `WeaponComponent`, `EffectiveStats` | `"blockSwitchDuringReload"` | `blockSwitchDuringReload` | `blockSwitchDuringReload` | n/a |
| Mod switching block | `ModManifestResult::switching` | `"switching"` | `switching` | `switching` | n/a |
| Direct-select commit rule | `SwitchingDescriptor::commit_on_direct_select` | `"commitOnDirectSelect"` | `commitOnDirectSelect` | `commitOnDirectSelect` | n/a |
| Cycle dwell | `SwitchingDescriptor::cycle_commit_dwell_ms` | `"cycleCommitDwellMs"` | `cycleCommitDwellMs` | `cycleCommitDwellMs` | n/a |
| Reload-block rule | `SwitchingDescriptor::block_during_reload` | `"blockDuringReload"` | `blockDuringReload` | `blockDuringReload` | n/a |
| Dwell player override | `PlayerOptions::switch_cycle_dwell_ms` | `switch_cycle_dwell_ms` (TOML, snake_case) | n/a — no SDK surface | n/a | n/a |
| Scroll notch threshold | `PlayerOptions::scroll_notch_pixels` | `scroll_notch_pixels` (TOML, snake_case) | n/a — no SDK surface | n/a | n/a |
| Lowering state | `WieldableState::Lowering` | `"Lowering"` | n/a | n/a | n/a |
| Raising state | `WieldableState::Raising` | `"Raising"` | n/a | n/a | n/a |
| Current weapon slot | n/a (catalog entry) | `"player.weapon.current"` | `getGameState().player.weapon.current` | same path | n/a |
| Pending weapon slot | n/a (catalog entry) | `"player.weapon.pending"` | `getGameState().player.weapon.pending` | same path | n/a |
| Switching flag slot | n/a (catalog entry) | `"player.weapon.switching"` | `getGameState().player.weapon.switching` | same path | n/a |

## Script syntax examples

```ts
// Proposed design — a weapon is a `weapon` block on defineEntity.
// There is no defineWeapon (plans/done/M10--weapon-primitives).
export const referenceShotgunEntity = defineEntity({
  canonicalName: "reference_shotgun",
  components: {
    weapon: {
      damage: 12, range: 1200, fireRateMs: 700,
      fireMode: "semi", resolution: "hitscan",
      resource: { kind: "ammo", type: "shells.buck", magazine: 8,
                  costPerShot: 1, reserve: 32, reloadMs: 450,
                  reloadStyle: "perShell" },
      lowerMs: 220,
      raiseMs: 350,
      // Optional override of the mod-global rule, for this weapon only.
      // A per-shell reload is cheap to abandon; a launcher's load is not.
      blockSwitchDuringReload: false,
    },
  },
})
```

```ts
// Proposed design — the loadout holds descriptor references, not strings.
// A typo is an unresolved import; a weapon-less reference is a type error.
import { referenceShotgunEntity } from "./reference-shotgun";
import { referencePistolEntity } from "./reference-pistol";

export const playerEntity = defineEntity({
  canonicalName: "player",
  components: {
    movement: playerMovement,
    // Replaces the shipped TOP-LEVEL `defaultWeapon` string. M10 placed equip
    // at the top level deliberately ("equip is a different concern at the same
    // level"); moving it into `components.inventory` reverses that, because
    // equip now carries per-pawn runtime state and not just a name.
    inventory: { loadout: [referenceShotgunEntity, referencePistolEntity] },
  },
})
```

```ts
// Proposed design — switch rules are declared once per game on the manifest,
// not per character class. Only blockDuringReload reaches the simulation;
// the other two are read by the input layer and never cross the wire.
export default defineMod({
  id: "dev", name: "Dev Mod", version: "0.1.0",
  entities: [playerEntity, referenceShotgunEntity, referencePistolEntity],
  switching: {
    commitOnDirectSelect: true,
    cycleCommitDwellMs: 500,
    blockDuringReload: false,
  },
})
```

## Open questions

- **Pixel-delta scroll normalization has no precedent.** Task 6 routes the notch threshold to player options rather than hardcoding it, but the shipped default is picked against nothing in the tree and no fixture exercises trackpad input. Reversible; the risk is that it reads badly out of the box on hardware nobody tested.
- **Whether the per-weapon reload-block override earns its keep.** It is the plan's only two-level resolution, and neither reference policy needs it. It is here because the design call it encodes is genuinely per-weapon, but a reviewer should judge whether one authored knob justifies a resolution order that the weapon component, the effective-stats projection, and the payload all have to carry.
- **Local `player.weapon.*` slots are a new scope precedent for `player.*`.** Every other `player.*` slot is owner-private replicated. Making these three local is what lets a client's HUD follow its own prediction, but it means a mod crossing on them fires per-machine. Worth confirming that no shipped authoring pattern assumes `player.*` is authoritative.
