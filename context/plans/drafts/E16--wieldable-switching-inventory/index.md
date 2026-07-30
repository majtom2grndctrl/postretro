# Wieldable Switching + Inventory

## Goal

Give a pawn an ordered inventory of wieldable instances and one active reference that repoints between them, preserving each instance's own state. Converge the three divergent active-weapon holders onto that inventory so single-player, listen host, and connected client read one source of truth. The engine owns the inventory, the timed lower/raise, and whether a switch is permitted; the input layer owns how a switch intent is produced, and the mod declares both once per game.

## Scope

### In scope

- An `Inventory` component on the pawn holding an ordered set of wieldable slots plus the active index. Its length is an **engine capacity constant**, not the authored loadout length — a shorter loadout leaves trailing slots empty.
- Separation of **selection** (moving a local cursor — changes nothing held, never leaves the machine it runs on) from **commitment** (an intent that repoints the active reference).
- `Lowering` and `Raising` equip states on the shipped wieldable machine, with per-archetype durations.
- Retirement of `App.active_wieldable` and `App.active_wieldable_descriptor`; `WeaponOwners`' pawn→weapon map replaced by the component. All read sites rewired, including the headless observability driver.
- Retirement of `ClientWeaponState` **as a holder**: a connected client runs the same `Inventory` component, so "which wieldable is active" has one implementation in every session role. The client keeps a distinct *fire-prediction carrier*, now per slot — see the note below.
- Per-slot fire tuning and equip durations in the host→client tuning payload, so a connected client predicts a switch locally.
- A mod-global `switching` block on the mod manifest, with a per-weapon override for its one simulation-side rule.
- A player-options field overriding the cycle-commit dwell, persisted and resolved in the input layer, with no settings UI.
- Discrete mouse-wheel physical inputs, direct-select and cycle actions, and a bounded commit intent on the command path.
- `player.weapon.current`, `player.weapon.pending`, `player.weapon.switching` engine state slots.
- A `loadout` array on the player descriptor's inventory block, holding **descriptor references** rather than name strings, replacing the single `defaultWeapon` string.

### The two-layer split, and why it is the load-bearing decision

Switch behavior divides cleanly along a line the codebase already draws:

| Layer | Owns | Replicated |
|---|---|---|
| **Input** (local to the machine the player sits at) | the pending cursor, the dwell timer, direct-select-versus-cycle, the player's dwell override | never — it produces a commit intent, nothing more |
| **Simulation** (authoritative) | whether a commit intent is honored, the repoint, the timed lower/raise | yes, through the existing command and snapshot paths |

`PlayerOptions.crouch_mode` is the precedent and it is exact: toggle-versus-hold is resolved in the input layer, and `crates/postretro/src/movement/mod.rs:51` records that the movement intent *"NEVER sees the raw button or the mode."* A wheel dwell is the same kind of value. Keeping it out of the simulation means the dwell never has to agree across peers — the client decides *when* it wants to switch, the host decides *whether* it may.

### Out of scope

- **Pickup and drop.** A weapon-only descriptor is rejected as a direct map placement (`is_directly_map_placeable`, `scripting/builtins/data_archetype.rs:771`; rejection at `:1323`, "equip targets, not direct map placements"), so no weapon instance can exist in the world to be picked up. Roadmap `E16 › Weapon Systems › pickup` owns it; its prompt affordance is owned by the unbuilt **combat presentation substrate** (`context/plans/roadmap.md:229`).
- **A settings UI for the dwell override.** The options store ships the field and its persistence; the menu that edits it is a separate deliverable by standing policy — `context/lib/player_options.md` §4 splits the store from the E13 settings menu, and §3 records that no save-on-change occurs at runtime until that menu is wired.
- **Dual-wield.** No descriptor field can express an off-hand wieldable — `EntityTypeDescriptor` (`crates/entities/src/data_descriptors/types/entity.rs:315`) carries a single weapon slot, and this plan's active reference is a single index. The state is unrepresentable, so the case is unreachable.
- **Augments, rolls, and non-passthrough stat resolution.** `WeaponComponent::effective` takes `&self` only and is a pure projection (`crates/entities/src/components/weapon.rs:316`); the component stores no modifier or roll data, so no composed stat can exist.
- **Heat and cell resources.** `WeaponResource` is a tagged union with exactly one arm, `{ kind: "ammo" }` (`sdk/types/postretro.d.ts:262`).
- **Secondary activation / alt-fire.** `Action::AltFire` is bound (`crates/postretro/src/input/defaults.rs:30`) with zero consumers outside `input/`; roadmap `E16 › Weapon Systems › secondary activation` owns it.
- **Mod-authored input bindings.** The SDK exposes no `Action` type and no action or axis read surface, so a mod cannot name a physical input to bind.
- **Radial / ring weapon selector.** `UiInstance` (`crates/ui/src/output.rs`) and `ui_quad.wgsl` draw axis-aligned quads only; a ring needs the radial primitive deferred at `context/plans/roadmap.md:149`. This plan publishes the slots a selector binds to; a list-shaped selector is authorable on shipped widgets.
- **Compile-time enforcement of the loadout's descriptor references.** There is no `tsc` step in CI — `content/dev/scripts/typed-handles-fixture.ts:1-10` states this in its own header, and the repo has no typecheck job. Handles buy editor-time and rename safety; the enforced gate stays the descriptor-parse validation this plan adds.

