# E10 — Agent Diagnostics (all-agent overlay + movement-feel fixture map)

> **Wave:** E10 enemy-AI follow-up. **Runs first** — before steering feel, stuck recovery, and combat positioning — because every one of those specs tunes against "how does the agent move," and this spec is the instrument that makes that observable. Later specs extend the overlay with their own fields (stuck counter, combat-slot scores) as they land; this spec ships the substrate.
>
> **Builds on (all `dev-tools`-gated, already shipped):** the immediate-mode `DebugLineRenderer` (`crates/renderer/src/render/debug_lines.rs` — lines, AABBs, markers, capsules; depth-tested + x-ray overlay pipelines), the navmesh region/portal overlay (`crates/postretro/src/render/nav_diagnostics.rs`), the single-agent path overlay `emit_agent_path_overlay` (`crates/renderer/src/render/renderer_diagnostics.rs:339`, currently drawn only for the `debug_chase_agent`), and the egui Diagnostics panel (`crates/renderer/src/render/debug_ui/mod.rs`, tabbed via `DiagnosticsTab`).

## Goal

Make enemy movement observable: a per-agent debug overlay (path corridor, waypoints, velocity vector, destination marker, state label) for **every** live agent, an Agents tab in the Diagnostics panel, and a dedicated movement-feel fixture map — so tuning the steering/animation/positioning specs is a look-and-see loop instead of rebuild-and-squint.

## Background (what exists, what's missing)

- `emit_agent_path_overlay` already draws the remaining path corridor plus per-waypoint cross markers — but only for the one `Alt+Shift+G` `debug_chase_agent` (call at `main.rs:2503`, block ~`main.rs:2494-2510`), and its body early-returns on `if !self.full().show_navmesh` (`renderer_diagnostics.rs:346`), so today it is gated on the navmesh toggle (`Alt+Shift+N`).
- The read seam is established: the binary borrows the registry after game logic, reads `AgentComponent` (`crates/entities/src/components/agent.rs:35-95` — `path`, `waypoint_cursor`, `velocity`, `destination`, `planned_destination`, `arrived`, `blocked`) plus `Transform`, translates to plain `Vec3` segments, and calls renderer `push_debug_line*` emitters. The public renderer surface exposes `push_debug_line` (`renderer_diagnostics.rs:315`) and `push_debug_line_overlay` (`:322`); `push_marker` exists only on the internal `DebugLineRenderer` (`debug_lines.rs:342`), not on the renderer-facing API. Renderer API never names game/nav types — that boundary rule holds for everything here.
- No text primitive exists in the debug-line path. World-anchored labels come from egui screen-space text at projected world positions — a small binary-side projection helper, **not** a new GPU text pass.
- No dedicated movement fixture map exists. `content/dev/maps/combat-demo.map` (219-line arena with pillars, hand-authored raw brush-plane triples) is the closest precedent; feel work needs purpose-built stations.
- Frame order: overlays are emitted after game logic, before the render pass, alongside the existing `emit_*` diagnostics calls (`main.rs:2464-2526`, following the `clear_debug_lines` call at `:2464`); `clear_debug_lines` owns the per-frame buffer reset. Note the egui `run_ui` closure that paints the panel is built earlier (`main.rs:2435`), i.e. BEFORE this emit block — screen-space labels must therefore have their data ready before `run_ui` runs (see Task 2).

## Scope

### In scope

- **All-agent overlay.** Generalize the chase-agent block into an emit pass over every entity carrying `AgentComponent`: remaining path corridor + waypoint markers (reusing the existing emit shape), a velocity vector (line from position to position + velocity, distinct color), and a destination marker (a small cross emitted as `push_debug_line` segments — the pattern `emit_agent_path_overlay` already uses for waypoints — centered on `planned_destination` if set, else `destination`; skipped when both are `None`).
- **State labels.** Screen-space egui text near each agent's head: FSM state (`BrainComponent::state.label()`), XZ speed, and steering flags (`arrived` / `blocked` / `has_path`). Label content is a plain string assembled at the read site so later specs (stuck recovery, combat positioning) append fields without touching the projection or draw path. A world→screen projection helper (camera view-proj from the frame's camera state) returns `None` for behind-camera / off-screen positions; labels for those agents are skipped.
- **Agents panel tab.** New `DiagnosticsTab` variant with: a live list of agents (id, state, speed, flags) and per-layer overlay toggles (paths, velocities, destinations, labels, navmesh regions/portals). A new `Alt+Shift+A` `DiagnosticAction` chord toggles the agent overlay as a whole; panel checkboxes refine layers. The navmesh regions/portals checkbox binds to the existing `nav_overlay_enabled` state (single source of truth) so it and the `Alt+Shift+N` chord always agree; the `Alt+Shift+N` toggle keeps its behavior.
- **Movement-feel fixture map.** `content/dev/maps/movement-feel.map` (committed TrenchBroom source, `.prl` built on demand via `prl-build`) with named stations, each labeled in an accompanying `movement-feel.README.md`:
  1. **Pillar wedge** — freestanding pillar with concave approach corners (stuck-recovery repro).
  2. **Corridor corners** — a run of 90° corners (waypoint snap / turn-rate reading).
  3. **Straight run** — a long unobstructed lane (accel/decel and walk-cycle reading).
  4. **Arena ring** — open room, player start centered, multiple enemy spawns around the rim (combat-positioning pressure reading).
  5. **Narrow doorway** — an opening authored near the canonical agent diameter (clearance reading).
