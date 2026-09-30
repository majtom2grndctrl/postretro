# Implementation Roadmap

> **Lifecycle:** reviewed and updated at the start of each epic. Keep shipped work as compact status and outcome; at quarter close, move the file to `roadmap-q<N>-archive.md` and carry open items forward. Delete when all epics are complete.
> **Purpose:** the live roadmap from Q4 2026 onward: open work only. Shipped work and its design rationale are archived: Epics 1-9 in `context/plans/roadmap-q1-archive.md`, and Epics 10-23 as of Q3 close in `context/plans/roadmap-q3-archive.md`. Read an epic's archive section before drafting against it. Each epic produces something visible and testable.
> **Sequencing:** epics are parallel tracks in related domains, not a strict prerequisite chain. Specs within an epic ship in sequence. Epic numbers are stable across archives; a closed epic (13, 14, 19) appears only in its archive.
> **Nomenclature (Epic › Milestone › Spec):** a *spec* (a.k.a. plan) is the unit drafted with `/draft-plan` and run through `/build-spec` — one testable feature, a handful of tasks. A *milestone* groups a collection of specs within an epic into a visible, testable outcome. An *epic* is the top-level body of work — every roadmap unit is an epic. Milestone sub-structure is optional.
> **Related:** `context/lib/index.md`, `context/plans/roadmap-q3-archive.md`
> **Status markers:** `[x]` shipped and in the tree · `[ ]` not yet built · cut-after-build items keep `[x]`, strike the description, and append **✂ Cut (YYYY-MM):** with the reason — so a "done" item that no longer exists in the tree reads as such · a later-revived cut item appends **↩ Revived (YYYY-MM):** pointing to the active work.

---

## Epic 10: Animated Enemies

The skinned-mesh render path, combat loop, navigation, statechart behavior, line-of-sight, and faction sentiment have all shipped. What remains is follow-up.

- [ ] **Runtime-movable home anchor** *(additive; deferred to a named consumer)* — a write path to `BrainComponent.home_anchor` (a set-piece re-homes an enemy: rotate a guard post, relocate a patrol) plus the re-path to the new anchor, authored as an additive consequential verb in E18's enemy-command family (`updateEnemyState` / `enemies({ tag })` precedent). Built when a set-piece wants it, not before.
- [ ] **Movement-feel capstone** — integrated tuning pass across the landed feel specs on the movement-feel fixture, plus durable capture: steering pipeline order-of-operations, determinism invariants, and tuning vocabulary migrate into a `context/lib/` enemy-AI doc with an index-router entry. Closes the follow-up wave.

---

## Epic 11: Advanced Movement

The movement state machine, cross-cutting policies (momentum, input forgiveness), crouch, view feel, and slide have shipped. Design intent lives in `context/lib/movement.md`.

- [ ] **movement--wall-run** — first environment-probe state; consumes the momentum policy. Detail-on-open.
- [ ] **movement--vault** — environment-probe state; parallelizable with wall-run once the momentum policy is fixed. Detail-on-open.
- Grapple is explicitly deferred — constraint physics, renderer rope, and aiming make it its own future draft (the one place a scoped Rapier solver may earn a place; see `movement.md` §1).

**Testable outcome:** the player chains modern-FPS traversal — dash, crouch, slide, wall-run, vault — on top of grounded movement, all tuned through descriptor data.

---

## Epic 12: Sound Foundation

kira integration and positional sound events (step 1) have shipped: gameplay sounds come from their emitters, and `playSound` can be positioned.