### The client's prediction carrier is not a fourth answer

The three holders answer "which wieldable is active." All three lose that job: single-player, listen host, host-simulated remote pawn, and connected client all read the pawn's `Inventory`. What survives on the client is narrower and differently shaped — the locally predicted fire scalars (cooldown remaining, its authority generation, the consumed-press latch) that Epic 15 Phase 3 requires any predicted action to carry. Today those scalars sit on a single-weapon struct that *also* implies which weapon is held; after this plan they are per slot and imply nothing about the active one. A carrier of predicted values is not a holder of identity, and the distinction is testable: with the loadout and active index in the component, no code path can ask the client which wieldable is active and get a different answer than the host would give.

### Ships knowingly broken — owner decision

**Inventory and ammo reserve do not survive a level transition.** Nothing in source forecloses carrying them; this is a choice. The durable per-player key that carry needs is the host-minted **seat**, unbuilt in E15 Phase 3.75 (`context/plans/roadmap.md:202`), which `drafts/E16--per-player-currency` is already parked on. Building carry now means either blocking on that spec or standing up a single-player-only carry path — a fourth divergent holder, the exact disease this plan cures. Consequence shipped: a campaign cannot carry weapons or ammo across levels; every level re-equips from the player descriptor and re-seeds the reserve. Owner decision, 2026-07.

## Direction

**Problem.** A pawn's active weapon is stored three different ways, and none of them can change at runtime. `App.active_wieldable` is written only at level install and teardown; `WeaponOwners` is host-only; a connected client owns no weapon entity at all and models its weapon as four floats resolved from the pawn class's `default_weapon`. There is no place to put a second wieldable, and no path by which the active one could change. The cause is that "the weapon" was modeled as a property of the *session role* rather than of the *pawn*.

**Prior commitments.** `context/research/weapon-model.md` §6/§7 pins the shape: switching repoints an active reference, per-instance state survives because instances own it, and the container plus its equip/switch machinery are named for **wieldables**, not weapons (invariant 7), with inventory a peer of the pawn's `Health` and `AmmoReserve` rather than a parent (§1). `crates/entities/src/components/wieldable_state.rs:9` states outright that equip states join that enum when switching owns their behavior, and `E16--weapon-state-machine` shipped its preemption seam for this.

`E21--coop-avatar-weapon-presentation` deferred the switch input path to this plan explicitly (`index.md:33`: *"This plan renders whatever weapon the host assigns"*) and shipped the machinery that assignment needs — the replicated active-archetype field, the client-side change detection, and the hand-socket rewrite. AC 9 is close to free as a result, and this plan must not stand up a second presentation path beside it.

The mod-global block follows the shipped manifest rule rather than inventing a home: `context/lib/scripting.md:49` records that store declarations, UI trees, theme data, map catalog entries, and frontend declarations all arrive as **manifest data, not import-time side effects**, and `game-state-sdk-surface` (`plans/done/`) migrated `defineStore` from import-time FFI to a pure returned declaration for exactly this reason. `defineMod` is already a pure identity builder returning a `ModManifest`, so a `switching` block is an entry in an established pattern.

E15's session-lifecycle spec set the rule that a client's predicted values are **replicated, not hashed** — the four weapon fire fields are deliberately absent from the content-parity digest because the host sends them. This plan honors that by growing the payload rather than the digest: the moment a pawn holds N wieldables, replicating one archetype's tuning while the client predicts with another would break the guarantee that made the exclusion safe.

Where this diverges: the player descriptor's `defaultWeapon` string is **replaced** by a `loadout` array rather than kept as sugar. E15's parity reasoning names `default_weapon` explicitly as the path a client reads its fire fields through, so leaving a second, one-weapon path alive would preserve exactly the divergence this plan exists to remove. The tree is pre-stable and `content/dev/scripts/player.ts:5` is the sole consumer, so the call sites move in the same change (`context/lib/index.md`, pre-stable note).

