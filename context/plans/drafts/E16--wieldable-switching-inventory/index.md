# Wieldable Switching + Inventory

## Goal

Give a pawn an ordered inventory of wieldable instances and one active reference that repoints between them, preserving each instance's own state. Converge the three divergent active-weapon holders onto that inventory so single-player, listen host, and connected client read one source of truth. A client declares its own switches and the host validates them, matching the shipped client-authoritative combat model.

## Scope

### In scope

- An `Inventory` component on the pawn holding an ordered set of wieldable slots plus the active index.
- Separation of **selection** (a local cursor — changes nothing held, never leaves the machine it runs on) from **commitment** (a declaration that repoints the active reference).
- `Lowering` and `Raising` equip states on the shipped wieldable machine, with per-archetype durations, advanced by a new timed-state predicate that is **not** the reload-activity predicate.
- Retirement of `App.active_wieldable` and `App.active_wieldable_descriptor`, of `WeaponOwners`' pawn→weapon map, and of `ClientWeaponState` as a holder — including the two structs that carry the value out of level install.
- A **switch declaration** from client to host on the reliable path, with host validation and a rejection the client applies.
- **Possession-based fire validation**: the host resolves a declared shot's weapon from the client rather than from its own active pointer, and accepts it if the pawn possesses that weapon.
- Per-slot archetype identity and equip durations in the host→client tuning payload, so a client can run and present its own switch.
- A mod-global `switching` block on the mod manifest, with a per-weapon override for its one simulation-side rule.
- Two player-options fields — the cycle-commit dwell override and the pixel-scroll notch threshold — resolved in the input layer, with no settings UI.
- Discrete scroll physical inputs; direct-select, cycle, and last-weapon-toggle actions; an input-layer cursor and dwell.
- A **deploy clamp** so switching cannot shorten a fire cooldown.
- `player.weapon.current`, `player.weapon.pending`, `player.weapon.switching` engine state slots, local on every role.
- A `loadout` array on the player descriptor's inventory block, holding **descriptor references** rather than name strings, replacing the single `defaultWeapon` string.

### Authority model

Switching follows the model `E16--client-authoritative-combat` established for per-shot geometry: the client acts, the host validates. It is not predicted and reconciled the way movement is; the Direction section argues why.

| Concern | Owner |
|---|---|
| Cursor, dwell, direct-versus-cycle, last-weapon memory, player overrides | Input layer, local, never replicated |
| When to switch, and running the local lower/raise | Client, authoritative for itself |
| Whether a switch is permitted; the authoritative active slot | Host, validating the declaration |
| Ammo, fire rate, magazine debit | Host — **already shipped, unchanged by this plan** |
| Which weapon a declared shot came from | Client declares it; host validates possession |

Two consequences follow. Equip timers do **not** need to land on the same tick index on both peers. And a refused switch leaves only a presentational difference, because the client's shots still validate on possession.

**Possession-based validation accepts a cheat surface.** A client can declare a shot from any owned weapon at any time, including while visibly holding another, or alternate declarations to get two weapons' fire rates. Accepted because `context/lib/index.md` §4 non-goals anti-cheat and competitive PvP and this is co-op, and because each shot still debits a real per-instance magazine. Remote players may briefly see the held and firing weapons disagree during an equip window, bounded by the equip duration.

### Out of scope

- **Pickup and drop.** The map-placement spawn path never attaches a weapon component, even to a descriptor that is otherwise placeable — pinned by `map_sweep_skips_weapon_component_on_otherwise_placeable_descriptor` in `scripting/builtins/data_archetype.rs`. So no weapon instance can exist in the world to be picked up. (`is_directly_map_placeable` alone does **not** foreclose this: it returns true for any descriptor carrying `light`, `emitter`, `movement`, `mesh`, or `health`.) Roadmap `E16 › Weapon Systems › pickup` owns the feature.
- **Prediction and reconciliation of weapon state.** Deliberately not built: under the authority model above there is nothing to reconcile, because the client is authoritative for its own switch and the host validates rather than re-derives. Weapon state stays out of the movement replay set.
- **A settings UI for either player option.** `context/lib/player_options.md` §4 splits the store from the E13 settings menu, and §3 records that no save-on-change occurs until that menu is wired.
- **Radial-selector time dilation.** Under authoritative co-op, dilation is a server-side gameplay decision and a client-local slowdown desyncs on contact. Selecting a weapon never alters time. (Throughout this spec *scroll* means the physical mouse wheel; *radial selector* means the unbuilt ring widget, deferred at `context/plans/roadmap.md:149`.)
- **Mod-level loadout selection and pre-level loadout menus.** The direction is that a loadout is chosen at the mod level and eventually through a menu before entering a level. Neither exists, so nothing produces the case; this plan resolves composition from the player descriptor at pawn spawn.
- **Dual-wield.** This plan's `Inventory` carries a single active index and the authored loadout has no off-hand position — a type that cannot represent the state. Reachable by extension, and the roadmap sequences it after switching (`roadmap.md:247`).
- **Augments, rolls, and non-passthrough stat resolution.** `WeaponComponent::effective` takes `&self` and projects only stored fields; the component stores no modifier data.
- **Heat and cell resources.** `WeaponResource` has exactly one arm.
- **Secondary activation / alt-fire.** `Action::AltFire` is bound with zero consumers outside `input/`.
- **Mod-authored input bindings.** The SDK exposes no `Action` type or action-read surface.
- **Compile-time enforcement of loadout references.** No `tsc` step in CI — `content/dev/scripts/typed-handles-fixture.ts` states this in its own header. The enforced gate is the validation this plan adds.

### Ships knowingly broken — owner decision

**Inventory and ammo reserve do not survive a level transition.** Nothing in source forecloses carrying them; this is a choice. The durable per-player key carry needs is the host-minted **seat**, unbuilt in E15 Phase 3.75 (`roadmap.md:202`), which `drafts/E16--per-player-currency` is already parked on. Building carry now means blocking on that spec or standing up a single-player-only carry path — a fourth divergent holder, which is what this plan exists to remove. Consequence: every level re-equips from the player descriptor and re-seeds the reserve. Owner decision, 2026-07.

## Direction

**Problem.** A pawn's active weapon is stored three different ways and none can change at runtime. `App.active_wieldable` is written only at level install and teardown; `WeaponOwners` is host-only; a connected client owns no weapon entity and models its weapon as two floats and two enums resolved from the pawn class's `default_weapon`. There is no place to put a second wieldable and no path by which the active one could change. The cause is that "the weapon" was modeled as a property of the *session role* rather than of the *pawn*.

