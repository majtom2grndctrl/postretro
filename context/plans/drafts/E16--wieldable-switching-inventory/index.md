# Wieldable Switching + Inventory

## Goal

Give a pawn an ordered inventory of wieldable instances and one active reference that repoints between them, preserving each instance's own state. Converge the three divergent active-weapon holders onto that inventory so single-player, listen-host, and connected client read one source of truth. The engine owns selection, commitment, and the timed lower/raise; the mod authors *when* a selection commits.

## Scope

### In scope

- An `Inventory` component on the pawn holding an ordered set of wieldable slots plus the active and pending indices. Its length is an **engine capacity constant**, not the authored loadout length — a shorter loadout leaves trailing slots empty.
- Separation of **selection** (moving a pending cursor — changes nothing held) from **commitment** (repointing the active reference).
- `Lowering` and `Raising` equip states on the shipped wieldable machine, with per-archetype durations.
- Retirement of `App.active_wieldable` and `App.active_wieldable_descriptor`; `WeaponOwners`' pawn→weapon map replaced by the component. All read sites rewired, including the headless observability driver.
- Retirement of `ClientWeaponState` **as a holder**: a connected client runs the same `Inventory` component and the same cursor, so "which wieldable is active" has one implementation in every session role. The client keeps a distinct *fire-prediction carrier*, now per slot — see the note below.
- Per-slot fire tuning plus switch policy in the host→client tuning payload, so a connected client predicts a switch locally.
- A `@wieldable.*` IR input namespace and an authored `commitWhen` guard, mirroring the `@brain.*` construction.
- Discrete mouse-wheel physical inputs and direct-select / cycle actions, carried to the sim as bounded intents.
- `player.weapon.current`, `player.weapon.pending`, `player.weapon.switching` engine state slots.
- A `loadout` array on the player descriptor, replacing the single `defaultWeapon` string.
- A dev-mod reference script authoring both input styles — number key commits immediately, wheel commits after a dwell — over one vocabulary.

### Out of scope

- **Pickup and drop.** A weapon-only descriptor is rejected as a direct map placement (`is_directly_map_placeable`, `scripting/builtins/data_archetype.rs:771`; rejection at `:1323`, "equip targets, not direct map placements"), so no weapon instance can exist in the world to be picked up. Roadmap `E16 › Weapon Systems › pickup` owns it; its prompt affordance is owned by the unbuilt **combat presentation substrate** (`context/plans/roadmap.md:229`).
- **Dual-wield.** No descriptor field can express an off-hand wieldable — `EntityTypeDescriptor` (`crates/entities/src/data_descriptors/types/entity.rs:315`) carries a single weapon slot, and this plan's active reference is a single index. The state is unrepresentable, so the case is unreachable.
- **Augments, rolls, and non-passthrough stat resolution.** `WeaponComponent::effective` takes `&self` only and is a pure projection (`crates/entities/src/components/weapon.rs:316`); the component stores no modifier or roll data, so no composed stat can exist.
- **Heat and cell resources.** `WeaponResource` is a tagged union with exactly one arm, `{ kind: "ammo" }` (`sdk/types/postretro.d.ts:262`).
- **Secondary activation / alt-fire.** `Action::AltFire` is bound (`crates/postretro/src/input/defaults.rs:30`) with zero consumers outside `input/`; roadmap `E16 › Weapon Systems › secondary activation` owns it.
- **Mod-authored input bindings.** The SDK exposes no `Action` type and no action or axis read surface, so a mod cannot name a physical input to bind. The engine names the switch intents; the mod authors policy over them.
- **Radial / ring weapon selector.** `UiInstance` (`crates/ui/src/output.rs`) and `ui_quad.wgsl` draw axis-aligned quads only; a ring needs the radial primitive deferred at `context/plans/roadmap.md:149`. This plan publishes the slots a selector binds to; a list-shaped selector is authorable on shipped widgets.

### The client's prediction carrier is not a fourth answer

The three holders answer "which wieldable is active." All three lose that job: single-player, listen host, host-simulated remote pawn, and connected client all read the pawn's `Inventory`. What survives on the client is narrower and differently shaped — the locally predicted fire scalars (cooldown remaining, its authority generation, the consumed-press latch) that Epic 15 Phase 3 requires any predicted action to carry. Today those scalars sit on a single-weapon struct that *also* implies which weapon is held; after this plan they are per slot and imply nothing about the active one. A carrier of predicted values is not a holder of identity, and the distinction is testable: with the loadout, cursor, and active index all in the component, no code path can ask the client which wieldable is active and get a different answer than the host would give.