**Placement.** Three placements, each on a different axis. The *inventory and the repoint* sit in the engine because they are authoritative state a host owns and a client predicts. The *commit rule's simulation half* — may a switch interrupt a reload — sits in mod-declared data because it is a game-design decision the engine should not have an opinion about, and it is per-weapon-overridable because a shotgun abandoning a per-shell reload and a launcher abandoning a load are genuinely different calls. The *commit rule's input half* — dwell, direct-versus-cycle, and the player's override — sits in the input layer beside `crouch_mode`, because it describes how one person's hardware is interpreted, not what the simulation does.

**Alternatives rejected.** The strongest rival is *host-authoritative switching with no client prediction*: the client sends a switch request and applies the result when the host's snapshot says so. Markedly cheaper — no payload change, no epoch bump. Rejected because switching is an input-driven state change on the local pawn, the category Epic 15 Phase 3 built prediction for; shipping it unpredicted makes weapon switching the one player action that visibly waits for the network, and retrofitting prediction later touches the same payload, the same machine, and the same reconciliation path a second time.

The second rival, and the one this plan drafted before rejecting: *express the commit rule as an authored IR guard over a `@wieldable.*` input namespace*, mirroring `@brain.*`. It was rejected on two grounds. **Placement:** the guard would hang off the player entity descriptor, so every character class in a game would re-declare an identical rule — boilerplate with a configuration surface attached. Switch commit behavior is uniform across characters in every comparable game (TF2, Borderlands, Destiny, Deep Rock, Titanfall pilots); what genuinely varies per-thing is equip *duration*, which is already per-weapon. **Structure versus thresholds:** `scripting.md` §11 warrants IR for authored behavior that depends on live state, and the sharper test is whether the author chooses the *structure* or only the *numbers*. A behavior-graph author invents states and edges; a switch-commit author sets "instant on direct select, dwell on cycle, block during reload" — engine-fixed structure, authored thresholds. That is `weapon-model.md`'s own "Declare, don't drive," which data satisfies. The cost accepted: adding a guard later means deprecating authored fields, the expensive migration direction. That is priced against a case nobody can currently name — the two reference policies this plan ships are both expressible in the declared fields.

The third rival is *storing the inventory as a side-table keyed by pawn*, generalizing `WeaponOwners` in place. Rejected because a side-table is precisely what made the current state divergent — it lives on `NetEndpoint::Host` and therefore cannot exist single-player, which is why `App.active_wieldable` exists at all. A component is despawned with its pawn, replicates through the existing entity paths, and has one home in every session role.

**Foreclosures and one-way doors.** The tuning payload epoch bump is one-way in that a client on the old epoch is rejected; this is what an epoch is for and costs a further bump to undo. Replacing `defaultWeapon` with a handle-bearing `loadout` is a breaking descriptor change; undoing it means re-editing the descriptor type, the Rust mirror, both typedef surfaces, and dev content — bounded, not free. Declaring switch policy as manifest fields forecloses a later move to expression-authored policy without a deprecation path. Nothing here forecloses pickup, dual-wield, or augments: all three extend the slot array or the instance, not the active reference.

## Acceptance criteria