**Prior commitments.** `weapon-model.md` §6/§7 pins the shape: switching repoints an active reference, per-instance state survives because instances own it, and the container and its equip machinery are named for **wieldables**, not weapons (invariant 7), with inventory a peer of the pawn's `Health` and `AmmoReserve` rather than a parent (§1). `crates/entities/src/components/wieldable_state.rs` states that equip states join that enum when switching owns their behavior, and `E16--weapon-state-machine` shipped its preemption seam for exactly this.

`E21--coop-avatar-weapon-presentation` deferred the switch input path to this plan (`index.md:33`) and shipped what assignment needs — the replicated active-archetype field, client-side change detection, and the hand-socket rewrite. Remote-avatar presentation follows once the holder moves. Two paths are **not** free and are named as work in Task 3: the host snapshot fill reads the map Task 3 reduces, and the client's own viewmodel resolves through a replicated archetype value that must yield to its local `Inventory`.

`E16--client-authoritative-combat` is the model this plan follows for authority, and `E16--ammo-resource` already made ammo host-authoritative and per-instance. This plan changes neither; it changes only which weapon instance a declared shot resolves to.

The mod-global block follows the shipped manifest rule: `scripting.md:49` records that stores, UI trees, themes, map catalogs, and frontend declarations arrive as **manifest data, not import-time side effects**.

This diverges from two shipped decisions. `defaultWeapon` is **replaced** by a `loadout` array rather than kept as sugar, because a second one-weapon path preserves the divergence this plan removes. And that loadout moves from the descriptor's **top level** into `components.inventory`, reversing M10's placement ("equip is a different concern at the same level") — justified because equip now carries per-pawn runtime state rather than a name. `content/dev/scripts/player.ts` is the sole content consumer.

**Placement.** The inventory and repoint sit in the engine: state a host owns and a client mirrors. The reload-interrupt rule sits in mod-declared data, per-weapon-overridable, because abandoning a per-shell reload and abandoning an atomic load are different design calls. The cursor, dwell, last-weapon memory and player overrides sit in the input layer beside `crouch_mode`, because they describe how one person's hardware is interpreted.

**Alternatives rejected.** *Movement-style predict-and-reconcile* — put weapon state in the replayed set and correct by snapping to authoritative state and replaying unacked commands. Rejected because it solves drift, and a discrete declaration does not drift; it would require a correction channel, a snapshot-recency gate with no live comparand for an unmoving pawn, and tick-exact equip agreement, all to arbitrate an event the host can simply accept or refuse.

*Host-authoritative switching with no client-side run* — the client requests and waits. Rejected because the lower would not begin until a round trip completed, making switching the one player action that visibly waits for the network.

*An authored IR guard for the commit rule* over a `@wieldable.*` namespace is rejected: it would hang off the player entity descriptor, so every character class re-declares an identical rule, and a switch-commit author sets numbers against engine-fixed structure rather than inventing structure.

*A side-table keyed by pawn*, generalizing `WeaponOwners`, is rejected because a side table is what made the state divergent — it lives on `NetEndpoint::Host` and cannot exist single-player, which is why `App.active_wieldable` exists at all.

**Foreclosures and one-way doors.** The tuning payload epoch bump rejects older peers, which is what an epoch is for. Replacing `defaultWeapon` is a breaking descriptor change, bounded but not free. Possession-based fire validation is reversible but its cheat surface is accepted deliberately. Nothing here forecloses pickup, dual-wield, or augments: all three extend the slot array or the instance.

## Acceptance criteria

- [ ] A pawn spawns holding every wieldable its loadout references, first slot active; each slot holds a distinct instance, and two slots referencing the same descriptor hold two independent instances.
- [ ] A direct-select for an occupied non-active slot plays the outgoing weapon's lower, then the incoming weapon's raise, and the incoming weapon becomes active exactly once.
- [ ] Scrolling moves the pending selection without changing what is held; the switch begins only after the resolved dwell elapses with no further scroll, and a scroll during the dwell restarts it. Scroll cycles only occupied slots, so a loadout smaller than the slot capacity still cycles.
- [ ] A last-weapon-toggle returns to the previously active slot, and pressing it twice returns to where the player started.
- [ ] A weapon switched away from and back to retains its own magazine, cooldown, and reload progress — it is not re-created and does not inherit another weapon's values.
- [ ] Switching cannot shorten a fire cooldown: a weapon switched away from and back to is no readier to fire than if held throughout.
- [ ] Firing and reloading are refused for the whole lower and raise, and the reload indicator reads false from the first tick of the lower — including when the switch preempted a reload that had already published feedback, and including when that reload completed on the same tick the switch began.
- [ ] With the mod's reload-interrupt rule off, switching during a per-shell reload keeps the shells already loaded and during an atomic reload loads none; with it on, the switch does not begin until the reload resolves. A per-weapon override wins for that weapon only.
- [ ] Ammo reserve is seeded once per ammo type regardless of how many slots draw on it; two weapons sharing a type draw from one pool, and switching moves, duplicates, or resets nothing.
- [ ] A connected client's switch begins on the input frame, without waiting for a host round trip.
- [ ] A host-refused switch leaves the client and host agreeing on the active slot once the refusal is applied, with no second visible transition.
- [ ] A client's declared shot is accepted when the pawn possesses that weapon, including during an equip transition on either peer, and rejected when it does not. The shot debits that weapon's magazine, not the host's notion of the active one.
- [ ] A remote player's avatar shows the weapon they switched to, and the local player's viewmodel shows theirs, on both host and client. A pawn that spawns and never switches shows its weapon at the hand socket on both roles.
- [ ] The HUD's ammo, reserve, reload, and cooldown values follow the active weapon across a switch on both roles.
- [ ] Two players with different dwell overrides each switch on their own timing, and neither affects the other.
- [ ] Mod init aborts with a diagnostic naming the offending entry when a loadout references a descriptor with no weapon block, a descriptor with no canonical name, or a value that is not a descriptor at all; and when the switching block declares a negative or non-finite dwell.
- [ ] Every ordering in the Orderings table resolves to its stated outcome.
- [ ] A client whose tuning payload predates this change is rejected with a diagnostic rather than running on stale weapon values.
- [ ] Selecting an empty slot, or an index beyond the loadout, leaves the active weapon unchanged and logs no error at gameplay severity.
- [ ] Every weapon instance a pawn owns is despawned when that pawn's slot closes or is demoted; none leak.
- [ ] The engine reports the same active wieldable in single-player, on a listen host, for a host-simulated remote pawn, and on a connected client.
- [ ] A mid-level descriptor reload that changes a weapon's equip durations updates both roles; one that changes the loadout list does not recompose a live inventory and takes effect at the next level install.
- [ ] Reload behavior is unchanged: atomic reload still cannot be cancelled by the existing cancel path, per-shell reload still cancels with its shells credited, and both transfer the same rounds for the same inputs.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Exactly one live wieldable machine per pawn, and exactly one instance in an equip state at end of tick | Task 2 | Task 3 removes the global that could name a second; a slot holding a despawned instance must clear | AC 1, 21 |
| A repoint preserves the instance's own state | Task 2 | Task 5 must not rebuild a component from tuning; Task 3's rewires must not re-seed | AC 5, 9 |
| Exactly one repoint per accepted commit, and no fire authorized on the repoint tick | Task 2 | a commit arriving on a repoint tick; a zero-duration lower | AC 2, 7, O4, O5 |
| Fire and reload are refused for the whole switch, and a switch is never reload activity | Task 2 | the feedback stream reports active from a queued endpoint regardless of state | AC 7 |
| Ammo reserve is pooled on the pawn, seeded once per type | Task 2 | Task 9's multi-slot loadout is the first case with more than one seeding source | AC 9 |
| Every owned instance is despawned with its pawn | Task 3 | slot close despawns the pawn before cleanup runs, so instances must be read first | AC 20 |
| The hand socket reflects the active weapon from spawn, not only after a switch | Task 3 | reducing `WeaponOwners` to a dirty set must keep its spawn and removal marking sites | AC 13 |
| No cursor, dwell, or player preference reaches the simulation | Task 7 | Task 5 must not carry them; Task 8's slots are local | AC 15 |
| A declared shot is validated on possession, never on the host's active pointer | Task 4 | Task 3 rewires the site that resolves the firing weapon | AC 12 |

