# E16--per-player-currency — review round findings

DELETE BEFORE PROMOTING THE SPEC, alongside `syntax-exploration.ts`.

Round run against `index.md` at commit `09dc29a`, after the `byPlayer` syntax
redesign landed. Three parallel Opus reviewers: broad, codebase-anchored,
temporal. All spec line numbers are `index.md:<line>` at `09dc29a`; all source
line numbers were read from the files in that session.

Prior rounds closed 3 Blockers and 7 Complicates. This round is larger because
the syntax redesign had never been reviewed and because the temporal lens went
deeper into source than previous rounds.

**Nothing in this round has been applied to the spec.** Too many mechanical
fixes are downstream of design decisions listed under "Open design calls".

---

## Deduped blocker index

| # | Blocker | Found by | Spec site |
|---|---|---|---|
| B1 | `addSlot` on a trigger never binds — `classify()` falls through to `Presentation`, and the binder refuses a sentinel target on a non-consequential primitive | temporal 2 | `index.md:113`, `:198` |
| B2 | A crossing-fired `addSlot` hits a `RefCell` already-borrowed panic | temporal 1 | `index.md:113` |
| B3 | Owner token resolves at plan time for reads, apply time for writes — one fire can address two different seats | temporal 8 | `index.md:113` vs `:117` |
| B4 | The pawn→seat mirror is many-to-one and goes stale on seat rebind — double credit | temporal 4 | `index.md:99`, `:101` |
| B5 | `finish_host_poll` has four call sites and `admit_or_reclaim` two; the spec threads the release clear through one of each | temporal 5, anchor 2 | `index.md:103` |
| B6 | Level-load leg is reachable only through the tag arm; no tag is named | broad 5, anchor 1 | `index.md:81`, `:113` |
| B7 | The reaction example passes a per-owner slot bare, which the spec's own bind rule rejects | broad 1 | `index.md:198` vs `:69`, `:83` |
| B8 | No task owns adding `byPlayer` to the SDK; the ref shape it must attach to is load-bearing elsewhere | broad 3, anchor 6 | `index.md:166`, `:107`, `:113`, `:117` |
| B9 | Task 1 says a bare read of a per-owner slot is legal; Task 4 says it fails at bind | broad 2 | `index.md:97` vs `:117` |
| B10 | An owner-addressed read whose token has no seat has no defined value | broad 4 | `index.md:117` |
| B11 | The seat-mirror storage mechanism is unpinned — component vs registry-side map | broad 6 | `index.md:99`, `:159` |

---

## Open design calls

These are the owner's, not the reviewers'. Each blocks a set of mechanical fixes.

1. **When does an owner token resolve?** Recommend: once, at plan time, with
   `ImpactEffect::AddOwnerSlot` carrying a resolved `Seat` rather than an
   `EntityId`. Settles B3, constrains B2's deferred-command shape, and blunts B4.
2. **Reaction targeting spelling.** `addSlot(ref.byPlayer(on.activators), delta)`
   or a positional target carved out of the `byPlayer` rule. Four sites must
   agree: Decisions `:69`, Boundary inventory `:163`, AC 6 `:83`, example `:198`.
3. **`onStateCrossing` on a `perOwner` slot.** Reject at install alongside
   `accumulate`/`persist`, or allow as local-seat-only at frame cadence.
   Recommend rejecting.
4. **`byPlayer` SDK attachment mechanics.** Non-enumerable, or attached only to
   `perOwner` refs. See anchor 6 for why this is not free.
5. **Non-replicated `perOwner` on a connected client's HUD.** Accepted footgun
   with a reference-mod comment, or a bind-time warning.
6. **`scripting.md` §5.** `:141` forecloses member calls on state refs and every
   helper it sanctions is a free function. Either move the Boundary inventory's
   "holds unchanged" claim to Open questions as a §5 update at promotion, or
   ground it in a §5 sentence that permits the shape.

---

## Broad reviewer

Opened `index.md` and four `context/lib/` docs. **Opened no source files** —
findings 3, 6, and 19 are caveated accordingly and are settled by the anchored
reviewer.

### Pass 1

**1.** `index.md:198`, Script syntax examples — `defineReaction((on: TriggerEventParams) => addSlot(on.activators, progression.xp, 100))`
- The example passes a per-owner slot (`progression.xp`) bare, with the owner in a separate positional argument. `index.md:69` states an access without `byPlayer` fails at bind; `index.md:83` repeats it for reactions. The canonical example is the case its own bind rule rejects. The redesign reached the impact arm (`:162`, `:188`) but not the reaction arm (`:163`, `:198`).
- Fix: carve the reaction primitive out of the bare-access rule explicitly, or respell as `addSlot(progression.xp.byPlayer(on.activators), 100)` and update the Boundary inventory reaction row. Pick one; make Decisions, Boundary inventory, AC 6, and the example agree.
- **Blocker**

**2.** `index.md:97`, Task 1 — "any read that doesn't address an owner … sees whatever the publisher last wrote there" vs `index.md:117`, Task 4 — "a bare read fails at bind the same way"
- Direct contradiction. `index.md:83` scopes the failure to "the offending policy or reaction", which resolves `bindState` but not a crossing predicate — crossings bind Bool IR over live store slots (`scripting.md:357-365`) and are neither.
- Fix: add a Decisions bullet enumerating which descriptor kinds the rejection covers (impact-policy IR, reaction IR) and which read the retained scalar unchecked (UI bind descriptors, crossing predicates, the UI snapshot). Extend AC 6 with a crossing leg.
- **Blocker**

**3.** `index.md:166`, Boundary inventory note; tasks at `:107`, `:113`, `:117`
- No task owns adding `byPlayer` to the authoring surface. `defineStore` returns the `state` reference tree SDK-side (`scripting.md:112`); Task 2 threads only `perOwner`, Task 3 exports the write builders, Task 4 adds the IR leaf. The method that makes every example compile is unassigned, in both runtimes and the generated typedefs.
- Fix: add to Task 2 or 3 — "Add `byPlayer(owner)` to the slot reference type `defineStore` returns, in `data_script.{ts,luau}` and the generated typedefs, returning an owner-addressed ref accepted by `slot(...)` and by the store-read leaf. Reject `byPlayer` on a global-slot ref."
- **Blocker**