- [ ] A pawn spawns holding every wieldable its loadout references, with the first slot active; each slot holds a distinct instance, and two slots referencing the same descriptor hold two independent instances.
- [ ] Pressing a direct-select input for an occupied slot other than the active one plays the outgoing weapon's lower, then the incoming weapon's raise, and the incoming weapon becomes active exactly once.
- [ ] Scrolling the wheel moves the pending selection without changing what is held; the switch begins only after the declared dwell elapses with no further scroll, and a scroll during the dwell restarts it.
- [ ] A weapon switched away from and back to retains its own magazine count, cooldown remaining, and reload progress state — it is not re-created and does not inherit the other weapon's values.
- [ ] Firing and reloading are both refused for the whole lower and raise, and the reload indicator does not read as active during a switch.
- [ ] With the mod's block-during-reload rule off, switching away during a per-shell reload keeps the shells already loaded and during an atomic reload loads none; with it on, the switch does not begin until the reload resolves. A per-weapon override wins over the mod-global value for that weapon only.
- [ ] Ammo reserve is shared: two weapons of the same authored ammo type draw from one pool on the pawn, and switching between them does not move, duplicate, or reset reserve rounds.
- [ ] A connected client's switch is visible locally on the input frame — the lower begins without waiting for a host round trip — and the client and host agree on the active slot after reconciliation with 150 ms RTT, 5% loss, and jitter.
- [ ] A remote player's avatar shows the weapon they switched to, and the local player's viewmodel shows theirs, on both host and client.
- [ ] The HUD's ammo, reserve, reload, and cooldown values follow the active weapon across a switch, on both host and connected client.
- [ ] Two players in one session with different dwell overrides each switch on their own timing, and neither player's timing affects the other or produces a reconciliation correction.
- [ ] A loadout referencing a descriptor that declares no weapon is rejected when the mod's declarations are validated, naming the offending entry — not warned about at level install with the player spawned unarmed.
- [ ] Every ordering in the Orderings table resolves to its stated outcome.
- [ ] A client whose tuning payload predates this change is rejected with a diagnostic rather than predicting with stale weapon values.
- [ ] Selecting an empty slot, or a slot index beyond the loadout, leaves the active weapon unchanged and logs no error at gameplay severity.
- [ ] The engine reports the same active wieldable in single-player, on a listen host, for a host-simulated remote pawn, and on a connected client — no session role has its own answer, and the client's answer comes from the same place the host's does.
- [ ] Reload behavior is unchanged from before this plan: atomic reload still cannot be cancelled by the existing cancel path, per-shell reload still cancels with its loaded shells credited, and both still transfer the same rounds for the same inputs.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Exactly one live wieldable machine per pawn | Task 2 (`Inventory.active`) | Task 3 removes the global that could name a second; a slot holding a despawned instance must clear, not linger | AC 1, 16 |
| A repoint preserves the instance's own state | Task 2 (repoint touches indices only) | Task 4 must not rebuild a component from tuning on switch; Task 3's rewires must not re-seed | AC 4, 7 |
| The lower→raise handoff crosses instances exactly once per commit | Task 2 (latched target, repoint on lower expiry) | Task 6 can emit a second commit intent during `Lowering`; re-latching must not emit a second repoint | AC 2, Orderings O3, O4 |
| Fire and reload are refused for the whole switch, and a switch is not reload activity | Task 2 (state predicates) | Task 7's projection reads the same predicates for `player.reloadActive` | AC 5 |
| Ammo reserve is pooled on the pawn, never on an instance | shipped (`AmmoReserve`) | Task 2 must not move it into `Inventory`; Task 3's rewires re-source the projection | AC 7 |
| No dwell or cursor value reaches the simulation | Task 6 (input layer owns both) | Task 4 must not carry the dwell in the payload; Task 7's pending slot is local-scope, not replicated | AC 11 |
| Host and client converge on the same active slot | Task 2 (host authority), Task 4 (per-slot tuning), Task 5 (same resolved rule both ends) | a per-weapon override resolved differently on the two peers would split them | AC 8 |
| Presentation reflects the committed active, not the pending cursor | Task 2 (dirty at repoint) | Task 6's cycle intents move the cursor every notch — presentation must not follow | AC 3, 9 |

## Orderings

Scenario, ordering, expected outcome. Task 9 cites these rows; other tasks must not restate them.

| # | Scenario | Ordering | Expected |
|---|---|---|---|
| O1 | Direct-select and cycle on one input frame | both present | direct-select wins and commits; the cycle is discarded, not applied after |
| O2 | Two or more wheel notches inside one frame | N notches, one frame | cursor moves N steps, bounded by the capacity clamp; one dwell restart, not N |
| O3 | Cycle arrives while `Lowering` | commit already latched slot B, cursor moves to C and its dwell elapses | the new commit intent re-latches the target to C; the lower timer is **not** restarted, and only one repoint occurs |
| O4 | Commit intent on the tick the lower expires | expiry and intent in the same tick | expiry runs first — the pending repoint completes before a new latch is taken |
| O5 | Commit intent on the tick a reload expires | reload expiry and commit collide | reload entry/expiry ordering is unchanged; the reload completes and credits, then the lower begins |
| O6 | Commit target equals active | cursor cycles back to the held weapon | no commit intent is emitted — no lower, no presentation dirty |
| O7 | `lowerMs` or `raiseMs` authored zero | commit with a zero-duration step | the state is entered and expires on the same tick; the repoint still happens exactly once |
| O8 | Target slot is empty | loadout shorter than the selected index, or a slot whose instance despawned | cursor does not move; no commit intent; active unchanged |
| O9 | Commit intent on the tick of level unload | intent latched, then unload | no repoint after teardown; no dangling instance reference survives |
| O10 | Pawn despawns mid-switch | `Lowering` in flight, pawn removed | both instances despawn with the pawn; no orphan weapon entity |
| O11 | Sim refuses a commit the input layer emitted | block-during-reload on, reload running | the commit is dropped, the cursor resets to the active slot, and the client's prediction and the host agree because both evaluate the same resolved rule |
| O12 | Tuning payload arrives mid-switch | new payload installed while `Raising` | in-flight switch completes; new tuning applies to subsequent switches, not retroactively |
| O13 | Loadout with one entry | single-entry loadout | cycle and direct-select emit no commit intent; behavior matches today's single-weapon pawn |
| O14 | Two slots referencing the same descriptor | loadout `[pistol, pistol]` | two independent instances; each keeps its own magazine |
| O15 | Dwell override changes between switches | player option edited on disk and reloaded at boot | the new dwell applies to subsequent switches; no in-flight switch changes duration |