## Orderings

Scenario, ordering, expected outcome. Task 10 cites these rows; other tasks must not restate them.

| # | Scenario | Ordering | Expected |
|---|---|---|---|
| O1 | Equip timer advance | `Lowering` entered with a non-zero duration; later ticks run with no input | the remaining duration decrements every tick and expiry fires at zero, asserted against the timed-state predicate directly rather than through reload-activity |
| O2 | Commit and a reload expiry on one tick | the reload started earlier this tick expires, and a commit is present | the reload completes and credits, then the lower begins; the reload indicator reads false on that same tick; exactly one reload lifecycle reaches scripts |
| O3 | Commit while a reload is running, rule off | atomic reload in flight | the reload is forfeited with nothing transferred and a single terminal reload event; per-shell keeps its credited shells and delivers one cancellation carrying that count |
| O4 | Commit arriving on a repoint tick | a lower expires and repoints this tick; a commit to a third slot arrives the same tick | the commit applies to the newly active instance, not the outgoing one; exactly one instance is in an equip state at end of tick; exactly one repoint occurs |
| O5 | Zero-duration lower | commit with a zero lower duration | exactly one repoint, resolved within the tick; the outgoing instance ends idle with cleared timed fields and that survives the tick's write-back |
| O6 | Zero-duration raise | commit with a zero raise duration | the incoming instance leaves the raise on the first tick it is itself ticked, which is the tick after the repoint |
| O7 | Timer overshoot at the handoff | a tick overshoots the lower's expiry | the overshoot is discarded; the raise starts at its full authored duration |
| O8 | Outgoing instance's terminal state | repoint completes; the player re-selects that slot much later | it was returned to idle with timed fields cleared, and re-equips with a full raise from a clean start |
| O9 | Fire held across a switch | fire held through commit, lower, repoint, raise, expiry | stated per fire mode and naming the instance: an auto weapon fires on the first tick the **incoming** instance is idle and its cooldown permits; a semi weapon waits for release and re-press |
| O10 | Reload held across a switch | reload held throughout; incoming weapon has a non-full magazine and live reserve | the incoming weapon does not auto-reload; the player must release and re-press. Mechanism: the repoint initializes `reload_press_consumed = true` on the incoming instance when reload is currently held, mirroring the `shoot_press_consumed` pattern for semi-fire in O9 |
| O11 | Deploy clamp arithmetic | a weapon with cooldown remaining is switched away from and back, total equip time shorter than that cooldown | the exact remaining cooldown on re-equip, accounting for the per-tick decrement that runs during that weapon's own lower. This row defines the deploy duration |
| O12 | Cooldown while inactive | a weapon is left inactive for far longer than its cooldown | its countdown does not advance while inactive; on re-equip the remaining value is the clamp applied to its value as of the tick it went inactive |
| O13 | Commit target equals active | the cursor returns to the slot the pawn is holding | no declaration is emitted. "Active" means `Inventory.active`, not a latched target |
| O14 | Cursor returns to the active slot during a lower | a commit to B is in flight; the player selects A again | the switch to B completes; selecting A emits nothing |
| O15 | Last-weapon toggle during a lower | active 0, memory 1, commit to 1 in flight, toggle pressed again | no declaration — the toggle's target is the still-active slot 0, consistent with O13. Last-weapon memory updates when a commit is accepted |
| O16 | Last-weapon toggle with no history | toggle pressed before any switch, or after the remembered slot emptied | no declaration; active unchanged |
| O17 | Target slot is empty | an index beyond the loadout, or a slot whose instance despawned | no declaration; active unchanged; the cursor does not rest there |
| O18 | Cycle across trailing empty capacity | slot capacity larger than the loadout, cycle-next from the last occupied slot | it wraps to the first occupied slot. Cycle iterates occupied slots only and never stalls on an empty one |
| O19 | Cycle across an interior emptied slot | a middle slot's instance despawns; cycle-next from before it | the cursor lands on the next occupied slot |
| O20 | Slot emptied between cursor move and declaration | the cursor rests on a slot whose instance then despawns | no repoint; active unchanged; the cursor resets to the active slot; no error at gameplay severity |
| O21 | Active instance despawns during its own lower | the instance is removed while the pawn lives | one named outcome and the pass that produces it: the switch is abandoned, the active index moves to the first occupied slot, and no raise starts |
| O22 | Pawn despawns mid-switch | a lower in flight, pawn removed | every slot's instance despawns with the pawn; no orphan weapon entity |
| O23 | Slot close with a multi-slot inventory | a remote client with three occupied slots disconnects | all three weapon entities despawn, given the pawn is already despawned when cleanup runs |
| O24 | Demote then re-promote mid-switch | a client with a switch in flight is demoted, then re-promoted with a fresh payload | the pre-demotion instances are despawned exactly once; no pre-demotion slot index survives in the holder, cursor, or last-weapon memory |
| O25 | Host refuses a declaration | the reload-interrupt rule is on and a reload is running | the host refuses, the client applies the refusal by returning to its prior active slot and clearing any equip state, and the cursor resets |
| O26 | Declared shot during an equip transition | the client declares a shot from slot 1 while the host still believes slot 0 is active | accepted on possession; slot 1's magazine is debited; the host does not gate on equip state |
| O27 | Declared shot from an unowned weapon | a declaration names a slot the pawn does not possess | rejected, nothing debited, logged at diagnostic severity |
| O28 | Declaration lost or duplicated | the reliable path delivers a commit twice, or not at all before the next one | a repeated declaration for the slot already active is a no-op; a superseded declaration does not produce a second repoint |
| O29 | Two declarations inside one inter-tick interval | direct-select 2, then direct-select 5, before the host processes either | one repoint, to 5; last-weapon memory names the slot actually held, never the intermediate |
| O30 | Two direct-selects across zero-tick frames | two different number keys pressed on consecutive frames producing no ticks | exactly one declaration, naming a stated winner; the other is discarded, not deferred |
| O31 | Multiple scroll notches in one frame | N notches inside one frame | the cursor moves N steps against an explicit notch count, and the dwell restarts once |
| O32 | One notch per frame, frame longer than the dwell | low frame rate with a short resolved dwell, several notches on consecutive frames | one declaration, at the last notch — not one per notch |
| O33 | Dwell elapses on a zero-tick frame | the dwell expires on a frame producing no ticks | the declaration is still emitted; the input-layer holder persists it until consumed, and a second expiry before consumption replaces the target rather than queueing |
| O34 | Commit swallowed by a modal | a key is pressed and a modal opens the same frame | no declaration reaches the simulation while gameplay is captured; the holder is cleared wherever the input latch is cleared |
| O35 | Level unload with a pending declaration | the holder carries a slot when a level unloads | the holder, cursor, and last-weapon memory are cleared; no stale slot index reaches the new level |
| O36 | Loadout with one entry | a single-entry loadout | cycle, direct-select, and last-weapon emit nothing; behavior matches today's single-weapon pawn |
| O37 | Two slots referencing the same descriptor | duplicate entries | two independent instances each with its own magazine; the shared ammo type is seeded once, at the first occupied slot's authored reserve |
| O38 | Switch to a weapon with no ammo resource | the incoming weapon declares no resource | the ammo and reserve slots do not retain the outgoing weapon's values |
| O39 | Switch shorter than one publish interval | equip durations shorter than a frame | accepted lossy: the slots are published once per frame, so a switch contained between two publishes is never observed as switching |
| O40 | Switch contained in one multi-tick frame | several ticks in one frame with zero equip durations | the publisher runs once and publishes the final active weapon only |
| O41 | Tuning payload arrives mid-switch | a valid payload is delivered while a raise is running | the in-flight timer and the active index survive; the payload merges into the live inventory rather than re-materializing the pawn |
| O42 | Tuning payload decode failure mid-switch | a truncated or wrong-epoch payload arrives during a lower | the install path clears tuning before decoding, by design, so the previous tuning does not survive; the `Inventory` persists and the in-flight switch completes on durations already latched into its components |
| O43 | Descriptor reload of equip durations mid-switch | a duration is edited while a lower is running | the in-flight step completes on its old total; the next switch uses the new one; both roles receive the change |
| O44 | Descriptor reload changes the loadout mid-level | the loadout gains an entry while a client participates | the live inventory is not recomposed and the new slot is a no-op on both roles until the next level install |
| O45 | Cursor occupancy source | the cursor is evaluated at frame rate against a component the tick mutates | the occupancy source is named per role and the frame it reflects is asserted |
| O46 | Expiry loop admits and exits the equip states | a lower expires | the loop is entered because the re-gated condition admits a timed non-reload state, and exits on the first iteration because the repoint transition is not a reload step |

