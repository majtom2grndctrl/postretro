# Doors as Occluders (dynamic portals for the camera flood)

## Goal

A closed door occludes what is behind it. The runtime camera portal flood learns a per-frame
"this portal is blocked" input, driven by which kinematic movers are fully docked-closed and which
portals their closed geometry seals. Sealing associations are auto-detected at bake (each mover's
closed brush vs. the generated portals) and carried as baked per-mover data; the runtime derives the
blocked-portal set each frame from already-present mover phase. This is the first consumer of
dynamic-portal occlusion — E17-F's core, unblocking E18-E's sealed monster-closets. It extends the
runtime portal flood; it does **not** touch the baked `CellVisibility` Cell→Cell substrate (id 46).

## Scope

### In scope

- Bake-time auto-detection: for each `kinematic_mover`, test its closed-pose (waypoint-0) brush
  geometry against the compiler's generated portals; record the portal ids it fully seals.
- A new `sealed_portal_ids: Vec<u32>` per-mover field on the `KinematicGeometry` PRL section (id 43),
  bumping its version 4 → 5. Loader carries it; spawn seeds it onto the runtime mover.
- A per-frame blocked-portal input to the camera flood: computed in the frame loop from final
  post-tick mover phase (movers fully docked-closed contribute their sealed portal ids), consumed by
  `crates/visibility`'s `flood` as one additional per-step rejection alongside `cell_is_solid`.
- A `docked-closed` predicate over existing mover phase (mirror of `mover_is_at_open_terminus`,
  closed = waypoint 0), conservative toward non-occlusion.
- A `rejected_blocked_portal` traversal stat and a debug view of currently-blocked portals + baked
  door→portal associations (the authoring feedback a silent auto-detect otherwise lacks).
- Backward/forward compatibility: v4 (and older) PRLs load with empty sealed lists (no occlusion);
  maps with no movers or all doors open behave byte-identically to today.

### Out of scope

- The baked `CellVisibility` Cell→Cell substrate (id 46) and its deferred blocker-mask
  `(a,b,portal-state)` query — untouched here. Door-aware **coupling** for the non-camera consumers
  (network relevance, audio PAS, AI perception, VFX cull) is that substrate's later additive mask
  layer, which will reuse this spec's door→portal association. This spec is camera occlusion only.
- Door interaction with **light and shadow** transport. A closed door occludes the camera's sightline,
  not light; movers already participate in dynamic shadow maps as occluders (E17-B) independently.
  Moving shadow-casting lights are a separate deferred E17-F spec.
- The rest of the roadmap's E17-F grab-bag: kinematic clusters / sub-worlds, chunk-primitive
  consolidation. Each splits into its own later spec.
- An explicit occluder opt-in/opt-out FGD KVP. Auto-detect by full coverage is the v1 rule; a
  per-mover override is an additive KVP if a real need appears.
- Per-waypoint occluder poses. v1 tests and blocks the waypoint-0 (closed) pose only, matching the
  engine's closed = index-0 convention. A door whose sealing pose is not waypoint 0 does not occlude
  (safe over-draw); the general `(portal, waypoint)` form is a named additive extension.
- Partial-occlusion while ajar. A door blocks only when fully docked-closed and at rest.

## Direction

**Problem.** The runtime camera portal flood (`crates/visibility/src/portal_vis.rs::flood`) has no
notion of a dynamically-closable portal. Its three per-step decisions are portal-cycle,
`cell_is_solid(neighbor)`, and the geometric frustum-narrow — none consults door state, and nothing
associates a mover with a portal. So a closed door never occludes: the engine draws, lights, and
shadows everything behind a sealed doorway. Anticipated for E18-E's sealed monster-closet set-piece —
E18-E contains the closet only at the AI layer (the aggro gate keeps enemies from pathing out;
`plans/done/E18--spawner-and-closet-containment`), so with the flood door-blind the camera still draws
the closet's enemies straight through the closed door. The cause is the flood's door-blindness; this
spec fixes the cause.

**Prior commitments.**
- *Portal traversal is the sole visibility path* (`index.md` §2, `rendering_pipeline.md` §2). This
  extends that path with one rejection reason; it adds no parallel visibility system.
- *Baked over computed* (`index.md` §2). The door→portal association is spatial data, so it bakes.
  Homed per-mover on section 43, its footprint is a handful of u32s — no per-cell or per-portal
  inflation.
- *Cell-visibility substrate neutrality* (`research/cell-visibility-substrate.md`; shipped
  `plans/done/cell-visibility-relation`). That substrate deferred dynamic geometry as an additive
  `(a,b,portal-state)` blocker-mask seam for its non-camera consumers. This spec honors the doc's "do
  not invent a parallel dynamic-vis path" by making one neutral, portal-keyed dynamic-portal primitive
  (the baked association + the runtime blocked set) that the camera consumes now and the substrate's
  future mask layer can later reuse — **without** building that mask layer or editing section 46. (That
  future reuse is a plausible seam, not a locked contract: §43 keys per-mover portal-seal lists, the
  §46 mask keys per-sightline blocker sets, so the later layer derives from this association rather
  than reading it verbatim.) The way
  to not skew the substrate is to not modify it: its deferred seam stays deferred, its neutrality
  intact.