### Ships knowingly broken — owner decision

**Inventory and ammo reserve do not survive a level transition.** Nothing in source forecloses carrying them; this is a choice. The durable per-player key that carry needs is the host-minted **seat**, unbuilt in E15 Phase 3.75 (`context/plans/roadmap.md:202`), which `drafts/E16--per-player-currency` is already parked on. Building carry now means either blocking on that spec or standing up a single-player-only carry path — a fourth divergent holder, the exact disease this plan cures. Consequence shipped: a campaign cannot carry weapons or ammo across levels; every level re-equips from the player descriptor and re-seeds the reserve. Owner decision, 2026-07.

## Direction

**Problem.** A pawn's active weapon is stored three different ways, and none of them can change at runtime. `App.active_wieldable` is written only at level install and teardown; `WeaponOwners` is host-only; a connected client owns no weapon entity at all and models its weapon as four floats resolved from the pawn class's `default_weapon`. There is no place to put a second wieldable, and no path by which the active one could change. The cause is that "the weapon" was modeled as a property of the *session role* rather than of the *pawn*.

**Prior commitments.** `context/research/weapon-model.md` §6/§7 pins the shape: switching repoints an active reference, per-instance state survives because instances own it, and the container plus its equip/switch machinery are named for **wieldables**, not weapons (invariant 7), with inventory a peer of the pawn's `Health` and `AmmoReserve` rather than a parent (§1). `crates/entities/src/components/wieldable_state.rs:9` states outright that equip states join that enum when switching owns their behavior, and `E16--weapon-state-machine` shipped its preemption seam for this. E15's session-lifecycle spec set the rule that a client's predicted values are **replicated, not hashed** — the four weapon fire fields are deliberately absent from the content-parity digest because the host sends them. This plan honors that rule by growing the payload rather than the digest: the moment a pawn holds N wieldables, replicating one archetype's tuning while the client predicts with another would break the guarantee that made the exclusion safe.

`E21--coop-avatar-weapon-presentation` deferred the switch input path to this plan explicitly (`index.md:33`: *"This plan renders whatever weapon the host assigns"*) and shipped the machinery that assignment needs — the replicated active-archetype field, the client-side change detection, and the hand-socket rewrite. AC 9 is close to free as a result, and this plan must not stand up a second presentation path beside it.

Where this diverges: the player descriptor's `defaultWeapon` string is **replaced** by a `loadout` array rather than kept as sugar. E15's parity reasoning names `default_weapon` explicitly as the path a client reads its fire fields through, so leaving a second, one-weapon path alive would preserve exactly the divergence this plan exists to remove. The tree is pre-stable and `content/dev/scripts/player.ts:5` is the sole consumer, so the call sites move in the same change (`context/lib/index.md`, pre-stable note).

**Placement.** The floor sits in the engine because every piece of it is either state a client must predict (the cursor, the timed states) or authority a host must own (the repoint, the reserve debit). The commit *rule* sits in the mod because it is a game-design decision — Doom commits instantly, Halo toggles, a wheel dwells — and none of those is more correct. The rule is expressed as IR rather than a descriptor scalar because a scalar only lets an author retune the engine's policy, while a guard lets them write one the engine never anticipated. This mirrors the enemy behavior graph, where the engine keeps target selection and think-stride and the graph owns ordered guards.

**Alternatives rejected.** The strongest rival is *host-authoritative switching with no client prediction*: the client sends a switch request and applies the result when the host's snapshot says so. It is markedly cheaper — no payload change, no epoch bump, no client-side guard evaluation. It was rejected because switching is an input-driven state change on the local pawn, which is precisely the category Epic 15 Phase 3 built prediction for; shipping it unpredicted would make weapon switching the one player action that visibly waits for the network, and retrofitting prediction later touches the same payload, the same machine, and the same reconciliation path a second time.