- [ ] **0 — kira 0.12.4 bump** *(chore, not a spec)* — lockfile-only update from 0.12.0. No API breaks; brings cpal 0.18 macOS/Windows backend fixes, symphonia 0.6, and a streaming seek fix. Costs a duplicate glam 0.33 inside kira until the workspace moves. Own commit, then audio tests plus a listen check.
- [ ] **2 — Directional cues and stereo spread** — extends the one spatial pipeline step 1 built, in place, with no second pipeline and no renderer tiers.
  - **Front/back cue:** stereo panning stays, and a per-voice low-pass filter plus a slight level drop, tweened by how far behind the listener the source is, make behind sound duller than in front. The filter is kira's `FilterBuilder` on each voice's spatial track, the same hook occlusion reuses.
  - **Stereo spread:** a stereo-authored clip keeps its stereo width within a defined range of the listener, and collapses to a mono panned point beyond it. kira keeps stereo at spatialization strength 0, so tweening strength with distance gives the blend.
  - The range and the cue strength are mod-wide manifest settings beside attenuation, with seeded defaults. Whether a sound can override the range is this step's brief to decide.
  - Surround output is out: kira mixes stereo only. HRTF stays an `audio.md` non-goal.
- [ ] **3 — Reverb zones** — full stack; nothing exists yet. An `env_reverb_zone` FGD brush entity, a compiler bake to a per-runtime-cell zone index in a new PRL section (fog volumes, ids 30/31, are the template), loader validation against the `Cells` count, a per-frame listener cell lookup (`locate_cell`), and a shared kira reverb send whose parameters crossfade on zone change. The send must be coupled to the SFX volume explicitly, because kira sends tap before the parent fader. Smallest-zone-wins per `audio.md` §6. Independent of step 2; can run in parallel.
- [ ] **4 — Networked peer audio** — world-event sounds (doors, crushers, weapons, enemies) heard by remote co-op peers, not only by the peer whose simulation produced the event. Step 1's requests carry emitter and point, so this step appends a sound variant to the append-only `ServerPresentationPayload` and routes it through the existing `recipients_for_world_point` seam, excluding the owner who already heard their predicted fire. Door open/close/blocked can instead derive client-side from replicated `WireKinematicMoverState` phase. That struct has `direction`, `segment_index`, `completed` and `blocked` but no terminus field, so terminus is inferred. Crush has no replicated edge and needs a pulse. E17-E's AC11 (a client emits no mover sound) is lifted here.
- [ ] **5 — Baked BVH audio occlusion** *(later / additive)* — bake an occlusion/obstruction dataset by casting source→listener segments through the compile-time geometry BVH. At runtime, a sound whose path crosses solid geometry is attenuated and low-pass filtered (kira `FilterBuilder` on the emitter's spatial track) — true *obstruction*, beyond reverb zones' coloring by listener region. The coarse **potentially-audible-set** is a consumer of the shared cell-visibility relation (`context/research/cell-visibility-substrate.md`), the same substrate E15 Phase 4 network relevance wants; whichever epic builds it first does so consumer-agnostically. Speculative; sequenced after the core foundation.

**Testable outcome:** sounds behind the player are audibly distinct from sounds in front, and wide stereo sounds stay wide up close. Reverb zones audibly change acoustics. Co-op peers hear world events near them.

---


## Epic 15: Multiplayer Foundations and Host Flow

Transport, replication, prediction/reconciliation, state-slot replication, session lifecycle, and seat/identity/roster have shipped. Full design: `context/research/netcode/`.

**Open — network model under reconsideration.** The current lean is the **session-scoped co-op posture** (L4D2, Vermintide) over the persistent-world posture: a run ends when the host leaves, nothing persists server-side, and player hosting is the whole shipping model. Under that posture dedicated-server readiness stops being a north star, and CI plus agentic observability alone justify the headless seam. Settle this before Phase 4 opens, because it decides whether host symmetry is a requirement or an option. **Host migration** remains a long-term aspiration, modelled as a graceful **handoff** (the departing host packages authoritative session state; a successor loads it). Two nearly-free consequences to honor: whatever survives a level transition is an explicit, enumerable session-state set, and the lifecycle distinguishes a host-initiated leave from a crash or timeout. Full reasoning in the Q3 archive.

- [ ] **Phase 3.75 — Lobby authoring surface** — the session scope and its fixed append-only fact table, the join predicate, lifecycle reaction addresses (joined / left / phase changed) auto-fired like `levelLoad`, and a reference lobby in the dev mod. The engine owns identity, admission execution, map authority, and the relevel protocol; the mod owns lobby policy and presentation. Open: a roster is a list, slots are scalars, and the IR cannot iterate, so a variable-length player list is unreadable by every authoring surface that exists — likely a `ui.md` gap rather than a netcode one. Research: `context/research/coop-session-lobby.md`.
- [ ] **Phase 4 — Scale + host flow + dedicated-server readiness.** Validate 16 players within a host-upstream byte budget. If the budget needs interest management, consume the shipped view-independent cell-visibility relation in two stages: binary relevance first, then graded throttling only if measured pressure warrants it. Resolve the host-play execution model before this phase, then prove that hosted-session flow and the standalone-server entry point. Reuse the existing headless session substrate; do not build another headless path. Fold in the deferred interpolation refinements unless a feature playtest pulls them forward:
  - [ ] **Per-entity interpolation delay** — the starvation-driven delay is global today (one starved moving remote adds latency to every remote); make it per-entity. Tradeoff: per-entity delay introduces per-entity time offsets.
  - [ ] **Smoothed interpolation-delay transitions** — a delay *increase* currently pauses the remote render target (the monotonic no-rewind clamp) until the server estimate catches up, which can read as a brief freeze on a sustained-jitter link. Rate-limit the transition instead. Delicate: trades directly against the no-rewind guarantee.

**Testable outcome:** two-plus players use the selected hosted-session flow, with one joining mid-level and another dropping while the session survives, through a mod-authored lobby.

---

## Epic 16: Combat

Organized as five milestones along dependency seams. Shipped: the impact-policy substrate and impact-derived death, combat presentation, resource grants, ammo, the weapon state machine, switching/inventory, pickup, client-authoritative hits, pellet spread, projectiles, enemy ranged projectiles, AoE/splash, knockback, and spread/recoil. Design intent: `context/research/weapon-model.md` and `context/research/combat-events.md`. Combat networking folds into the specs that introduce each authoritative interaction.

### Combat Feedback & Economy

- [ ] **`onDamage` per-attack aggregate** — the per-attack reduction over impact facts (sums, counts, per-bucket) and the projectile-aggregation window; the impact-policy substrate ships per-*impact* facts, this adds the per-*attack* rollup.
- [ ] **DoT / environmental death policy** — un-stub the app-drain producer so impact policies (and thus authored death) fire for damage-over-time and environmental sources, not only in-tick weapon fire.

### Weapon Systems

- [x] **heat + cell resources** — the other two resource-union variants plus the per-tick resource update (heat dissipates, cells regen — independent of fire).
- [ ] **dual-wield** — generalize the single active reference to a primary/off-hand pair; resolves the activation-trigger fork (`weapon-model.md` §9).
- [ ] **augments / attachments** — the unified slotted-modifier system (internal augments and visible attachments are one mechanism); composes stat deltas + behavior hooks through the `effective()` seam. Visible attachments ride the Epic 21 socket system.
- [ ] **charge-on-activation** — charge level (0..1) scales listed stats at release; orthogonal to the resource, so it composes with any resource kind.
- [ ] **secondary activation** — alt-fire, filling the `secondary` block seam; includes a primary-use that spawns a persistent tracked entity for a secondary to resolve (the detonator pattern).

### Resolution Modes

**Non-goal:** server rewind / favor-the-shooter lag compensation — the engine predicts and reconciles, it does not rewind (`context/lib/index.md` §4).

- [ ] **melee** — a short-range shape-sweep query (a non-ray query family shared with AoE), exposed both as a resolution mode and as a universal quick-melee activation, with a lunge (a combat↔movement impulse).
- [ ] **Combat / projectile VFX relevance culling** *(later / additive)* — wire the shared cell-visibility relation into the `recipients_for_world_point(P)` seam (broadcast today) so the host stops sending cosmetic combat VFX to clients whose Cell cannot perceive the event's Cell: `locate_cell(P)` + `perceivable` per client cell, plus host-side per-client cell tracking, with no wire change. Shares the relation with E15 Phase 4 and the audio potentially-audible-set. Presentation-only — never culls authoritative simulation. Sequenced on measured co-op VFX-bandwidth need.
- [ ] **executions / glory kills** — an animation-locked finisher; reuses the death-sweep deferred-despawn seam.

### Damage & Defenses

- [ ] **damage types / elemental** — a damage type grows inside the payload; unlocks the combat-events `element` / `damageOf(element)` facts.
- [ ] **crit math** — a crit multiplier and flag, applied per hit zone.
- [ ] **status effects / DoT** — a new engine-owned per-tick component (burn / shock / corrode) with duration and stacking; DoT ticks are server-authoritative and emit through the combat-events chokepoint.
- [ ] **damage application order** — the `applyDamage` chokepoint drains author-nominated absorption layers before health, in an authored order. Today the chokepoint reduces health then fires the impact policy, so a shield can only refund health post-hit, never absorb before it. The engine owns only the drain order; each layer's value, max, recharge, and type stay author state (per-entity `@state`) + IR policy. Foundation for shields.
- [ ] **shields + shield types** — shields as author-declared state layered over health through damage application order: recharge authored as IR *policy* (fast like Halo, slow-and-delayed like Borderlands), elemental shield types, resistance interactions; unlocks the combat-events `brokeShield` fact.

### Weapon Feel

- [ ] **ADS** — an aim-down-sights view/FOV transition plus an accuracy modifier (the `adsSpeed` stat already exists).
- [ ] **scope rendering** — zoom / picture-in-picture optic rendering (renderer work).
- [ ] **aim assist** — autoaim / aim-assist on the input/aim layer (controller support).

**Testable outcome:** carry heat and cell weapons beside ammo weapons, slot augments that change effective stats, fire an alt-fire and a remote-detonated charge, melee with a lunge, deal typed damage that crits on headshots and ticks over time, absorb hits on recharging shields, and aim down sights and through scopes — all server-authoritative.

---

## Epic 17: Kinematic Geometry and Moving Platforms

The platform foundation, visual parity, trigger/command surface, rotation, and doors/blocking movers have shipped. The substrate is deliberately deterministic kinematic geometry — not a general physics engine, dynamic BSP, or author-scripted per-tick motion.

- [ ] **F - Visibility-bearing moving world.** Doors-as-occluders, dynamic portals, sector-graph updates, and any chunk-primitive consolidation. Extend the shared cell-visibility relation for moving geometry; do not invent a parallel dynamic-visibility path. Moving lights that cast shadows use the dynamic shadow tier; reuse the existing depth-cache substrate when a real moving shadow caster needs it, invalidating from the mover's authoritative at-rest signal. Start only for a concrete set-piece (E18-E's sealed closets) or measured performance need. The non-visibility-bearing kinematic-cluster slice lives in Epic 22.
- [ ] **G - Destruction bridge.** Pre-fracture, promotion to dynamic debris, and rigid-body debris stay outside the deterministic kinematic substrate. Revisit only after mover payloads and the speculative destruction work both have real consumers.

