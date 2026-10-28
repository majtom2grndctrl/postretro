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

- [ ] **movement--wall-run** — environment-probe state; consumes the momentum policy. Detail-on-open. Vault shipped the probe pattern first; generalize its vault-only seams here rather than copying them: riding the contact's mover (`carry_ground`, `VaultFrame`), early-end settle (`settle_broken_vault`), the dead-pawn bar (`vault_barred`) and entry layer folding. Wall-run also needs a per-tick wall-contact fact before the intent.
- [x] **movement--vault** — environment-probe state: auto-mantle onto ledges and out of fluids, with a held camera pose and a per-weapon charge opt-out. Done (`plans/done/movement--vault`).
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
- [ ] **4 — Networked peer audio** — world-event sounds (doors, crushers, weapons, enemies) heard by remote co-op peers, not only by the peer whose simulation produced the event. Step 1's requests carry emitter and point, so this step appends a sound variant to the append-only `ServerPresentationPayload` and selects its recipients per world point, excluding the owner who already heard their predicted fire. The `recipients_for_world_point` seam this once reused retired with the splash presentation route, so this step reintroduces one. Door open/close/blocked can instead derive client-side from replicated `WireKinematicMoverState` phase. That struct has `direction`, `segment_index`, `completed` and `blocked` but no terminus field, so terminus is inferred. Crush has no replicated edge and needs a pulse. E17-E's AC11 (a client emits no mover sound) is lifted here.
- [ ] **5 — Baked BVH audio occlusion** *(later / additive)* — bake an occlusion/obstruction dataset by casting source→listener segments through the compile-time geometry BVH. At runtime, a sound whose path crosses solid geometry is attenuated and low-pass filtered (kira `FilterBuilder` on the emitter's spatial track) — true *obstruction*, beyond reverb zones' coloring by listener region. The coarse **potentially-audible-set** is a consumer of the shared cell-visibility relation (`context/research/cell-visibility-substrate.md`), the same substrate E15 Phase 4 network relevance wants; whichever epic builds it first does so consumer-agnostically. Speculative; sequenced after the core foundation.

**Testable outcome:** sounds behind the player are audibly distinct from sounds in front, and wide stereo sounds stay wide up close. Reverb zones audibly change acoustics. Co-op peers hear world events near them.

---


## Epic 15: Multiplayer Foundations and Host Flow

Transport, replication, prediction/reconciliation, state-slot replication, session lifecycle, and seat/identity/roster have shipped. Full design: `context/research/netcode/`.

- [x] **coop--networked-by-default** — descriptor-world replication, canonical held ownership, bound host-triggered light/fog/emitter state, presentation audiences and shared combat feedback. Client alternate-fire receive preserves live burst/charge. Owner acceptance complete, including OS Reduce Motion. Brief: `context/plans/done/coop--networked-by-default/`.

**Open — network model under reconsideration.** The current lean is the **session-scoped co-op posture** (L4D2, Vermintide) over the persistent-world posture: a run ends when the host leaves, nothing persists server-side, and player hosting is the whole shipping model. Under that posture dedicated-server readiness stops being a north star, and CI plus agentic observability alone justify the headless seam. Settle this before Phase 4 opens, because it decides whether host symmetry is a requirement or an option. **Host migration** remains a long-term aspiration, modelled as a graceful **handoff** (the departing host packages authoritative session state; a successor loads it). Two nearly-free consequences to honor: whatever survives a level transition is an explicit, enumerable session-state set, and the lifecycle distinguishes a host-initiated leave from a crash or timeout. Full reasoning in the Q3 archive.

- [ ] **Phase 3.75 — Lobby authoring surface** — the session scope and its fixed append-only fact table, the join predicate, lifecycle reaction addresses (joined / left / phase changed) auto-fired like `levelLoad`, and a reference lobby in the dev mod. The engine owns identity, admission execution, map authority, and the relevel protocol; the mod owns lobby policy and presentation. Open: a roster is a list, slots are scalars, and the IR cannot iterate, so a variable-length player list is unreadable by every authoring surface that exists — likely a `ui.md` gap rather than a netcode one. Research: `context/research/coop-session-lobby.md`.
- [ ] **Phase 4 — Scale + host flow + dedicated-server readiness.** Validate 16 players within a host-upstream byte budget. If the budget needs interest management, consume the shipped view-independent cell-visibility relation in two stages: binary relevance first, then graded throttling only if measured pressure warrants it. Resolve the host-play execution model before this phase, then prove that hosted-session flow and the standalone-server entry point. Reuse the existing headless session substrate; do not build another headless path. Fold in the deferred interpolation refinements unless a feature playtest pulls them forward:
  - [ ] **Per-entity interpolation delay** — the starvation-driven delay is global today (one starved moving remote adds latency to every remote); make it per-entity. Tradeoff: per-entity delay introduces per-entity time offsets.
  - [ ] **Smoothed interpolation-delay transitions** — a delay *increase* currently pauses the remote render target (the monotonic no-rewind clamp) until the server estimate catches up, which can read as a brief freeze on a sustained-jitter link. Rate-limit the transition instead. Delicate: trades directly against the no-rewind guarantee.

- [x] **Network lag simulator** — inject latency, jitter and loss into a session from a dev-toolbar control or CLI flag, so co-op playtests at realistic RTT need no OS-level link conditioning.

- [x] **coop--lossy-link-delivery** — under the dev lag profile a connected client's own pawn rubber-bands, worst in slides and vaults, because one lost datagram stalls reliable-ordered gameplay traffic for a 300 ms resend. Input resends at most one tick later and carries every gameplay family, switches included; Control resends at about 100 ms; the harness runs production channel semantics. Brief: `context/plans/done/coop--lossy-link-delivery/`.

- [x] **coop--timed-slot-projection** — connected-client reload, cooldown, heat and cell meters project between host samples for smooth display; dev HUD heat bar, unchanged raw gameplay readers and no feature wire change. Automated and owner manual proofs pass. Brief: `context/plans/done/coop--timed-slot-projection/`.

- [ ] **coop--pre-arm-fire-resolves-slot-zero** — before a connected client's pawn arms, its Input carries `firing_slot` 0, and a fresh fire press there fires the host pawn's slot-0 weapon whatever its active slot. Traced in source, not yet seen in play. Trace: `context/plans/done/coop--client-inventory-awaits-tuning/research.md`. Needs a draft session.

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
- [x] **charge-on-activation** — charge level (0..1) scales listed stats at release; orthogonal to the resource, so it composes with any resource kind.
- [x] **secondary activation** — an optional alternate action beside primary fire. Each activation declares its behavior and events; charge composes with either.
- [ ] **tracked deployables** — primary activation places persistent entities owned by the weapon instance; secondary activation acts on them, such as detonating placed charges. Defines ownership, limits, and cleanup.

### Resolution Modes

**Non-goal:** server rewind / favor-the-shooter lag compensation — the engine predicts and reconciles, it does not rewind (`context/lib/index.md` §4).

- [ ] **melee** — a short-range shape-sweep query (a non-ray query family shared with AoE), exposed both as a resolution mode and as a universal quick-melee activation, with a lunge (a combat↔movement impulse).
- [ ] **Combat / projectile VFX relevance culling** *(later / additive)* — wire the shared cell-visibility relation into recipient selection for presentation events and observer weapon cues (broadcast today; the `recipients_for_world_point(P)` seam retired with the splash presentation route) so the host stops sending cosmetic combat VFX to clients whose Cell cannot perceive the event's Cell: `locate_cell(P)` + `perceivable` per client cell, plus host-side per-client cell tracking, with no wire change. Shares the relation with E15 Phase 4 and the audio potentially-audible-set. Presentation-only — never culls authoritative simulation. Sequenced on measured co-op VFX-bandwidth need.
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
- [x] **movement--mover-pinned-rider** — a mover under the default `displace` policy can carry or push a player into static geometry it cannot push them clear of, and nothing frees them: only `crush` handles a pinned body (`movement.md` §6). Own movement can no longer embed a body (`plans/done/movement--static-overlap-invariant`), so this is the remaining way a pawn ends up stuck inside static geometry. Lands before enemy–mover interaction, which moves agents onto the same mover-contact code and inherits the rule. Also fixes a `once` mover that a reversal sends back to its start staying unstartable.
- [ ] **Enemy–mover interaction** — enemies blocked, crushed, carried on lifts and pushed by movers, as players already are. Research: `context/research/enemy-mover-interaction.md`.

---

## Epic 18: Co-op Set-Pieces

Trigger fan-out, activation policy, dispatch parameters, enemy groups, spawning, trap pools, timed reactions, and projectile traps have shipped. The load-bearing E18-A decisions — effect-based dispatch split, bind-at-install, the co-op atmosphere channel — are recorded in the Q3 archive; later specs inherit them. Design intent: `context/research/co-op-triggers-trap-pools.md`.

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
- [x] **U3 — Gamepad and input** — focus-group fix, console menu conventions (restore-on-return, hold-repeat, tabs, scroll, glyphs, confirmation dialogs), full remapping, gamepad look options, hold/toggle sprint. No dependency; concurrent with U1.
- [ ] **U4 — Screen reader** — engine accessibility snapshot projected through `accesskit_winit`; window created hidden and shown after the adapter exists. After U3's focus-group fix (the snapshot's focusable set). Repeat the Windows saved-window-mode boot matrix after integrating hidden-window visibility order.
- [ ] **U5 — Hearing and directional cues** — captions per sound asset, subtitle primitive with speaker, directional damage indicator, sound-direction cues. Damage direction after U1; captions and cues after U2.