The rival an owner is most likely to be tempted by is a *scope split*: ship the convergence, the states, the payload, the input, and the slots, but express the commit rule as descriptor scalars (`commitDwellMs`, `commitOnDirect`, `blockDuringReload`) and defer the `@wieldable.*` namespace and its guard — Tasks 5 and 8 — until pickup and dual-wield have shown what switch policy needs to say. It is genuinely cheaper: no name table in the foundation crate, no twin declaration-time binding, no validation scope, no runtime scope, no two SDK preludes, no drift tests, no guard carriage in the payload. And the tell is real — both reference policies in Task 8 *are* expressible as three scalars, so "a guard lets an author write a rule the engine never anticipated" is asserted here, not demonstrated. It is rejected on two grounds that do not depend on that demonstration. First, the migration runs the wrong way: adding the guard later means deprecating authored scalars, which this plan's own reversibility analysis identifies as the expensive direction, whereas a scalar is trivially re-derivable from a guard. Second, this is a trodden construction, not a novel one — `@brain.*` shipped it and `M14--movement-dash-runtime-values` shipped clients evaluating authored IR for prediction, so the cost being avoided is mostly the cost of doing a known thing again.

A third rival is *storing the inventory as a side-table keyed by pawn*, i.e. generalizing `WeaponOwners` in place rather than adding a component. Rejected because a side-table is precisely what made the current state divergent — it lives on `NetEndpoint::Host` and therefore cannot exist single-player, which is why `App.active_wieldable` exists at all. A component is despawned with its pawn, replicates through the existing entity paths, and has one home in every session role.

**Foreclosures and one-way doors.** The tuning payload epoch bump is one-way in the sense that a client on epoch 1 is rejected by `decode_tuning_payload`; this is the intended behavior of an epoch and costs nothing to undo beyond a further bump. Replacing `defaultWeapon` with `loadout` is a breaking descriptor change; undoing it means re-editing the descriptor type, the Rust mirror, both typedef surfaces, and dev content — bounded, but not free. Making the commit rule an IR guard forecloses a *later* move to a plain scalar without a deprecation path, since authored guards would need migrating. Nothing here forecloses pickup, dual-wield, or augments: all three extend the slot array or the instance, not the active reference.

## Acceptance criteria

- [ ] A pawn spawns holding every wieldable named in its descriptor's loadout, with the first slot active; each slot holds a distinct instance, and two slots naming the same archetype hold two independent instances.
- [ ] Pressing a direct-select input for an occupied slot other than the active one plays the outgoing weapon's lower, then the incoming weapon's raise, and the incoming weapon becomes active exactly once.
- [ ] Scrolling the wheel moves the pending selection without changing what is held; the switch begins only after the authored dwell elapses with no further scroll, and a scroll during the dwell restarts it.
- [ ] A weapon switched away from and back to retains its own magazine count, cooldown remaining, and reload progress state — it is not re-created and does not inherit the other weapon's values.
- [ ] Firing and reloading are both refused for the whole lower and raise, and the reload indicator does not read as active during a switch.
- [ ] Switching away during a per-shell reload keeps the shells already loaded; switching away during an atomic reload loads none, and neither leaves the reserve short or over-credited.
- [ ] Ammo reserve is shared: two weapons of the same authored ammo type draw from one pool on the pawn, and switching between them does not move, duplicate, or reset reserve rounds.
- [ ] A connected client's switch is visible locally on the input frame — the lower begins without waiting for a host round trip — and the client and host agree on the active slot after reconciliation with 150 ms RTT, 5% loss, and jitter.
- [ ] A remote player's avatar shows the weapon they switched to, and the local player's viewmodel shows theirs, on both host and client.
- [ ] The HUD's ammo, reserve, reload, and cooldown values follow the active weapon across a switch, on both host and connected client.
- [ ] A mod authors "number key commits immediately, wheel commits after 500 ms" without engine changes, and a second policy that refuses to commit while a reload is running, using the same vocabulary.
- [ ] Every ordering in the Orderings table resolves to its stated outcome.
- [ ] A client whose tuning payload predates this change is rejected with a diagnostic rather than predicting with stale weapon values.
- [ ] Selecting an empty slot, or a slot index beyond the authored loadout length, leaves the active weapon unchanged and logs no error at gameplay severity.
- [ ] The engine reports the same active wieldable in single-player, on a listen host, for a host-simulated remote pawn, **and on a connected client** — no session role has its own answer, and the client's answer comes from the same place the host's does.
- [ ] Reload behavior is unchanged from before this plan: atomic reload still cannot be cancelled, per-shell reload still cancels with its loaded shells credited, and both still transfer the same rounds for the same inputs.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Exactly one live wieldable machine per pawn | Task 2 (`Inventory.active`) | Task 3 removes the global that could name a second; a slot holding a despawned instance must clear, not linger | AC 1, 15 |
| A repoint preserves the instance's own state | Task 2 (repoint touches indices only) | Task 4 must not rebuild a component from tuning on switch; Task 3's rewires must not re-seed | AC 4, 7 |
| The lower→raise handoff crosses instances exactly once per commit | Task 2 (latched target, repoint on lower expiry) | Task 5's guard may re-fire during `Lowering`; re-latching must not emit a second repoint | AC 2, Orderings O3, O4 |
| Fire and reload are refused for the whole switch, and a switch is not reload activity | Task 2 (state predicates) | Task 7's projection reads the same predicates for `player.reloadActive` | AC 5 |
| Ammo reserve is pooled on the pawn, never on an instance | shipped (`AmmoReserve`) | Task 2 must not move it into `Inventory`; Task 3's rewires re-source the projection | AC 7 |
| Host and client converge on the same active slot | Task 2 (host authority), Task 4 (per-slot tuning) | Task 5's guard evaluates on both peers; divergent authored data would split them | AC 8 |
| Presentation reflects the committed active, not the pending cursor | Task 2 (dirty at repoint) | Task 6's cycle intents move pending every notch — presentation must not follow | AC 3, 9 |