- *Movers are presentation-derivable from replicated phase* (E17 networking; `WireKinematicMoverState`
  already carries `segment_index`/`direction`/`segment_elapsed_ms`/`blocked`). The blocked-portal set
  is per-peer-local presentation derived from that phase — no new wire, no authority question. It
  culls the camera only, never simulation (matching the substrate doc's "presentation/relevance only,
  never simulation authority").
- *E17-A / E17-E deferred this decision to E17-F.* E17-A's open question ("whether doors ever
  participate in visibility/portal blocking") and E17-E ("E17-F owns whether a door affects
  visibility") are resolved here, as committed.

**Alternatives rejected.**
- *(1) Route door-occlusion through the baked `CellVisibility` substrate (id 46) via blocker masks —
  the roadmap's literal "reuse the dynamic-geometry model."* Rejected for the camera: the camera does
  not query section 46 (it walks portals per-frame), so wiring 46 would build the deferred
  `(a,b,portal-state)` mask machinery with no live consumer — the exact skew toward destructibles /
  net / audio to avoid. Section 46 is the right home for door-aware **coupling** seen by the
  non-camera consumers, later and additively, reusing this spec's association.
- *(2) Derive the association at load instead of bake — no format change at all.* This is the
  strongest rival: the coverage test is a per-mover point-in-convex-brush check over a handful of
  portals, the runtime already holds both mover brushes and portals in memory, and load-derivation is
  *also* deterministic and *also* paid once — so "baked over computed" alone does not settle it, and
  choosing load-time would delete the entire v5 wire surface (AC5, AC6) and the v4 back-compat path
  (AC4). Baking wins on two grounds that load-time cannot match: the association becomes **inspectable
  baked data** (the diagnostic authoring feedback a silent auto-detect needs — Task 4 reads it), and it
  is the durable primitive the deferred §46 blocker-mask layer can later read, whereas a load-derived
  map is transient per-session. Owner-chosen bake-first on those grounds. Load-time derivation is the
  documented fallback if a future change makes the bake pass inconvenient.
- *(3) Explicit FGD opt-in KVP for occluders.* Rejected per owner preference for auto-detect. The
  incidental-alignment risk (a moving platform that happens to span a portal) is contained by the
  conservative rule: mark sealed only on **full** coverage, and block only when **fully docked-closed
  at rest** — a platform docked over a portal it fully covers occluding is defensible, and any
  uncertainty errs toward not-occluding.
- *(4) Make the doorway its own cell and dynamically mark it solid (reuse `cell_is_solid`).* Rejected:
  a door's grain is the *portal* between two real rooms, not a cell; solidifying either room hides it
  wrongly, and carving doorway cells is a larger compiler change that mutates cell semantics.
  Portal-level blocking is the correct grain.

**Foreclosures / one-way doors.** Two-way and cheap to undo: backing out deletes the section-43 v5
field (revert to v4), the frame-loop blocked-set computation, the flood's fourth predicate + stat, and
one slice param on the visibility entry point — no consumer churn beyond the internal visibility
signature, and section 46 untouched. Portal-index stability across bake→load is not a new risk:
`convert_usable_portals` (`crates/level-loader/src/prl_loader.rs`) is all-or-nothing — it lowers every
portal 1:1 in order, or on any malformed portal returns the no-portals fallback for the whole map — so
the emit chain (`generate_portals` → `encode_portals` → `PortalsSection` → `convert_usable_portals` →
`LevelWorld.portals`) is order-preserving whenever a portal graph exists, and when it does not the
flood never runs so the baked ids are never consulted. A baked `sealed_portal_id` thus cannot be
out-of-range against a present portal array; the load-time in-range validation stays only as cheap
belt-and-suspenders against a corrupt or internally-inconsistent PRL.

## Acceptance criteria

- [ ] AC1 — On a fixture with a closet door, when the door is fully docked-closed the closet
  interior's cells are absent from the drawable `VisibleCells` set (not drawn); when the door is open
  or moving they are present. Verified from the visible-cell set, not pixels.
- [ ] AC2 — Conservative cull, zero wrongly-hidden geometry: a portal is marked sealed only when the
  **union** of the mover's closed brushes fully covers it — a multi-brush door (double / segmented
  blast door whose halves meet at a seam) is detected, while any portal with an uncovered gap is not
  marked — and no docked-closed door ever removes a cell that still has an open sightline path in the
  flood. No configuration hides geometry the player can still see.
- [ ] AC3 — A map with no `kinematic_mover`, or one whose doors are all open/moving, produces a
  byte-identical drawable `VisibleCells` set to the pre-feature build (empty blocked input → flood
  unchanged).
- [ ] AC4 — A pre-feature v4 (or older) `KinematicGeometry` PRL loads and runs; every mover gets an
  empty sealed list; no door occludes; no error, no panic.
- [ ] AC5 — Two compiles of the same fixture produce byte-identical `KinematicGeometry` (v5) section
  bytes. Each mover's `sealed_portal_ids` is ascending and duplicate-free; detection order does not
  leak (no HashSet/HashMap iteration into the emitted ids).