**Testable outcome:** a player on first boot gets OS contrast, reduced-motion, and text-scale preferences without touching a menu; a mod's strobe is tamed unless the player turns the limiter off; NVDA, VoiceOver, and Orca read and operate every dev menu; a gamepad-only player completes every dev menu and remaps any action; captions show for authored sounds with a direction arrow; a hit from the left shows a left-side indicator.

---

## Epic 25: Map Transitions

Players walk from one map into the next as if into the next room. A seam is a stretch of level built identically in two maps; the next map loads in the background, and the engine swaps maps once no player in the seam can see past it. Doors and bends keep a seam blind. Today a trigger can already fire `loadLevel`, but every change is a hard cut to the next map's player starts. Epic hub: `context/plans/drafts/E25--map-transitions/`.

- [ ] **U1 — Off-main level load** — staged load pipeline; prep and GPU creation off the main thread, a short commit, stages free to run beside a live level. Brief: `context/plans/ready/level-load-pipeline/`. Concurrent with U2.
- [ ] **U2 — Landmark level change** — seam entity, landmark-relative arrival through the seamless-teleport transform, carried state, co-op arrival; loading screen kept.
- [ ] **U3 — Seamless seam** — generated or authored prefetch region, background load during play, blind swap, door wait with `waiting` / `ready` events. Co-op clients take the classic relevel. After U1 and U2.
- [ ] **U4 — Co-op seamless seam** — clients prefetch on host instruction and cross with no loading screen and no movement hold, predicting through the swap as through a teleport; participation stays one predicate. After U3.