## Tasks

### Task 1: Split `sim/weapon_stage.rs`

The file is 3,452 lines, roughly 800 of them production code and the rest its test module. Split it along the seams already visible in it: the ordered per-tick machine driver, the state/event dispatch and its transition helpers, the fire authorization path, the local and remote command entry points, and the impact-damage application path the file's own header lists as a first-class concern and which no other task touches. Behavior-preserving: no behavior, signature, or test assertion changes, and the existing tests must pass unmodified. Carry the driver's numbered-step doc comment forward verbatim — it records that reload entry deliberately runs before expiry and fire, and later tasks depend on that order being legible. After the split there is still exactly one function matching state against event, with no wildcard arm.

### Task 2: Thin slice — inventory, equip states, local switch, end to end

Build the narrow real version and integrate it before anything fans out.

**Component.** Add an `Inventory` component on the pawn in `crates/entities`: an ordered array of optional wieldable entity ids, an active index, and the target of an in-flight switch. Its length is an engine capacity constant of 10, matching the ten number-key bindings, not the authored loadout length. Keep it a peer of the pawn's `Health` and `AmmoReserve` — the reserve is not moved inside — and name it for wieldables, not weapons. It holds no cursor and no dwell; selection is the input layer's (Task 7). `ComponentKind` uses explicit discriminants, so append at the next free value (19). The component is never replicated, so it adds no wire payload arm — but a new `ComponentKind` variant is a compile error at several exhaustive drift guards with no wildcard arm, listed in the Rough sketch, and all of them must gain an arm.

**States.** Add `Lowering` and `Raising` to `WieldableState`. Both deny fire and reload, and neither is reload activity. **That alone freezes the timer**: the driver's timer-advance step and its expiry loop are both gated on the reload-activity predicate, so a state that is timed but not reload activity never decrements and never expires. Add a fourth `const fn` predicate meaning "is a timed state" — true for both reload states and both equip states — and re-gate those two sites on it, leaving reload-activity for the reload-indicator fallback. The exhaustive-match argument does not surface these: they are calls, not matches. For the same reason audit equality comparisons against `WieldableState` by hand; exactly one exists in production, in the fire authorization path.

