# Combat Presentation Substrate — Core, Damage Numbers, Enemy Status Bars

## Goal

A passive, capped presentation layer for combat-adjacent UI: renderer-local, world-anchored, pooled transient instances that reuse the shipped UI draw-list/theme/text machinery without the retained tree, modal stack, focus, hit-testing, or input dispatch. This spec builds the shared **core** (projection, pool, one-shot layout, per-instance facts, composition fold) and two surfaces on it: **floating damage numbers** (spawn archetype, authored as an impact effect) and **damaged-enemy status bars** (overlay archetype). Pickup prompts are a follow-on. The engine renders; the author decides what a hit *means* and what it *shows* — the combat "facts, not policy" line (`combat-events.md` §2) carried into presentation.

## Scope

### In scope

- **Core presentation layer.** A CPU-side pool of world-anchored transient instances (fixed cap, spawn, TTL, fade, evict-oldest); a CPU world→screen projection using the app camera; one-shot template layout (widget subtree → relative draw list); per-instance producer-stamped facts; a per-frame screen-space `UiDrawData` folded into the frame `UiComposition`.
- **Two archetypes, three verbs.** `definePresentationTemplate` (the reusable look), `present(template, {...})` (spawn from an impact policy), `defineOverlay({ over, template })` (fact-driven overlay). All under `postretro/ui`.
- **Damage numbers.** A `present` impact effect joining the existing presentation lane; single-player/host-local spawn, then co-op via a host-addressed unreliable presentation event routed to the damaging client.
- **Enemy status bars.** A `damagedEnemies` overlay source: host-side per-frame read of `HealthComponent` (health track) and author-declared per-entity `@state` (shield track); recently-damaged linger; tween; evict on death. Co-op via host-pushed facts to the damaging client.
- **Theme/widget reuse.** Templates consume the shipped `UiTheme` and the `Bar`/`Text`/`Image`/`VStack`/`HStack` widgets, `styleRanges`, `visibleWhen`, `Switch` (SDK sugar), and value `tween`.

### Out of scope

- **Pickup prompts.** Follow-on spec: needs a net-new client-side local overlap pass (the touch pass is host-only), plus `awarenessRadius` and net-new `label`/`icon` descriptor fields. Named, not built.
- **Aimed-at enemy bars in co-op.** The host does not cheaply know a client's aim, so co-op enemy bars are **recently-damaged-by-me only**. Host/single-player may show an aimed-at bar; co-op does not. (`include: "aimedAt"` on the source is host/SP-only here.)
- **Client-side prediction of the shooter's own damage numbers.** The lean first cut is host-addressed for all numbers. Spawning client-locally off `resolution.hits` (`weapon/mod.rs:58`, populated at `main.rs:6217`) is a deferred feel-refinement, named not built.
- **Coalescing stacked hits** into a running total. Evict-oldest is the first-cut pool policy; per-target coalesce is a deferred opt-in.
- **Continuous distance facts**, tiered templates, per-tier mini-templates. Those are the pickup-prompt surface's needs.
- **Full `HealthComponent`/`EntityStateComponent` replication.** We push presentation *facts*, we do not replicate combat *state*. The replicated component set stays the shipped four kinds.
- **Reconciling a cosmetic.** A transient expires before a correction would land; the replicated HP slot is the source of truth.

## Direction

**Problem.** Combat produces facts the player cannot see: a hit's damage, a damaged enemy's remaining health. The impact-effect set is closed and has no path to presentation (`E16--resource-grant-chokepoint` deferred damage numbers here explicitly), and the UI layer only draws a single retained, screen-anchored tree bound to global slots — it has no transient, world-anchored, pooled instance. The cause is a missing *layer*, not a missing widget: the draw-list machinery exists but only one producer (the retained modal stack) is wired to it.