- [ ] AC6 — `sealed_portal_ids` are validated in-range (`< portal count`) at load; an out-of-range id
  is dropped with a warning and never panics — that door simply does not occlude that portal. On a
  validly compiled map no id is ever out of range (portal indices preserved bake→load 1:1).
- [ ] AC7 — Symmetric occlusion: with the camera inside a sealed room the closed door culls the space
  outside; with the camera outside it culls the space inside. The flood rejects the portal regardless
  of traversal direction.
- [ ] AC8 — Networked two-peer: host and each client derive door occlusion from the **live
  `KinematicMoverComponent` phase in the registry**, read in the render stage — the same component the
  door pose is drawn from, on both peers. The client's mover phase is locally predicted
  (`client_predict_loaded_movers_tick` advances it each fixed tick, reseeded to the predicted phase at
  `mover_target_tick ≈ estimated_server_tick` on snapshot apply), so its door pose and its occlusion
  phase share the present predicted tick and it never under-draws. PRL door movers are locally
  predicted, **not** interpolation-delayed like remote entities (they are excluded from
  `sample_into_registry`), so there is no `render_server_tick` history seam — reading a past-tick phase
  against the present door would itself under-draw an opening door. No new wire field is added;
  `sealed_portal_ids` is excluded from the multiplayer static-content parity hash `level_content_digest`
  (`crates/postretro/src/runtime_movers.rs`), satisfied by leaving that function untouched
  (presentation-only, beside block policy/timers). Client occlusion may differ from the host by at most
  the prediction/snapshot gap during a door transition, and never causes desync (camera-only, never
  gates simulation).
- [ ] AC9 — Docked-closed edge states resolve per the Orderings table: a freshly-spawned door occludes
  because `was_active_this_tick` defaults false at spawn (`crates/entities/src/components/
  kinematic_mover.rs`), independent of `started` (a `start_on_spawn` door still at waypoint 0 that frame
  also occludes); a door mid-open, mid-close, blocked-partway, or completed-at-open does not; a
  **running** ping-pong door never occludes even at the instant of full closure (accepted over-draw —
  the predicate cannot express a parked-closed oscillating door), while one **stopped** at the closed
  dock (`started == false`, `was_active_this_tick == false`) occludes; the blocked set is recomputed
  each frame from phase, never latched. A regression asserts `was_active_this_tick` is false on a
  just-spawned mover (so it occludes on frame 1) and that a settled door reports docked-closed
  identically on host and client from the predicted registry phase.
- [ ] AC10 — Consistency: a fog volume reachable only through a closed door is not marched (fog culling
  shares the portal-traversal `fog_reachable`, which the blocked portal shrinks); the feature adds no
  new behavior to the shadow passes (a closed door occludes the camera, not light transport).
- [ ] AC11 — The camera flood exposes a `rejected_blocked_portal` traversal stat, incremented once per
  portal skipped for a docked-closed door, surfaced in the portal-walk trace like the existing
  `rejected_solid` counter.
- [ ] AC12 — Review/grep gate: no task edits the `CellVisibility` section (id 46), its loader lowering,
  or its query API. Door-occlusion data lives only in the `KinematicGeometry` section (id 43) and the
  runtime blocked-portal input. Verified by a section-46 diff check.
- [ ] AC13 — A debug view lists (and/or draws) the currently-blocked portals and the baked door→portal
  associations for the loaded map, so an author can confirm auto-detection sealed the intended doorway.
