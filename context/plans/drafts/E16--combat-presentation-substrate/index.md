# Combat Presentation Substrate (E16)

## Goal

Combat produces no presentation. Damage lands, health changes, and nothing the player can read comes out of it — the closed impact-effect set has no path to the UI, and the UI has no way to show a thing that appears, lives briefly, and disappears. Give the engine two combat facts on the wire and one pooled, keyed, lifetime-bounded UI instance primitive, then let mods author damage numbers and enemy health bars entirely in script.

The engine ships no damage-number emitter and no health-bar widget. It ships the facts, the prediction, and the pool.

## Prerequisites

- **`E16--impact-policy-substrate`** (shipped) — the `ImpactDispatch` this spec's impact fact is derived from, and the evaluate-then-apply snapshot model.
- **`E16--client-authoritative-combat`** (shipped) — the declared-hit round trip (`ClientFireResolution` → `HitDeclaration` → `ShotVerdict`) whose prediction ledger this spec extends to carry a presentation fact.
- **`E15--session-lifecycle`** (shipped) — the content-parity lanes this spec adds an entity-health lane to, and the "hash only what cannot be replicated" doctrine that decides the zone-multiplier question.

## Scope

### In scope

- **Two engine-defined combat facts on the wire** — an *impact fact* (one hit occurred) and an *absolute health snapshot* (a target's current and max). Closed schema, engine-owned, versioned.
- **Health-snapshot emission from every health mutation**, not only from damage — so a heal and a future `max` change are visible to a bar.
- **Client-side prediction of the impact fact**, reconciled by the host's authoritative copy, using the shipped shot-id ledger.
- **Third-party archetype tuning replication** — an enemy's `max` health and `zoneMultipliers` reach clients, so a predicted number is the number the host computes.
- **A keyed, pooled, lifetime-bounded UI instance primitive** carried on the UI snapshot, with an optional world anchor, an engine-owned hard cap, and script-owned keys.
- **SDK authoring surface** — a script declares an instance template, binds it to fact fields, and chooses the key.
- **Reference mod** — `content/dev/scripts/` authors floating damage numbers and a damaged-enemy health bar.

### Out of scope

- **Pickup prompts.** A prompt is a per-player level, not an instance with a lifetime. It rides slots, not this pool, and its facts belong to a sibling spec.
- **Per-player mod stores and per-owner slot cardinality.** `drafts/E16--per-player-currency` owns the `ownerPrivate` unlock and the `perOwner` declaration.
- **Shields and armor.** No `ComponentKind` for either exists — the roadmap line's "health/shield bars" is health-only until **Damage & Defenses** ships one.
- **Crit.** Unbuilt. The impact fact reserves the field and always reports false; a deterministic per-shot roll both peers reproduce is that spec's problem.
- **Mod-defined wire events.** Mods get replicated *state* (shared today; per-player under the currency spec). Events stay engine-defined because prediction and reconciliation need semantics the engine must understand.
- **New UI draw primitives.** No rotation, no radial, no arc. Numbers are `Text`; bars are `Bar`.
- **Aggregation policy.** Whether hits stack into a running total or fly separately is a script branch, not an engine mode.
- **Interest management.** Snapshot fan-out is per-target subscription, not a spatial or visibility relation. The general relevance substrate is Epic 15 Phase 4's.
- **A keybind display layer.** The reference prompt-free scripts here need no key glyphs.

## Direction

**Problem.** Two independent gaps meet in one feature, and neither is a symptom of the other.

The first is that policy has no outward channel. `ImpactEffect` is five arms — `Despawn`, `SetHealth`, `GrantHealth`, `GrantAmmo`, `PlayAnimation` (`crates/postretro/src/impact_effects.rs:41-47`) — and none reaches presentation. `E16--resource-grant-chokepoint` recorded this as a deliberate boundary.

The second is that the UI cannot express a transient. Every shipped widget is a retained node bound to a slot level. The renderer's reuse gate is whole-descriptor equality — `needs_build` is true when `retained.descriptor != *tree` (`crates/renderer/src/render/ui/mod.rs:752-757`) — so adding or removing a node per frame forces a full `UiTree::from_descriptor` rebuild. Retained state is keyed by stack position, never by identity: the snapshot's own doc says the renderer keys by stack position, not by name (`crates/ui/src/output.rs:174-176`), and the layer assert is `layer <= self.gameplay_trees.len()` (`ui/mod.rs:742-745`). There is no repeat or template-instancing concept anywhere; `RepeatPolicy` (`crates/ui/src/tree/draw.rs:33-36`) is key-repeat-on-hold for focus nav and unrelated.

Those two gaps decide the shape between them. The fact must be engine-defined because it crosses the wire and must be predicted. The instance must ride the *snapshot* rather than the descriptor, because the descriptor is the thing that must not change per frame — which is exactly why `slot_values` and `cell_values` exist (`output.rs:216-220`).

**Prior commitments.**

- *Presentation never writes back.* `ui.md:4` and `:62` — authoritative values live in the store; anything the UI animates is retained-UI-local and no store write originates in the UI module. Honored: the instance stream is snapshot-inbound only, with no reverse channel.
- *Presentation time is deterministic.* `UiReadSnapshot.time_seconds` is `App::script_time`, dt-accumulated, never wall clock (`crates/ui/src/output.rs:222-228`), advanced at exactly one site under a freeze gate (`crates/postretro/src/main.rs:2942`). Instance lifetimes read that clock, so pausing game logic pauses presentation.
- *No authoritative state crosses to the client viewer.* `crates/postretro/src/netcode/remote_materialize.rs:458-470` asserts a materialized remote enemy attaches no `Brain`, `Agent`, `Health`, `Weapon`, or `PlayerMovement`. Honored: no `HealthComponent` appears client-side. A health *snapshot* is a presentation fact addressed to a subscriber, not a component.
- *Hash only what cannot be replicated.* `E15--session-lifecycle` established that the host sends the values a client predicts with, "so replication makes peers *agree* where a digest only lets them refuse each other." This decides the zone-multiplier question against hashing — see *Alternatives rejected*.
- *Net is a registry-blind courier.* `ServerControlMessage::Tuning(Vec<u8>)` is documented as a payload net must never decode (`crates/net/src/wire/control.rs:243-245`). Honored: archetype tuning extends that opaque payload rather than teaching net about health.
- *Caps refuse the newest and warn.* The emitter drops with a 1 Hz-throttled warn (`crates/postretro/src/scripting/systems/emitter_bridge.rs:208-209`, `:228-230`, `:420-429`); the audio bus returns `false` and logs (`crates/postretro/src/audio/buses.rs:157-169`). Honored: the instance pool follows the emitter's throttled shape, since a combat pool overflows in bursts.
- **Divergence, named:** the client gains third-party archetype data — another entity's `max` and `zoneMultipliers` — where `TuningPayload` today carries strictly the recipient's own pawn and wieldables (`crates/postretro/src/netcode/tuning_payload.rs:21-42`). This is deliberate. Prediction of a *number* is impossible without the mitigation inputs, and the alternative is a number that visibly pops one round-trip after every zone hit. The values are static per archetype and non-secret; nothing about an enemy's damage profile is hidden information in a co-op PvE game.

**Placement.** Three placements, each on a different axis.

*Engine, not mod, for the fact vocabulary.* Not because mods can't be trusted with presentation, but because the engine cannot predict or reconcile a payload whose meaning it does not know — it cannot match a predicted mod-defined event to its confirmed twin, or decide whether one is predictable at all. Wire versioning and rate bounds need a closed schema for the same reason.

*Mod, not engine, for every payload.* Damage numbers, health bars, aggregation, styling, and lifetimes are authored. The engine's cap is a bound, not a policy.

*Snapshot, not descriptor, for the instance stream.* Forced by the reuse gate above.

*CPU-side, not renderer, for projection.* World→screen projection happens app-side before `set_ui_snapshot`, where `view_projection` is already in scope, and the snapshot carries reference-space coordinates. `crates/ui/Cargo.toml:10-17` has no `glam`, and adding one to move projection into the UI crate would buy nothing — the app already holds the camera.

**Alternatives rejected.**

- *Hash entity health descriptors into the content-parity digest instead of replicating them.* Cheaper, and the hole is real: `crates/postretro/src/netcode/mod_digest.rs:478-493` pins that entity-descriptor lanes are ignored, with a `HealthDescriptor` edit in the fixture set, so two peers can differ on an enemy's `max` today and pass admission. Rejected because a digest can only refuse a peer, and the session-lifecycle doctrine reserves hashing for what cannot be replicated. These values can be replicated, so they are — and peers agree instead of disconnecting.
- *Deltas on the health snapshot rather than absolutes.* Rejected on two counts. A delta obliges the client to correlate the host's copy of its own predicted damage against its local prediction, or double-count it — which needs a per-hit acknowledged-prediction ledger the absolute form does not. And a dropped delta leaves the bar permanently wrong, where a dropped absolute is repaired by the next one.
- *Generalize `SlotValue` with a record or list-of-record variant and carry instances as state.* Tempting because slots already replicate and already reach the UI. Rejected: slots are levels with last-write-wins, so two hits inside one tick collapse to one and anything faster than the sample rate is lost; the variant would also widen the replication, validation, and SDK surface for every existing consumer. `SlotValue` stays `Number|Boolean|String|Enum|Array(Vec<f32>)` (`crates/entities/src/slot_table.rs:11-17`).
- *Continuously replicate enemy health so bars read live state.* Rejected: it inverts `remote_materialize.rs:458-470`, costs per-tick traffic for every enemy rather than per-event traffic for damaged ones, and a damaged-enemy bar does not want live truth anyway — it wants what you just did.
- *Spawn instances as entities, like particles.* The particle system spawns a full ECS entity per live sprite inside the game-logic tick. Rejected: that makes presentation into simulation state, which replicates, serializes, and diverges. Instances are frame-local presentation with no entity identity.

## Decisions

- **A displayed number is never corrected.** Once an instance is spawned its value is fixed for its lifetime; reconciliation retires or amends the *bar*, never a number in flight. A number that changes value mid-flight reads as a bug; a bar that jumps reads as a teammate's hit. This is what makes occasional prediction divergence acceptable rather than a defect.
- **Snapshots are absolute and atomic.** Each carries `(current, max)` together. A consumer never pairs `current` from one snapshot with `max` from another — with a mutable `max` that renders a fraction that never existed.
- **The client declares hits; the host computes damage.** Unchanged from `E16--client-authoritative-combat`: `LocalHitRecord` carries no amount (`crates/postretro/src/weapon/mod.rs:56-61`) and `ShotVerdict` returns two booleans (`crates/net/src/wire.rs:1086-1095`). A client that could assert damage amounts could assert any damage amount.
- **Predicted facts carry absent fields, not zeroed ones.** A client predicting a hit knows target, zone, and amount; it does not know `healthBefore`. Those fields are absent until the authoritative fact arrives, and a script binding an absent field renders nothing rather than a zero.
- **Instance keys are script-assigned, with spawn-or-update semantics.** Emit with key K: no live instance holds K, create one; one does, update its value and refresh its lifetime. A unique key per hit yields flying numbers; a target-entity key yields a running total or a bar. One primitive, no engine mode.
- **Updates to a live key never consume pool capacity.** Only a new key can hit the cap, so the aggregating configuration is bounded by target count and the per-hit configuration is the one throttled — the mode that generates soup is the mode that is capped.
- **Snapshot fan-out is per-target subscription.** A client receives snapshots for targets it currently holds a live instance on, established by that client's own impact facts. Not a broadcast of every enemy's health.
- **The impact fact reserves `crit` and always reports false.** Reserving costs a bool now; widening the schema later costs a `WIRE_VERSION` bump.
- **New netcode code lands in sibling modules, not in `netcode/mod.rs`.** That file is 4603 lines. Combat facts get their own module beside it; do not restructure the existing one.

## Acceptance criteria

- [ ] In single-player, shooting an enemy floats a damage number authored entirely in `content/dev/scripts/`, with no engine code naming "damage number."
- [ ] The retained tree is not rebuilt while instances spawn and expire — a counter over `UiTree::from_descriptor` stays flat across a burst of numbers.
- [ ] The same script, with only its key expression changed, renders a single accumulating total per enemy instead of one number per hit — no engine setting changes.
- [ ] A damaged enemy shows a health bar over its head; moving the camera keeps the bar on the enemy, and an enemy behind the camera or outside the frustum shows no bar rather than a clamped one at the screen edge.
- [ ] The same bar template with its world anchor omitted renders in the HUD at a fixed position.
- [ ] Instance lifetimes advance on `script_time`: with game logic frozen, live instances hold their value and position and do not expire.
- [ ] A burst exceeding the pool cap drops the newest instances, keeps the oldest alive, and logs at most one warning per second naming the cap.
- [ ] Updates to an already-live key do not consume pool capacity — a target-keyed template survives a burst that overflows a per-hit-keyed one.
- [ ] In co-op, a connected client sees its own damage number before the host could have answered — measurably at or under one frame after the local hit, not one round trip.
- [ ] A connected client's number for a zone hit matches the host's computed amount without a visible correction.
- [ ] A number already on screen never changes value, including when the host's authoritative fact differs from the prediction.
- [ ] When the host rejects a shot, the predicted number stops at its current lifetime rather than persisting to full duration; no new number appears for the rejected hit.
- [ ] Two clients damaging the same enemy each see the bar move for the other's hits.
- [ ] A heal applied through `setHealth` moves a live bar; so does a change to the target's `max`.
- [ ] A client never receives health snapshots for an enemy it has not damaged.
- [ ] A materialized remote enemy still attaches no `Health`, `Brain`, `Agent`, `Weapon`, or `PlayerMovement` component — the shipped assertion at `remote_materialize.rs:458-470` still passes unmodified.
- [ ] Host and client agree on an enemy's `max` and `zoneMultipliers` after admission, including for an enemy archetype whose descriptor the client had stale.
- [ ] A mod authoring an instance template with an unknown fact field fails at load with a diagnostic naming the field; other trees in the same manifest still load.
- [ ] `WIRE_VERSION` advances exactly once across the whole spec; the archetype-tuning lane rides the JSON payload epoch instead and bumps no bitcode constant.
- [ ] Two identical headless runs still produce byte-identical stdout — the instance pool contributes nothing to the observability dump.
- [ ] `crates/ui` still has no `glam` dependency.

## Tasks

### Task 1: Thin slice — one fact, one instance, single-player

Cut the narrowest path that crosses every seam, so the boundary assumptions fail now rather than after the fan-out. Single-player only, no wire, no prediction, no world anchor, no cap: emit one impact fact at the damage chokepoint (`crates/entities/src/components/health.rs:457`, the sole `push_impact_dispatch` site — declared `pub(crate)` at `crates/entities/src/registry.rs:979-981`), carry it into `UiReadSnapshot` as a third stream beside `slot_values` and `cell_values`, and render it as one HUD-space text instance from a script-declared template with a fixed lifetime. `UiReadSnapshot` is built in `App::build_ui_read_snapshot` (`crates/postretro/src/main.rs:4293-4331`, constructed at `:4324` via `UiReadSnapshot::with_trees`) and handed over by `Renderer::set_ui_snapshot` (`crates/renderer/src/render/renderer_splash.rs:71`); the renderer consumes it in `UiPass::layout_gameplay_tree` (`crates/renderer/src/render/ui/mod.rs:729-741`), which already receives `time_seconds` and threads it to the tween runtime. Add the instance stream as a new parameter on that path rather than smuggling it through `slot_values`. Prove the retained-descriptor invariant holds: the authored template is part of the descriptor and never mutates, so `needs_build` (`ui/mod.rs:752-757`) must stay false across frames while instances come and go — assert the tree is not rebuilt while a number is live. This task defines the instance record's shape (key, template name, numeric value, spawn time, lifetime, optional anchor), and every later task consumes it.

### Task 2: Health-snapshot emission from every mutation

Give the engine an absolute health fact emitted wherever health or `max` changes, not only where damage is applied. There are two funnels today and both are in `crates/entities/src/components/health.rs`: `apply_damage_with_context` (`:424-429`), which already pushes an `ImpactDispatch` carrying `health_before`, `health_after`, `max_health` (`:119-127`); and `set_health_absolute` (`:475-487`), which deliberately publishes nothing (`:471-473`) and is reached from `impact_effects.rs:83`, `impact_effects.rs:214`, `netcode/seat.rs:96`, and `crates/entities/src/components/grant.rs:43`. Emit a snapshot from both, carrying `(current, max)` together as one record. Do not reuse `ImpactDispatch` — it is a damage record with a `producer` and a `source`, and a heal has neither. Note that `health_after` on the impact dispatch is the raw pre-floor value (`:122-123`) while the stored `current` is floored at zero (`:436`-ish region); the snapshot reports the stored value, since a bar cannot render a negative fraction. `max` is mutable by contract even though no shipped path writes it outside `from_descriptor` (`:346-359`) and `refresh_from_descriptor` (`:366-374`) — emit it every time rather than caching it, so a future `max` setter inherits correctness without editing this code.

### Task 3: Combat-fact wire messages and per-target fan-out

Put both facts on the wire and route them per client. Add a server→client message carrying impact facts and health snapshots; it rides `Channel::Input` alongside the shipped `ServerMessage` (`crates/net/src/wire.rs:1108-1111`), sent with `NetServer::send_input` (`crates/net/src/transport.rs:685-687`) and drained client-side by `NetClient::drain_input` (`transport.rs:949-951`). Follow the append convention the roster work established — `handshake.rs:60-71` keeps a historical mirror of the pre-append variants and `handshake.rs:106` measures bitcode's enum-tag layout rather than assuming it; do the same for this append. Bump `WIRE_VERSION` from 16 (`crates/net/src/handshake.rs:17`), update the two literal drift assertions at `:85-89`, re-point the `PRE_DROP_PRESSED_WIRE_VERSION` pattern at `:81` to the now-previous version, and extend the change-log doc comments at `:8-16`. `SNAPSHOT_VERSION` (`crates/net/src/wire.rs:90`) is untouched — nothing here lands on the snapshot record. **Fan-out:** a client receives an impact fact for hits it caused and health snapshots for targets it holds a live subscription on, where a subscription is opened by that client's own impact fact on that target and expires on a host-side timer sized above the longest authored lifetime. Per-recipient divergence has direct precedent — owner-private slots filter by `if owner != client_id { continue; }` (`crates/net/src/state_replication.rs:332-335`) and the roster encodes separately per recipient to avoid leaking `your_seat` (`crates/net/src/wire/control.rs:277-282`). Cap subscriptions per client and drop the oldest, mirroring `MAX_PENDING_HIT_DECLARATIONS_PER_CLIENT = 64` (`crates/postretro/src/netcode/mod.rs:451`). Land this in a new module beside `netcode/mod.rs`, which is 4603 lines; do not extend it.

### Task 4: Client prediction and reconciliation

Make the client's own hits produce a local impact fact immediately, and reconcile it against the host's. The shipped ledger already carries what this needs: `ClientPredictedShots::predict` records a shot at `crates/postretro/src/weapon/mod.rs:105-127` with `status: Pending`, and `apply_verdict` (`:149-178`) resolves it on `ShotVerdict`, restoring cooldown only when the weapon's `cooldown_authority_generation` still matches the snapshot taken at predict time (`:158-169`) and removing the record either way (`:177`). Extend the record to carry the presentation facts the prediction spawned, so a rejection can retire them. Predicted facts carry `amount` — computable client-side from replicated weapon tuning times the zone multiplier Task 5 replicates — plus `target` and `zone`, and leave `healthBefore`/`healthAfter`/`max` absent. On acceptance the authoritative fact supersedes: a live bar snaps to the authoritative `(current, max)`, and a number already spawned keeps its predicted value untouched. On rejection the spawned instances are retired at their current lifetime rather than removed instantly, so a rejected shot decays rather than blinking out. Duplicate verdicts must stay no-ops, as they are today. Note the demultiplexing quirk this rides: shot verdicts are returned by the time-sync driver (`crates/postretro/src/netcode/mod.rs:1908-1949`, dispatch at `:1929-1941`) because both share the `Channel::Input` drain — follow that shape rather than adding a second drain.

### Task 5: Third-party archetype tuning

Get an enemy's `max` health and `zoneMultipliers` to clients so a predicted amount matches the host's. Clients already hold entity descriptors locally — `materialize_net_mesh_presentation` resolves `entity_class` against a client-local descriptor list (`crates/postretro/src/scripting/builtins/net_descriptor.rs:395`, `:409`) — but agreement is not guaranteed: `crates/postretro/src/netcode/mod_digest.rs:478-493` pins that entity-descriptor lanes are excluded from the mod digest, and its fixture set includes a `HealthDescriptor` edit (`:376-381`, listed `:404`). Extend the shipped tuning payload rather than inventing a channel: `TuningPayload` (`crates/postretro/src/netcode/tuning_payload.rs:37-42`) is JSON behind the opaque `ServerControlMessage::Tuning(Vec<u8>)` (`crates/net/src/wire/control.rs:243-245`), versioned by `TUNING_PAYLOAD_EPOCH` (`tuning_payload.rs:13`, currently 3) independently of `WIRE_VERSION`, and deduped per client by `host_send_tuning_if_changed` (`netcode/mod.rs:1465-1481`). Add a per-archetype lane carrying canonical name, `max`, and the `zoneMultipliers` map, and bump the epoch. Scope it to archetypes present in the installed level rather than every declared descriptor. Because the payload is JSON and net never decodes it, this costs no `WIRE_VERSION` bump of its own — Task 3's bump is the only one. The multiplier is applied strictly before the chokepoint (`crates/postretro/src/sim/weapon_stage/impact.rs:77-89`), so the client must apply it the same way to land on the same `amount`.

### Task 6: Keyed instance pool, cap, and world anchor

Build the pool the earlier tasks have been feeding. Instances live in one CPU-side collection keyed by the script-assigned key with spawn-or-update semantics: an emit for a live key updates its value and resets its spawn time; an emit for a new key allocates. Expire on `time_seconds` — `App::script_time`, advanced at `crates/postretro/src/main.rs:2942` under a freeze gate and carried as `UiReadSnapshot.time_seconds` (`crates/ui/src/output.rs:222-228`) — never `Instant`, so a frozen game freezes presentation. Cap live instances at a named `pub const` beside the pool; at cap, refuse the newest and warn at most once per second, following the emitter bridge's shape (`crates/postretro/src/scripting/systems/emitter_bridge.rs:200-212`, `:224-237`, throttle at `:420-429`) rather than the audio bus's unthrottled log (`crates/postretro/src/audio/buses.rs:157-169`), because combat overflows in bursts. Updates to a live key must not consume capacity. **World anchor:** an instance may carry a world position; project it app-side before `set_ui_snapshot`, where `view_projection` is already in scope, into reference-space coordinates (1280×720, `crates/ui/src/layout.rs:103`), and publish those. Reject `clip.w <= 0.0`, non-finite results, and NDC outside `[-1,1]×[-1,1]×[0,1]`, marking the instance not-visible rather than clamping — the existing dev-tools projection at `crates/postretro/src/agent_diagnostics.rs:93-102` is the correct math in the wrong output space; mirror its rejections. The UI side maps reference→device with the shipped `device_scale` and `canvas_origin` (`crates/ui/src/tree/draw.rs:330`), the same projection the focus export is required to share (`crates/ui/src/tree/ui_tree_focus.rs:31-32`). `crates/ui/Cargo.toml:10-17` has no `glam` and must not gain one — coordinates cross as `[f32; N]`.

### Task 7: SDK authoring surface

Give scripts a way to declare an instance template, bind it to fact fields, and choose a key. A template is an ordinary widget subtree declared in the tree descriptor, so it reuses `Text`, `Panel`, and `Bar` and inherits theming; what is new is the declaration that marks a subtree as a template plus the binding source that reads instance fields rather than slots. Follow the shipped binding shape: `BindSource` is `#[serde(untagged)]` with `{ "slot": "..." }` and `{ "local": "..." }` arms (`crates/scripting-core/src/ui/descriptor/values.rs:76-87`), and `bindState` in `sdk/lib/ui/state.ts:35-56` is a pure shape composer that throws when `slot` or `local` is passed in options. Add the instance arm to both, mirrored in the Luau twin and in the generated typedefs (`sdk/types/postretro.d.ts`, `.d.luau`), with the drift fixtures updated in the same pass — `sdk/lib/data_script.ts` and the generated typedefs are the files this spec shares with `drafts/E16--per-player-currency`, so land the SDK edits in one pass. An unknown fact field is a load-time diagnostic naming the field, reported where malformed widget descriptors already report, with the rest of the manifest still loading. Update `docs/scripting-reference.md`, where the impact fact vocabulary is documented as four numbers with no source-scoped facts (`:1423`).

### Task 8: Reference mod

Author both payloads in `content/dev/scripts/`, and nothing in the engine that names either. Declare a damage-number template keyed uniquely per hit and a health-bar template keyed by target entity with a world anchor, both bound to impact-fact and health-snapshot fields. Ship the aggregation toggle as a commented one-line key change in the damage-number template, so the flying-numbers/running-total choice is visible as script rather than described as a capability — the running-total form reads its accumulator from `EntityStateComponent` (`crates/entities/src/components/entity_state.rs:15-17`, `get` at `:21` reading 0.0 when unset, `set` at `:26`) written through the shipped `setState` effect. Add the second bar shape — the same template with its anchor omitted, rendering in the HUD — beside it. Extend the combat demo README walkthrough with the two-client case: both players damaging one enemy, each seeing their own numbers and a shared bar.

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice, falsifies the snapshot-vs-descriptor boundary and pins the instance record shape everything else consumes.
**Phase 2 (concurrent):** Task 2, Task 5, Task 6 — snapshot emission, archetype tuning, and the pool touch disjoint crates (`entities`, `netcode`+`sdk`, `ui`+`main`).
**Phase 3 (sequential):** Task 3 — the wire messages and fan-out consume Task 2's snapshot record and Task 6's subscription lifetime.
**Phase 4 (sequential):** Task 4 — prediction consumes Task 3's authoritative facts and Task 5's replicated multipliers.
**Phase 5 (sequential):** Task 7 — the authoring surface binds the field set every prior task settled.
**Phase 6 (sequential):** Task 8 — consumes all of it.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| The tree descriptor never changes while instances come and go | Task 1 (instances ride the snapshot) | `needs_build` is whole-descriptor `PartialEq` (`ui/mod.rs:752-757`) — any per-frame descriptor edit silently forces a full rebuild | AC 1, 2 |
| Presentation time is `script_time`, never wall clock | Task 1, Task 6 (expiry reads `time_seconds`) | Any `Instant`/`SystemTime` reach in the pool breaks the freeze contract and headless determinism | AC 6, 20 |
| A number's value is fixed at spawn | Task 4 (reconciliation amends bars only) | Shared with Task 3's authoritative fact, which is the tempting thing to write back into a live number | AC 11, 12 |
| No authoritative combat state attaches client-side | Task 3 (facts are messages, not components) | `remote_materialize.rs:458-470` asserts the negative — a materialize path that attaches `Health` to serve a bar breaks it | AC 16 |
| `(current, max)` is consumed atomically | Task 2 (one record), Task 4 (snap both together) | Splitting them across two fields or two messages lets a mutable `max` pair mismatched halves | AC 14 |
| Updates to a live key never consume pool capacity | Task 6 (spawn-or-update) | Any refactor that clears-then-reinserts on update turns the aggregating mode into the throttled one | AC 8 |
| A client receives snapshots only for targets it damaged | Task 3 (per-target subscription) | Broadcasting is the simpler implementation and the one a later change drifts toward | AC 15 |
| `crates/ui` stays wgpu-free and glam-free | Task 6 (projection app-side, `[f32; N]` across) | `crates/ui/Cargo.toml:10-17`; no CI enforces this — see Open questions | AC 21 |
| Exactly one `WIRE_VERSION` bump | Task 3 (messages), Task 5 (JSON payload epoch instead) | Task 5 would bump a second time if it added a bitcode message rather than extending the opaque tuning payload | AC 19 |

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| impact fact | combat-fact record | bitcode variant on `Channel::Input` | `"impact"` bind source | same |
| health snapshot | `(current, max)` record | bitcode variant on `Channel::Input` | `"health"` bind source | same |
| instance key | script-assigned string | not replicated — instances are frame-local | `key` | same |
| instance template | template-marked widget subtree | tree descriptor, existing widget vocabulary | `template` | same |
| world anchor | `[f32; 3]` world, `[f32; 2]` reference-space out | not replicated — projected app-side | `anchor` | same |
| archetype tuning | canonical name + `max` + zone map | JSON inside opaque `Tuning(Vec<u8>)` | n/a (engine-internal) | n/a |
| zone multipliers | `HashMap<String, f32>` | already declared as `zoneMultipliers` | `zoneMultipliers` | same |

## Script syntax examples

```ts
// Proposed design. The engine ships no damage-number concept — this file is it.

// Flying numbers: a unique key per hit means every impact spawns its own instance.
// Change the key to `impact.target` and the same template accumulates instead.
const damageNumber = defineInstanceTemplate("damage", {
  on: "impact",
  key: (impact) => impact.hitId,        // → impact.target for a running total
  lifetimeMs: 900,
  anchor: (impact) => impact.point,     // omit for a HUD-space number
  render: Text({
    content: "0",
    color: color.critical,
    fontSize: 28.0,
    bind: bindInstance("amount", { format: "{}" }),
  }),
});

// The damaged-enemy bar: keyed by target, so one bar per enemy, refreshed by
// every hit and snapped by every authoritative health snapshot.
const enemyBar = defineInstanceTemplate("enemyHealth", {
  on: "health",
  key: (health) => health.target,
  lifetimeMs: 2500,
  anchor: (health) => health.headPoint,  // omit → a fixed HUD bar instead
  render: Bar({
    bind: bindInstance("current", { tween: { durationMs: 120.0, easing: "easeOut" } }),
    max: bindInstance("max"),
    fill: color.ok,
    styleRanges: { max: 1.0, entries: [{ upTo: 0.3, color: color.critical }, { color: color.ok }] },
  }),
});
```

## Open questions

- **Subscription expiry versus authored lifetime.** The host times out a client's per-target snapshot subscription, but the authored lifetime lives in script and the host cannot read it. Sizing the timeout above the longest plausible lifetime is a constant the engine picks; a script authoring a longer one gets a bar that stops updating before it expires. Alternatives are a declared maximum enforced at load, or letting the client renew its own subscription — the second is a client-asserted lifetime and needs its own bound.
- **No CI enforces the wgpu and glam firewalls.** There is no `.github/` directory in the repo and no cargo-tree gate in-tree; `crates/xtask/src/crate_graph.rs:495-537` checks four workspace-layering invariants, none mentioning wgpu, and reads `cargo metadata --no-deps` so external crates are structurally invisible to it. The E19 specs stated these as manual grep gates in plan text. AC 21 is therefore a net-new check, not an inherited one — decide whether this spec builds it or states it as a convention.
- **`crates/postretro/src/sim/weapon_stage.rs` is 3829 lines** and Task 4 extends the client fire path near it. Its zone-multiplier references at `:3756-3759` and `:3807` are `#[cfg(test)]` fixtures, but a production zone-scaling site elsewhere in that file was not ruled out. Confirm before Task 4 lands, and split first if the extension is substantial.
- **Overkill is derivable but unexposed.** `ImpactDispatch.health_after` is the raw pre-floor value (`crates/entities/src/components/health.rs:122-123`), so an overkill number is expressible — but the health snapshot deliberately reports the floored stored value. Whether the impact fact should also carry the unfloored value, so a mod can render overkill, is an authoring question this spec defaults to no on.