**Dispatch.** The dispatch matches `(state, event)` with no wildcard arm. Two new states and one new event make the domain twenty pairs, of which **eleven are new**: three `BeginLower` rows out of the shipped states, and eight covering the two equip states against all four events. Do not modify the existing cancel rows. `BeginLower` from atomic reload forfeits the in-flight reload — nothing has transferred, since the atomic path takes from the reserve only in its expiry arm; from per-shell reload the credited shells stay; from a lower it re-targets without restarting the timer. **Every** `BeginLower` row must drain the outgoing weapon's reload feedback unconditionally — including the row from idle, because a reload that completed earlier in the same tick leaves a queued completion that samples as active regardless of state. Do not reuse the shell-loading cancel row's helper, which deliberately retains a same-tick completion; add a sibling that clears entries, reseats both cursors' sequence, and resets their separator latches, or the next identical endpoint the incoming weapon publishes is suppressed.

**Repoint.** Two facts about the driver bound the placement: the dispatch function receives only a component and a reserve, so it cannot reach an `Inventory` or a second component; and the driver clones one component for the whole tick and writes it back afterwards. Any placement satisfying all of the following is acceptable:

- Exactly one repoint per accepted commit, and exactly one instance in an equip state at end of tick.
- No fire is authorized on the repoint tick, from either instance.
- The outgoing instance ends idle with its timed fields cleared, and that survives the tick's write-back.
- The incoming instance begins its raise and first advances on the next tick it is itself ticked.
- The lower's overshoot is discarded rather than carried into the raise.
- A zero-duration lower still produces exactly one repoint within the tick.
- The expiry loop terminates on the first iteration for an equip expiry.

At the repoint, initialize `reload_press_consumed = true` on the incoming instance when the reload input is currently held, so a held reload does not auto-fire on the new weapon — the player must release and re-press. This mirrors the `shoot_press_consumed` pattern that O9 relies on for semi-fire.

Apply a **deploy clamp** in remaining-milliseconds terms — the incoming weapon's `cooldown_remaining_ms` becomes the larger of its existing value and the deploy duration, which O11 defines. Note that the fire authorization path decrements that countdown on every tick the instance is ticked, including its own lower.

**Spawn and seeding.** Spawn instances from a `loadout` on a new inventory block of the player descriptor, replacing the top-level `defaultWeapon` string. `EntityTypeDescriptor` has no serde path — two hand-written adapters read it, one per runtime, and both must gain a reader; a helper in the runtime core also carries `default_weapon`. Three pawn spawn sites must build the inventory: the player-start path, the network-slot path for a host-simulated remote pawn, and the connected client's own-pawn materialization, which today spawns no weapon and attaches no reserve — Task 5 assigns its trigger. Seed the ammo reserve **once per distinct ammo type**, taking the first occupied slot's authored reserve; the seeding helper credits additively, so a per-slot loop would double a shared pool. Loadout composition is resolved here and only here.

Weapons are a `weapon` block on `defineEntity`; there is no `defineWeapon` (`plans/done/M10--weapon-primitives/index.md:101`). Add `lowerMs` and `raiseMs` to that block, to the weapon component, its constructor, its effective-stats projection, and its descriptor-refresh path. Both are `u32` milliseconds like every sibling on the timer path, optional, defaulting to 0, with no lower-bound rejection — a deliberate divergence from the `>= 1` rule the ammo numerics use, because a zero-duration equip is a legal authored choice.

**Input, one path only.** Add the ten direct-select actions and their number-key bindings; Task 7 adds cycle and last-weapon. Emit a commit immediately on press. Single-player and listen-host only in this slice — the client path lands in Task 5. Until Task 6 lands the simulation always permits preemption. Presentation dirties at the repoint, not at the commit.

### Task 3: Retire the global holder and converge every reader

Delete `App.active_wieldable` and `App.active_wieldable_descriptor`. The descriptor field has no production read site — hot reload refreshes weapons through descriptor provenance — so delete rather than migrate. The value is produced by the player-spawn result struct and carried through the level-install products struct; both lose it, along with the `default_weapon` spawn branch that fills them. The headless observability driver reads the install-products field, not `App`.

`WeaponOwners` cannot simply lose its map: the map **is** its change-detection mechanism and its drain payload. Reduce it to an explicitly-marked dirty set whose drain resolves the weapon from the pawn's `Inventory`. Its drain is not a switch follow-up — it is the third-person hand-socket attachment pass, and it is fed today by marks at three kinds of site that must all survive: **inventory materialization at each of the three spawn paths**, **slot removal and pawn unregistration**, and now the repoint. Marking only at the repoint would leave a pawn that spawns and never switches with no weapon at its socket, and a despawned pawn with a stale one.

**Slot close must read before the pawn dies.** The lifecycle handler despawns the pawn before the owned-state cleanup runs; that works today only because the map outlives the pawn. Read the pawn's slot instances out of `Inventory` before the despawn, or have the dirty set retain last-known instance ids for this drain. Otherwise every slot's weapon entity leaks on close and demote.

Every reader resolves through `Inventory`: the fire path, the HUD publisher, the per-frame viewmodel resolution on all three roles including the single-player fallback that synthesizes a throwaway owner map each frame, the level-install attachment sync, the snapshot's active-archetype fill, the owner-private ammo and cooldown projections, the remote-pawn command preparation, and the despawn cleanup — extended to every slot. On a connected client the viewmodel resolves from the local `Inventory`, not the replicated archetype string.

Remove the listen-host bridge that copied the global into the owner map after install. Level teardown clears the component with the pawn.

This task owns the **shape** of the HUD publisher's role gate, which today is a whole-function early return on a connected client that also suppresses the sampled-weapon id driving reload-feedback acknowledgment. Split it so Task 8's three slots publish on a client while the host-authoritative slots stay suppressed, and **decide here** that a client does acknowledge its own locally-owned weapon's feedback stream, since the client now owns real weapon components whose streams would otherwise never drain.

### Task 4: Switch declaration and possession-based fire validation

Give the client a way to tell the host what it did, and loosen shot validation so the two never have to agree about which slot is active.