---

## Epic 18: Co-op Set-Pieces

Trigger fan-out, activation policy, dispatch parameters, enemy groups, spawning, trap pools, and timed reactions have shipped. The load-bearing E18-A decisions — effect-based dispatch split, bind-at-install, the co-op atmosphere channel — are recorded in the Q3 archive; later specs inherit them. Design intent: `context/research/co-op-triggers-trap-pools.md`.

- [ ] **R — Respawn + player-leave policy.** The co-op respawn and player-leave handling the capstone encounter needs — charter-named, unspecified elsewhere.
- [ ] **E — Playable encounter.** One authored reveal-closet set-piece that proves the loop is fun — the capstone consuming A–R. Its sealed closets motivate E17-F (doors as occluders); E18-E is that spec's consumer, not the reverse.

---

## Epic 20: Observability & Agent Tooling

The headless batch runner, frame capture, and the read-only live socket channel have shipped. **MCP is a frontend adapter, not a capability** — it earns its place over the socket's *live* verbs, not as a wrapper around a CLI an agent can already run.

- [ ] **Renderer target inspection** *(follow-up to frame capture)* — read back named renderer-owned intermediate targets at controlled frame times, starting with animated-lightmap composition. A capture pair or difference must show whether an animated bake changes before the forward pass samples it. Sequence before Scripted-run capture so its richer scenes inherit the diagnostic.
- [ ] **Scripted-run capture** *(follow-up to frame capture)* — run N scripted ticks like the batch runner, then capture from the resulting player-camera pose, adding entities (skinned meshes, particles, dynamic lights) and full fog volumes. Pre-draft research: `plans/drafts/E20--scripted-run-capture/research.md`.
- [ ] **Live mutating verbs + sim-tick clock** — the live channel's mutating verbs, gated on the host-authoritative sim and needing care, not a bespoke path. A monotonic sim-tick clock as its own live-only field (never overloading the batch `ticks_run`); no session-wide tick counter exists today.
- [ ] **MCP frontend** — expose the live channel's verbs as MCP tools. A batch fallback rides the same frontend for shell-less hosts but does not lead. The MCP tool schema doubles as the typed contract the protocol crate defers.
- [ ] **Streaming telemetry + structured log sink** — a continuous event/metric stream and a log ring buffer, versus the one-shot dump. Logging is `env_logger` stderr text today.
- [ ] **Protocol-crate extraction** (`postretro-observability-protocol`) — typed runspec/output. Gated on a second typed consumer. A refactor, not a feature.
- [ ] **Record / replay** *(candidate — greenlight on need)* — record a command/input trace from a live or batch run, replay it deterministically on the shipped determinism foundation.