- Everything `dev-tools`-gated; zero cost and zero code in default builds.

### Out of scope

- A GPU text-billboard path in `DebugLineRenderer` — labels are egui screen-space only.
- Recording/replay, CSV/telemetry export, frame capture.
- Overlays for non-agent entities (movers, projectiles).
- Live-tuning steering constants from the panel — the constants are compile-time by design until a spec promotes them; the panel reads state, it does not write steering parameters.
- Navmesh bake changes; the fixture map is authored against the existing bake.

## Acceptance criteria

- [ ] In a `dev-tools` build on the movement-feel map, enabling the agent overlay draws, for every live agent simultaneously: remaining path corridor, waypoint markers, a velocity vector, and a destination marker — with a wave of at least 8 agents and no debug-segment overflow warnings (manual check, documented in the fixture README).
- [ ] Each on-screen agent shows a screen-space label with FSM state, XZ speed, and arrived/blocked/has-path flags; agents behind the camera or off-screen produce no label (runnable unit test on the projection helper with a known view-proj matrix: in-front point maps into the viewport, behind-camera point returns none; label-string assembly covered by a pure-function test).
- [ ] The Diagnostics panel has an Agents tab listing every live agent (id, state, speed, flags) with working per-layer toggles: disabling a layer removes exactly that layer's geometry (or labels) next frame (manual check).
- [ ] The agent overlay has its own diagnostic chord, independent of the navmesh toggle; the navmesh toggle's existing behavior is unchanged (manual check; existing diagnostics-chord tests remain green).
- [ ] `movement-feel.map` compiles clean through `prl-build` (navmesh present, no leak), loads in the engine, and contains the five named stations with at least one `reference_enemy` spawn and a `player_spawn`; the README documents each station and its target playtest question (manual check).
- [ ] A build without the `dev-tools` feature compiles with none of the new overlay/panel code present (`cargo check -p postretro` without the feature stays green).

## Tasks

### Task 1: All-agent overlay emit pass
Replace the single `debug_chase_agent` overlay call with a pass over every entity carrying `AgentComponent`. The per-agent read runs ONCE, in the shared pre-egui snapshot pass this spec introduces (see Task 2): before the `run_ui` closure (`main.rs:2435`) the binary borrows the registry once, reads `Transform` + `AgentComponent` (`path`, `waypoint_cursor`, `velocity`, `destination`, `planned_destination`, `radius`), and builds a plain per-agent snapshot (`Vec3` geometry inputs plus the Task 2 label data). Task 1 owns the geometry half of that snapshot; Task 2 owns the label half. The geometry emit stays in its existing lifecycle slot in the diagnostics emit block (after `clear_debug_lines`, `main.rs:2464`): it drains the snapshot (no second registry borrow) and emits corridor/waypoints (existing `emit_agent_path_overlay` shape), velocity vectors, and destination markers through the renderer's `push_debug_line` surface. Destination marker = a small cross of `push_debug_line` segments centered on `planned_destination` if set, else `destination`; skip when both are `None`. Do NOT introduce a renderer-facing `push_marker` — it is not on the public surface. Renderer signatures stay game-type-free.

The reused `emit_agent_path_overlay` must drop its internal `if !self.full().show_navmesh { return; }` gate (`renderer_diagnostics.rs:346`) — or a sibling emitter is authored without it — so the corridor/waypoint draw is gated ONLY by the new agent-overlay chord/flags, independent of the navmesh toggle (per AC4). Gate the whole pass on a new `Alt+Shift+A` `DiagnosticAction` variant (chord in `crates/postretro/src/input/diagnostics.rs`, following the `SpawnChaseAgent` precedent; `A` is unused by the current eight bindings — Backslash, Digit1, V, P, Backquote, N, L, G) plus per-layer boolean flags stored where the panel can reach them (renderer diagnostics state, following the `nav_overlay_enabled` precedent). Adding the variant breaks the exhaustive match and `assert_eq!(count, 8, ...)` at `crates/postretro/src/input/diagnostics.rs:452-469`; update both (and add a chord test mirroring the existing per-action ones) in this same change. The `debug_chase_agent` spawn chord is untouched; its agent is simply one of the iterated agents.