The **switch declaration** is a client→host message on the reliable path carrying the target slot. It is not a per-tick input field: it is a discrete event, so it needs none of the edge-gating, gap-hold neutralization, or catch-up-drop handling the fire and use edges require. The host validates it — the slot is occupied, and the reload-interrupt rule permits it — then repoints its own `Inventory`. On refusal the host tells the client, which returns to its prior active slot and clears any equip state it had started. A declaration naming the already-active slot is a no-op.

**Possession-based fire validation.** The host today resolves a firing weapon from its own active pointer, via the owner map, when preparing a remote pawn's command. Change it to resolve from the client: the firing slot rides the per-tick input command as a **level**, the way `reload` already does, so it repeats harmlessly across a gap-hold and needs no edge derivation. The host accepts the shot if the pawn's `Inventory` holds a weapon in that slot, and debits that instance. It does **not** gate on equip state — that is what removes the boundary case where a client's raise completes slightly before the host's and a legitimate shot is refused. Ammo and fire rate are already host-authoritative and per-instance; this changes only which instance is resolved. A slot the pawn does not possess is rejected and logged at diagnostic severity.

### Task 5: Per-slot tuning payload

Grow the host→client tuning payload from one weapon's values to the set the pawn holds, and bump the payload epoch so a client on the old shape is rejected with a diagnostic. It becomes an ordered per-slot set sized by the slot capacity, each occupied entry carrying that weapon's **canonical archetype name**, its fire values, and its lower and raise durations, alongside the same movement descriptor. The archetype name is required: without it the client cannot spawn per-slot instances, pick the incoming viewmodel, or publish a weapon name. The payload derives plain serde with no rename attribute, so its fields serialize snake_case — the camelCase convention in the Boundary inventory governs descriptor surfaces, not this payload.

**This task assigns the client's inventory creation.** The client's first payload for its pawn builds its `Inventory` and spawns one instance per occupied slot from the archetype names; later payloads merge into the live inventory rather than re-materializing the pawn. Name the ordering hazard both ways — payload before pawn baseline, and pawn baseline before payload. The install path clears tuning **before** decoding, deliberately, so a bad replacement cannot leave stale state live; preserve that and state its consequence, that the `Inventory` survives the clear and an in-flight switch completes on durations already latched into its components.

Resolve the payload from the pawn's live `Inventory`, not the authored descriptor array. The send is change-detected but reachable only from slot lifecycle and post-install; add one trigger, a descriptor refresh that changes any weapon's tuning, so a hot-reloaded equip duration reaches both roles. Loadout composition is not re-sent, because composition is resolved at spawn. Keep the canonical-JSON control-channel transport, the per-client change detection, and the cleared view-feel field. Update the committed fixture.

### Task 6: Mod-global switch policy and its per-weapon override

Add a `switching` block to the mod manifest and a per-weapon override for the one rule the simulation evaluates. The manifest already carries declarations as data, so this needs no new registration primitive — but `defineMod` is an identity builder needing no change and the typedef surface is generated, so the real sites are the canonical Rust manifest struct in `crates/scripting-core`, the registered SDK type a parity test holds in lockstep with it, and the manifest drain and validation path. Put the switching descriptor in foundation, beside the other cross-crate descriptor types the manifest already carries.

The block declares whether a direct-select commits immediately, the cycle-commit dwell in milliseconds, and whether a commit may interrupt reload activity. Only the last is a simulation rule; the first two are read by the input layer.

**Name the carrier for each.** The weapon component's constructor and refresh path receive only a descriptor, so the component can store the per-weapon `Option<bool>` override and nothing more — the effective-stats projection surfaces the override, not a resolved value. Resolution against the mod-global default therefore happens at the two sites that consume the rule: the host's declaration validation (Task 4) and the local commit gate (Task 2). Both need the resolved mod config. The mod-global `SwitchingDescriptor` lives on `App` after mod-init commit, following the `PlayerMovementDescriptor` precedent — mod-global config, set once at mod init, consumed by simulation sites through `&App`. The two consume sites resolve the effective rule as `weapon.block_during_reload.unwrap_or(app.switching.block_during_reload)`. Use the same field name at both scopes; the scopes disambiguate.

Validate the block where the mod's declarations are validated: a negative or non-finite dwell **aborts mod init** with a diagnostic naming the field. Note that this departs from the manifest's dominant convention — map catalog and theme both warn-and-skip; only the frontend declaration propagates an error, and the stricter precedent is behavior-graph transition targets, which are rejected at descriptor parse with the whole descriptor refused. Both parsers must degrade identically.

### Task 7: Input layer — scroll, cursor, dwell, last-weapon, and the player overrides

Own everything between the player's hardware and a commit, and let nothing below it see the parts that are local.

Add discrete scroll physical inputs and a window-event arm producing them; a scroll notch is a momentary button, not an analog axis. Normalize the two scroll-delta units the windowing layer reports, accumulating the pixel-based one to a threshold. Two consequences the shipped input path forces: a scroll wheel emits **no OS release event** and the per-frame reset clears only the mouse accumulators, so the scroll inputs must be cleared explicitly each frame or they latch active forever; and a pressed/held button cannot express two notches in one frame, so carry an explicit notch count.

Bind them to cycle-next and cycle-previous, and add a **last-weapon-toggle** returning to the previously active slot — near-universal in the genre and the basis of quick-switch binds. Its memory updates when a commit is accepted.

Hold the pending cursor and its dwell timer here. A cycle moves the cursor over **occupied slots only**, wrapping past empty ones — with a capacity of 10 and a two-weapon loadout, a cursor that refuses to move onto an empty slot would stall permanently. A direct-select onto an empty slot is the case that emits nothing and does not move. Resolve the dwell as the player's override when set, else the mod's declared value, resolved once at cursor-move. Name where the cursor reads slot occupancy on each role and how stale that read may be.

Add both overrides to the player-options store as optional fields with `serde(default)`, following the crouch-mode precedent, and range-check both in the store's existing sanitize routine. The dwell is an `Option`, so "the player's value when set" is expressible; the notch threshold defaults to `120.0` — the OS-standard scroll quantum (see Resolved questions) — and is a concrete value, not an override.

The dwell runs at frame rate and can elapse on a frame producing no ticks. Name an input-layer holder that persists the emitted commit until consumed, the clear point after consumption, that a second emission before consumption replaces the target, and that the holder is cleared wherever the input latch is cleared — a modal opening must not leave a commit to fire while gameplay is captured — and at level unload.