**Non-goals:** MCP-over-batch as the first build; a general RPC or plugin framework; human-facing dashboards or a debug TUI; multiplayer session replay; external state writes that fight sim authority.

**Testable outcome:** an agent captures a frame from a scripted run and drives a running session over a socket, through the vocabulary the batch runner shipped, with the batch path still byte-identical.

---

## Epic 21: Player & Weapon Models

The pose-modifier stack, foot IK + locomotion descriptor, bone sockets, and co-op avatar + weapon presentation (third-person weapons and the first-person viewmodel) have shipped.

- [ ] **Co-op enemy aim presentation** — on co-op clients a remote enemy's aim-bend never renders: `PoseInputs` derive host-side from `BrainComponent.acquired_target`, but `MeshComponent.pose_inputs` is `#[serde(skip)]` and `Brain`/`Agent` never attach client-side (the presentation-only remote-enemy invariant). Extend the replicated `WireMeshAnimationState` with host-computed scalar aim (pitch/yaw) — never `BrainComponent`. A scoped `WIRE_VERSION` bump; coalesce with any adjacent bump. Draft: `context/plans/drafts/E21--coop-enemy-aim-presentation/`.
- [ ] **Multi-legged & melee-capable legs** *(later; extends foot IK)* — author and tune monsters with more than two IK legs, and wire front legs as melee attackers. The foot-IK data path is already N-leg-sized, so this adds content, per-monster leg tuning, and the leg↔melee-activation bridge (couples to Epic 16 melee). Draft when a multi-legged enemy archetype is on deck.