## Orderings

Scenario, ordering, expected outcome. Task 9 cites these rows; other tasks must not restate them.

| # | Scenario | Ordering | Expected |
|---|---|---|---|
| O1 | Direct-select and cycle intents on one tick | both present in the same command | direct-select wins; cycle is discarded, not applied after |
| O2 | Two or more wheel notches inside one frame | N notches, one command | cursor moves N steps, bounded by the sanitize clamp; one dwell restart, not N |
| O3 | Cycle arrives while `Lowering` | commit already latched slot B, cursor moves to C | guard decides: if it commits, the latch becomes C and the lower timer is **not** restarted; if it does not, the switch still completes to B |
| O4 | Commit guard true on the tick the lower expires | expiry and commit in the same tick | expiry runs first — the pending repoint completes before a new latch is taken |
| O5 | Commit guard true on the tick a reload expires | reload expiry and switch commit collide | reload entry/expiry ordering is unchanged; the reload completes and credits, then the lower begins |
| O6 | Pending equals active | cursor cycles back to the held weapon | no commit, no lower, no presentation dirty — the switch is a no-op |
| O7 | `lowerMs` or `raiseMs` authored zero | commit with a zero-duration step | the state is entered and expires on the same tick; the repoint still happens exactly once |
| O8 | Selected slot is empty | loadout shorter than the selected index, or a slot whose instance despawned | cursor does not move; active unchanged |
| O9 | Switch intent on the tick of level unload | commit latched, then unload | no repoint after teardown; no dangling instance reference survives |
| O10 | Pawn despawns mid-switch | `Lowering` in flight, pawn removed | both instances despawn with the pawn; no orphan weapon entity |
| O11 | Client predicts a commit the host does not take | client committed, host's snapshot disagrees | client reconciles to the host's active slot; the correction does not replay as a second switch |
| O12 | Tuning payload arrives mid-switch | new payload installed while `Raising` | in-flight switch completes; new tuning applies to subsequent switches, not retroactively |
| O13 | Loadout authored with one entry | single-slot loadout | cycle and direct-select are no-ops; behavior matches today's single-weapon pawn |
| O14 | Two slots naming the same archetype | loadout `["pistol", "pistol"]` | two independent instances; each keeps its own magazine |

## Tasks

### Task 1: Split `sim/weapon_stage.rs`

`crates/postretro/src/sim/weapon_stage.rs` is 3,452 lines and this plan adds two states, a new event variant, and its transition rows to the machine it hosts. Split it first, behavior-preserving, along the seams already visible in the file: the ordered per-tick machine driver, the state/event dispatch and its transition helpers, the fire authorization path, and the local/remote command entry points. No behavior, signature, or test assertion changes — this is a move, and the existing tests must pass unmodified. Do not extend anything here; the states land in Task 2. Leave the single-dispatch-point property intact: after the split there is still exactly one function that matches state against event, and its doc comment still says new states add rows rather than changing component shape.