## Tasks

### Task 1: Split `sim/weapon_stage.rs`

`crates/postretro/src/sim/weapon_stage.rs` is 3,452 lines and this plan adds two states, a new event variant, and its transition rows to the machine it hosts. Split it first, behavior-preserving, along the seams already visible in the file: the ordered per-tick machine driver, the state/event dispatch and its transition helpers, the fire authorization path, and the local/remote command entry points. No behavior, signature, or test assertion changes — this is a move, and the existing tests must pass unmodified. Do not extend anything here; the states land in Task 2. Leave the single-dispatch-point property intact: after the split there is still exactly one function that matches state against event, and its doc comment still says new states add rows rather than changing component shape.

### Task 2: Thin slice — inventory, equip states, direct-select switch, end to end

Build the narrow real version of the whole path and integrate it before anything fans out. Add an `Inventory` component on the pawn in `crates/entities` — an ordered array of optional wieldable entity ids plus an active index. **The array's length is an engine capacity constant, not the authored loadout length.** A shorter loadout leaves trailing slots empty, and every bound in this plan — this component's indices, the wire sanitize clamp, the payload's per-slot cardinality, and the direct-select action count — binds to that constant, never to what a descriptor happened to author; sizing to the loadout would make pickup, the next roadmap item, a change to all four at once. The component holds no cursor and no timer: selection lives in the input layer (Task 6), and the simulation sees only a commit intent naming a slot. Name it for wieldables, not weapons, and keep it a peer of the pawn's `Health` and `AmmoReserve` — the ammo reserve stays where it is and is not moved inside. Add `Lowering` and `Raising` to `WieldableState`; both deny fire and reload and neither is reload activity, so a switch never reads as a reload. The enum's predicates use exhaustive matches, so the compiler will name every site that must decide. Add a `BeginLower` event to the machine's event type with rows from every existing state — from idle, from atomic reload (the in-flight reload is forfeited; nothing has transferred yet), from per-shell reload (already-credited shells stay, mirroring the existing cancel row), and from `Lowering` itself (re-latch the target, do **not** restart the timer). Do not modify the existing cancel rows: atomic reload stays uncancellable by cancel, and shipped reload behavior must not change. A commit intent latches the target slot; when `Lowering` expires the engine repoints the active index to the latched slot and starts `Raising` on the incoming instance. Spawn the pawn's instances from a `loadout` on a new inventory block of the player descriptor, replacing the single `defaultWeapon` string — update the descriptor type, its Rust mirror, both typedef surfaces, and `content/dev/scripts/player.ts`, which becomes a two-entry loadout using the already-registered reference pistol beside the shotgun. Weapons are declared as a `weapon` block on `defineEntity`; there is no `defineWeapon` and this plan must not add one (`plans/done/M10--weapon-primitives/index.md:101`). Add per-archetype `lowerMs` and `raiseMs` to that weapon block. Wire one input path only — direct-select actions for slots, no wheel, no dwell — emitting a commit intent immediately on press, carried to the sim on the command struct and mirrored onto the movement input for the client-prediction boundary the same way `use_pressed` already is, with the wire mirror and a sanitize clamp against the engine slot capacity. Make the connected-client path work in this slice too, even though its per-slot tuning is incomplete: the client must run its own machine so the slice actually falsifies the prediction boundary rather than deferring it. Presentation must dirty at the repoint, not at the commit intent.

### Task 3: Retire the global holder and converge every reader

Delete `App.active_wieldable` and `App.active_wieldable_descriptor`, and remove the pawn→weapon map from `WeaponOwners`, keeping only its attachment dirty set. The descriptor field is write-only — no read site exists in the tree, and hot reload refreshes weapons through descriptor provenance instead — so delete it rather than migrating it. Every reader of the two holders resolves through the pawn's `Inventory` instead: the fire path, the HUD publisher, the per-frame viewmodel resolution (host, client, and the single-player fallback that synthesizes a throwaway owner map each frame), the level-install attachment sync, the snapshot's active-archetype fill, the owner-private ammo and cooldown projections, the remote-pawn command preparation, and the despawn cleanup that treats pawn and weapon as one ownership unit. Rewire the headless observability driver's two read sites as well — it is a real consumer with no gameplay acceptance criterion covering it. Remove the listen-host bridge that copied the global into the owner map after install: the host's own pawn now carries an inventory like any other pawn, so the special case disappears rather than moving. Level teardown must clear the component with the pawn and leave no dangling instance reference; the level-scoped host reset no longer has a weapon map to replace. On the connected-client side, retire the single-weapon prediction struct **as a holder**: which wieldable is active comes from the client's own `Inventory`, spawned from the host-sent per-slot tuning, so the client answers that question the same way every other role does. What remains client-side is a per-slot carrier of locally predicted fire scalars — cooldown remaining, its authority generation, the consumed-press latch — which Epic 15 Phase 3 requires any predicted action to keep. Keep the existing rule that a connected client never resolves weapon *tuning* from its local registry: the values come from the payload, and only the component's shape is shared.