- [ ] AC14 — The E18-E payload: with the closet door docked-closed, entities located only in the sealed
  interior (the closet's enemies) are not collected into the render instance set. Instance collection
  gates on the drawable `VisibleCells` set — mesh/entity via `mesh_render.collect_with_hit_zones(…,
  &visible_cells, …)` and billboards/particles via `particle_render.collect_at_tick(…, &visible_cells,
  …)` (both `crates/postretro/src/main.rs`), and kinematic-mover instances via
  `rebuild_visible_cell_bounds` / `mover_visible_against_cell_bounds` (`crates/postretro/src/
  runtime_movers.rs`) — so shrinking that set transitively drops the interior's entities with no
  separate per-entity door test. Opening the door restores them. (Shadow-cone instances are collected
  ignoring camera PVS by design, so shadows are unaffected — consistent with AC10.)

## Tasks

### Task 1: Thin slice — v5 format, one door end to end

Stand the full pipe end to end with one real door on one real portal, to falsify portal-index
stability, the section format, the docked-closed predicate, the flood threading, and the visible-set
effect before broadening. **Format:** bump `KINEMATIC_GEOMETRY_VERSION` (`crates/level-format/src/
kinematic_geometry.rs`, currently `u16 = 4`) to 5; add `sealed_portal_ids: Vec<u32>` to
`KinematicMoverRecord`, serialized exactly like the existing `indices` field (`write_count(len)` then
each id as `u32` little-endian) under a `version >= 5` guard in `write_mover`, with the mirror decode
in `read_mover` (v4 and older decode an empty list); `from_bytes` keeps rejecting trailing bytes and
duplicate mover ids. Because bumping the pub const drops plain `4` from the accepted-version `matches!`
in `from_bytes`, add a `KINEMATIC_GEOMETRY_VERSION_V4 = 4` const and extend that match to include it
(else a v4 PRL rejects — breaking AC4); update the enumerated version-error string (`expected 1, 2, 3,
or {VERSION}` → `1, 2, 3, 4, or 5`, and the `rejects_unsupported_section_version` assertion that checks
that text) and repoint that test's fixture off `5` (now valid) to a still-unsupported version. Ids
are indices into the Portals section (id 15), ascending, unique. **Loader:**
copy the field through `impl From<KinematicMoverRecord> for LoadedKinematicMover`
(`crates/level-loader/src/prl.rs`) onto `LoadedKinematicMover` (no validation at this seam — it has no
portal count). Do the in-range check in `load_prl` (`crates/level-loader/src/prl_loader.rs`), after
`convert_usable_portals` produces `portal_data` and the `kinematic_geometry` conversion runs — both
coexist there before `LevelWorld` is built: drop each `sealed_portal_id >= portal_data.len()` with a
single `log::warn!` (AC6); when `portal_data` is `None` (no-portals fallback) the flood never runs, so
the ids are inert. Seed the surviving field onto the spawned `KinematicMoverComponent` in
`spawn_from_geometry_with_auto_close_default` (`crates/postretro/src/runtime_movers.rs`) beside the
existing `block_policy`/event seeding. **Minimal bake:** at the Geometry stage in
`crates/level-compiler/src/pipeline.rs` (~line 376, where both `generated_portals: Vec<Portal>` from
line 352 and `map_data.kinematic_movers[..].brush_volumes` are in scope), pass `generated_portals` into
`encode_kinematic_geometry_section` (add a `generated_portals: &[Portal]` param) and compute each
mover's `sealed_portal_ids` inside a helper it calls — the helper is the seam Task 2 also uses, so this
freezes the encode signature. The thin-slice coverage test assumes a **single-brush** door: a portal is
sealed iff every vertex of its `polygon: Vec<DVec3>` lies inside the mover's single closed convex
`BrushVolume` (each vertex behind or on every `BrushPlane { normal, distance }`, within an epsilon); the
mover's `brush_volumes` are already at the authored waypoint-0 (closed) world position. This is exact
for a single convex brush but **over-marks** multi-brush movers (it can seal a portal with a real seam
gap), so it is provisional until Task 2 replaces the helper body with the conservative union carve; the
Task 1 fixture door is single-brush. Non-pipeline (test) callers of `encode_kinematic_geometry_section`
(in `crates/level-compiler/src/parse.rs`) pass `&[]` for `generated_portals` (⇒ empty sealed lists).
**Runtime consumption:** thread a blocked-portal
input — an `&[bool]` indexed by portal id (empty slice ⇒ nothing blocked) — from
`postretro_visibility::determine_visible_cells` (`crates/visibility/src/visibility.rs`, add the param)
down through `determine_visible_cell_set` → `portal_traverse_detailed` →
`portal_traverse_with_step_limit` → `portal_traverse_inner` into `DfsState`
(`crates/visibility/src/portal_vis.rs`, built ~line 179). Pass `&[]` (empty ⇒ nothing blocked) at every
other caller the new param breaks: the non-frame-loop production callers
(`crates/postretro/src/capture/driver.rs`, `crates/postretro/src/candidate_cull_probes.rs`), the public
`portal_traverse` wrapper (`portal_vis.rs`), and the `crates/visibility` test callers — only the frame
loop supplies a real slice. In `flood`, at the portal step where
`portal_idx` is resolved (~line 321, beside the `cell_is_solid(neighbor)` reject ~line 347), skip a
portal whose `blocked_portals.get(portal_idx).copied().unwrap_or(false)` is true, incrementing a new
`rejected_blocked_portal` counter on `PortalTraversalStats` and emitting it in the trace summary. In
the frame loop (`crates/postretro/src/main.rs`, the render stage ~line 3498, after the fixed-tick loop
closes ~line 2967), build the `&[bool]` from final post-tick mover phase: each frame first resize it
to the **current** map's portal count filled `false` (full clear-and-refill, never a partial `resize`
that leaves stale entries), then for each mover fully docked-closed set its `sealed_portal_ids` true —
so a previous map's blocked portals never survive a level change (a stale `true` at a reused index would
cull an open doorway; see the Orderings level-change row). This build must complete **before** the
`determine_visible_cells` call (~line 3498) that consumes the buffer, so the flood never reads the
previous frame's contents. "Docked-closed" mirrors `mover_is_at_open_terminus`
(`crates/postretro/src/kinematic_mover.rs`) but for the closed waypoint (index 0):
`waypoints.len() >= 2 && segment_index == 0 && segment_elapsed_ms <= f32::EPSILON`, at rest
(`current_linear_velocity ≈ 0`, `was_active_this_tick == false`), and `!blocked`. Mover phase lives on
the `KinematicMoverComponent` in the entity registry (not on `MoverTickState`), so read it from the
registry post-tick; the reusable buffer may live on `App` beside `scratch_cells`. Fixture: a
closet-with-door `.map` under `content/dev/maps/`. Tests: closed → interior cells absent from the
drawable `VisibleCells`; open/moving → present (AC1); a no-door map's visible set is unchanged (AC3);
a v4 PRL loads with empty sealed lists (AC4); the flood exposes `rejected_blocked_portal` (AC11).