**Non-goals:** cloth/hair sim, facial animation, full-body IK beyond legs, per-player cosmetics/skins, and any change to hit-zone authority.

---

## Epic 22: Kinematic Assemblies & Instanced Groups

The canonical assembly with carried members and TrenchBroom group recognition have shipped. Epic hub: `context/plans/drafts/E22--kinematic-assemblies/`.

- [ ] **`E22--kinematic-instancing`** — recognize shared-template linked-group instances, dedupe to one template + per-instance transforms, add the net-new instanced draw path (kinematic tier only). Forces the Chunk-Primitive converge-or-defer decision. Efficiency win is anticipated, not yet observed — most deferrable leg.
- [ ] **`E22--runtime-addressable-assemblies`** — an assembly addressable by scripts/reactions (toggle, target, batch). Gated on a concrete gameplay consumer; may land in Epic 18.

**Testable outcome:** a linked-group prefab stamped N times compiles to one shared template drawn at N transforms; no `func_group`/`_tb_*` vocabulary reaches runtime.

---

## Epic 23: Accessibility

Meet the Game Accessibility Guidelines basic tier and the common FPS accessibility failures as engine mechanisms. Mods author where content must (theme variants, captions), OS accessibility settings seed defaults, menus reach native screen readers through AccessKit, and gamepad menu navigation meets console conventions. Preference resolution is player-set > OS value > engine default; the flash limiter is engine-owned and mods cannot bypass it.