### Task 4: Per-slot tuning payload

Grow the host→client tuning payload from one weapon's fire values to the set the pawn holds, and bump its epoch so a client on the old shape is rejected with a diagnostic rather than predicting with stale values. The payload today carries one optional block of four fields resolved from the pawn class's `default_weapon`; it becomes an ordered per-slot set sized by the engine slot capacity, each occupied entry carrying that weapon's fire values, its lower and raise durations, and its resolved block-during-reload rule, alongside the same movement descriptor. **Resolve it from the pawn's live `Inventory` component, not the authored descriptor array** — the component is what survives a runtime change to what the pawn holds, so resolving from the descriptor would need re-sourcing the moment pickup lands; the payload's per-client change detection and re-send already handle a set that varies at runtime. Carry no cursor, no dwell, and no player preference: those are input-layer values that never enter the simulation, and putting them on the wire would reintroduce the divergence the layering exists to prevent. The payload is canonical JSON on the control channel, change-detected per client and re-sent on slot accept and after level install; keep all of that, and keep the rule that the movement descriptor's view-feel field is always cleared because view feel is local presentation. Update the committed payload fixture. A payload arriving mid-switch must not retroactively alter an in-flight switch.

### Task 5: Mod-global switch policy and its per-weapon override

Add a `switching` block to the mod manifest carrying the game's switch rules, and a per-weapon override for the one rule the simulation evaluates. `defineMod` is already a pure identity builder returning a manifest, and stores, UI trees, themes, map catalogs, and frontend declarations already arrive as manifest data rather than import-time side effects, so this is an entry in an established pattern and needs no new registration primitive. The block declares whether a direct-select commits immediately, the cycle-commit dwell in milliseconds, and whether a commit may interrupt reload activity. Only the last of those is a simulation rule; the first two are read by the input layer in Task 6 and never leave the machine they run on. The block-during-reload rule takes an optional per-weapon override on the weapon block, because abandoning a per-shell reload and abandoning an atomic load are different design calls — resolve it through the weapon's effective-stats accessor rather than reading the authored field directly, so a future augment that alters fixed classifiers has somewhere to land, matching how `reloadStyle` is already projected. Validate the block when the mod's declarations are validated: a negative or non-finite dwell is rejected there rather than clamped at use. The resolved per-weapon value is what Task 4 puts in the payload, so host and client evaluate an identical rule.

### Task 6: Input layer — wheel, cursor, dwell, and the player override

Own everything between the player's hardware and a commit intent, and let nothing below it see the parts that are local. Add discrete scroll physical inputs and the actions that consume them: a scroll notch is a momentary button, not an analog axis, because the tree has no scroll variant, no window-event arm for it, and routing an analog delta to the sim would mean a new analog field on the movement input, the wire mirror, prediction replay, and sanitization — where the existing per-frame axis clear explicitly handles only look axes. Add up and down scroll physical inputs, a window-event arm producing them, and normalization for the two scroll-delta units the windowing layer reports (line-based and pixel-based, the latter accumulated to a threshold); there is no precedent for either in the tree, so pin the threshold and cover both units in tests. Bind them to cycle-next and cycle-previous actions and add direct-select actions up to the engine slot capacity, beyond whatever Task 2 wired. Hold the pending cursor and its dwell timer here, not in any component: a cycle moves the cursor and restarts the dwell; a direct-select moves it and, if the mod declared immediate commit, emits at once; the dwell elapsing with no further movement emits. A cursor movement that would land on the active slot or an empty one emits nothing and does not move. Resolve the dwell as the player's override when set, else the mod's declared value, and add that override to the persisted player-options store as an optional field with `serde(default)` — the crouch-mode field is the precedent for an input-layer interpretation preference with no SDK surface, and per standing policy this task ships the stored field and its resolution, not a settings menu. The only thing crossing into the simulation is a bounded commit intent naming a slot, on the command path Task 2 established, clamped by sanitization against the engine slot capacity so a hostile or glitched client cannot drive an unbounded walk. Scroll must remain unclaimed by the debug overlay during gameplay focus, which is already the case: the overlay's consumed flag is honored only outside gameplay focus and its whole block sits behind a non-default feature.

### Task 7: `player.weapon.*` state slots