### Task 2: Robust bake auto-detection

Generalize Task 1's minimal coverage test into the full, conservative, deterministic detection pass in
`crates/level-compiler` (the `encode_kinematic_geometry_section` path and a helper it calls). Detection
is a pure function of each `kinematic_mover`'s closed-pose `brush_volumes` (from
`crates/level-compiler/src/map_data.rs`, `BrushVolume { planes: Vec<BrushPlane { normal: DVec3,
distance }>, aabb, sides }`, at the authored waypoint-0 world position) and the generated portals
(`crates/level-compiler/src/portals.rs`, `Portal { polygon: Vec<DVec3>, front_leaf, back_leaf }`). The
correctness direction is conservative **toward non-occlusion**: mark a portal sealed only when the
**union** of the mover's closed brushes provably covers the whole portal polygon, because a
wrongly-marked occluder hides geometry the player can still see (the unacceptable failure). Coverage is
**union coverage**, not single-brush containment — a double or segmented door whose brushes meet at a
seam must be detected even though no single brush covers the portal alone. Compute it by **convex
carving**: start with the portal polygon as the sole uncovered fragment; for each brush, replace each
fragment with the sub-fragments lying outside that brush (split the fragment by each brush plane,
keeping the outside pieces as still-uncovered and recursing the all-inside piece — the piece inside
every plane is covered and discarded), reusing the compiler's existing winding half-space clip
(`clip_winding_to_half_spaces`, defined in `crates/level-compiler/src/geometry_utils.rs`, already used
by `portals.rs::generate_portals`); the portal is sealed iff no
uncovered fragment with area above a conservative epsilon remains after all brushes. The epsilon errs
toward *uncovered*, so a hairline seam gap never falsely seals. Handle: a mover sealing several portals
(all recorded); a mover sealing none (empty list, no warning — auto-detect has no declared intent to
violate); and degenerate/invalid portals or brushes (skip, never mark). Use an AABB pre-reject
(`Aabb::intersects`, union of the mover's brush AABBs vs. the portal's) before the carve for speed. Emit each mover's
`sealed_portal_ids` **ascending and duplicate-free**, with no HashSet/HashMap iteration feeding the
ids, so two compiles are byte-identical (AC5). Log the detected associations at bake (mover id →
sealed portal ids) so the mapping is inspectable. Tests: single-brush full-coverage door marks its
portal; a two-brush door whose halves meet at a seam covers the portal in union and is marked; a
two-brush door with a real gap at the seam marks nothing (AC2); a partial single-brush overlap marks
nothing (AC2); a mover sealing several portals; byte-identical recompile (AC5); ascending/unique ids.
Do not touch the `CellVisibility` section (id 46) (AC12).

### Task 3: Runtime blocking across all mover states + consistency

Harden the runtime blocked-portal derivation in `crates/postretro` — the docked-closed predicate and
the blocked-set assembly — without changing the `crates/visibility` signature Task 1 froze. Make the
predicate correct across every mover phase (cite the Orderings table): a freshly-spawned door occludes
via `was_active_this_tick == false` at spawn (not via `started`); a door mid-open (leaving the closed
dock: velocity ≠ 0 or `segment_elapsed_ms > EPSILON`) unblocks that same frame; a door mid-close blocks
only on the **settle tick** after it reaches the waypoint-0 dock at rest — the at-rest clause
(`was_active_this_tick == false` and/or `current_linear_velocity ≈ 0`) is load-bearing: predicate-true
then implies ≥ 1 full tick since physical arrival, so both interpolation endpoints the renderer blends
are the sealed pose ⇒ never under-draw; a door `blocked` partway (obstruction hold, not at the dock)
does not occlude; a door `completed` at the open terminus does not; a **running** ping-pong or a
completes-with-end-wait door does not occlude while `was_active_this_tick` stays true (accepted
over-draw), only a stopped/completed door at the dock does; the blocked set is **recomputed each frame
from phase, never latched** (Invariants: per-frame stateless derivation). Two doors sealing one portal ⇒ blocked if
either is docked-closed (OR over sealers); one door sealing two portals ⇒ both blocked. Confirm the
networked path: host and each client derive occlusion from the **live `KinematicMoverComponent` phase in
the registry**, read in the render stage from the same component the door pose is drawn from. The
client's phase is locally predicted — `client_predict_loaded_movers_tick` advances it each fixed tick,
reseeded to `mover_target_tick ≈ estimated_server_tick` on snapshot apply — so its occlusion reads the
present predicted phase exactly as the host reads its authoritative phase; PRL door movers are excluded
from the interpolation sampler `sample_into_registry`, so there is **no** `render_server_tick` history
seam (`WireKinematicMoverState` still carries `segment_index`/`direction`/`segment_elapsed_ms`/`blocked`
for the phase seed). Add no wire field; keep `sealed_portal_ids` out of the multiplayer static-content
parity hash `level_content_digest` (`crates/postretro/src/runtime_movers.rs`) by leaving that function
untouched, beside the other presentation/host-only mover fields; the client-vs-host divergence bound is
the prediction/snapshot gap, never desync (AC8). Confirm the
fallback paths: on the solid-cell, exterior-camera, no-portals, and step-limit fallbacks the flood is
not used, so no occlusion applies — conservative over-draw, never wrong. Confirm consistency: the
blocked portal shrinks the portal-traversal `fog_reachable` so a fog volume reachable only through a
closed door is not marched, and the shadow passes are unchanged (doors occlude the camera, not light)
(AC10). Confirm the E18-E payload transitively: instance collection already gates on the drawable
`VisibleCells` set — mesh/entity via `mesh_render.collect_with_hit_zones` and billboards/particles via
`particle_render.collect_at_tick` (both `crates/postretro/src/main.rs`, threading `&visible_cells`),
movers via `rebuild_visible_cell_bounds` / `mover_visible_against_cell_bounds`
(`crates/postretro/src/runtime_movers.rs`) — so a shrunk visible set drops entities located only in the
sealed interior with no per-entity door test — assert the closet's enemies are not collected while the
door is closed and return when it opens (AC14). Tests: each edge state (AC9), including the
arrival-vs-settle never-under-draw row and the running-vs-stopped ping-pong split; a level-change test
that a prior map's blocked portal is all-false on frame 1 of the next map (clear + resize + refill, not
partial `resize`); symmetric inside/outside camera (AC7); a two-peer replication test that a client
derives the same occlusion the host does *from its predicted registry phase* (AC8); fog-behind-closed-
door culled; closet-enemies-not-collected (AC14).