**4.** `index.md:117`, Task 4 — "The read resolves per fire through the same registry-mirrored lookup Task 1 adds"
- Undefined behavior when the addressed entity has no seat (enemy, prop, pawn mid-disconnect, or the window between level unload and the next install's mirror write). The write path warns and skips (`:71`); the read path has no escape — the §11 evaluator is "pure, total, bounded" (`scripting.md:424`) and must produce an `f32`.
- Fix: Decisions bullet fixing the value (declared default or `0.0`) plus a warn policy, and an AC leg.
- **Blocker**

**5.** `index.md:81`, AC 4 — "The level-load leg credits pawns live at install"
- `levelLoad` is typed `Reaction<{}>` with no published inputs (`scripting.md:464`), so the only reachable targeting is a tag (`scripting.md:397`). No tag is named, nor whether player pawns carry one, nor who adds it. Task 3's pin-1 test cannot be written without guessing.
- Fix: name the tag in Task 3 and in Ordering pins row 1, or state that Task 5's reference authors it.
- **Blocker** — see anchor 1, which supplies the mechanism.

**6.** `index.md:99`, Task 1 — "add a registry-readable seat association for a pawn"
- Mechanism unpinned: component, registry side map, or per-entity state. It matters — a component enters `ComponentKind`, which `networking.md:48` pins numerically equal to the wire `u16` discriminant, called "a load-bearing contract across the crate boundary" at `:50` with drift-guard tests both sides; the component vocabulary is engine-closed. `index.md:159` says "not replicated" but never says what it is.
- Fix: state the mechanism in Task 1 and the Boundary inventory Rust cell, e.g. "a registry-side `EntityId → Seat` map, not a component — it takes no `ComponentKind` discriminant and never enters a wire mirror."
- **Blocker**

**7.** `index.md:113`, Task 3 — "The primitive is host-only at dispatch"
- No plumbing stated for how a reaction handler learns its role. Handlers are dispatched by name (`scripting.md:411`); role selection works by the absent endpoint being the branch (`networking.md:209`). Whether that is reachable from a handler's arguments is left to the implementer.
- Fix: name the source of the host predicate, or state the guard is applied at the dispatch site.
- **Complicates** — see anchor 4.

**8.** `index.md:117`, Task 4 — "resolving the resolver's existing `pawn: EntityId` through Task 1's registry mirror" and "resolve per-owner slots against the local seat there"
- Neither publish path states how it reaches what it needs. The resolver is given a `pawn: EntityId` but the spec never says it holds or gains a registry handle. The HUD publisher's "local seat" has no stated source — mirror off the local pawn, or the constant `Seat(0)` named at `:103`.
- Fix: state where each gets its access.
- **Complicates**

**9.** `index.md:65`, Decisions vs `index.md:84`, AC 7
- Unnamed dead case: on a connected client, a `perOwner` slot with no `network` scope is never written by snapshot-apply (nothing replicates it) and never by the host-only publisher, so it reads the declared default forever. An author binding `killStreak` (`:178`) gets a silently-wrong readout, and `:74`'s "Widgets … receive the local player's value" is false for exactly this slot.
- Fix: name the case in Decisions; warn at bind or state it as an accepted footgun.
- **Complicates** — see temporal 10, which supplies the mechanism.

**10.** `index.md:113`, Task 3 — "give it an optional target token at bind time"; `:162`
- The read leaf gets a full wire pin at `:117`; the write effect gets none — no field name, no serde attributes. Task 3's own "byte-for-byte" claim depends on a pin the spec never sets.
- Fix: pin the write side as Task 4 pins the read.
- **Complicates**

**11.** `index.md:117` vs `index.md:73`, Decisions — "No wire version change."
- The bullet reasons only about the replication wire and the two build constants (`networking.md:67`). `scripting.md:424` pins a separate contract it never addresses: "The IR envelope carries an exact-match version epoch validated at load," shared with the persist format and the deferred `setState` IR. Adding a leaf field and an `ImpactEffect` variant touches that vocabulary.
- Fix: extend the bullet with an explicit sentence on the IR epoch.
- **Complicates**

**12.** `index.md:113`, Task 3 — "a new `ImpactEffect::AddOwnerSlot { slot, delta }` variant"
- The variant carries no owner, yet the next clause says "resolve the addressed player's seat". The implementer must infer the target rides `PlannedEffect::Command`'s token field from `:12`.
- Fix: say it explicitly.
- **Complicates**

**13.** `index.md:22`, Scope; `index.md:117`, Task 4
- The exclusion of reaction scopes from owner-addressed reads is real but never justified. Non-obvious: `scripting.md:466`, `:482` show the trigger-event scope publishing `activators`, which looks like an owner. The reason — `activators` is a set — appears nowhere.
- Fix: Decisions bullet stating only the impact-policy scope resolves an owned input, and why.
- **Complicates**

**14.** `index.md:79`, AC 2 — "A per-owner value survives a level transition"
- No task names a test, though `:45` identifies it as the invariant two prior shapes of this spec violated. Tasks 1, 3, 4 each name their Ordering-pin assertions; AC 2 has no owner in either direction.
- Fix: add the test to Task 1.
- **Complicates**

**15.** `index.md:127`, Sequencing — "Phase 3 (concurrent): Task 3, Task 4 … touch disjoint files"
- Not disjoint. Task 4 (`:117`) changes the binding-scope trait; Task 3 (`:113`) binds the write's target token in the same impact-policy scope. `scripting.md:424` describes one pluggable scope abstraction shared across adopters.
- Fix: assign the trait change to one task (making Phase 3 sequential), or state exclusive file ownership.
- **Complicates**

**16.** `index.md:162-164`, Luau column; `index.md:107`, Task 2
- A full Luau column exists and `scripting.md:17` makes the two descriptor parsers behavioral twins, but no AC (`:78-91`) exercises the Luau surface at all.
- Fix: add a Luau leg to AC 5 or a new AC.
- **Complicates**

**17.** `index.md:160`, Boundary inventory Wire cell "slot declaration key" — names a category, not a key, unlike every other row (`:158-164`). Fix: `` `perOwner: bool`, `serde(default)` ``. **Nit**

**18.** `index.md:74` "The UI is untouched"; `index.md:24` "visible in one file"; `index.md:121` — Task 5 edits `hud.ts` and touches four files. Both true in narrow senses, both read as contradictions. Fix: "The UI **layer** is untouched"; drop "in one file". **Nit**

**19.** `index.md:121`, Task 5 — the store declaration names no file, unlike the other two targets. Store declarations arrive through the mod manifest, not `setupLevel` (`scripting.md:49`), so it is not derivable. Fix: name the dev mod's start-script path. **Nit**

**20.** `index.md:113`, `:121` — present tense ("generalizes", "adds") for a prerequisite listed as shipped at `:12`. Fix: past tense. **Nit**

### Pass 2 — scope-eliminating claims

**21.** `index.md:65`, Decisions — "Per-owner storage is host-only. A client … runs none of the host-only policies or reactions"
- Asserts as a property of the world what `:113` says requires a guard: "without a guard it would run — and … diverge — on every client as well as the host." An implementer reading Decisions and skipping Task 3's fourth sentence ships no guard.
- Fix: reword to name the guard, and add an AC leg that a client-side `levelLoad` fire is a no-op.
- **Complicates**

**22.** `index.md:113`, Task 3 — "keeps today's global `Write` lowering **byte-for-byte**"
- Nothing in the spec makes this true; no serde behavior is pinned for the added field (finding 10). An unconditionally-serialized `Option` writes `null`. AC 8 (`:85`) verifies behavior, not bytes. Contrast `:117`, where the identical claim names its mechanism.
- Fix: pin the mechanism or downgrade to "unchanged".
- **Complicates**

**23.** `index.md:166` — "`scripting.md` §5's rule … holds unchanged"
- Restates the conclusion. `scripting.md:141` reads "There is no `.get()`, `.set()` … Nouns select state. Helpers describe how a reference is used:" and the three helpers listed at `:143-145` (`bindState`, `stateEquals`, `updateState`) are all free functions. §5 as written forecloses member calls on state refs; `.byPlayer()` is one. Whether "selects, never acts" is the distinguishing axis is what the note assumes rather than establishes.
- Fix: move to Open questions as a §5 update at promotion, or ground it in a permitting §5 sentence.
- **Complicates**

**24.** `index.md:117`, Task 4 — "rejects an owner-addressed read at bind, **for free**"
- The mechanism is named one sentence earlier, so the elimination is warranted. What it also eliminates is a test: AC 6 (`:83`) covers bare-access and global-with-owner, but no AC covers an owner-addressed read inside a sourceless reaction — the exact case the `scripting.md:474` divergence exists for.
- Fix: add the AC leg.
- **Nit**

### Warranted, no action

- `index.md:117` "re-serializes byte-identical" — mechanism named in the same clause.
- `index.md:103` "by construction, since it is keyed by `Seat`" — keying argument stated; AC 12 (`:89`) verifies.
- `index.md:97` "no new crate edge opens" — `scripting.md:504`, `networking.md:163`.
- `index.md:103` "no path branches on player count" — `networking.md:209`.
- `index.md:101` "no bind-vs-dispatch ordering can reach it" — `networking.md:100`.
- `index.md:30` "`scripting.md` §11's same-entity seam is untouched" — `scripting.md:450` scopes that seam to per-entity state fields.
- `index.md:73` "No wire version change" for the replication wire and build constants — `networking.md:67`. (The IR-epoch gap is finding 11.)
- `index.md:109` "The global path stays untouched" — follows from `:107`; AC 8 verifies.
- `index.md:72` "per-fire frozen snapshot" — `scripting.md:45`; AC 10 verifies. (But see temporal 12 on per-dispatch vs per-tick.)
- `index.md:82` "rejects the whole declaration set atomically" — `scripting.md:127`.

---

## Codebase-anchor reviewer

**1.** `index.md:81`, AC 4 — level-load fan-out
- `fire_named_event_with_sequences` (`crates/scripting-core/src/reaction_dispatch.rs:141`) skips any `ReactionDescriptor::Primitive` whose `target` is set — the arm at `reaction_dispatch.rs:167` warns "named dispatch `{event_name}` has no trigger fire context for sentinel target; skipping primitive" (`:170`) and `continue`s. `dispatch_primitive` (`reaction_dispatch.rs:355`) resolves recipients *only* from `descriptor.tag` (`:361`) via `query_by_component_and_tag(ComponentKind::Transform, Some(tag))` (`:371`). The level-load leg is reachable exclusively through the tag arm, and "pawns live at install" are credited only if the player pawn carries that tag — a map/descriptor authoring fact, not an engine guarantee. Task 3's pin-1 test at `index.md:113` inherits the same constraint.
- Fix: state in AC 4 and Task 3 that the level-load leg uses the tag form, and say what tag the reference pawn carries (or that the pin-1 test authors one).
- **Blocker**

**2.** `index.md:103`, Task 1 — "keep the clear there rather than distributing it, so that later hook has one place to land"
- The "one place" warrant does not hold. Four production `netcode::finish_host_poll(server, seats)` call sites — `crates/postretro/src/main.rs:3960` (worldless-poll failure), `:4050` (host-poll drain), `:5457` and `:5465` (host-update success and error arms) — plus the test harness at `crates/postretro/src/netcode/seat.rs:800`. Two production `admit_or_reclaim` call sites: `main.rs:4026` and `:5283`. The clear lands in six places, or `finish_host_poll` (`seat.rs:644`) grows a store-handle parameter threaded through all four production callers plus the harness.
- Fix: name the single seam explicitly, or drop the "one place to land" warrant for `E16--per-player-persistence`.
- **Complicates** — temporal 5 raises this to Blocker with the leak sequence.

**3.** `index.md:85`, AC 8 — "the shipped dev-mod policies that use it are unchanged"
- No shipped dev-mod content uses `slot.add` or the `slot(...)` builder. A grep of `content/` for `slot(` and `slot.add` across `.ts` and `.luau` returns zero matches. The reward-shaped policy at `content/dev/scripts/combat-lifecycle.ts:81` (`ammoOnKill`) uses `impact.source.grantAmmo("shells.buck", 8)` at `:87`. The only exercisers are the Rust unit tests at `crates/postretro/src/impact_policy.rs:1382` and `:1394`.
- Fix: cite the untargeted `slot.add` bind/lowering tests in `impact_policy.rs`, or add untargeted `slot.add` usage in Task 5 and make the AC depend on that.
- **Complicates**

**4.** `index.md:113`, Task 3 — "The primitive is host-only at dispatch"
- Nothing shipped gives a reaction primitive its network role. The handler signature is `Fn(&mut EntityRegistry, &[EntityId], &serde_json::Value) -> Result<(), ReactionError>` (`crates/scripting-core/src/reaction_registry.rs:40`; boxed alias at `:22`), and the tagged variant at `:57` adds only `&str`. Registration happens once at session construction — `register_emitter_reaction_primitives` at `crates/postretro/src/session/mod.rs:689` and `register_grant_reactions` at `:691` — not parameterized by role. `ScriptCtx` (`crates/entities/src/ctx.rs:23`), the only thing a `move` closure can capture for slot-table access (`pub slot_table` at `:34`), carries no host/client flag. The role signal used elsewhere (`suppress_ai_enemies`, derived from `NetEndpoint`; cf. `session/mod.rs:425`) never reaches a primitive handler.
- Fix: name the mechanism — a role cell captured at registration alongside the `ScriptCtx` clone (the pattern documented for `worldSetGravity` on `ScriptCtx::gravity`), or a host-only registration site.
- **Complicates**

**5.** `index.md:117`, Task 4 — the HUD publisher
- The publisher is not a loop over declared slots. `tick_and_report_sampled_weapon` (`crates/postretro/src/scripting/systems/ui_proxy.rs:180`) and `publish_local_weapon_state` (`:229`) call `write_hud_slot` against a hardcoded list of engine slot names — `player.health` (`:191`), `player.maxHealth` (`:194`), `player.ammo` (`:207`), `player.ammoReserve` (`:208`), `player.reloadProgress` (`:219`), `player.reloadActive` (`:221`), `player.weapon.current/pending/switching` (`:232`–`:234`). There is no enumeration of mod slots to hang a per-owner projection on, and the "no-value skip" is a per-source `if let Some(...)` guard (`:189`), not a general mechanism.
- Fix: say Task 4 adds a new publisher pass that iterates the slot table's `perOwner` mod records and writes each one's local-seat value into `SlotRecord::value`, mirroring — not extending — the per-source skip behavior.
- **Complicates**

**6.** `index.md:188`, `:166`, `:162` — `byPlayer` on the ref
- `byPlayer` has to live on the store ref object, and that object's shape is load-bearing elsewhere. `defineStore` (`sdk/lib/data_script.ts:759`) builds each ref as `Object.freeze({ slot: \`${namespace}.${slot}\` })` at `:774`, and `ReadonlyStateRef<T>` (`sdk/lib/ui/widgets.ts:47`) is documented "Runtime shape is exactly `{ slot }`" at `:46`. The same ref objects are passed into widget binds and lowered by `js_to_json_inner`, whose object arm (`crates/scripting-core/src/conv.rs:184`) **enumerates every own property** via `obj.props::<String, JsValue>()` (`:186`) — an object-literal method is enumerable, so a `byPlayer` member would appear in lowered bind descriptors. Separately, `StoreStateRefForSlot` (`data_script.ts:141`) and `StateValueForSlot` (`:144`) project only to `ReadonlyStateRef`/`WritableStateRef`, so `progression.xp.byPlayer` does not typecheck without widening them, and no task says to. The consuming builder `slot(ref: WritableStateRef<number>)` is at `data_script.ts:369`.
- Fix: state that `defineStore` attaches `byPlayer` only to refs whose schema declares `perOwner`, that `StateValueForSlot` gains a per-owner branch, and that the added member must not leak into lowered widget binds.
- **Complicates**

**7.** `index.md:64`, Decisions — "Both call sites surface the seat they released"
- Stated as present fact; neither does. `release_expired_holds` (`crates/postretro/src/netcode/seat.rs:339`) returns `()` and discards its `expired: Vec<Seat>` after the loop at `:345`–`347`; `admit_or_reclaim` (`:191`) returns `Option<Seat>` carrying the *winning* seat (`:243`), never the stale duplicate released at `:231`. `release_seat` itself (`:350`) returns `()`. Only Task 1 (`index.md:103`) proposes to change this.
- Fix: rewrite as the change it is.
- **Nit**

**8.** `index.md:101`, Task 1 — the install hook
- (a) `self.host_spawn_points = products.spawn_points` is assigned at `crates/postretro/src/startup/lifecycle.rs:830`, *after* the `install_world_cpu(...)` call at `:816`, while the existing `capture_player_spawn_placements` call at `:845` reads it — so the hook must receive the installer's local `spawn_points` (bound at `crates/postretro/src/startup/lifecycle_world_cpu.rs:150`) plus a registry borrow. (b) `install_world_cpu` (`lifecycle_world_cpu.rs:70`, hook param at `:73`) has four call sites, each of which must pass a no-op: `lifecycle.rs:816`, `crates/postretro/src/observability/driver.rs:184` (non-test), and the harnesses at `lifecycle.rs:2104` and `:3659`.
- Fix: state both in Task 1.
- **Nit** — temporal 11 raises the same two points independently.

**9.** `index.md:117`, Task 4 — the IR leaf
- Feasible as written, but `IrNode` (`crates/foundation/src/ir/mod.rs:111`, `Input` variant at `:115`) carries a doc-comment table headed "**Wire format (pinned — Task 3 byte-matches this):**" at `:94`, whose row `| input | name | …` sits at `:102`. Adding a field without updating that row leaves a pinned-format doc contradicting the type.
- Fix: note that the table gains the optional owner field on the `input` row in the same change.
- **Nit**

### Load-bearing claims CONFIRMED in source

- **`release_seat` has exactly two call sites** — `fn release_seat` is private at `crates/postretro/src/netcode/seat.rs:350`, called only from `:231` (inside `admit_or_reclaim`, the `seat != winner` branch) and `:346` (inside `release_expired_holds`). Both *have* the seat; neither returns it (finding 7).
- **`SeatTable::seat_for_pawn` is unreachable from all three consumer layers** — `seat_for_pawn` is `pub(crate)` on the binary crate at `crates/postretro/src/netcode/seat.rs:298`, reachable only via `Session::seat_table` (`crates/postretro/src/session/mod.rs:196`). Against that: `apply_effect(registry: &mut EntityRegistry, target: EntityId, effect: &ImpactEffect)` at `crates/postretro/src/impact_effects.rs:53`; `bind_policy(descriptor, base_filter_tag, scope: &EntityScope)` at `crates/postretro/src/impact_policy.rs:330` and `bind_effect(entry, scope: &EntityScope)` at `:378`; reaction handlers `Fn(&mut EntityRegistry, &[EntityId], &serde_json::Value)` at `crates/scripting-core/src/reaction_registry.rs:40`.
- **`install_world_cpu` fires `levelLoad` internally; `bind_pawn(Seat(0), …)` runs after it returns** — `fire_named_event_with_sequences("levelLoad", …)` at `crates/postretro/src/startup/lifecycle_world_cpu.rs:410`, inside `install_world_cpu` (`:70`); the call returns at `crates/postretro/src/startup/lifecycle.rs:816` and `seats.bind_pawn(postretro_foundation::Seat(0), pawn)` runs at `:851`. The archetype sweep — `spawn_from_player_starts_with_carried_loadout` at `lifecycle_world_cpu.rs:297` — precedes the fire, so pawn and `spawn_points` (`:150`) exist at the proposed hook point.
- **`capture_player_spawn_placements` populates a placement `bind_pawn` reads** — `capture_player_spawn_placements` at `crates/postretro/src/main.rs:6797` calls `seats.bind_level_spawn_placement(pawn, placement)` at `:6812`, writing `level_spawn_placements` (`crates/postretro/src/netcode/seat.rs:417`, field at `:120`), which `bind_pawn` (`:406`) reads at `:409` to populate `placement_assignments`. This is the warrant for moving the pair, not the bind alone.
- **`apply_effect` cannot reach the slot table; `apply_planned`'s `Command` arm can intercept** — `fn apply_planned` at `crates/postretro/src/impact_policy.rs:266` is a method on `ImpactPolicyRuntime`, which holds `ctx: ScriptCtx` at `:25` (and thus `slot_table`, `crates/entities/src/ctx.rs:34`); its `PlannedEffect::Command` arm at `impact_policy.rs:294` is the sole caller of `apply_effect`, at `:302`.
- **The shipped `slot.add` rejects any target and lowers to a self-referential `Write`** — the `"slot.add" if target.is_none()` arm at `crates/postretro/src/impact_policy.rs:457` builds `{op:"add", a:{op:"input", name:slot}, b:delta}` and passes it to `bind_number_write(slot.to_string(), &value, scope)` (`:507`); the fall-through arm at `:470` returns `Err("slot.add must not carry a target")`. `plan_effect` (`:519`) lowers `BoundEffect::Write` to `PlannedEffect::Write { recipient: CommandRecipient::Target, … }` at `:521`. SDK side matches: `slot(ref: WritableStateRef<number>): NumberSlot` at `sdk/lib/data_script.ts:369` emits `{primitive:"slot.add", args:{slot, delta}}` with no target (`:370`ff).
- **The expected-token checker is already generalized** — `fn require_impact_token(target, primitive, expected_token)` at `crates/postretro/src/impact_policy.rs:595`, called with `IMPACT_TARGET_TOKEN` (e.g. `:395`) and `IMPACT_SOURCE_TOKEN` (`:422`, `:434`). `PlannedEffect::Command` carries `CommandRecipient` (enum at `:73`, variant use at `:294`).
- **`replication_scope_for` errors on `ownerPrivate` for mod stores with the quoted diagnostic** — `fn replication_scope_for` at `crates/scripting-core/src/store_bridge.rs:498`; the `Some("ownerPrivate") => Err(...)` arm at `:505`, message text beginning at `:507`, matching the spec's quotes at `index.md:13` and `:43` verbatim.
- **The owner-private resolver dispatches named projections ahead of a global fall-through and exposes `pawn: EntityId`** — `fn owner_private_source_value` at `crates/postretro/src/netcode/state_slots.rs:538` with `pawn: EntityId` at `:542`; it tries `descriptor_health_for_pawn` (`:636`), `descriptor_weapon_cooldown_for_pawn` (`:662`), `AmmoSlotProjection::slot_value` (`:619`), then falls through to `slot_table.get(name)` at `:555`. Dispatch into it is the `ReplicationScope::OwnerPrivatePlayer` arm at `:495`.
- **Declaration validation is atomic at mod-init; bind failures skip one descriptor** — atomic: `store_declaration_set_from_values` (`crates/scripting-core/src/store_bridge.rs:210`) propagates the first error with `?` at `:215` and `:220`; its caller `drain_store_declarations_js` (`:225`) is invoked at `crates/scripting-core/src/runtime/mod_init_exec.rs:326`, whose failure sets `out = Err(...)` for the whole manifest (Luau counterpart at `:565`). Per-descriptor: `ImpactPolicyRuntime::rebuild` logs "[Impact] policy `{}` was skipped during bind" at `crates/postretro/src/impact_policy.rs:190` and continues; `validate_sequence_primitives` (`crates/scripting-core/src/reaction_dispatch.rs:471`) filters out only invalid reactions.
- **The resolver's file is past size guidance** — `crates/postretro/src/netcode/state_slots.rs` is 2449 lines with `mod tests` beginning at `:1010` (its `#[cfg(test)]` at `:1009`), i.e. ~1000 non-test lines against "~600+ lines: split before adding more code" at `context/lib/development_guide.md:189`.
- **The shipped damage builder has an activators-or-tag dual** — `damage(target: ActivatorsTarget | string, amount: number)` at `sdk/lib/data_script.ts:505`, with identical `grantHealth` at `:514` and `grantAmmo` at `:526`. The shipped usage the spec cites at `index.md:195`–`196` is real: `grantAmmo(on.activators, "shells.buck", 24)` at `content/dev/scripts/combat-demo-reaction.ts:89`. The anonymous `defineReaction(tracer)` overload used at `index.md:198` exists at `data_script.ts:606` (name-carrying overloads at `:611`–`618`).
- **The store-read IR leaf can take an optional owner field** — `IrNode` is `#[serde(tag = "op", rename_all = "snake_case")]` (`crates/foundation/src/ir/mod.rs:110`–`111`) with struct variant `Input { name: String }` at `:115`; the bind seam `BindingScope::resolve_input` is at `crates/foundation/src/ir/scope.rs:77` on the trait declared at `:71`, and can take a defaulted owner-resolving method.
- **`defineStore` returns `Object.freeze({ declaration, state })`** — `defineStore` at `sdk/lib/data_script.ts:759` returns it at `:776`; `StoreDefinition` declares `declaration` + `state` at `:150`–`153`, so `index.md:173` typechecks. Accessor compatibility is qualified — finding 6.
- **Supporting facts** — `Seat` lives at `crates/foundation/src/seat.rs:6` and `entities` already depends on `foundation` (`use postretro_foundation::IrNode;` at `crates/entities/src/slot_table.rs:7`), so no new crate edge opens as `index.md:97` claims; `SlotRecord` (`crates/entities/src/slot_table.rs:81`) retains `pub value: Option<SlotValue>` at `:83`, the field `bindState` reads via `stateSlot(ref, "bindState")` at `sdk/lib/ui/state.ts:44`; a connected client holds no seat table — `seat_table = None` for `NetEndpoint::Client` at `crates/postretro/src/session/mod.rs:425`–`426`, field at `:196`; `Seat(0)` is never held or released — `debug_assert_ne!(seat, Seat(0), "the local seat is never held or released")` at `crates/postretro/src/netcode/seat.rs:351`; the wire tracker keys owner-private values by `(StateSlotId, u64)` at `crates/net/src/state_replication.rs:67`, ingests at `:145`, drops on client removal at `:111`; the shipped source-skip is silent — `dispatch.source.filter(|source| registry.exists(*source))` at `crates/postretro/src/impact_policy.rs:298`, contrasted at `index.md:71`; `content/dev/scripts/combat-lifecycle.ts`, `content/dev/scripts/hud.ts`, and `content/dev/maps/combat-demo.README.md` all exist as Task 5 (`index.md:121`) assumes; the three prerequisite spec folders named at `index.md:11`–`14` are present under `context/plans/done/` (`E15--seat-session-identity-roster`, `E16--resource-grant-chokepoint`, `E16--impact-policy-substrate`; the fourth prerequisite, Epic 15 Phase 3.5, is a phase rather than a spec folder).

---

## Temporal reviewer

**1.** `index.md:113`, Task 3 — "register a primitive crediting a named per-owner slot for every target"
- Reaction primitives receive `(&mut EntityRegistry, &[EntityId], args)` (`crates/postretro/src/grant/mod.rs:16-31`) — no slot table — so an `addSlot` handler must capture a `ScriptCtx` and take `slot_table.borrow_mut()`. Sequence: (1) a mod declares `onStateCrossing` on any slot; (2) `dispatch_state_crossings` calls `dispatch_state_crossings_with_sequences(&mut session.crossing_detector, &script_ctx.slot_table.borrow(), …)` (`main.rs:3898-3906`) — the `Ref` is a temporary alive for the whole call expression; (3) `fire_named_event_with_sequences` dispatches the crossing's reaction; (4) that reaction is `addSlot`; (5) the handler calls `slot_table.borrow_mut()` → already-borrowed panic. The shipped `setState` path avoids this by queueing `SystemReactionCommand::SetState` and writing in `dispatch_system_commands` (`main.rs:5024`), one drain later.
- Fix: state which shape `addSlot` takes. If inline, forbid it from crossing-dispatched reactions or restructure the borrow; if deferred, say the command carries **resolved `Seat`s plus the delta** (not `EntityId`s), because activator entities are gone by the drain, and pin that a same-frame crossing-fired `addSlot` lands in the second drain, after the HUD publisher — so the credit is visible to the UI only next frame.
- **Blocker**

**2.** `index.md:113`, `:198` — trigger-bound `addSlot`
- A trigger-bound reaction is partitioned at level install by `classify()` (`crates/postretro/src/trigger_bindings.rs:624-632`) against the hardcoded `CONSEQUENTIAL_PRIMITIVES` list (`:39-54`). Sequence: (1) the mod ships `onTriggerEvent({tag:"objective"}, "enter", [defineReaction(on => addSlot(on.activators, …))])`; (2) install runs `partition_direct_reaction`; (3) `classify("addSlot")` falls through to `PrimitiveClass::Presentation`; (4) `primitive.target.is_some()` is true, so the binder logs "sentinel target on non-consequential primitive `addSlot` cannot drain app-side; not binding" and **returns without binding** (`:496-501`); (5) the trigger fires forever and credits no one, with no runtime diagnostic. The classification is not cosmetic: consequential primitives execute in-tick via `BoundTriggerCommand` with a live `TriggerFireContext` activator set, while presentation-class descriptors drain app-side one drain later.
- Fix: Task 3 must name `CONSEQUENTIAL_PRIMITIVES` and the `bind_command` arm in `trigger_bindings.rs` (sibling of the shipped `"grantAmmo" =>` arm at `:781`) as required work, and state that `addSlot` runs in-tick on the trigger path. Note the two-registration-surface fact: the reaction-registry handler serves `levelLoad`/crossings, the trigger `bind_command` arm serves trigger and sequence steps.
- **Blocker**

**3.** `index.md:97`, Task 1 — "a crossing watcher"
- `CrossingDetector::detect(slot_table)` reads `record.value` and caches a per-watcher `previous` (`crates/scripting-core/src/state_crossings.rs:93,176-183`), running once per frame after the HUD publisher (`main.rs:2813` publisher → `:3884` crossings). Sequence: (1) a mod declares `xp` `perOwner` with an `onStateCrossing` at 100; (2) remote seat 1 is credited past 100 during tick k; (3) the publisher writes seat 0's projection into `value`; (4) `detect` compares seat 0's value against seat 0's previous — the crossing never fires for seat 1, ever, for any owner but the local one. Separately, with 3 sim ticks in one frame, a value that rises above and falls back below the threshold within those ticks crosses nothing.
- Fix: reject `onStateCrossing` on a `perOwner` slot at install with a diagnostic naming the slot, or state explicitly that crossings watch the local seat only at frame cadence, and add the AC.
- **Complicates**

**4.** `index.md:99`, `:101`, Task 1 — the mirror write set
- `SeatTable::bind_pawn` is `pawn_bindings.insert(seat, pawn)` — seat→pawn, one-to-one by overwrite (`crates/postretro/src/netcode/seat.rs:406-413`). The mirror is pawn→seat, many-to-one, and nothing in the enumerated write set removes the *previous* pawn's mirror entry on rebind. Sequence: (1) client C bound to seat 3, pawn P1; (2) C demoted without a lifecycle cleanup event (shipped comment at `:317-319`: "A drop while demoted or Loading can have no lifecycle event"), so P1 is still live; (3) C re-promotes, `host_handle_accept_descriptor_at_placement` mints P2 and `bind_pawn(seat 3, P2)` runs (`main.rs:5423`); (4) SeatTable maps 3→P2 and `seat_for_pawn(P1)` is `None`, but the mirror still says P1→3 and P2→3; (5) a tag-targeted `addSlot({tag:"player"}, xp, 100)` resolves both pawns to seat 3 and credits it **200**. Also `bind_pawn` silently no-ops when `carried` has no entry for the seat (`:407`), while the spec writes the mirror unconditionally "in the same call" (`index.md:101`).
- Fix: (a) rebinding a seat clears the outgoing pawn's mirror entry first; (b) the mirror write is conditional on `bind_pawn` actually binding; (c) state whether owner-addressed fan-out dedupes by resolved `Seat` — it must.
- **Blocker**

**5.** `index.md:103`, Task 1 — the release chain
- `finish_host_poll` (`seat.rs:644`) has four call sites: `main.rs:3960` (world-less poll `Failed` arm, commented "Hold expiry is session-clock work, not successful socket-I/O work"), `:4050` (world-less `Host` arm), `:5457` (with-world success), `:5465` (with-world `Err`, commented "A persistently failing socket must not freeze session-clock hold expiry"). Sequence: (1) host socket errors persistently while a seat's hold expires; (2) `finish_host_poll` runs from `:5465` and `release_expired_holds` drops the seat from `carried`/`connect_claims`; (3) that caller is not the one the spec threads the clear through, so per-owner entries stay in every store; (4) seat numbers never advance backward (`mint_fresh_seat`, `:268-288`), so those entries are unreachable for the session's life — exactly the leak `index.md:64` promises to prevent. `admit_or_reclaim` likewise has two production callers (`main.rs:4026`, `:5283`).
- Fix: enumerate all four `finish_host_poll` call sites and both `admit_or_reclaim` call sites, or collapse the clear into a single helper both polls call. Add the acceptance leg for the world-less/`Failed` path — expiry there happens with no level installed, so the test must prove the store handle is reachable off the boot state.
- **Blocker**

**6.** `index.md:150`, Ordering pins row 3
- The row pins the wrong event. Seat minting is not what gates the per-owner projection; the **mirror**, written at `bind_pawn`, is, and `HostStateReplication::ingest_from_sources` iterates `MovementOwners` (`crates/postretro/src/netcode/state_slots.rs:467-476,495-509`), not seats. Within `main.rs:5390-5424` the real order is: `host_handle_accept_descriptor_at_placement` registers the pawn in `owners` → `replication.register_client` → `state_slots.register_client` → **then** `seats.bind_pawn(seat, pawn)`. A promotion that closes out before reaching `bind_pawn` (the two `close_relay_connection` early-continues at `:5375`, `:5411`) leaves an `owners` entry with no mirror. Task 4's rule at `index.md:117` saves correctness, but the pin as written would be tested against seat minting and pass while the real hazard is untested.
- Fix: replace the Ordering column — "the mirror is written at `bind_pawn`, which runs after `owners`/`state_slots.register_client` in the same promotion block". Expected: that slot is skipped for that owner this frame and never falls through to the global value.
- **Complicates**

**7.** `index.md:71`, Decisions — "a player mid-disconnect writes nothing"
- `hold_disconnected_client` (`seat.rs:320-334`) removes `pawn_bindings` but has no registry handle and cannot clear a registry-side mirror; its own doc says the pawn may still be live. Sequence: (1) client C, seat 3, pawn P, disconnects during a Loading frame; (2) `hold_disconnected_client` starts the hold, drops the binding, P is not despawned; (3) the mirror still maps P→3; (4) a level-load or trigger `addSlot` targeting tag `player`, or a policy crediting `impact.source` where the source is P, resolves seat 3 and **credits a held seat** rather than taking the documented skip-with-warning; (5) if the hold expires, the credit is dropped at release.
- Fix: thread the pawn id out of `hold_disconnected_client` so the caller clears the mirror at that edge (and say so in Task 1's write-site list at `index.md:99`, which names `hold_disconnected_client` without noting it has no registry access), or change the Decision to say a held seat is a legal write target for the hold's duration and pin the outcome.
- **Complicates**

**8.** `index.md:113` vs `index.md:117` — owner resolution point
- In `evaluate_dispatch` (`impact_policy.rs:239-263`), **all** planning precedes **all** application; `apply_planned` then applies consequential effects in authored order, then presentation. A read's owner resolves during planning; a write's owner resolves during apply — two points with mutations between. Sequence: (1) one group contains `[ setHealth(0) on target, slot(xp.byPlayer(impact.target)).add(bonus) ]` where `bonus` reads `xp.byPlayer(impact.target)`; (2) planning resolves the read's owner (seat 3) and freezes `bonus`; (3) apply runs `setHealth`, whose command arm can retire the target; (4) the `AddOwnerSlot` arm re-resolves the mirror, now missing, and the credit is silently dropped — or, with finding 4's stale mirror, resolves to a different seat than the read used.
- Fix: state that a fire resolves each owner token **once**, at plan time, and that `PlannedEffect`/`ImpactEffect::AddOwnerSlot` carries the resolved `Seat` (not an `EntityId`) into apply. Add the AC: an owner-addressed read and write of the same token in one fire always address the same seat, even if the entity dies mid-apply.
- **Blocker**

**9.** `index.md:63`, `:89` — the deadline tie
- Both boundaries are inclusive: `admit_or_reclaim` accepts with `deadline.0 >= self.hold_clock` (`seat.rs:211`); `release_expired_holds` releases with `deadline.0 <= self.hold_clock` (`seat.rs:343`). At equality both are true, so the outcome depends purely on which runs first that frame. Sequence: (1) `advance_hold_clock` lands the clock exactly on seat 3's deadline (`main.rs:3926`); (2a) if the frame reaches the with-world host poll, admissions run at `:5283` before `finish_host_poll` at `:5457` — the reclaim wins and per-owner values survive; (2b) if the same frame takes the world-less `Failed` arm at `:3960`, `finish_host_poll` runs first, seat 3 is released, its per-owner storage cleared, and the identical reclaim mints a fresh seat at declared defaults. Same instant, opposite currency outcome; AC 12 does not say which is correct.
- Fix: add a Decision stating reclaim-on-the-deadline-frame wins, matching `finish_host_poll`'s own doc comment ("a connection arriving on its deadline frame keeps its held seat"), and pin the world-less path.
- **Complicates**

**10.** `index.md:117`, Task 4 — the client publisher
- `tick_for_role_and_report_sampled_weapon` early-returns for a connected client after `publish_local_weapon_state()` (`crates/postretro/src/scripting/systems/ui_proxy.rs:147-161`); only host/single-player reaches `tick_and_report_sampled_weapon`. Sequence: (1) a mod declares `killStreak: { perOwner: true }` with no `network` — the spec's own example at `:178`, and AC 7's case at `:84`; (2) a connected client instantiates it as a plain scalar; (3) the publisher's new per-owner leg never runs there (early return) and no snapshot carries it; (4) the client's HUD reads the declared default forever while the host credits its seat every kill.
- Fix: state it in Decisions and add the AC. Comment Task 5's reference mod so an author doesn't read `killStreak` in a co-op HUD and see zeros with no explanation.
- **Complicates**

**11.** `index.md:101`, Task 1 — the install hook
- (a) `capture_player_spawn_placements` reads `self.host_spawn_points`, assigned from `products.spawn_points` **after** `install_world_cpu` returns (`crates/postretro/src/startup/lifecycle.rs:816,835`); inside the installer the equivalent is the local `spawn_points` produced at `crates/postretro/src/startup/lifecycle_world_cpu.rs:150`. The closure cannot read `self.host_spawn_points`; the hook must pass spawn points in. The spec's "The pawn and the spawn-point products exist by the hook point" is true only of the installer's local. (b) `install_world_cpu` has four call sites — `lifecycle.rs:816`, `:2104`, `:3659`, `crates/postretro/src/observability/driver.rs:184` — three with no `SeatTable`. A level-load `addSlot` in the observability driver fires with no mirror and credits no one, silently — a different outcome from the windowed path for the same content.
- Fix: give the hook an explicit shape (receives `&[MapEntity]` spawn points and the registry, returns nothing) and state that the three non-windowed callers pass a no-op.
- **Complicates**

**12.** `index.md:87`, AC 10 vs `index.md:72`, Decisions
- The snapshot is per **dispatch**, not per tick: `evaluate_pending_in_registry` loops dispatches and calls `evaluate_dispatch` for each, re-seeding the scope (`impact_policy.rs:136-142, 211`). Two hits in one tick see *different* snapshots — the second reads the first's write. AC 10's prose reads as if the snapshot spans the tick, which would make a "double past 100 XP" gate behave differently from what ships. Both increments accrue (deltas, not absolutes), but hit 2's gate operand sees hit 1's credit.
- Fix: separate the two claims — within one fire a gate never sees its own write; across two fires in one tick the second gate does see the first's credit, and both deltas accrue.
- **Nit**

### Proposed pin table

Rows the spec should state and does not. Rows marked correct existing spec rows.

| Scenario | Ordering | Expected outcome |
|---|---|---|
| Crossing-fired `addSlot` | `dispatch_state_crossings` holds `script_ctx.slot_table.borrow()` for the whole call (`main.rs:3898-3906`); the crossing's reaction is `addSlot` | No `RefCell` panic. The credit lands in the second system-command drain of that frame, after the HUD publisher — the crediting frame's UI snapshot shows the pre-credit value, the next frame shows the credit |
| Trigger-bound `addSlot(on.activators, …)` at install | `partition_direct_reaction` classifies the primitive at level install, before any fire | `addSlot` classifies Consequential, binds with `BoundTarget::Activators`, executes in-tick with the live activator set. It must never take the presentation branch, which refuses a sentinel target and logs "cannot drain app-side; not binding" |
| **(corrects spec row 3, `index.md:150`)** First owner-private ingest for a promoting client | `host_handle_accept_descriptor_at_placement` registers the pawn in `MovementOwners` → `replication.register_client` → `state_slots.register_client` → **then** `seats.bind_pawn` writes the mirror (`main.rs:5390-5424`) | An `owners` entry with no mirror skips that slot's ingest for that owner this frame and never falls through to the global value; the next frame delivers the seat's real value. The pin is on pawn binding, not seat minting |
| Reclaim lands exactly on the deadline frame | `advance_hold_clock` puts `hold_clock == deadline.0`; both `admit_or_reclaim` (`>=`) and `release_expired_holds` (`<=`) match | The reclaim wins and the seat's per-owner values survive. Assert on the world-less-poll path too (`main.rs:3960`), where `finish_host_poll` can run before the next admission batch |
| Seat rebound to a new pawn while the old pawn is still live | `bind_pawn(seat, P2)` overwrites `pawn_bindings`; P1 was never despawned | The mirror entry for P1 is cleared in the same call. A tag-targeted `addSlot` over both pawns credits the seat exactly once |
| `bind_pawn` no-ops | The seat is absent from `carried`, so `bind_pawn` silently returns without binding (`seat.rs:407`) | No mirror entry is written. SeatTable and the mirror stay in lockstep for every N, including the no-op case |
| Hold expiry on the failing-socket poll path | Host update returns `Err`; `finish_host_poll` runs from `main.rs:5465`, releasing the expired seat | That seat's per-owner entries are cleared across every store on this path too. Same assertion for `main.rs:3960` and `:4050` |
| Owner token resolved twice in one fire | Plan phase resolves the read's owner; apply phase would re-resolve the write's owner after earlier effects applied | Both resolve to the same `Seat`, resolved once at plan time and carried into apply. A target retired by an earlier effect still receives its credit |
| Two impact fires in one tick, gate reads the slot it credits | `evaluate_pending_in_registry` re-seeds the scope per dispatch (`impact_policy.rs:136-142,211`) | Within a fire, the gate sees the pre-fire value. Across the two fires, the second gate does see the first's credit; both deltas accrue |
| N targeted adds to one seat in one fire | `apply_planned` reverses then pops, applying in authored order (`impact_policy.rs:277-278`) | For N = 0 nothing is written and nothing warns; for N ≥ 1 the seat's entry equals the sum of all N deltas, in authored order, no overwrite |
| Two pawns resolving to one seat in one fan-out | A tag- or activators-targeted `addSlot` whose resolved target list contains two entities mirroring the same seat | The seat is credited once per fire, not once per entity — fan-out dedupes by resolved `Seat` |
| Per-owner slot with `onStateCrossing` | `CrossingDetector::detect` reads `record.value` once per frame, after the publisher wrote the local seat's projection (`main.rs:2813` then `:3884`) | Either rejected at install with a diagnostic naming the slot, or: only the local seat's value is watched, at frame cadence, and a value that rises and falls within one frame's ticks crosses nothing |
| N per-owner credits in one frame, one snapshot | The tick loop applies N adds; `net_serialize_and_send` runs once, after the loop (`main.rs:2881`) | One owner-private baseline carrying the frame-final total. Intermediate per-tick values never reach the wire, and `upsert` allocates no baseline when the frame-final value is unchanged |
| Non-replicated per-owner slot on a connected client | The client's publisher early-returns before the per-owner leg (`ui_proxy.rs:147-161`); no snapshot carries the slot | The client's local record stays at the declared default for the session; only host and single-player read live values |
| Level unload with the mirror live | `harvest_bound_pawns` → `registry.clear_for_level_unload()` → `seats.clear_pawn_bindings_for_level_unload()` (`crates/postretro/src/startup/lifecycle_net.rs:170-190`) | The registry clear is the mirror's own unload path; no separate clear runs at `clear_pawn_bindings_for_level_unload`, and no stale mirror entry survives to be matched by a recycled entity index |
| Disconnect during Loading, pawn still live | `hold_disconnected_client` drops the SeatTable binding but has no registry handle and does not despawn the pawn (`seat.rs:315-334`) | An owner-addressed write resolving through that surviving pawn takes the documented skip-with-warning, not a credit to the held seat — or the spec states the opposite and pins it |
| Level-load award on a non-windowed installer | `install_world_cpu` called from `lifecycle.rs:2104`, `:3659`, `observability/driver.rs:184` with no `SeatTable` | The pre-`levelLoad` hook is a no-op there; the award credits no one and warns once, rather than panicking or resolving a bogus seat |

### Existing spec pin rows, verified

- Row 1 (`index.md:148`, level-load bind before fire) — **confirmed** against `lifecycle_world_cpu.rs:410` vs `lifecycle.rs:851`, subject to finding 11's two corrections.
- Row 2 (`index.md:149`, N=0 activators, no warning) — **not directly verifiable**, `addSlot` does not exist. Consistent with the shipped analogue `dispatch_ammo`, which logs at `debug` on an empty target set (`grant/mod.rs:80`).
- Row 3 (`index.md:150`) — **wrong**, see finding 6.
- Row 4 (`index.md:151`) — **confirmed**: `admit_or_reclaim` releases losing duplicates inline at `seat.rs:229-232`, before `finish_host_poll`'s `release_expired_holds` at `:645`, which every production caller invokes after its admission batch.
- Row 5 (`index.md:152`) — **confirmed**: `remove_client` purges owner-private entries at `crates/net/src/state_replication.rs:111-116` while `register_client` (`:101`) inserts an empty record, so the first post-reclaim `upsert` is newly-seen and emits a full baseline sourced from the store.
