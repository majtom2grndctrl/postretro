# E10 — Agent Diagnostics (all-agent overlay + movement-feel fixture map)

> **Wave:** E10 enemy-AI follow-up. **Runs first** — before steering feel, stuck recovery, and combat positioning — because every one of those specs tunes against "how does the agent move," and this spec is the instrument that makes that observable. Later specs extend the overlay with their own fields (stuck counter, combat-slot scores) as they land; this spec ships the substrate.
>
> **Builds on (all `dev-tools`-gated, already shipped):** the immediate-mode `DebugLineRenderer` (`crates/renderer/src/render/debug_lines.rs` — lines, AABBs, markers, capsules; depth-tested + x-ray overlay pipelines), the navmesh region/portal overlay (`crates/postretro/src/render/nav_diagnostics.rs`), the single-agent path overlay `emit_agent_path_overlay` (`crates/renderer/src/render/renderer_diagnostics.rs:339`, currently drawn only for the `debug_chase_agent`), and the egui Diagnostics panel (`crates/renderer/src/render/debug_ui/mod.rs`, tabbed via `DiagnosticsTab`).

## Goal

Make enemy movement observable: a per-agent debug overlay (path corridor, waypoints, velocity vector, destination marker, state label) for **every** live agent, an Agents tab in the Diagnostics panel, and a dedicated movement-feel fixture map — so tuning the steering/animation/positioning specs is a look-and-see loop instead of rebuild-and-squint.

## Background (what exists, what's missing)

- `emit_agent_path_overlay` already draws the remaining path corridor plus per-waypoint cross markers — but only for the one `Alt+Shift+G` `debug_chase_agent` (`main.rs:2502-2507`), gated on the navmesh toggle (`Alt+Shift+N`).
- The read seam is established: the binary borrows the registry after game logic, reads `AgentComponent` (`crates/entities/src/components/agent.rs:35-95` — `path`, `waypoint_cursor`, `velocity`, `destination`, `arrived`, `blocked`) plus `Transform`, translates to plain `Vec3` segments, and calls renderer `push_debug_line*` emitters. Renderer API never names game/nav types — that boundary rule holds for everything here.
- No text primitive exists in the debug-line path. World-anchored labels come from egui screen-space text at projected world positions — a small binary-side projection helper, **not** a new GPU text pass.
- No dedicated movement fixture map exists. `content/dev/maps/combat-demo.map` (231-line arena with pillars) is the closest precedent; feel work needs purpose-built stations.
- Frame order: overlays are emitted after game logic, before the render pass, alongside the existing `emit_*` diagnostics calls (`main.rs:2463-2508`); `clear_debug_lines` owns the per-frame buffer reset.

## Scope

### In scope

- **All-agent overlay.** Generalize the chase-agent block into an emit pass over every entity carrying `AgentComponent`: remaining path corridor + waypoint markers (reusing the existing emit shape), a velocity vector (line from position to position + velocity, distinct color), and a destination marker (`push_marker` at `destination` / `planned_destination`).
- **State labels.** Screen-space egui text near each agent's head: FSM state (`BrainComponent::state.label()`), XZ speed, and steering flags (`arrived` / `blocked` / `has_path`). Label content is a plain string assembled at the read site so later specs (stuck recovery, combat positioning) append fields without touching the projection or draw path. A world→screen projection helper (camera view-proj from the frame's camera state) returns `None` for behind-camera / off-screen positions; labels for those agents are skipped.
- **Agents panel tab.** New `DiagnosticsTab` variant with: a live list of agents (id, state, speed, flags) and per-layer overlay toggles (paths, velocities, destinations, labels, navmesh regions/portals). A new `DiagnosticAction` chord toggles the agent overlay as a whole; panel checkboxes refine layers. The existing `Alt+Shift+N` navmesh toggle keeps its behavior.
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
- [ ] The Diagnostics panel has an Agents tab listing every live agent (id, state, speed, flags) with working per-layer toggles: disabling a layer removes exactly that layer's geometry next frame (manual check).
- [ ] The agent overlay has its own diagnostic chord, independent of the navmesh toggle; the navmesh toggle's existing behavior is unchanged (manual check; existing diagnostics-chord tests remain green).
- [ ] `movement-feel.map` compiles clean through `prl-build` (navmesh present, no leak), loads in the engine, and contains the five named stations with at least one enemy spawn and a player start; the README documents each station and its target playtest question (manual check).
- [ ] A build without the `dev-tools` feature compiles with none of the new overlay/panel code present (`cargo check -p postretro` without the feature stays green).

## Tasks

### Task 1: All-agent overlay emit pass
In the binary's diagnostics emit block (alongside the existing `emit_*` calls after game logic), replace the single `debug_chase_agent` overlay call with a pass iterating every entity with `AgentComponent`: read `Transform` + `AgentComponent` (`path`, `waypoint_cursor`, `velocity`, `destination`, `planned_destination`), translate to `Vec3` segments, and emit corridor/waypoints (existing `emit_agent_path_overlay` shape), velocity vectors, and destination markers through the renderer's `push_debug_line`/`push_marker` surface. Renderer signatures stay game-type-free. Gate the pass on a new `DiagnosticAction` variant (chord in `crates/postretro/src/input/diagnostics.rs`, following the `SpawnChaseAgent` precedent) plus per-layer boolean flags stored where the panel can reach them (renderer diagnostics state, following the `nav_overlay_enabled` precedent). The `debug_chase_agent` spawn chord is untouched; its agent is simply one of the iterated agents.

### Task 2: Screen-space labels
Add a binary-side world→screen projection helper (input: world position + the frame's camera view-proj + viewport size; output: optional screen position, none for behind-plane/out-of-viewport) and a pure label-assembly function building the per-agent string from FSM state label, XZ speed, and steering flags (read from `BrainComponent` + `agent_steering::path_state`). Paint labels through the existing egui context as a transparent full-screen layer during the same UI pass that draws the Diagnostics panel. Unit-test both helpers.

### Task 3: Agents panel tab
Add an `Agents` variant to `DiagnosticsTab` (plus its `ALL`/`label` entries) and a `draw_agents_tab` in `crates/renderer/src/render/debug_ui/mod.rs`: per-layer overlay checkboxes wired to the Task 1 flags, and a live agent list (id, state, speed, flags). Agent rows are fed from the binary as plain strings/values (renderer stays game-type-free) — pass a prepared row list into the panel draw call, following the `FrameTimingSnapshot` precedent.

### Task 4: Movement-feel fixture map
Author `content/dev/maps/movement-feel.map` with the five stations (pillar wedge, corridor corners, straight run, arena ring with rim spawns, narrow doorway) using existing entity classnames only (player start, reference enemy, lights), plus `movement-feel.README.md` documenting each station and the playtest question it answers. Verify it compiles through `prl-build` with a navmesh and no leak.

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 4 — independent (code vs content).
**Phase 2 (concurrent):** Task 2, Task 3 — Task 2 consumes Task 1's per-agent read pass; Task 3 consumes Task 1's layer flags; they touch different files (binary vs renderer panel).

## Open questions

- **Label density at wave scale.** Sixteen overlapping labels may be unreadable; if so, cap labels to the N nearest agents or add a panel slider. Decide during the first manual check — not up front.
- **Row-feed shape for the Agents tab.** Prepared-rows (binary assembles strings) is the boundary-clean default; if the panel later needs interaction per agent (select-to-highlight), a small ID-keyed struct may replace strings. Defer until a consumer needs it.