Before shipping, confirm the debug-line segment budget covers the AC1 wave: `MAX_DEBUG_SEGMENTS` is `256 * 1024` (`debug_lines.rs:9`) — with the per-agent budget (corridor legs + 3 crosses/waypoint + velocity line + destination cross, on the order of a few dozen segments/agent) times ~8+ agents plus the navmesh overlay, this is comfortably under cap; verify the observed count and, only if it is not, raise the cap. This makes AC1's "no debug-segment overflow" a checked property, not a spot-check.

All Task 1 code (the emit pass, the new `DiagnosticAction` variant and flags, the snapshot's geometry half) is `dev-tools`-gated so a build without the feature carries none of it (AC6).

### Task 2: Screen-space labels
Add a binary-side world→screen projection helper (input: world position + the frame's camera view-proj + viewport size in egui logical points; output: optional screen position in egui logical points with a top-left origin, `None` for behind-plane/out-of-viewport) and a pure label-assembly function building the per-agent string from FSM state label, XZ speed, and steering flags (read from `BrainComponent` + `agent_steering::path_state`).

Label DATA (projected screen position + assembled string) is gathered in the shared pre-egui snapshot pass described in Task 1 — one registry iteration, before the `run_ui` closure (`main.rs:2435`) so the strings and positions exist when egui paints. Task 2 is thus the label half of Task 1's single per-agent read pass; there is no second iteration. (The geometry half is emitted later, after `clear_debug_lines`; only the label half is consumed inside `run_ui`.) Paint labels from that snapshot through the existing egui context as a transparent full-screen layer during the same UI pass that draws the Diagnostics panel, gated on the labels-enabled per-layer flag — which the binary reads out of the renderer-diagnostics state that Task 1 stores the layer flags in (same accessor the panel checkboxes bind to). Unit-test both helpers. All Task 2 code is `dev-tools`-gated (AC6).

### Task 3: Agents panel tab
Add an `Agents` variant to `DiagnosticsTab` and a `draw_agents_tab` in `crates/renderer/src/render/debug_ui/mod.rs`: per-layer overlay checkboxes wired to the Task 1 flags, and a live agent list (id, state, speed, flags). Adding the variant requires bumping `const ALL: [Self; 4]` to `[Self; 5]` and the `label` match (`debug_ui/mod.rs:56`) and updating the tab tests at `:915`/`:923` in this same change. The navmesh regions/portals checkbox binds to the existing `nav_overlay_enabled` state (its getter/toggle at `renderer_diagnostics.rs:417`/`:425`) — not a new flag — so it reflects and drives the same value as the `Alt+Shift+N` chord. Agent rows are fed from the binary as plain strings/values (renderer stays game-type-free) — pass a prepared row list into the panel draw call, following the `FrameTimingSnapshot` precedent. All Task 3 code is `dev-tools`-gated (AC6).

### Task 4: Movement-feel fixture map
Author `content/dev/maps/movement-feel.map` with the five stations (pillar wedge, corridor corners, straight run, arena ring with rim spawns, narrow doorway) using existing entity classnames only: `player_spawn` (player start), `reference_enemy` (enemy spawns), `light_spot` (lights) — copy the exact key/value spelling from `content/dev/maps/combat-demo.map` and `sdk/TrenchBroom/postretro.fgd`; do not invent classnames or keys. Plus `movement-feel.README.md` documenting each station and the playtest question it answers, including the AC1 debug-segment overflow check.

Recommended authoring method: do NOT hand-edit brush-plane triples — `combat-demo.map`'s 219 lines of raw planes are exactly the failure mode to avoid. Write a small disposable generator (a throwaway script/tool) that emits the `.map` programmatically — parametric axis-aligned box brushes per station, entities appended — then compile the emitted `.map` through `prl-build`. The generator is scaffolding, not a deliverable; the committed artifacts are the `.map`, its `.README.md`, and (built on demand) the `.prl`. Verify it compiles through `prl-build` with a navmesh present and no leak.

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 4 — independent (code vs content).
**Phase 2 (concurrent):** Task 2, Task 3 — Task 2 is the label half of Task 1's single shared pre-egui per-agent read pass (one registry iteration produces both the geometry snapshot and the label snapshot); Task 3 consumes Task 1's layer flags. They touch different files (binary vs renderer panel). Because Task 2's labels share Task 1's read pass, land Task 1's snapshot pass first, then Task 2/Task 3 concurrently on top of it.

## Open questions

- **Label density at wave scale.** A dense wave's overlapping labels may be unreadable; if so, cap labels to the N nearest agents or add a panel slider. Decide during the first manual check — not up front.
- **Row-feed shape for the Agents tab.** Prepared-rows (binary assembles strings) is the boundary-clean default; if the panel later needs interaction per agent (select-to-highlight), a small ID-keyed struct may replace strings. Defer until a consumer needs it.