### Task 4: Blocked-portal diagnostics

Give the silent auto-detect a feedback surface: a debug view that lists and/or draws the map's baked
door→portal associations and the currently-blocked portals this frame, so an author can confirm a door
sealed the doorway it was meant to (AC13). Reuse the existing debug-line / debug-overlay renderer and
the agent-diagnostics panel precedent (`crates/postretro` debug views); draw each blocked portal's
polygon (from `LevelWorld.portals[idx].polygon`) highlighted, and list per-mover sealed-portal ids with
each mover's live docked-closed state. Extend the `rejected_blocked_portal` stat surfacing Task 1 added
into whatever portal-walk diagnostic readout exists. This consumes the runtime blocked set and the
baked associations only; it adds no gameplay behavior. Tests: the diagnostic reports the expected
blocked set for the closet fixture with the door closed vs open.

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice; falsifies portal-index stability, the v5 section
round-trip, the docked-closed predicate, the flood-threading contract, and the visible-set effect
before the broadening tasks land. Locks the v5 wire layout and the `crates/visibility` blocked-portal
signature.

**Phase 2 (concurrent):** Task 2 (bake auto-detection — `crates/level-compiler` only) ‖ Task 3 (runtime
blocking robustness — `crates/postretro` only, does not touch the frozen `crates/visibility`
signature). Disjoint crates; both consume Task 1's frozen format and seam.

**Phase 3 (sequential):** Task 4 — diagnostics; reads the final blocked set and baked associations.

## Wire format

`KinematicGeometry` (PRL section id 43), version **4 → 5**. Little-endian, u32 counts, mirroring the
existing section conventions (source of truth `crates/level-format/src/kinematic_geometry.rs`).

- Add one per-mover field `sealed_portal_ids: Vec<u32>` to each `KinematicMoverRecord`, after the
  existing v4 fields, encoded as a `u32` count then that many `u32` portal ids (identical shape to the
  existing `indices` list). Empty list encodes as count `0`.
- Ids are indices into the Portals section (id 15), ascending, duplicate-free, each `< portal count`.
- v5 is emitted whenever the map has at least one `kinematic_mover` (replacing v4 emission). Versions
  4/3/2/1 stay loadable; on them every mover's sealed list decodes empty (no occlusion) — the
  conservative fallback. Keeping v4 loadable requires adding `KINEMATIC_GEOMETRY_VERSION_V4 = 4` to the
  accepted-version match in `from_bytes` when the pub const bumps to 5 (see Task 1).
- Presentation-only: `sealed_portal_ids` is excluded from the multiplayer static-content parity hash
  `level_content_digest` (`crates/postretro/src/runtime_movers.rs`) — satisfied by leaving that
  function untouched, beside the existing host-only block policy and timer fields.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Conservative cull — never hide geometry with an open sightline (zero wrongly-culled) | T2 (coverage marks only full seal), T3 (block only fully docked-closed at rest) | Any looser coverage test, or blocking an ajar/moving door | AC1, AC2 |