### Task 2: Thin slice — inventory, equip states, direct-select switch, end to end

Build the narrow real version of the whole path and integrate it before anything fans out. Add an `Inventory` component on the pawn in `crates/entities` — an ordered array of optional wieldable entity ids, an active index, a pending index, and the engine-accumulated milliseconds since the pending index last changed. **The array's length is an engine capacity constant, not the authored loadout length.** A loadout shorter than capacity leaves trailing slots empty, and every bound in this plan — this component's indices, the wire sanitize clamp, the payload's per-slot cardinality, and the direct-select action count — binds to that constant, never to what a descriptor happened to author. Sizing to the loadout would make pickup, the next roadmap item, a change to all four at once. Name it for wieldables, not weapons: a weapon is the first kind it holds. It is a peer of the pawn's `Health` and `AmmoReserve`, never a parent — the ammo reserve stays where it is and is not moved into this component. Add `Lowering` and `Raising` to `WieldableState`; both deny fire and reload and neither is reload activity, so a switch never reads as a reload. The enum's predicates use exhaustive matches, so the compiler will name every site that must decide. Add a `BeginLower` event to the machine's event type with rows from every existing state — from idle, from atomic reload (the in-flight reload is forfeited; nothing has transferred yet), from per-shell reload (already-credited shells stay, mirroring the existing cancel row), and from `Lowering` itself (re-latch the target, do **not** restart the timer). Do not modify the existing cancel rows: atomic reload stays uncancellable by cancel, and shipped reload behavior must not change. Commitment latches the target slot; when `Lowering` expires the engine repoints the active index to the latched slot and starts `Raising` on the incoming instance. Spawn the pawn's instances from a `loadout` array on the player descriptor, replacing the single `defaultWeapon` string — update the descriptor type, its Rust mirror, both typedef surfaces, and `content/dev/scripts/player.ts`, whose `reference_shotgun` becomes a two-entry loadout with the already-registered `reference_pistol`. Add per-archetype `lowerMs` and `raiseMs` to the weapon descriptor. Wire one input path only — direct-select actions for slots, no wheel — and carry the intent to the sim on the command struct, mirrored onto the movement input for the client-prediction boundary the same way `use_pressed` already is, with the wire mirror and a sanitize clamp against the engine slot capacity. Commit immediately on selection in this task; the authored guard lands in Task 5. Make the connected-client path work in this slice too, even though its per-slot tuning is incomplete: the client must move its own cursor and run its own machine so the slice actually falsifies the prediction boundary rather than deferring it. Presentation must dirty at the repoint, not at commit.

### Task 3: Retire the global holder and converge every reader

Delete `App.active_wieldable` and `App.active_wieldable_descriptor`, and remove the pawn→weapon map from `WeaponOwners`, keeping only its attachment dirty set. The descriptor field is write-only — no read site exists in the tree, and hot reload refreshes weapons through descriptor provenance instead — so delete it rather than migrating it. Every reader of the two holders resolves through the pawn's `Inventory` instead: the fire path, the HUD publisher, the per-frame viewmodel resolution (host, client, and the single-player fallback that synthesizes a throwaway owner map each frame), the level-install attachment sync, the snapshot's active-archetype fill, the owner-private ammo and cooldown projections, the remote-pawn command preparation, and the despawn cleanup that treats pawn and weapon as one ownership unit. Rewire the headless observability driver's two read sites as well — it is a real consumer with no gameplay acceptance criterion covering it. Remove the listen-host bridge that copied the global into the owner map after install: the host's own pawn now carries an inventory like any other pawn, so the special case disappears rather than moving. Level teardown must clear the component with the pawn and leave no dangling instance reference; the level-scoped host reset no longer has a weapon map to replace. On the connected-client side, retire the single-weapon prediction struct **as a holder**: which wieldable is active comes from the client's own `Inventory` component, spawned from the host-sent per-slot tuning, so the client answers that question the same way every other role does. What remains client-side is a per-slot carrier of locally predicted fire scalars — cooldown remaining, its authority generation, the consumed-press latch — which Epic 15 Phase 3 requires any predicted action to keep. Keep the existing rule that a connected client never resolves weapon *tuning* from its local registry: the values come from the payload, and only the component's shape is shared.

### Task 4: Per-slot tuning payload