Publish the active wieldable's identity as engine-owned state so a HUD can show what is held and what is pending. Add three built-in engine state slots — the current weapon's canonical archetype name, the pending selection's archetype name, and whether a switch is in flight — declared in the engine state catalog beside the existing weapon-adjacent slots. The current and switching slots take the same owner-private replication scope the existing weapon slots use, so a connected client receives its own values and no one else's. **The pending slot is local scope, not replicated**: the cursor is input-layer state that exists only on the machine the player sits at, and giving it a replication scope would put a value on the wire that the simulation never sees. Two slots are string-typed and one boolean; the catalog already supports both, and nested SDK paths are already expressed as explicit segment arrays. The current-weapon slot names the committed active instance, so it flips at the repoint and not at the commit intent — during the lower it still names the outgoing weapon. Feed the replicated pair from the same host-and-single-player publisher that already writes health, ammo, reserve, and reload progress, and from the owner-private projection on the replication side, so both roles agree; feed the pending slot from the input layer on whichever machine owns the cursor. The publisher must stay short-circuited on a connected client so a client's non-authoritative local pawn cannot overwrite replicated values.

### Task 8: Typed loadout references and the dev-mod reference

Make the loadout hold descriptor references rather than name strings, and author the dev mod against them. Today a weapon is named by a string resolved at level install, where an unregistered name and a name resolving to a weapon-less descriptor both degrade to a warning and an unarmed player. Change the loadout's element type to the descriptor value that `defineEntity` already returns — it is a pure identity builder, mod scripts already import each other across files, and the referring script sits in the same bundle as the referenced one, which is the condition the durable naming rule gives for preferring a reference over a string. Constrain the accepted type to descriptors declaring a weapon block so a weapon-less reference is an editor-time error, and reject it again when the mod's declarations are validated, naming the offending entry — the type constraint is not an enforced gate, because the repo runs no typecheck step in CI. Lower the references to canonical names when the manifest is built, so nothing new crosses the FFI and the Rust and wire sides are unchanged. **Depend only on value equality, never object identity**: the Luau require implementation performs no module caching, so requiring one file twice yields distinct objects, and mod-init and level data scripts are separate bundles in separate VMs. Update the dev mod's player script to import both reference weapon descriptors and hold them in its loadout, and declare the mod's switching block in its start script with the wheel dwell and the direct-select rule, so the number-key and wheel behaviors are both exercisable in-game.

### Task 9: Ordering and edge coverage

Cover every row of the Orderings table in the plan with a test that names its scenario, and cover the cross-instance state-preservation and reserve-pooling invariants directly rather than through the switch path alone. The rows span three layers — input-layer cursor and dwell resolution, the state machine's transition rows, and host/client convergence — so place each test at the layer that owns its outcome rather than driving everything through a full-session harness. The zero-duration, empty-slot, single-entry-loadout, and duplicate-descriptor rows are cheap and belong at the machine and component level. The two-players-with-different-dwells row and the refused-commit row need the two-peer path; use the existing latency-simulation harness for the loss-and-jitter convergence criterion rather than standing up a new one. Cover the dwell override's resolution order — player value when set, mod value otherwise — at the input layer, where no session is needed.

## Sequencing

**Phase 1 (sequential):** Task 1 — behavior-preserving split; every later task edits this file.
**Phase 2 (sequential):** Task 2 — thin slice, falsifies the boundary assumptions across input, sim, component, presentation, and the client prediction path before anything fans out.
**Phase 3 (concurrent):** Task 3, Task 4, Task 5, Task 7 — independent surfaces on the slice's contracts.
**Phase 4 (sequential):** Task 6 — consumes Task 5's declared dwell and Task 2's commit-intent path.
**Phase 5 (concurrent):** Task 8, Task 9 — Task 8 authors against Task 5's block and Task 6's inputs; Task 9 covers the full table.

## Rough sketch

`Inventory` lands in `crates/entities/src/components/inventory.rs` with a new `ComponentKind` and `ComponentValue` arm, following `AmmoReserve` (`crates/entities/src/components/ammo_reserve.rs`) as the nearest shape precedent — private storage, small accessor surface.

Machine work is confined to the dispatch function Task 1 splits out of `sim/weapon_stage.rs`. `WieldableState` gains `Lowering`/`Raising` in `crates/entities/src/components/wieldable_state.rs`; its three `const fn` predicates have exhaustive matches, so adding variants surfaces every decision site as a compile error. The new `BeginLower` event sits beside `BeginReload`/`Expired`/`Cancel`; the existing `(Reloading, Cancel) => Noop` row is untouched.

The cursor and dwell live beside the crouch-mode resolution in `crates/postretro/src/input/`, and the override field beside `crouch_mode` in `crates/postretro/src/options/mod.rs`.