### Task 8: `player.weapon.*` state slots

Publish the active wieldable's identity so a HUD can show what is held and what is pending. Add three built-in engine state slots — the current weapon's canonical archetype name, the pending selection's name, and whether a switch is in flight — two string-typed and one boolean, with nested SDK paths as explicit segment arrays.

They do **not** share one source. `current` and `switching` project the owning machine's `Inventory`; `pending` projects the **input layer's cursor**, which is why it changes during the dwell before any commit exists. Ship `pending` defaulted empty here; Task 7 attaches its producer.

**All three are local scope on every role**, fed by the same per-frame publisher, so a client's HUD follows its own switch. Local scope follows from the authority model — the client is authoritative for its own switch, so no host value exists to replicate; this is a new scope precedent for `player.*` (see Resolved questions). Every engine state slot generates an SDK path and appears in the generated tree — there is no hidden-slot affordance — so document all three as display values whose crossings fire per-machine rather than authoritatively. Adding `player.weapon.current` creates a `weapon` object node beside the existing flat `player.weaponCooldownMs`; that inconsistency is knowingly accepted, with the nested form the convention going forward.

The current-weapon slot names the committed active instance, so it flips at the repoint, not at the commit; during the lower it still names the outgoing weapon.

### Task 9: Typed loadout references and the dev-mod reference

Make the loadout hold descriptor references rather than name strings. Change its element type to the value `defineEntity` already returns, constrained to descriptors declaring a weapon block so a weapon-less reference is an editor-time error. Because there is no typecheck step in CI the type constraint is not a gate, so **the identity builders throw**, identically in both runtimes, with a diagnostic naming the offending entry, for three cases: a descriptor with no weapon block, a descriptor with no canonical name, and a value that is not a descriptor at all. The lowering to canonical names happens in those same builders, before the manifest crosses the FFI, so the Rust adapters continue to read an array of strings. The Rough sketch lists the adapters as relevant sites without claiming they need no edit; the adapters currently read a single `default_weapon` string and must be edited to read a `loadout` array of strings, even though the manifest's outer shape is unchanged. **Depend only on value equality, never object identity**: the Luau require implementation performs no module caching, and mod-init and level scripts are separate bundles in separate VMs.

Update the dev mod's player script to import both reference weapon descriptors into its loadout, and declare the switching block in the start script, so number-key, scroll, and last-weapon behaviors are all exercisable in-game.

### Task 10: Ordering and edge coverage

Cover every row of the Orderings table with a test that names its scenario, plus the state-preservation, reserve-seeding, deploy-clamp, and no-leak invariants directly. The rows span three layers — input-layer cursor, dwell and notch handling; the machine's transition rows and tick order; and host/client interaction — so place each test where its outcome is owned rather than driving everything through a full-session harness. Zero-duration, empty-slot, single-entry, duplicate-descriptor, overshoot, frozen-countdown, and terminal-state rows belong at the machine and component level. Declaration, refusal, possession-validation, demote/re-promote and slot-close rows need the two-peer path; use the existing latency-simulation harness rather than standing up a new one. The tick-order row deserves a direct assertion on the step sequence. Cover the headless observability driver's read against the rewired install products, and the spawn-time hand-socket attachment, neither of which any other test reaches.

## Sequencing

**Phase 1 (sequential):** Task 1 — behavior-preserving split; every later task edits this file.
**Phase 2 (sequential):** Task 2 — thin slice, integrated end to end before anything fans out. It is deliberately **single-player and listen-host only**: under the declaration model the netcode surface is one reliable message plus one command level, and the client's inventory is created from the tuning payload, so the slice cannot cross that seam before Task 5 exists. The consequence is that the client path's assumptions are not falsified until Phase 5 — Task 5 carries that risk and should be treated as a second integration point, not a fan-out task.
**Phase 3 (sequential):** Task 6 — Tasks 2, 4 and 7 all consume its declared rules.
**Phase 4 (sequential):** Task 3 — owns the `WeaponOwners` reduction and the HUD publisher's role gate, which Tasks 4, 5 and 8 build on.
**Phase 5 (concurrent):** Task 4, Task 5, Task 8 — declaration and validation, the payload and the client's inventory, and the state slots.
**Phase 6 (sequential):** Task 7 — consumes Task 6's rules, Task 2's commit path, and attaches Task 8's pending producer.
**Phase 7 (concurrent):** Task 9, Task 10.

## Rough sketch

`Inventory` lands in `crates/entities/src/components/inventory.rs`, following `AmmoReserve` as the nearest shape precedent. Adding a `ComponentKind` variant is a compile error at every exhaustive drift guard with no wildcard arm: the discriminant map and its successor-walk test in `crates/postretro/src/netcode/mod.rs`, the kind list, snake-name map and sample-value builder in `crates/postretro/src/observability/mod.rs`, and the kind-name match in `crates/entities/src/ffi.rs`. The `VARIANTS` array in `crates/entities/src/registry.rs` is a dependent site requiring manual update — it is a const listing, not an exhaustive match, so a missing entry compiles cleanly and silently undercounts `ComponentKind::COUNT`.

Machine work is confined to what Task 1 splits out of `sim/weapon_stage.rs`. `WieldableState` and its predicates are in `crates/entities/src/components/wieldable_state.rs`. The unconditional feedback drain is a new sibling of `WeaponComponent::clear_cancelled_reload_feedback` — that helper calls a routine which deliberately retains a same-tick completion, so it must not be reused.

Manifest work: the canonical struct in `crates/scripting-core/src/runtime/types.rs`, the registered shape in `crates/postretro/src/scripting/primitives/manifest.rs`, and regenerated typedefs. `EntityTypeDescriptor` (`crates/entities/src/data_descriptors/types/entity.rs`) has no serde path — the readers are `crates/scripting-core/src/data_descriptors/js/entity.rs` and `.../lua/entity.rs`. A test fixture helper in `crates/scripting-core/src/runtime/core.rs` (`#[cfg(test)]` only) also constructs `EntityTypeDescriptor` values with a `default_weapon` parameter and must be updated. The hand-written doc comments naming `defaultWeapon` in `sdk/lib/data_script.ts` and `.luau` are not generated and need updating.

The weapon block's new fields are on `WeaponDescriptor` in `crates/foundation/src/data_descriptors/types/combat.rs` and on `WeaponComponent`/`EffectiveStats` in `crates/entities/src/components/weapon.rs`.