**Testable outcome:** a player opens an airlock door, or rounds a bend, into the next map with no loading screen and keeps their speed, facing and loadout; a co-op group crosses together.

---

## Fluid Rendering

Water-surface briefs outside a numbered epic.

**Planar sequence.** Ships in order: `sh-streaming--pool-growth-device-limit` (done) → `portal-walk-bounded-regions` (done) → planar-water-reflections (done) → reflection-probe fluid-pass performance. The mirror rides the bounded-region walk's seeded entry and its bake-gap gate, so the walk lands first. Planar precedes the performance brief because it routes per draw and leaves the fluid shaders unedited; the performance brief then rewrites those shaders and carries the routing with it. Planar mirror fog follows planar-water-reflections in any order.

- [x] **planar-water-reflections** — at `reflection_quality` very_high and ultra, the largest visible water plane reflects the room, movers, characters (the local player's body included), sprites and particles from a mirrored view, on screen or not, in the author's reflection style; every other reflecting surface, puddles included, keeps SSR plus probe. One plane per frame behind an area floor; mirror visibility and demand follow the bounded-region walk's rules. Mac 5300M, 1440p: planar 1.0–1.1 ms GPU replaces a 7.4–7.6 ms very_high SSR pass. Narrows `resource_management.md` §9's mirror non-goal. Done (`plans/done/planar-water-reflections`).
- [ ] **planar mirror fog** — fog and smoke raymarching in the mirror view, so reflections over fogged rooms carry the fog. Measured on its own; the mirror skips fog until then. Not yet drafted.
- [x] **reflection-probes** — every map's water mirrors its static room from compile-time, distance-corrected box-projected probes the compiler derives per pool, with live per-light brightness and Fresnel weighting; a `reflection_volume` brush overrides the fit. The static base layer a later screen-space layer composites over. Done (`plans/done/reflection-probes`).
- [ ] **reflection-probe fluid-pass performance** — per-probe light-share precompute for drawn volumes (the blend-cost spike cut a single sample 21% and a 3-probe blend from +78% to +3–4%), pixel style at cell rate with hidden cells skipped (a Fresnel early-out was measured on the stress map and dropped). Follows planar-water-reflections and keeps its per-draw layer routing (planar on the selected plane, SSR elsewhere) in whatever draw shape it picks. Draft: `context/plans/drafts/reflection-probe-fluid-pass-performance/`; four `/review-brief` rounds, the last with no blockers, but drafted before fluid base-pass cost, fluid-ssr-reflections and planar-water-reflections landed, so it needs a refresh pass against them before promotion. The IP review with counsel blocks build. Spike: `claude/cell-rate-spike-1660`.
- [x] **fluid base-pass cost** — fluid baked light (ambient floor, SH indirect, baked direct SH) moved to the vertex stage on an install-time SH-lattice grid: the 5300M's reflection-free fluid pass at full-screen 4K fell 52–54 → 2.2 ms, all views faster on the GTX 1660. Done: `context/plans/done/fluid-base-pass-cost/`. Follow-up: fluid surface T-junction closure (below).
- [x] **fluid surface T-junction closure** — every fluid face pair meeting along a boundary now shares bit-identical vertices, within and across volumes: level-wide weld-then-insert at bake replaces the push-cut gate, and faces with inserted vertices ear-clip at install. stress-env-volumes went from 6,173 open seams to 0 at 75 ms more bake and 13 ms more install. Done: `context/plans/done/fluid-surface-t-junction-closure/`.
- [x] **fluid-ssr-reflections** — screen-space reflections composited over the probe term by confidence, so movers, characters and dynamic lights appear in fluid surfaces; player `reflection_quality` (off/low/medium/high/very_high/ultra). Done (`plans/done/fluid-ssr-reflections`).
- [x] **reflection-probe-capture-exclusion** — probes leave out static brushes small relative to their water, so pillars, posts and trim reflect through SSR with true parallax while room-scale walls and undersides stay in the probe; mapper override `_reflection_capture`. Done (`plans/done/reflection-probe-capture-exclusion`).
- [ ] **reflection-probe streaming and seams** — streamed probe residency, and blending across probe boundaries. Drafts on branch `claude/probe-streaming-brief-364a5e`; the seams brief depends on the performance brief's precompute. With planar shipped, a non-resident probe also drops planar on its water, and planar's mirror-visible cells stay out of probe demand.

---

## Lighting and GI

Today's stack, finished, plus grafts and bake-only artefacts from the RTX-lite exploration (`context/research/gi-rtx-lite/`). Each lands behind its own measurement gate, in the doc's build order.

- [x] **lightmap penumbra floor** — each texel's baked visibility is filtered over a one-texel-radius cone footprint, for every static light including authored `_light_size 0`, so no baked shadow edge shows a texel stair-step; static layers, shadowmask and animated weight maps alike. A warn-only texel-step detector in the lightmap and weight-map stages makes the rule a compile check, and lights whose emitter sphere reaches into geometry trace their full sample set and warn. Bake-only; no format or runtime change; lightmap stage about 2× slower cold (owner-accepted). Done: `context/plans/done/lightmap-penumbra-floor/`.
- [x] **lightmap exact skips** — two bake skips, bytes unchanged. Tile shaft culling: an 8×8 tile whose padded shaft to the emitter no triangle crosses stores visibility 1 and traces nothing. Chart coverage cull: drop a (light, chart) pair when the light is behind the chart, outside a spot's cone, or out of range of its texels. Prototyped together on hallway-mini (cold): Lightmap Bake 374.7 → 210.4 s, reclaiming ~84% of the penumbra floor's cost; shaft culling alone −34%, and −46% on hard-light groups-capture. Sizing: `context/plans/drafts/lightmap-stage-sizing/`. Done: `context/plans/done/lightmap-exact-skips/`.
- [ ] **lightmap dark-texel cost** — after the exact skips, fully dark texels are ~55% of Lightmap Bake. Size a cell-visibility reach cull (exact only if cell visibility bounds segment visibility) and the emitter-pairing cost (~0.46 µs per covered texel) before drafting either. Not yet drafted.
- [ ] **lightmap chart plane drift** — chart `v_axis` is built from the quantised stored vertex normal, so reconstructed texel centres drift off their face plane linearly across the chart. The drift is ~0.5 mm per 33 m on axis-aligned faces and faster on oblique ones; charts reach ~82 m at the default density. Past ~1–2 mm, shadow-ray origins sit below the receiver and can self-shadow. Found in the exact-skips review, and unobserved in content. Measure first, then a bounded iterated search: a role-split chart frame as the primary goal, an in-plane frame as the fallback, owner A/B. Moves bytes, so it lands apart from the exact skips. Drafted: `context/plans/drafts/lightmap-chart-plane-drift/`.
- [ ] **embedded-emitter bias in SH probe bakes** — the four-probe path the SH base, bounce, delta and direct-SH bakes (and billboard scatter) share keeps the bias the lightmap fix removed: a light whose emitter sphere intersects geometry reads too bright where the probes miss the embedded cap. Out of scope for the penumbra floor, which kept probe sections byte-identical. Not yet drafted.
- [ ] **door and mover light flow** — doors change direct light (owner ruling; door bounce optional). First, door classification on the existing seal auto-detect (tri-state override KVP), a door influence graph and baked light → cell reach, with a compile report that sizes the rest. Then a baked state channel (per-state visibility masks per gated baked light, blended by occluder state, composed on state events, carried to entity direct SH), with doors as its first consumer; light → cell reach filtered by `blocked_portals` for runtime-evaluated lights and fog; analytic convex-brush shadows for moving doors and movers past the promoted-slot budget. Not yet drafted.
- [x] **one-sample SH bounce visibility** — the SH base and delta bakes trace one emitter sample per (hit, light), stratified across a probe's gather rays, instead of 4–32. Same expectation as the full emitter lattice, a little more noise (within BC6H error, bias ≤ 0.5%); hard lights unchanged. Cold SH Bake 1.86× on the full hallway, ~2.4–2.8× on `stress-warren-mini`; Delta SH Bake ~1.1–1.4×. Direct build, from the `surface-radiance-cache` session.
- [x] **coloured SH bounce** — bounce takes each surface's diffuse colour (one linear mean per texture, glow texels excluded, snapped to 8-bit sRGB) instead of the grey constant; a material stage resolves textures ahead of the lighting bakes into one shared material module; a bake-wide saturation constant; movers, skinned meshes and billboards take the world's indirect scale. A look change judged by owner A/B, after the one-sample step. Brief: `context/plans/done/coloured-sh-bounce/` (done).
- [ ] **surface lights** — a light entity templated across every face of a named texture (ericw-tools `_surface` precedent; Quake 2 surface lights, Half-Life `lights.rad`), so a marked emissive texture lights the room through the existing static and animated light paths; a flickering screen rides per-light deltas. Emissive maps alone still light nothing. Light count grows with marked area, which the surface radiance cache's per-patch shading answers. After coloured SH bounce; IP check before drafting. Not yet drafted.
- [ ] **surface radiance cache** — bake outgoing radiance once on fixed-size surface patches, gather probe SH base and deltas from it instead of re-shadowing every light at every ray hit, with multi-bounce under a later-bounce scale (owner A/B). Its case is multi-bounce, which has no practical route without it, and light-count scaling: lights shade once per patch instead of once per gather hit. Not bake time or colour, since one-sample and the lightmap culls cover the near-term bake win and coloured bounce needs no cache (owner, 2026-10-25). Sequenced after the lightmap culls and coloured bounce; a sizing pass (the share of SH Bake still spent on shadow rays) comes before any build. Also feeds specular occlusion. Bake-only; low IP risk. Drafted: `context/plans/drafts/surface-radiance-cache/`.
- [ ] **SH probe classification and relocation** — keep only probes that bracket a receiver in their cell, and move probes out of walls instead of invalidating them (Majercik et al. 2021). Bake-only plus a small per-probe offset; shrinks stored tiles and SH residency. Leak counts from the radiance cache and navmesh-guided coarsening feed it; air probes stay. IP appears clear. Not yet drafted.
- [ ] **SH compose scoping** — compose only the resident, visible rows whose output can change this frame. Exact, with no player setting; the parked `animated-light-update-rate` brief names it as the alternative to half-rate compose. After the contributing-row filter the 5300M still pays ~4.4 ms of compose per frame at the kinematic-platform station. How "can change" is detected is a counsel question: a per-light change trigger reads toward Tencent US12293451B2 element (a); a residency and visibility gate does not. Not yet drafted.
- [ ] **hybrid GI measurements** — grid shrink after probe classification and navmesh-guided coarsening (stored tiles, mandatory set; hallway and campaign), the 5300M forward pass with world SH on vs off, and lights within reach of each mover on `stress-warren-lit` and `stress-warren-crates` (slab shadows vs promotion rationing). Plan in the research doc. Not yet drafted.

---

## Future / Speculative

Features below are intended but not yet sequenced. Rough priority ordering within each group.

### Gameplay systems

- **NPC entities** — additional enemy archetypes and behavior beyond the shipped navigation, line-of-sight, and statechart foundation.
- **Baked spatial-AI data** — the Epic 10 navmesh bake is the first layer of a broader compile-time hint set for intelligent enemies. Speculative extensions reuse the same additive PRL section: cover points, jump/drop links, hint nodes (sniper perches, ambush spots), and precomputed influence/flow data. Authored in TrenchBroom where useful, derived in prl-build where it follows from geometry.
- **World Entities** — common base scripts for doors, pickups, trigger volumes, timeline/sequence helpers; a scripted ambush set piece with destruction choreography.
- **Revisited-map state** — a player returning to a map finds it as they left it: dead enemies stay dead, opened doors stay open, taken pickups stay gone. Today every load is fresh; only per-seat carried state and mod slots cross a level change. Follows Epic 25. The per-map record joins the session-state ledger (`networking.md` §Session-state ledger), so a host-migration handoff carries it too. Open: whether save games share its format.

### Moving and destructible geometry

- **Destruction (Pre-Fracture + Promotion)** — brushes pre-fractured into pieces with dependency edges at compile time. Runtime promotes pieces from static to dynamic on damage; reveals pre-authored interior break-faces. Requires a full rigidbody solver (Rapier) for debris physics. Latent portals activated on fracture to open hidden areas. Lighting rides the baked state channel from door and mover light flow. Each break is authored and one-way: one state change, never a lighting state per damaged piece. Break-faces get lightmap charts and probe coverage baked in the after state, hidden until the break. Cells behind a latent portal are resident before it can open, so a reveal never shows unstreamed light. The compiler counts breaks against each light's door-channel mask budget; overflow is a named error.

### Rendering and visual polish

- **Rigid model reuse** — the shipped skinned/rigid mesh path can later serve props and pickups. New model consumers remain unsequenced.
- **UI compositor fidelity + image asset pipeline** — exact source-over ordering between translucent quads/images and glyphon text (one glyphon render call cannot interleave with translucent UI geometry), and an authored registration path for UI image assets through the renderer-owned registry. Left over from Epic 13.
- **Indirect-light redesign** — parked. Per-cell probes read for both diffuse and glossy, or patch radiosity re-solved on change. Unparks only if the research doc's conditions hold (door bounce across several doors required, cleanup fails to shrink the grid, Mac SH cost stays high, a carrier for entity direct exists). Gated by its own measurements (hallway cells and the large-cell tail, patch and link sizes, door-event reach, a per-cell probe A/B) and counsel on Square Enix US10137372B2, Tencent US12293451B2 and Tencent WO2026113651A1. Options, costs and IP triage: `context/research/gi-rtx-lite/`.
- **Lighting extensions** — optional hybrid grafts, each behind its own gate: shadowmask-mode owner lights (crisp-or-soft edges from distance-to-edge masks), specular occlusion cones, mover rest poses as a second state-channel consumer, and glossy world reflections (after counsel on Tencent WO2026113651A1). `context/research/gi-rtx-lite/`.
- **Directional fog** — the remaining fog enhancement. Existing fog volumes and portal-aware culling are shipped.
- **Moving-light shadow-depth invalidation** — re-render a moving light's cached static depth only when its transform changes, so projectile-attached and other moving lights do not redraw the full world every frame. Not sequenced until a moving shadow-caster needs it.
- **Cross-level vista** — see into the next map through an open seam: a straight hallway or a window onto it, with no door or bend hiding the swap. Needs two levels drawn in one frame, which Epic 25 forbids (its I2) and no plan owns; Epic 25 leaves a second level resident to draw from (`drafts/E25--map-transitions/`).
- **Post-processing** — optional CRT/scanline filter. HDR scene color, additive emissive, tonemapping, and bloom are shipped.

### Infrastructure

- **Large-map spatial residency** — future epic seed for clustered-cell, portal-driven residency of baked resources (SH, geometry, lightmap layers, and related spatial data). Distinct from regional-BVH culling. Research and staged planning seed: `context/plans/large-map-spatial-residency.md`. SH residency (stages 1–4) and lightmap/shadowmask cell blocks (stage 5, `done/spatial-residency--lightmap-cell-blocks/`) have shipped.
- **Teleporter arrival residency lever** — keep arrival cells touching a source region mandatory and move farther arrival cells into the evictable band. Measure on the first dense teleporter hub; today every arrival room within lead of a source is mandatory (`done/teleporters/plan.md`, External runbook).
- **Sector Graph + Portal Culling** — replace the compiler-derived runtime cell/portal graph with an author-defined sector graph. Latent portals (activate on event) support destruction reveals. Prerequisite for kinematic clusters that need their own sector graphs.
- **Chunk Primitive** — unify static world geometry, kinematic clusters, and dynamic debris into one record type (mesh + collider + transform + sector membership). Deferred until two or more of those consumers exist and the duplication cost is clear.
- **`canonicalName` rename** — rename `classname` to `canonicalName` in scripting API and PRL. Source formats translate their identifier (Quake `.map` `classname`, UDMF thing-type, Blender prop) to this canonical name at compile time. Absence on an archetype means not directly placeable from source — script-spawned or marker-indirected only. Subsumes the `spawn_only` / `map_entity_classname` patterns into one field's presence.
- **FGD generated from script registry** — scripts are the single source of truth for entity archetypes. FGD emitted at script compile time, not hand-edited. Removes the divergence class of bug where registry and FGD describe different archetypes.
- **Composable archetypes via `@BaseClass` mixins** — `@BaseClass` declarations map to component lists; property bags drive behavior instead of proliferating archetype names. Reference patterns from `bevy_trenchbroom`: `Default::default()` as the property-fallback source, recursive depth-first base spawn with TypeId dedup, two-phase spawn (component insertion at load, subsystem registration at lifecycle hook).
- **Property-driven editor previews** — TrenchBroom expression-language helpers (`model({{ ... }})`, `iconsprite({{ ... }})`) drive per-instance preview variation. One canonical name can display different models or icons based on property values, reducing pressure to multiply archetype names.
- **Entity pitch sign vs TrenchBroom** — engine positive pitch is nose-up, while TrenchBroom draws non-MDL models nose-down; reconcile at the one angle converter. Near-zero content exposure today.
- **Multi-format map support** — UDMF and others via `format/<name>.rs` sibling modules. All formats normalize to the canonical-name vocabulary at compile time, so runtime sees one identifier shape regardless of source.

### Dropped

- **Cubemap bake tool** — the standalone offline tool and the `env_cubemap` point entity are dropped; reflection probes capture inside prl-build instead (`reflection-probes`, above).