**Prerequisite:** Epic 13 G2 (accessibility metadata) ✓. U5's caption and cue part waits on Epic 12 step 1 (Positional sound events) ✓.

Epic hub + research: `context/plans/ready/E23--accessibility/`. Five units, each with its own brief drafted when the unit comes up:

- [x] **U1 — Preferences and comfort floor** — accessibility preference substrate with OS seeding and tolerant per-field storage; flash limiter on `screen.flash` and `screen.vignette` (on by default); reduce motion; per-bus volume and mono audio. No dependency; concurrent with U3. Brief: `context/plans/done/E23--preferences-comfort-floor/` (landed with gaps; frame limiter withdrawn).
- [ ] **U1 follow-up — Photosensitivity source floor** — the flash rules at every other primitive content can strobe with (light animation, UI, emissives, flipbooks, camera cuts, load loops), plus reduced flashing for engine effects. Seed: `context/plans/drafts/E23--photosensitivity-source-floor/`.
- [ ] **U2 — Visual accessibility** — theme variants (engine high-contrast fallback), tokenized engine visuals, authored focus visuals, contrast diagnostic, text scale. After U1 (its fields sit on U1's substrate).
- [ ] **U3 — Gamepad and input** — focus-group fix, console menu conventions (restore-on-return, hold-repeat, tabs, scroll, glyphs, confirmation dialogs), full remapping, gamepad look options, hold/toggle sprint. No dependency; concurrent with U1.
- [ ] **U4 — Screen reader** — engine accessibility snapshot projected through `accesskit_winit`; window created hidden and shown after the adapter exists. After U3's focus-group fix (the snapshot's focusable set).
- [ ] **U5 — Hearing and directional cues** — captions per sound asset, subtitle primitive with speaker, directional damage indicator, sound-direction cues. Damage direction after U1; captions and cues after U2.

**Testable outcome:** a player on first boot gets OS contrast, reduced-motion, and text-scale preferences without touching a menu; a mod's strobe is tamed unless the player turns the limiter off; NVDA, VoiceOver, and Orca read and operate every dev menu; a gamepad-only player completes every dev menu and remaps any action; captions show for authored sounds with a direction arrow; a hit from the left shows a left-side indicator.

---

## Future / Speculative

Features below are intended but not yet sequenced. Rough priority ordering within each group.

### Gameplay systems

- **NPC entities** — additional enemy archetypes and behavior beyond the shipped navigation, line-of-sight, and statechart foundation.
- **Baked spatial-AI data** — the Epic 10 navmesh bake is the first layer of a broader compile-time hint set for intelligent enemies. Speculative extensions reuse the same additive PRL section: cover points, jump/drop links, hint nodes (sniper perches, ambush spots), and precomputed influence/flow data. Authored in TrenchBroom where useful, derived in prl-build where it follows from geometry.
- **World Entities** — common base scripts for doors, pickups, trigger volumes, timeline/sequence helpers; a scripted ambush set piece with destruction choreography.