Payload work is in `crates/postretro/src/netcode/tuning_payload.rs` and its build/send path in `netcode/mod.rs`; the fixture is `crates/postretro/src/netcode/tests/fixtures/tuning_payload.expected.json`. The firing-slot level joins `InputCommand`/`WireMovementInput` in `crates/net/src/wire.rs` — where the bitcode field order is documented as part of the wire layout — converted in `netcode/wire_convert.rs`, and consumed where `prepare_remote_pawn_command` resolves the weapon in `main.rs`. The switch declaration and refusal are control messages beside the existing client and server control variants in `netcode/mod.rs`.

Input additions touch `crates/postretro/src/input/types.rs`, `input/defaults.rs`, the `WindowEvent` match in `main.rs`, and the options store in `crates/postretro/src/options/mod.rs`.

State slots are entries in `BUILTIN_ENGINE_STATE` (`crates/entities/src/engine_state_catalog.rs`), fed by `scripting/systems/ui_proxy.rs`.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Inventory component | `ComponentValue::Inventory` | `"inventory"` — serde tag only, never a replicated payload | `inventory` (descriptor block) | `inventory` | n/a |
| Loadout list | `InventoryDescriptor::loadout` | `"loadout"` (array of canonical-name strings) | `loadout` (array of descriptor refs) | `loadout` | n/a |
| Slot capacity | `WIELDABLE_SLOT_CAPACITY = 10` | bounds the payload cardinality and the firing-slot level | n/a | n/a | n/a |
| Switch declaration | `ClientSwitchDeclaration { slot: u8 }` | reliable client→host control message | n/a | n/a | n/a |
| Switch refusal | `ServerSwitchRefused { slot: u8 }` | reliable host→client control message | n/a | n/a | n/a |
| Firing slot | `SimCommand::firing_slot: u8` / `MovementInput` mirror | `firing_slot` on the wire movement input, a level like `reload` | n/a | n/a | n/a |
| Lower duration | `WeaponDescriptor::lower_ms: u32`, `WeaponComponent`, `EffectiveStats` | `"lowerMs"` | `lowerMs` | `lowerMs` | n/a |
| Raise duration | `WeaponDescriptor::raise_ms: u32`, `WeaponComponent`, `EffectiveStats` | `"raiseMs"` | `raiseMs` | `raiseMs` | n/a |
| Reload-interrupt rule | `SwitchingDescriptor::block_during_reload` and `WeaponDescriptor::block_during_reload: Option<bool>` | `"blockDuringReload"` at both scopes | `blockDuringReload` | `blockDuringReload` | n/a |
| Mod switching block | `ModManifestResult::switching` | `"switching"` | `switching` | `switching` | n/a |
| Direct-select commit rule | `SwitchingDescriptor::commit_on_direct_select` | `"commitOnDirectSelect"` | `commitOnDirectSelect` | `commitOnDirectSelect` | n/a |
| Cycle dwell | `SwitchingDescriptor::cycle_commit_dwell_ms` | `"cycleCommitDwellMs"` | `cycleCommitDwellMs` | `cycleCommitDwellMs` | n/a |
| Per-slot tuning entry | `SlotFirePayload` | snake_case JSON — plain serde, no rename | n/a | n/a | n/a |
| Dwell player override | `PlayerOptions::switch_cycle_dwell_ms: Option<u32>` | `switch_cycle_dwell_ms` (TOML, snake_case) | none | none | n/a |
| Scroll notch threshold | `PlayerOptions::scroll_notch_pixels: f32` | `scroll_notch_pixels` (TOML, snake_case) | none | none | n/a |
| Lowering state | `WieldableState::Lowering` | `"Lowering"` (no `rename_all` on the enum) | n/a | n/a | n/a |
| Raising state | `WieldableState::Raising` | `"Raising"` | n/a | n/a | n/a |
| Current weapon slot | catalog entry | `"player.weapon.current"` | `getGameState().player.weapon.current` | same path | n/a |
| Pending weapon slot | catalog entry | `"player.weapon.pending"` | `getGameState().player.weapon.pending` | same path | n/a |
| Switching flag slot | catalog entry | `"player.weapon.switching"` | `getGameState().player.weapon.switching` | same path | n/a |

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
      blockDuringReload: false,
    },
  },
})
```

```ts
// Proposed design — the loadout holds descriptor references, not strings.
// A typo is an unresolved import; a weapon-less reference throws at mod init.
import { referenceShotgunEntity } from "./reference-shotgun";
import { referencePistolEntity } from "./reference-pistol";

export const playerEntity = defineEntity({
  canonicalName: "player",
  components: {
    movement: playerMovement,
    // Replaces the shipped TOP-LEVEL `defaultWeapon` string. M10 placed equip
    // at the top level deliberately; equip now carries per-pawn runtime state.
    inventory: { loadout: [referenceShotgunEntity, referencePistolEntity] },
  },
})
```

```ts
// Proposed design — switch rules are declared once per game on the manifest,
// not per character class. Only blockDuringReload reaches the simulation.
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

## Resolved questions

- **Pixel-scroll notch default: 120.0.** The threshold converts a trackpad's continuous pixel-delta stream into discrete notches. 120 pixels is the OS-standard notch quantum — Windows `WHEEL_DELTA`, and the value most trackpad drivers normalize to. The player option exists for non-standard devices, the same reason `view_feel_scale` exists. No trackpad fixture is needed to pick the right number; the OS defines it.
- **The per-weapon reload-interrupt override earns its keep.** A shotgun with per-shell loading and a rocket launcher with an atomic reload are different interruption stories. A mod author with both in their loadout needs to express "you can interrupt the shotgun mid-reload, but you must wait for the rocket launcher." The resolution cost is one `Option<bool>.unwrap_or(mod_default)` expression at two sites, already named. The engine's purpose is to let modders make games, and this is the kind of per-weapon knob a modder reaches for first.
- **Local `player.weapon.*` follows from the authority model, not the naming prefix.** The client is authoritative for its own switch — there is no host value to replicate. `player.health` is replicated because the host owns HP; `player.weapon.current` is local because the client owns its own switch presentation. The `player.*` prefix groups slots by subject (the player), not by authority. A mod author reading `player.weapon.switching` in a HUD tree wants their own machine's answer, and local scope is what gives it to them. No shipped authoring pattern assumes `player.*` is authoritative — `bindState` and `onStateCrossing` document per-machine behavior already.