Manifest changes are in `sdk/lib/data_script.ts` and its Luau parity file plus the `ModManifest` type; the weapon block and `lowerMs`/`raiseMs` are on `WeaponDescriptor` in `crates/foundation/src/data_descriptors/types/combat.rs` and its typedef surfaces.

Tuning payload changes are local to `crates/postretro/src/netcode/tuning_payload.rs` plus its build/send path in `netcode/mod.rs`; the committed fixture is `crates/postretro/src/netcode/tests/fixtures/tuning_payload.expected.json`.

Input additions touch `crates/postretro/src/input/types.rs`, `input/defaults.rs`, the `WindowEvent` match in `main.rs`, the command build, and `netcode/wire_convert.rs` for the wire mirror and sanitize clamp.

State slots are entries in `BUILTIN_ENGINE_STATE` (`crates/entities/src/engine_state_catalog.rs`), fed by `scripting/systems/ui_proxy.rs` and projected in `netcode/state_slots.rs`.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Inventory component | `ComponentValue::Inventory` | `"inventory"` | `inventory` (descriptor block) | `inventory` | n/a |
| Loadout list | `InventoryDescriptor::loadout` | `"loadout"` (array of canonical-name strings) | `loadout` (array of descriptor refs) | `loadout` | n/a |
| Lower duration | `WeaponDescriptor::lower_ms` | `"lowerMs"` | `lowerMs` | `lowerMs` | n/a |
| Raise duration | `WeaponDescriptor::raise_ms` | `"raiseMs"` | `raiseMs` | `raiseMs` | n/a |
| Per-weapon reload-block override | `WeaponDescriptor::block_switch_during_reload` | `"blockSwitchDuringReload"` | `blockSwitchDuringReload` | `blockSwitchDuringReload` | n/a |
| Mod switching block | `ModManifest::switching` | `"switching"` | `switching` | `switching` | n/a |
| Direct-select commit rule | `SwitchingDescriptor::commit_on_direct_select` | `"commitOnDirectSelect"` | `commitOnDirectSelect` | `commitOnDirectSelect` | n/a |
| Cycle dwell | `SwitchingDescriptor::cycle_commit_dwell_ms` | `"cycleCommitDwellMs"` | `cycleCommitDwellMs` | `cycleCommitDwellMs` | n/a |
| Reload-block rule | `SwitchingDescriptor::block_during_reload` | `"blockDuringReload"` | `blockDuringReload` | `blockDuringReload` | n/a |
| Dwell player override | `PlayerOptions::switch_cycle_dwell_ms` | `switch_cycle_dwell_ms` (TOML, snake_case) | n/a — no SDK surface | n/a | n/a |
| Lowering state | `WieldableState::Lowering` | `"lowering"` | n/a | n/a | n/a |
| Raising state | `WieldableState::Raising` | `"raising"` | n/a | n/a | n/a |
| Current weapon slot | n/a (catalog entry) | `"player.weapon.current"` | `getGameState().player.weapon.current` | same path | n/a |
| Pending weapon slot | n/a (catalog entry) | `"player.weapon.pending"` | `getGameState().player.weapon.pending` | same path | n/a |
| Switching flag slot | n/a (catalog entry) | `"player.weapon.switching"` | `getGameState().player.weapon.switching` | same path | n/a |

## Script syntax examples

```ts
// Proposed design — a weapon is a `weapon` block on defineEntity.
// There is no defineWeapon (plans/done/M10--weapon-primitives).
export const referenceShotgunEntity = defineEntity({
  name: "reference_shotgun",
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
  name: "player",
  components: {
    movement: playerMovement,
    inventory: { loadout: [referenceShotgunEntity, referencePistolEntity] },
  },
})
```

```ts
// Proposed design — switch rules are declared once per game on the manifest,
// not per character class. Only blockDuringReload reaches the simulation;
// the other two are read by the input layer and never cross the wire.
export default defineMod({
  entities: [playerEntity, referenceShotgunEntity, referencePistolEntity],
  switching: {
    commitOnDirectSelect: true,
    cycleCommitDwellMs: 500,
    blockDuringReload: false,
  },
})
```

## Open questions

- **Pixel-delta scroll normalization has no precedent.** The threshold at which accumulated pixel-based scroll counts as one notch is picked in Task 6 against nothing in the tree, and no fixture exercises trackpad input. Reversible; the risk is that it reads badly out of the box on hardware nobody tested. The player-options override blunts it for the dwell but not for the notch itself.
- **Whether the per-weapon reload-block override earns its keep.** It is the only two-level resolution in the plan, and neither reference policy needs it. It is here because the design call it encodes is genuinely per-weapon, but a reviewer should judge whether one authored knob justifies a resolution order the payload also has to carry.