Grow the host→client tuning payload from one weapon's fire values to the set the pawn holds, and bump its epoch so a client on the old shape is rejected with a diagnostic rather than predicting with stale values. The payload today carries one optional block of four fields resolved from the pawn class's `default_weapon`; it becomes an ordered per-slot set sized by the engine slot capacity, each occupied entry carrying that weapon's fire values plus its lower and raise durations, alongside the same movement descriptor. **Resolve it from the pawn's live `Inventory` component, not from the authored descriptor array** — the component is what survives a runtime change to what the pawn holds, so resolving from the descriptor would need re-sourcing the moment pickup lands; the payload's per-client change detection and re-send already handle a set that varies at runtime. The payload is canonical JSON on the control channel, change-detected per client and re-sent on slot accept and after level install; keep all of that, and keep the rule that the movement descriptor's view-feel field is always cleared because view feel is local presentation. Update the committed payload fixture. The client installs the set and its machine reads the entry for its active slot; a payload arriving mid-switch must not retroactively alter an in-flight switch. Note for grounding: this payload already carries authored IR inside the movement descriptor's expression-capable fields, so carrying per-slot data and, in Task 5, a guard is continuous with what it already does.

### Task 5: `@wieldable.*` IR scope and the authored commit guard

Add a fixed IR input namespace for switch policy, built exactly like the `@brain.*` namespace it mirrors: the name table lives in the VM-free foundation crate so the twin descriptor parsers can bind authored guards at declaration time against a validation scope, while the runtime scope that reads live pawn state lives in the binary, both routing name resolution through one function so the namespaces cannot drift. The inputs are the milliseconds since the pending selection last changed, whether that change came from a direct-select rather than a cycle, whether a switch is already in flight, and whether reload activity is running. Order in the table is load-bearing — it is the runtime read handle — and each entry carries its projected IR type. Add the pre-wrapped leaf preludes to both SDK runtimes and keep the drift tests that fail when the tables disagree. The guard is a `commitWhen` boolean expression on an authored switch-policy block; the engine evaluates it only when the pending index differs from the active index, so committing to what you already hold is an engine-level no-op the author never has to write. Carry the guard through the tuning payload so a connected client evaluates the same policy and predicts locally rather than waiting for the host. The IR has no boolean and/or/not opcodes — `select` supplies them — so document that in the authoring surface rather than adding opcodes.

### Task 6: Wheel input and cycle intents

Add discrete scroll physical inputs and the actions that consume them. A scroll notch is a momentary button, not an analog axis: the tree has no scroll variant, no window-event arm for it, and routing an analog delta to the sim would mean a new analog field on the movement input, the wire mirror, prediction replay, and sanitization — where the existing per-frame axis clear explicitly handles only look axes. Add up and down scroll physical inputs, a window-event arm that produces them, and normalization for the two scroll-delta units the windowing layer reports (line-based and pixel-based, the latter accumulated to a threshold) — there is no precedent for this in the tree, so pin the threshold in the implementation and cover both units in tests. That threshold is a device-feel value in the same class as mouse sensitivity and invert-Y, so put it where those live — the player-options surface — rather than hardcoding it: a wheel and a trackpad disagree about what one notch is, and this plan's whole thesis is that feel decisions belong to someone other than the engine. Bind the scroll inputs to cycle-next and cycle-previous actions, and add the direct-select actions for the remaining slots up to the engine slot capacity, beyond whatever Task 2 wired. Cycle intent reaches the sim as a bounded signed step count on the same command path Task 2 established, clamped by sanitization against that same capacity so a hostile or glitched client cannot drive an unbounded cursor walk. Scroll must remain unclaimed by the debug overlay during gameplay focus, which is already the case: the overlay's consumed flag is honored only outside gameplay focus and its whole block is behind a non-default feature.

### Task 7: `player.weapon.*` state slots

Publish the active wieldable's identity as engine-owned state so a HUD can show what is held and what is pending. Add three built-in engine state slots — the current weapon's canonical archetype name, the pending selection's archetype name, and whether a switch is in flight — declared in the engine state catalog beside the existing weapon-adjacent slots, with the same owner-private replication scope those use so a connected client receives its own values and no one else's. Two are string-typed and one boolean; the catalog already supports both, and nested SDK paths are already expressed as explicit segment arrays. The current-weapon slot names the committed active instance, so it flips at the repoint and not at commit — during the lower it still names the outgoing weapon. Feed them from the same host-and-single-player publisher that already writes health, ammo, reserve, and reload progress, and from the owner-private projection on the replication side, so both roles agree. The publisher must stay short-circuited on a connected client so a client's non-authoritative local pawn cannot overwrite replicated values.