| Symmetric — flood rejects the portal regardless of direction | T1 (reject keyed on `portal_idx`, not a cell/side) | Reject must sit at the portal, not one neighbor | AC7 |
| Per-frame stateless derivation — no persistent blocked latch | T1, T3 (blocked set rebuilt each frame from phase) | No event-latch; ping-pong / auto-close correctness | AC9 |
| Never under-draw — a culled interior implies its door is drawn sealed | T3 (settle-tick + at-rest clause ⇒ both interpolation endpoints sealed; client reads the same locally-predicted registry phase the door is drawn from) | At-rest clause dropped, or client occlusion read off a past interp tick instead of the predicted registry phase | AC8, AC9 |
| Blocked buffer defaults false for every current-map portal each frame | T1/T3 (clear + resize-to-current-portal-count + refill, never a partial `resize`) | Partial resize leaves a stale `true` across a level change | AC3 |
| Portal index stability bake→load (ids reference `LevelWorld.portals` 1:1) | T1 (loader validates in-range, drops OOB with warning) | Any loader portal compaction; else OOB id | AC6 |
| Empty / missing-section fallback ⇒ no occlusion, identical to today | T1 (empty `&[bool]` early-out; v4 loads empty) | Flood check must default false; loader default empty | AC3, AC4 |
| Presentation-only, per-peer-local — no new wire, never gates simulation; each peer reads the live registry phase it draws the mover from | T3 (derived from predicted registry phase; culls camera only) | Must stay out of the parity hash; must not cull sim; must not occlude off an interp-delayed tick | AC8 |
| Cell-visibility substrate (id 46) untouched | whole spec (door data lives on id 43) | No task edits section 46 or its query | AC12 |
| Deterministic bake — `sealed_portal_ids` byte-identical across recompiles | T2 (ascending, HashSet-free emission) | HashSet/HashMap iteration into ids | AC5 |

## Orderings

Task 3 tests cite these rows; Task 1 covers the closed/open baseline.