### Moving and destructible geometry

- **Destruction (Pre-Fracture + Promotion)** — brushes pre-fractured into pieces with dependency edges at compile time. Runtime promotes pieces from static to dynamic on damage; reveals pre-authored interior break-faces. Requires a full rigidbody solver (Rapier) for debris physics. Latent portals activated on fracture to open hidden areas.

### Rendering and visual polish

- **Rigid model reuse** — the shipped skinned/rigid mesh path can later serve props and pickups. New model consumers remain unsequenced.
- **UI compositor fidelity + image asset pipeline** — exact source-over ordering between translucent quads/images and glyphon text (one glyphon render call cannot interleave with translucent UI geometry), and an authored registration path for UI image assets through the renderer-owned registry. Left over from Epic 13.
- **Directional fog** — the remaining fog enhancement. Existing fog volumes and portal-aware culling are shipped.
- **Moving-light shadow-depth invalidation** — re-render a moving light's cached static depth only when its transform changes, so projectile-attached and other moving lights do not redraw the full world every frame. Not sequenced until a moving shadow-caster needs it.
- **Post-processing** — optional CRT/scanline filter. HDR scene color, additive emissive, tonemapping, and bloom are shipped.
- **Baked cubemap reflections** — `env_cubemap` point entity baked to a cubemap atlas at compile time.

### Infrastructure

- **Large-map spatial residency** — future epic seed for clustered-cell, portal-driven residency of baked resources (SH, geometry, lightmap layers, and related spatial data). Distinct from regional-BVH culling. Research and staged planning seed: `context/plans/large-map-spatial-residency.md`. SH residency (stages 1–4) and lightmap/shadowmask cell blocks (stage 5, `done/spatial-residency--lightmap-cell-blocks/`) have shipped.
- **Sector Graph + Portal Culling** — replace the compiler-derived runtime cell/portal graph with an author-defined sector graph. Latent portals (activate on event) support destruction reveals. Prerequisite for kinematic clusters that need their own sector graphs.
- **Chunk Primitive** — unify static world geometry, kinematic clusters, and dynamic debris into one record type (mesh + collider + transform + sector membership). Deferred until two or more of those consumers exist and the duplication cost is clear.
- **`canonicalName` rename** — rename `classname` to `canonicalName` in scripting API and PRL. Source formats translate their identifier (Quake `.map` `classname`, UDMF thing-type, Blender prop) to this canonical name at compile time. Absence on an archetype means not directly placeable from source — script-spawned or marker-indirected only. Subsumes the `spawn_only` / `map_entity_classname` patterns into one field's presence.
- **FGD generated from script registry** — scripts are the single source of truth for entity archetypes. FGD emitted at script compile time, not hand-edited. Removes the divergence class of bug where registry and FGD describe different archetypes.
- **Composable archetypes via `@BaseClass` mixins** — `@BaseClass` declarations map to component lists; property bags drive behavior instead of proliferating archetype names. Reference patterns from `bevy_trenchbroom`: `Default::default()` as the property-fallback source, recursive depth-first base spawn with TypeId dedup, two-phase spawn (component insertion at load, subsystem registration at lifecycle hook).
- **Property-driven editor previews** — TrenchBroom expression-language helpers (`model({{ ... }})`, `iconsprite({{ ... }})`) drive per-instance preview variation. One canonical name can display different models or icons based on property values, reducing pressure to multiply archetype names.
- **Multi-format map support** — UDMF and others via `format/<name>.rs` sibling modules. All formats normalize to the canonical-name vocabulary at compile time, so runtime sees one identifier shape regardless of source.

### Dropped

- **Cubemap bake tool** — deferred indefinitely; baked cubemap reflections remain on the speculative list above but the standalone tool is dropped.