### Task 8: Reference switch policies in the dev mod

Author the switching example that proves the vocabulary is expressive rather than demonstrative. Ship two policies in the dev mod's scripts: the first commits immediately when the pending selection came from a direct-select and after a 500 ms dwell when it came from a cycle — the number-key-versus-wheel behavior, written as one guard; the second refuses to commit while reload activity is running, showing that a mod can block a switch mid-reload without the engine having an opinion. Both are authored over the same inputs with no engine change between them; if either needs one, the vocabulary is wrong and that is the finding. Give the dev player a loadout of the two already-registered reference archetypes so both policies are exercisable in-game, and note in the script's comments which input drives which path. Name the file so it does not collide with the existing UI switch demo in the same directory.

### Task 9: Ordering and edge coverage

Cover every row of the Orderings table in the plan with a test that names its scenario, and cover the cross-instance state-preservation and reserve-pooling invariants directly rather than through the switch path alone. The rows span three layers — input-to-command resolution, the state machine's transition rows, and host/client convergence — so place each test at the layer that owns its outcome rather than driving everything through a full-session harness. The zero-duration, empty-slot, single-entry-loadout, and duplicate-archetype rows are cheap and belong at the machine and component level. The client-predicts-what-the-host-rejects row and the mid-switch-payload row need the two-peer path. Use the existing latency-simulation harness for the loss-and-jitter convergence criterion rather than standing up a new one.

## Sequencing

**Phase 1 (sequential):** Task 1 — behavior-preserving split; every later task edits this file.
**Phase 2 (sequential):** Task 2 — thin slice, falsifies the boundary assumptions across input, sim, component, presentation, and the client prediction path before anything fans out.
**Phase 3 (concurrent):** Task 3, Task 4, Task 6, Task 7 — independent surfaces on the slice's contracts.
**Phase 4 (sequential):** Task 5 — consumes Task 4's payload carrier and Task 6's cycle intents.
**Phase 5 (concurrent):** Task 8, Task 9 — Task 8 authors against Task 5's vocabulary; Task 9 covers the full table.

## Rough sketch

`Inventory` lands in `crates/entities/src/components/inventory.rs` with a new `ComponentKind` and `ComponentValue` arm, following `AmmoReserve` (`crates/entities/src/components/ammo_reserve.rs`) as the nearest shape precedent — private storage, small accessor surface.

The machine work is confined to the dispatch function Task 1 splits out of `sim/weapon_stage.rs`. `WieldableState` gains `Lowering`/`Raising` in `crates/entities/src/components/wieldable_state.rs`; the three `const fn` predicates there have exhaustive matches, so adding variants surfaces every decision site as a compile error. The new `BeginLower` event sits beside `BeginReload`/`Expired`/`Cancel`; the existing `(Reloading, Cancel) => Noop` row is untouched.

The `@wieldable.*` namespace mirrors `crates/foundation/src/brain.rs` — a `&str` const per input, an ordered `(name, IrType)` table whose index is the runtime read handle, a validation scope for declaration-time binding, and a runtime scope in the binary. SDK preludes mirror `sdk/lib/brain.ts` and `sdk/lib/brain.luau`, whose drift tests live in `crates/scripting-core/src/data_descriptors/tests/behavior.rs`.

Tuning payload changes are local to `crates/postretro/src/netcode/tuning_payload.rs` plus its build/send path in `netcode/mod.rs`; the committed fixture is `crates/postretro/src/netcode/tests/fixtures/tuning_payload.expected.json`.

Input additions touch `crates/postretro/src/input/types.rs` (physical inputs, actions), `input/defaults.rs` (bindings), the `WindowEvent` match in `main.rs`, and the command build plus `netcode/wire_convert.rs` for the wire mirror and sanitize clamp.