| Scenario | Ordering / phase | Expected outcome |
|---|---|---|
| Freshly spawned door | segment 0, elapsed ≈ 0, `was_active_this_tick` false (default), at rest — with or without `start_on_spawn` | portal blocked; interior culled. Gate is `was_active_this_tick`, not `started` |
| Door begins opening | leaves closed dock (velocity ≠ 0 or elapsed > EPSILON) | portal unblocks that frame; interior visible for the whole open swing |
| Door closing — arrival tick T vs settle tick T+1 | T: last leg reaches seg0, `was_active` true (set by `advance_spin_phase`) and velocity ≠ 0 → predicate false; T+1: `completed`/`!started` ⇒ `advance_spin_phase` sets `was_active` false and `advance_mover` early-returns ⇒ velocity 0 → predicate true | unblocked at T, blocked from T+1; at T+1 both interpolation endpoints are seg0 ⇒ **never under-draw**; exactly one settle-tick of over-draw |
| Instant / degenerate close (segment ≤ min length) | one-tick jump to seg0 + `completed`, `was_active` true that tick | blocked from the following settle tick; one settle-tick over-draw; never under-draw |
| Door completes with end-wait at closed dock | arrival at seg0 with `wait_ms > 0`: `wait_remaining_ms` set, `completed` deferred, `was_active` true through the wait | portal open for the whole `wait_ms`, blocks once `completed`/at-rest — over-draw window = `wait_ms`, not ≤ 1 frame (safe) |
| Door blocked partway (obstruction) | `blocked == true`, not at the segment-0 dock | portal stays open (door not fully closed) |
| Door completed at open terminus (Once) | segment == last, elapsed ≈ 0 | portal open |
| Running ping-pong oscillating | flips at seg0, `completed` never set, `was_active` true even at full closure | portal stays open — never occludes even fully closed (accepted over-draw; predicate can't express parked-closed oscillation) |
| Stopped ping-pong at closed dock | Stop at seg0 ⇒ `started == false`, `was_active` false, velocity 0 | portal blocked (the only oscillating-door state that occludes) |
| Camera inside sealed region | flood starts in the interior cell | same door blocks outward; symmetric |
| Zero-tick render frame | frame runs 0 fixed ticks; render reads persistent component phase | blocked set = last-tick phase; settled-closed stays blocked, mid-close stays open (coherent — read off the component, not the cleared side-table) |
| N > 1 ticks in one frame across arrival | ticks T (arrival) and T+1 (settle) collapse into one frame; render reads final phase | blocked (T+1 phase); interpolation pair (T,T+1) both seg0 ⇒ no under-draw when ticks collapse |
| Networked client, door presentation source | client draws the door from the locally-predicted registry `KinematicMoverComponent` (advanced by `client_predict_loaded_movers_tick` to ≈`estimated_server_tick`); occlusion reads that same registry phase in the render stage | occlusion instant == drawn-door instant ⇒ never under-draw; divergence from host ≤ prediction/snapshot gap |
| Networked client, opening door across the interp window | enemy behind the door presented at `render_server_tick` (past); door presented + occluded at the present predicted phase | door drawn ajar ⇒ portal open ⇒ interior collected; must **not** occlude off a past-tick (`render_server_tick`) door phase, which would cull the interior while the door is drawn open = under-draw |
| Networked client, snapshot loss / sub-tick rate | client keeps predicting the mover forward each fixed tick (no freeze); occlusion reads the predicted registry phase | drawn door and occlusion stay coincident; divergence = prediction/snapshot gap, presentation-only, never desync |
| No occluder doors / all doors open | blocked `&[bool]` empty (or all false) | flood identical to today (early-out) |
| Camera on a fallback path | solid-cell / exterior / no-portals / step-limit | flood not used ⇒ no occlusion; conservative over-draw |
| Two doors seal one portal | OR over docked-closed sealers | blocked if either is docked-closed |
| One door seals two portals | `sealed_portal_ids` holds both | both blocked when docked-closed |
| Level change, reused blocked buffer | map A sealed portal 12; swap to B (per-map portal ids); frame 1 before B's movers rebuild | blocked set all-false for B's portal count on frame 1 (buffer cleared + resized + refilled, not partial `resize`); else stale `true` culls a real doorway = under-draw |
| Mover despawn / no movers after load | movers cleared, portals present | blocked set rebuilt from zero movers ⇒ all-false ⇒ flood byte-identical to today |
| Blocked-set build vs flood, within one render frame | build (clear + resize-to-current-portal-count + refill) runs **before** the `determine_visible_cells` call it feeds, same frame | flood reads this frame's freshly refilled buffer; on level-change frame 1 no map-A `true` survives into map B's flood (no intra-frame read-before-write) |

## Rough sketch

- **Blocked-portal input.** `determine_visible_cells(..., blocked_portals: &[bool])`, indexed by
  portal id, empty ⇒ nothing blocked; carried on `DfsState`; consumed in `flood` as
  `blocked_portals.get(portal_idx).copied().unwrap_or(false)` beside `cell_is_solid`. New
  `PortalTraversalStats::rejected_blocked_portal`.
- **Docked-closed.** Mirror of `mover_is_at_open_terminus` for waypoint 0 + at-rest + `!blocked`, read
  from the live `KinematicMoverComponent` phase in the registry post-tick — host and client both read it
  there (the client's phase is locally predicted by `client_predict_loaded_movers_tick` to
  ≈`estimated_server_tick`, the same tick the door pose is drawn at), so occlusion and the drawn door
  are coincident (never under-draw); movers are excluded from the interp sampler, so there is no
  `render_server_tick` history seam. The blocked buffer may live on `App` beside `scratch_cells`,
  cleared + resized to the current map's portal count + refilled each frame **before** the
  `determine_visible_cells` call it feeds (never a partial `resize`).
- **Coverage test.** Union coverage by convex carving: carve the portal polygon by each closed brush
  (reusing `clip_winding_to_half_spaces`); sealed iff no uncovered fragment above a conservative
  epsilon remains. AABB pre-rejected; conservative toward non-occlusion; detects multi-brush doors.
- **Format.** `KinematicMoverRecord.sealed_portal_ids: Vec<u32>`, section 43 v4→5, serialized like
  `indices`; loader validates in-range; spawn seeds it onto the component.
- **Substrate seam (not built here).** The same baked association is what the deferred `CellVisibility`
  blocker-mask layer will later read to make the Cell→Cell relation door-aware for net/audio/AI. This
  spec leaves that seam untouched.
- **Closet-reveal seam (downstream, not built here).** Occlusion lifts on door-*leave-closed*, not
  *fully-open*, so the whole door-open swing is the window in which a future revealed occupant is seen
  before it commits — preserve that early un-block. AC14's cull-while-sealed means a client holds no
  interpolation history for closet occupants until reveal, so the future lunge (E18-E / Epic 16 combat)
  must hold its launch pose ≥ interpolation delay before committing its arc (the door swing is bonus
  cover, not a guarantee — a door can open in one tick). Build nothing else here. See `research.md`
  §Closet-reveal seam.

## Open questions

- **Carve epsilon.** Union coverage is the pinned rule (convex carve, sealed iff no uncovered fragment
  above a conservative epsilon remains); err toward *uncovered* — a hairline seam gap must never falsely
  seal. The **plane-clip epsilon reuses `PORTAL_EPSILON` (`= 0.01`, `crates/level-compiler/src/
  portals.rs`)**, the same tolerance `generate_portals` already passes to `clip_winding_to_half_spaces`,
  so a flush seam (brush planes coinciding within 1 cm) carves clean. The **residual-area guard is a
  small numerical float-noise tolerance, tuned during Task 2 against the AC2 fixtures** (flush-seam
  double door → must seal; real-gap door → must not) — not a spike. It must **not** be set to
  `MIN_PORTAL_AREA_M2` (`0.1`): that is a visibility threshold and would tolerate a ~0.09 m² *real*
  authored gap, sealing a portal the player can see through (an AC2 violation). The residual guard is
  only a noise floor, well below any authored gap.
- **At-rest test fields — resolved.** The at-rest clause is load-bearing for never-under-draw
  (Invariants; the Orderings arrival-vs-settle row): predicate-true must imply ≥ 1 full tick since the
  door physically reached the dock. Use **`was_active_this_tick == false`** as the gate — on the settle
  tick `advance_spin_phase` sets it false for a `completed`/`!started` mover while `advance_mover`
  early-returns leaving `current_linear_velocity == 0`, so both flip together one tick after arrival and
  either alone gives the one-tick lag. Reading velocity≈0 additionally is redundant belt-and-suspenders;
  the constraint is never to block a door momentarily at the closed pose but about to move.