**Prior commitments.**
- *The engine emits facts; the author authors meaning* (`combat-events.md` §2). `present()` extends this to presentation — the engine renders, the author decides a hit shows a number. Reuses the shipped presentation lane in `evaluate_dispatch` (`impact_policy.rs:274`), where `PlayAnimation` is today the only member.
- *Renderer owns GPU* (`index.md` §2). All wgpu stays in `render/ui/`. The presentation producer is CPU-side and hands the renderer a `UiDrawData`, exactly as the retained tree does via `UiComposition::from_layer_draws` (`render/ui/mod.rs:296`); `push_focus_ring` (`mod.rs:1109`) is the precedent for a non-tree renderer producer.
- *Replicate state, present events* (this session's co-op transport decision; consistent with `networking.md` — presentation is host-local today). Durable per-player state (HP/ammo/XP) rides the shipped owner-private slot path; transient presentation rides a new unreliable, addressed, fire-and-forget event on `Channel::Snapshot` (`transport.rs:55`), mirroring the owner-private `ServerMessage::ShotVerdicts` send (`netcode/mod.rs:1749`).
- *Shields are author state* (this session; roadmap **damage application order** / **shields**). The enemy shield track reads author-declared per-entity `@state.shield` via the shipped direct accessor `registry.get_component::<EntityStateComponent>(id).get(name)` (`scopes.rs:370` precedent) — no engine shield component, no unbuilt dependency.

**Divergence, named.** The sample assumed enemy bars read "already-replicated enemy health." They do not: `HealthComponent` (kind 10) and `EntityStateComponent` (kind 17) are absent from the replicated `ComponentPayload` set (`net/wire.rs:395`), which is the shipped four kinds. Deliberate divergence: rather than replicate combat state (heavy, and against the state/event line), the host *pushes* a damaged enemy's health/shield facts to the client that damaged it — the same addressed-event transport the damage numbers use. This scopes co-op enemy bars to recently-damaged-by-me, stated above.

**Alternatives rejected.**
- *Replicate `HealthComponent` to clients (a fifth `ComponentPayload`).* Gives every client every enemy's health continuously. Rejected: it replicates state to drive a cosmetic, floods the wire with data no client reads unless it is fighting that enemy, and crosses the state/event line the whole substrate rests on. The host-pushed-fact model sends only what a client is about to see.
- *Damage numbers as a replicated owner-private slot.* Reuses the per-player-currency path. Rejected: a slot is reliable, retained, and reconciled — the opposite of a transient the player reads for 900 ms and forgets. Unreliable fire-and-forget is correct and cheaper.
- *A standalone `defineDamageNumbers` binding instead of a `present` effect.* Rejected earlier in design: it re-subscribes the engine to the impact stream and re-derives the per-attacker routing the policy already carries, re-prescribing meaning the author owns.
- *A second retained tree for presentation.* Rejected: it drags in taffy dirty-tracking, modal stack, focus, and input — everything a passive layer must not have. The draw-list layer is reusable without any of it.

## Acceptance criteria

- [ ] A `present`-effect damage number appears at the hit location in single-player, rises, fades, and is gone by its `lifetimeMs`; its color follows the template's `styleRanges` by magnitude.
- [ ] The presentation pool never exceeds its configured cap: with more live instances than the cap, the oldest are evicted; no unbounded growth, no panic.
- [ ] A damage number spawned by a killing blow (the target despawns the same frame) still displays the correct value — read from the dispatch's captured `amount`/`health_after`, not post-despawn registry state.
- [ ] The presentation layer captures no input and takes no focus: with a damage number or enemy bar on screen, pointer and gamepad input reach game logic and the retained UI exactly as with none on screen.
- [ ] In co-op, a damage number for a hit dealt by client A appears on **A's** screen only — not the host's, not client B's. In single-player / on the host's own hit, it appears locally with no wire round-trip.
- [ ] A lost presentation event degrades silently: dropping the packet shows no number (or a stale bar value), never a stall, error, or reconnect. (Unreliable channel; verified by a drop-injection harness test.)
- [ ] An enemy status bar appears over an enemy once it is damaged, tracks its health fraction (tweened), lingers `lingerMs` after the last hit, and is removed when the enemy dies or the linger expires. A full-health enemy shows no bar when `hideAtFull` is set.
- [ ] The enemy bar's shield track renders from author-declared per-entity `@state` (the `shield:` accessor); an enemy with no shield state shows no shield track (`hasShield` false). Vanilla/elemental/custom shields render through the same bar by pointing the accessor at a different state.
- [ ] In co-op, an enemy bar appears on the client that damaged the enemy, shows that enemy's health/shield, and never leaks another enemy's or another client's values; a late joiner shows no stale bars.
- [ ] Template layout is correct for a multi-widget template: an enemy bar's `VStack[Bar, Bar]` stacks, and a two-`Text` row lays out horizontally, at the projected anchor — one-shot layout, no retained tree.
- [ ] World→screen projection culls correctly: an instance whose anchor is behind the camera or off-screen is not drawn (no wrap-around, no NaN).
- [ ] On a frame with zero fixed ticks, live instances still animate (TTL/fade/tween advance by frame time) and project to current camera; on a frame with multiple fixed ticks, a spawn from tick 1 and tick 2 both appear.
- [ ] Adding the presentation `ServerMessage` variant bumps `WIRE_VERSION`; a peer built before the bump is refused at the handshake (no partial decode).

## Tasks

### Task 1: World→screen projection

Add a shipped CPU world→screen projector for presentation anchors. Today billboards project GPU-side only, and the one CPU projector (`agent_overlay_world_to_screen`, `crates/postretro/src/agent_diagnostics.rs:93`) is `dev-tools`-gated and typed to `egui::Pos2`. Lift its logic into a non-gated, non-egui helper: given a world `Vec3`, the camera `view_projection: Mat4` (the app owns this as `RenderCamera.view_projection`, `crates/postretro/src/camera.rs:33`, via `Camera::view_projection()` `:156`), and the device viewport size, return `Option<Vec2>` device-pixel position — `None` when `clip.w <= 0` (behind camera), non-finite, or outside NDC bounds (the exact cull ladder at `agent_diagnostics.rs:103-117`), else `(ndc.x+1)*0.5*w`, `(1-ndc.y)*0.5*h`. This is a pure function with a unit test over known camera/point pairs (in-front maps to expected pixels; behind and off-screen return `None`). Place it where the CPU presentation producer (Task 2) can call it without a `dev-tools` gate; it takes the camera matrix as an argument, so it needs no renderer internals (the renderer's `last_view_proj` is `pub(super)` with no getter — do not reach for it; the app camera is the source). No projection lives in the renderer's UI region, whose uniform is viewport-only (`UiUniform`, `crates/ui/src/output.rs:118`).

### Task 2: Pooled presentation layer + composition fold

Build the CPU-side presentation pool and wire its per-frame output into the UI composition. The pool holds a fixed-capacity set of live instances, each carrying `{ world_anchor: Vec3, spawn_time, lifetime, template handle, per-instance facts, motion/fade params }`; it owns spawn (append, evict-oldest at cap — never grow past the configured cap), age advance by **frame** time (so instances animate on zero-tick frames), fade, and expiry removal. Each frame it: projects every live anchor (Task 1) to device pixels, skips culled instances, asks Task 3 for each instance's relative draw list translated to the projected anchor, and assembles one screen-space `UiDrawData` (reusing `UiInstance`/`UiText`/`push_quad`/`push_text`, `crates/ui/src/output.rs:15,129` + `tree/draw.rs:229`). The renderer folds this `UiDrawData` into the frame `UiComposition` alongside the gameplay trees — extend the fold at `renderer_render_frame.rs:883-941` to include the presentation draw data (the retained-stack path builds `layer_draws` and calls `UiComposition::from_layer_draws`; add the presentation `UiDrawData` to that slice or a sibling `encode`, after scene / before the screen-effects resolve at `:962`). The pool needs a spawn intake the impact runtime (Task 4) and the overlay system (Task 6) push into; model it on the registry dispatch queue (`push_impact_dispatch`/`take_impact_dispatches`, `health.rs:460`) — a queue the producer drains each frame — so producers never touch renderer state directly. **Placement (Q2):** the pool + producer live app-side (`crates/postretro`), which already owns the render call, the camera, and (Task 6) the registry, and coordinates `crates/ui` for draw types; this keeps wgpu in the renderer and combat/registry reads out of `crates/ui`. Until Task 3 lands, exercise the pool with a temporary hardcoded single-quad instance (a test seam Task 4 removes) so the projection→pool→fold→draw path is falsified before authoring exists. `render/ui/mod.rs` is 1244 lines — the fold addition is a few lines at the existing call site; do not restructure the pass.

### Task 3: One-shot template layout + per-instance facts

Lay out a presentation template's widget subtree once into a relative draw list, resolving per-instance facts, without the retained tree. A template is a widget subtree in the shipped vocabulary (`Text`, `Bar`, `Image`, `VStack`, `HStack` — the `Widget` enum, `crates/scripting-core/src/ui/descriptor/widgets.rs:28`). Reuse the layout + widget-lowering path that `layout_gameplay_tree` (`render/ui/mod.rs:728`) drives — taffy layout plus the `push_quad`/`push_image`/`push_text` lowering in `crates/ui/src/tree/draw.rs` — but over a transient subtree, with **no** retained `UiTree`, dirty-tracking, modal stack, or focus/hit-test export. The result is a template-local (origin-relative) `UiDrawData` the pool (Task 2) translates to the projected anchor. **Per-instance facts:** a template's binds reference facts by name (`fact.number("value")`, `fact.number("healthFraction")`, `fact.bool("hasShield")`), and the value is a scalar the *producer* stamped for that instance — not a global slot. Resolve these through the same binding-resolution seam that already handles per-instance `localState` cells (`resolve_bindings(slot_values, cell_values, …)`, `crates/ui/src/tree/bindings.rs:25`; `cell_values` is the per-instance source, the G2 `localState` precedent) — supply each instance's fact bundle as that per-instance source rather than extending the global `slot_values` map (`UiRenderOutput.slot_values`, `crates/ui/src/output.rs:214`, is flat and global). Re-resolve facts every frame; re-run one-shot layout only when a fact that changes a node's measured size changes (a bound `Text`'s content width) — a fixed-structure template (`VStack[Bar,Bar]`) never re-layouts, only re-resolves fill fractions and advances `tween` (`TweenState<T>`, `crates/ui/src/tree/style.rs`). Templates consume the shipped `UiTheme` (`UiTheme::color/font/spacing`, `crates/scripting-core/src/ui/theme.rs:22`) the same way the focus ring does outside the tree walk, and `UiImageRegistry` (`render/ui/mod.rs:72`) for any image node. `styleRanges` and `visibleWhen` resolve as they do in the retained path (`draw.rs:442,468`).

### Task 4: `definePresentationTemplate` + `present()` effect (single-player)

Ship the authoring surface and the spawn trigger, host-local. **Descriptor:** a `PresentationTemplate` descriptor in `scripting-core` (a root widget subtree plus presentation params — `lifetimeMs` int, `motion` rise/easing, `fade`, `spawnScatter`), with SDK `definePresentationTemplate` under `postretro/ui` (the template id comes from the `const` binding per the naming sugar, no `name:` string), wire round-trip, and Luau + typedef-golden parity. **Effect:** add a `present` member to the impact-effect pipeline joining the **presentation lane**. Add a `BoundEffect` variant (`impact_policy.rs:51`); a `bind_effect` arm (`:441`) requiring the `@impact.target` token (the anchor is the damaged entity) and binding the value/anchor IR operands via the existing `bind_read`; sort it into `self.presentation` in `evaluate_dispatch` (`:274`, the `PlayAnimation` side); a `plan_effect` arm (`:604`) evaluating the value expression to a concrete scalar at plan time (the killing-blow case reads the dispatch's captured `amount`/`health_after`, never post-apply state); and intercept it in `apply_planned` (`:291`) — where `self.ctx` and the `&ImpactDispatch` are in scope — pushing a spawn (template id, world anchor from the target's transform, the value fact, the presenter = `dispatch.source`) into Task 2's intake queue. Do **not** route it through `apply_effect` (`impact_effects.rs:72`), which has only `&mut EntityRegistry`; interception is the `SetOwnerSlot` precedent (`:340`). SDK: a `present` builder in `sdk/lib/data_script.ts` (union member in `ImpactEffectWire` `:239`, builder on the effect surface) reached from a policy `do:` list, plus Luau/typedef parity. In single-player / on the host, the presenter is the local player, so the spawn goes straight to the local pool (Task 5 adds remote addressing). Remove Task 2's hardcoded test seam. `impact_policy.rs` impl is ~700 lines (the file's 2336 is mostly tests); the additions are localized arms — extend in place.

### Task 5: Host→client presentation-event transport

Route a host-side spawn to the client that earned it, unreliably. Append a `Presentation` variant to `ServerMessage` (`crates/net/src/wire.rs:1112`) carrying a **payload enum** whose first arm is `Spawn { template_id, anchor, value, facts }` (the `OverlayFact` arm lands in Task 7 on the same variant — Boundary inventory) — **appended** last (bitcode is positional, no unknown-variant skip), which bumps `WIRE_VERSION` (`handshake.rs:18`) once for the whole substrate; an older peer is refused at the handshake. Send it on the **unreliable** `Channel::Snapshot` (`transport.rs:55`) rather than the reliable Input channel `ShotVerdicts` uses — mirror the `send_shot_verdict` addressing shape (`netcode/mod.rs:1749`, `send_input(client_id, …)`) but through a snapshot-channel send. In Task 4's `apply_planned` intercept, resolve the presenter `dispatch.source` to its owning client: if it is the host-as-client's own pawn, spawn to the local pool as today; if it is a remote client, address the presentation event to that `ClientId` and send nothing to anyone else. Client-side: ingest the `Presentation` message in the client message path and spawn into the client's local pool. Loss is silent (unreliable) — a dropped spawn simply never appears; no ack, no resend, no reconcile. Verify with the latency/loss harness (drop injection): the number appears under clean conditions, is silently absent under drop, and never stalls the client. `net/wire.rs` (2728) and `netcode/mod.rs` (4781) are large; the variant + one send arm + one ingest arm are localized additions, not restructures.

### Task 6: `defineOverlay` + `damagedEnemies` source + enemy status bar (host/single-player)

Build the overlay archetype and the enemy status bar, host-authoritative. **Authoring:** `defineOverlay({ over, template, maxVisible })` and the `damagedEnemies({ lingerMs, hideAtFull, shield? })` source descriptor under `postretro/ui`, with wire + Luau + typedef parity. The `shield` accessor is an IR expression over a per-entity scope (e.g. `e.state("shield").dividedBy(e.state("maxShield"))`) evaluated host-side per tracked enemy. **Host system:** a game-logic-stage system that each frame finds recently-damaged enemies (enemies with a `HealthComponent` damaged within `lingerMs`; iterate via `registry.iter_with_kind(ComponentKind::Health)` / `query_by_component_and_tag`, `registry.rs:888,857`), and for each stamps facts into a pooled **overlay** instance keyed by enemy `EntityId`: `healthFraction = current/max` (`HealthComponent`, `health.rs:314`), and when a `shield` accessor is given, `shieldFraction` from the evaluated `@state` expression (`registry.get_component::<EntityStateComponent>(id).get(name)`, the direct read at `scopes.rs:370`) plus `hasShield`. Overlay instances differ from spawn instances in lifetime: they are **keyed** (one per enemy, updated in place, not appended), linger on a per-instance timer reset by each hit, tween the health fraction (`TweenState`), and are evicted when the enemy dies (health `current <= 0` or despawn) or the linger expires — bounded by `maxVisible` (evict the least-recently-damaged past the cap). `hideAtFull` suppresses the instance while `current == max`. **Anchor** is a model-authored named point the template references by name (`worldAnchor.socket`): reuse the skeletal hit-zone the model already tags (`head`, glTF `extras`; `plans/done/M10--skeletal-hit-zones`), which is bone-parented and posed each frame — not a parallel socket concept — the same spatial-data-from-the-model, referenced-by-name pattern hit zones establish. When a model has no such named point, degrade to an AABB-top offset (mirroring `canonical_name`'s degrade-don't-refuse), never fail. Projected by Task 1. Grounding for this task (not a design fork): confirm the accessor that yields the posed head hit-zone's world position, else compute the entity AABB top. This reuses Task 2's pool (extended with a keyed/overlay instance kind) and Task 3's layout/facts. Host/single-player only; Task 7 adds co-op. Aimed-at inclusion (`include: "aimedAt"`) is host/SP-only and optional here.

### Task 7: Enemy status bar — co-op fact push

Deliver enemy bars to the damaging client in co-op. Carry the enemy facts as the `OverlayFact` arm of Task 5's single `ServerMessage::Presentation` payload enum (Boundary inventory) — no second variant, no second `WIRE_VERSION` bump. The payload is `{ enemy_id, healthFraction, shieldFraction, hasShield, alive }`. **Push on change, never per-frame:** the host sends a fact when a tracked enemy's health/shield changes (i.e. on a hit) or on death (`alive = false`), addressed over the unreliable `Channel::Snapshot` to the client that dealt the change — reusing Task 5's addressing. Per-frame push is foreclosed by design: an enemy's health changes only on a hit, so on-change is sufficient (the client tweens last-fact → new-fact with no visible gap between hits) and it keeps the overlay an *event* rather than replicated *state* — the same lean, present-events discipline Phase 2's replicable-set scoping exists to enforce. The `alive = false` death fact is always sent so a bar cannot orphan on a dropped mid-value. The client ingests overlay facts into keyed overlay instances in its local pool (keyed by the replicated enemy `EntityId`, which the client already has for rendering), lingering/tweening/evicting client-side exactly as the host does, driven by the arriving facts rather than a local registry read. Recently-damaged-**by-me** only: the host pushes an enemy's facts solely to clients that damaged it, so a client never sees a bar for an enemy it has not engaged, and never another client's set. A dropped fact leaves the last value (unreliable, loss-tolerant); the enemy's death fact (or linger expiry with no further facts) evicts the bar. A late joiner has no prior facts and shows no bar until it damages something.

## Sequencing

**Phase 1 (sequential) — thin slice, falsifies the boundary:** Task 1 → Task 2 → Task 3 → Task 4. The thinnest path crossing every core seam — projection, pool, fold, one-shot layout, per-instance facts, the `present` effect, and the SDK authoring surface — delivered as a single-player floating damage number end to end. Each task consumes the prior (Task 2 folds Task 1's projection; Task 3 replaces Task 2's stub draw; Task 4 drives Task 2's pool and removes its test seam).

**Phase 2 (concurrent) — fan-out on the proven core:** Task 5 (co-op spawn transport) and Task 6 (host/SP enemy-bar overlay). Independent: Task 5 is the netcode send/ingest for spawns; Task 6 is a host-side overlay system reading the registry. Both build only on Phase 1; they touch disjoint files (net/netcode vs. a game-logic overlay system + the pool's keyed-instance extension).

**Phase 3 (sequential) — co-op enemy bars:** Task 7 — consumes Task 5's addressed-send transport and Task 6's overlay facts.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| presentation template | `PresentationTemplate` descriptor (`scripting-core`) | template descriptor (widget subtree + params) | `definePresentationTemplate({...})` (id from binding) | same |
| present effect | new `BoundEffect`/`ImpactEffect` presentation-lane member; intercepted in `apply_planned` | `ImpactEffectWire` member `{ primitive: "present", target: "@impact.target", args }` | `present(template, { at, to, value })` | same |
| overlay binding | host overlay system + keyed pool instance | not itself wired | `defineOverlay({ over, template, maxVisible })` | same |
| overlay source | `damagedEnemies` host reader | source config (lingerMs, hideAtFull, shield IR) | `damagedEnemies({ lingerMs, hideAtFull, shield? })` | same |
| per-instance fact | producer-stamped scalar, resolved via the `cell_values` per-instance seam | not wired (host stamps) / pushed for co-op | `fact.number/text/bool("name")` | same |
| presentation event | `ServerMessage::Presentation { payload: Spawn \| OverlayFact }` (appended), `Channel::Snapshot`, addressed to one `ClientId` | bitcode variant + payload enum; **bumps `WIRE_VERSION`** once | — (not author-facing) | — |
| — spawn payload | `Spawn { template_id, anchor, value, facts }` | bitcode payload arm | — | — |
| — overlay-fact payload | `OverlayFact { enemy_id, healthFraction, shieldFraction, hasShield, alive }` | bitcode payload arm (no separate variant) | — | — |

**Decided:** one `ServerMessage::Presentation` variant carrying a payload enum (`Spawn | OverlayFact`), designed as the single extensible presentation channel future surfaces (pickup prompts, toasts) will ride — the wire mirrors the substrate's archetype structure rather than incidental message count, one wire bump, one client-side ingest entry point. Task 5 introduces the variant with the `Spawn` arm; Task 7 adds the `OverlayFact` arm with no further bump. The byte layout is pinned here, not inline in the tasks.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| The presentation layer is passive — never captures input, focus, or hit-test | Task 2 (producer emits draw data only), Task 3 (no focus/hit-test export from one-shot layout) | Any reuse of the retained tree's input/focus path would break it; the one-shot layout must not call `export_top_focus_rects` | AC 4 |
| The pool is bounded — live instances never exceed the cap | Task 2 (evict-oldest at cap), Task 6 (`maxVisible` for keyed instances) | A spawn/overlay path that appends without evicting; a keyed instance that leaks on missed death | AC 2, 7 |
| A presentation event is loss-tolerant — never blocks, acks, or reconciles | Task 5 (unreliable `Channel::Snapshot`, fire-and-forget), Task 7 (facts overwrite last value) | Moving it to a reliable channel, or adding an ack/resend, reintroduces a stall path | AC 6 |
| A number/bar reaches only the client that earned it | Task 5 (address spawn to `dispatch.source`'s client), Task 7 (push facts only to damagers) | A broadcast send, or resolving the wrong client, leaks one player's feedback to another | AC 5, 8 |
| Facts are host-authoritative — a client never fabricates combat values | Task 6 (host reads registry), Task 7 (client renders pushed facts only) | A client-side registry read (enemy health is not replicated) would read nothing or stale; client must render only pushed facts | AC 8 |
| A killing blow's number reads captured dispatch facts, not post-despawn state | Task 4 (plan-time value from `ImpactDispatch`) | The target despawns the same frame; re-reading the registry at apply/spawn time yields a dead entity | AC 3 |

## Ordering pins

| Scenario | Ordering | Expected outcome |
|---|---|---|
| Killing blow, target despawns same frame | `present` plans its value from the dispatch (`amount`/`health_after`, captured at damage) before the consequential lane's `despawn` marks removal | The number shows the correct value; anchor uses the target transform captured for the spawn, not a post-removal lookup |
| Pool at cap, new spawn arrives | Task 2 evict-oldest before append | Oldest live instance disappears; count stays at cap; no growth, no panic |
| Frame with zero fixed ticks | Producer advances TTL/fade/tween by frame time and re-projects | Live instances still animate and track the camera; nothing freezes |
| Frame with two fixed ticks, a spawn each | Both `present` intercepts push to the intake queue; the producer drains both that frame | Both numbers appear |
| Overlay enemy dies | Task 6 evicts on `current <= 0` / despawn; Task 7 pushes an alive=false fact | Bar removed on host and (via the pushed fact) on the client; no orphan bar |
| Presentation event dropped (unreliable) | No retransmit | Spawn: number never appears. Overlay: bar holds its last value; next fact corrects, or linger expiry evicts. Never a stall |
| Co-op late joiner | No prior presentation events buffered | No stale numbers or bars; first bar appears only after the joiner damages an enemy |
| Two clients damage the same enemy | Host pushes that enemy's facts to each damager independently | Each sees the bar; neither sees the other's damage numbers |

## Script syntax examples

```ts
// Damage numbers — a present() effect in an impact policy. The number is
// anchored at the target and routed to the shooter (impact.source).
const damageNumber = definePresentationTemplate({
  root: Text({
    content: "0",
    bind: fact.number("value", { format: "{}" }),
    styleRanges: { bind: fact.number("value"), max: 100.0, entries: [
      { upTo: 25.0, color: color.dmg.normal },
      { upTo: 80.0, color: color.dmg.crit },
      { color: color.dmg.overkill },
    ] },
  }),
  lifetimeMs: 900,
  motion: { rise: 0.6, easing: "easeOut" },
  fade: { startMs: 500 },
  spawnScatter: { radius: 0.15 },
});

export const damageNumbers = defineImpactEvent("damage-numbers", { tag: "enemy" }, (impact) => [
  { do: [ present(damageNumber, { at: impact.target, to: impact.source, value: impact.amount }) ] },
]);

// Enemy status bar — an overlay. Health is engine-read; the shield track reads
// author-declared per-entity @state, so vanilla/elemental/custom all render here.
const enemyBar = definePresentationTemplate({
  root: VStack({ gap: 2.0 }, [
    Bar({ bind: fact.number("healthFraction", { tween: { durationMs: 160, easing: "easeOut" } }),
          max: 1.0, fill: color.enemy.fill, background: color.enemy.background }),
    Bar({ bind: fact.number("shieldFraction"), max: 1.0, visibleWhen: fact.bool("hasShield"),
          fill: color.enemy.shield, background: color.enemy.background }),
  ]),
  worldAnchor: { socket: "head", offsetY: 0.35 },
});

export const enemyStatusBars = defineOverlay({
  over: damagedEnemies({
    lingerMs: 2500,
    hideAtFull: true,
    shield: (e) => e.state("shield").dividedBy(e.state("maxShield")),
  }),
  template: enemyBar,
  maxVisible: 8,
});
```

## Open questions

No open design forks. The three prior questions — wire shape, push cadence, enemy-bar anchor — are decided against the project's own commitments (lean / present-events-not-state, foundation-to-grow, spatial-data-from-the-model) and folded into the Boundary inventory (one extensible `Presentation` variant), Task 7 (on-change push), and Task 6 (model-named anchor, AABB-top fallback).

Two build-time grounding confirmations remain (implementation, not design): the accessor yielding the posed head hit-zone world position (Task 6), and reuse-vs-parallel of the `cell_values` per-instance binding seam for producer-stamped facts (Task 3). Both are "read the code and confirm," not forks.