State slots are entries in `BUILTIN_ENGINE_STATE` (`crates/entities/src/engine_state_catalog.rs`), fed by `scripting/systems/ui_proxy.rs` and projected in `netcode/state_slots.rs`.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Inventory component | `ComponentValue::Inventory` | `"inventory"` | n/a (not map-authored) | n/a | n/a |
| Loadout list | `InventoryDescriptor::loadout` | `"loadout"` | `loadout` | `loadout` | n/a |
| Lower duration | `WeaponDescriptor::lower_ms` | `"lowerMs"` | `lowerMs` | `lowerMs` | n/a |
| Raise duration | `WeaponDescriptor::raise_ms` | `"raiseMs"` | `raiseMs` | `raiseMs` | n/a |
| Switch policy block | `SwitchPolicyDescriptor` | `"switch"` | `switch` | `switch` | n/a |
| Commit guard | `SwitchPolicyDescriptor::commit_when` | `"commitWhen"` | `commitWhen` | `commitWhen` | n/a |
| Lowering state | `WieldableState::Lowering` | `"lowering"` | n/a | n/a | n/a |
| Raising state | `WieldableState::Raising` | `"raising"` | n/a | n/a | n/a |
| Dwell input | `WIELDABLE_SELECTION_DWELL_MS_INPUT` | `"@wieldable.selectionDwellMs"` | `wieldable.selectionDwellMs` | `wieldable.selectionDwellMs` | n/a |
| Direct-select input | `WIELDABLE_SELECTION_IS_DIRECT_INPUT` | `"@wieldable.selectionIsDirect"` | `wieldable.selectionIsDirect` | `wieldable.selectionIsDirect` | n/a |
| Switch-in-flight input | `WIELDABLE_SWITCH_IN_FLIGHT_INPUT` | `"@wieldable.switchInFlight"` | `wieldable.switchInFlight` | `wieldable.switchInFlight` | n/a |
| Reload-active input | `WIELDABLE_RELOAD_ACTIVE_INPUT` | `"@wieldable.reloadActive"` | `wieldable.reloadActive` | `wieldable.reloadActive` | n/a |
| Current weapon slot | n/a (catalog entry) | `"player.weapon.current"` | `getGameState().player.weapon.current` | same path | n/a |
| Pending weapon slot | n/a (catalog entry) | `"player.weapon.pending"` | `getGameState().player.weapon.pending` | same path | n/a |
| Switching flag slot | n/a (catalog entry) | `"player.weapon.switching"` | `getGameState().player.weapon.switching` | same path | n/a |

## Script syntax examples

```ts
// Proposed design — the player descriptor's loadout replaces `defaultWeapon`.
defineEntityType({
  name: "player",
  components: {
    movement: playerMovement,
    inventory: {
      loadout: ["reference_shotgun", "reference_pistol"],
      switch: {
        // Number key commits at once; wheel commits 500 ms after the last notch.
        // The IR has no `or` opcode — `select(a, true, b)` is the idiom.
        commitWhen: runtime.select(
          wieldable.selectionIsDirect,
          true,
          runtime.ge(wieldable.selectionDwellMs, 500),
        ),
      },
    },
  },
})
```

```ts
// Proposed design — a policy that refuses to switch mid-reload.
// Same vocabulary, no engine change: `select(cond, false, …)` is `and`.
switch: {
  commitWhen: runtime.select(
    wieldable.reloadActive,
    false,
    runtime.select(
      wieldable.selectionIsDirect,
      true,
      runtime.ge(wieldable.selectionDwellMs, 500),
    ),
  ),
}
```

```ts
// Proposed design — per-archetype equip timing on the weapon descriptor.
defineWeapon({
  name: "reference_shotgun",
  // …existing fire and resource blocks unchanged…
  lowerMs: 220,
  raiseMs: 350,
})
```

## Open questions

- **Pixel-delta scroll normalization has no precedent.** Task 6 routes the notch threshold to the player-options surface rather than hardcoding it, but the default it ships with is picked against nothing in the tree, and no fixture exercises trackpad input. Reversible; the risk is that it reads badly out of the box on hardware nobody tested.
- **The guard's inputs, not its provenance, are what can diverge.** Guard *provenance* is settled: the host sends it in the payload, so both peers bind the same expression — the same property E15 relied on when it excluded the weapon fire fields from the parity digest. The open half is that two of the guard's inputs — the selection dwell and the switch-in-flight flag — are **peer-local accumulations**, not replicated values. An identical guard over divergent local inputs commits at different moments on the two peers. O11 is the absorber and reconciliation is the right mechanism, but a reviewer should check whether the correction can read as a visible double-switch under sustained jitter rather than a single snap.
