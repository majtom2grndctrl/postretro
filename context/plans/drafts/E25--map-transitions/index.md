# Epic 25 — Map Transitions (Epic hub)

> **Status:** draft — epic index. Records cross-cutting decisions and each unit's contract; no task decomposition. Each unit gets its own brief, drafted when the unit comes up. U1 is the existing `drafts/level-load-pipeline/`.
> **Research:** `research.md` — session-verified facts, the swap lifecycle, rejected shapes.
> **Related:** `context/lib/boot_sequence.md` §1, §2, §4, §8 · `context/lib/movement.md` §6 (Teleport) · `context/lib/entity_model.md` §7, §7d · `context/lib/rendering_pipeline.md` §2, §4 · `context/lib/networking.md` §Session-state ledger, §Admission and content parity, §Slot lifecycle · `context/lib/build_pipeline.md` §Custom FGD · `context/plans/large-map-spatial-residency.md` · `context/plans/done/E17--doors-as-occluders/` · roadmap Epic 15.

## Goal

Players walk from one map into the next as if into the next room. A **seam** is a stretch of level built identically in two maps, with a landmark at the same spot in each. The next map loads in the background while the current one plays. When no player in the seam can see anything outside it, the engine swaps maps and carries every player through the landmark. Doors, bends and airlocks are content that keep a seam blind; the engine needs no knowledge of which one an author built.

## Scope

### In scope

- **U1 Off-main level load** — the staged load pipeline of `drafts/level-load-pipeline/`, amended so it can run while a level plays.
- **U2 Landmark level change** — the seam author surface and a classic transition through it: loading screen, landmark-relative arrival, carried state, co-op arrival.
- **U3 Seamless seam** — prefetch, background load during play, blind swap, door wait and its events. Co-op clients take the classic path.
- **U4 Co-op seamless seam** — clients prefetch on host instruction and cross with no loading screen and no visible hold.

### Out of scope

- **Drawing a non-active level.** A straight hallway or a vista that sees into the next map needs two worlds in one frame. That capability belongs with large-map residency (`context/plans/large-map-spatial-residency.md`), which already owns "seeing more world than is installed". Nothing here forecloses it: U1 and U3 already make a second level resident.
- **Revisited-map state.** Planted separately in the roadmap (Future › Gameplay systems).
- **Non-player entities crossing.** Enemies following through a seam, dropped items carried across. Players only.
- **Co-op players straddling a seam with both levels simulated.** One level simulates at a time; the swap moves everyone.
- **A second, seamless-only load pipeline.** One pipeline serves both orders (D1).

## Direction

**Problem.** The engine has one level change, and it is a hard cut: `drain_level_requests` unloads the current level before the next one starts loading, the install freezes the main thread for one frame of about 0.7–1.0 s, and the player always arrives at a player start. Nothing lets a level change preserve where the player is or hide that it happened.

**Prior commitments.**
- `boot_sequence.md` §8 lists "multiple simultaneously resident levels or streaming" as a non-goal. U3 reverses it for exactly one prefetched, unpublished level beside the active one. The argument: the alternative to a resident next map is a load at the moment of crossing, which is the hitch this epic exists to remove. Rewritten at U3 promotion.
- `drafts/level-load-pipeline/` kept "unload-first" and listed spawn-before-unload as a non-goal. It is amended here to keep unload-first as the loading-screen order only, so its stages may run before an unload (D1).
- Large-map residency keeps "one authored level, one logical PRL". This epic keeps it: each map stays its own PRL and catalog entry. The two efforts meet only at the open-vista capability, which is out of scope.
- The session-state ledger (`networking.md`) says what survives a level change is an explicit, enumerable set. A seam swap carries exactly that set plus the landmark-relative pose (I3).
- Teleporters ship classic and seamless modes, and seamless mode's landmark → target transform is one pure `foundation` function shared with ghost links (`movement.md` §6, `entity_model.md` §7d). Seam arrival is that transform with the source and target in different maps (D4).
- Doors already seal portals: `doors-as-occluders` bakes which portals each mover's closed brush seals, and the runtime walk blocks them while the mover is docked closed. The blind check reuses that walk (D6).
- The runtime visible set is a conservative superset (`rendering_pipeline.md` §2). A cell absent from it is not visible, which is the direction the blind check needs (D6).
- Map authority is server-owned; a client never asks for a level change (`networking.md`). The host owns every swap (I6).
- **"Nothing waits on residency in play"** (`boot_sequence.md` §1; `done/sh-probe-streaming/` "no door waits on async residency"). D7 diverges: the swap, a gameplay event, holds on the next map's load. The argument: residency misses have a fallback (the ambient floor), but a missing level has none short of a loading screen, which D8 keeps for failure. The wait is bounded by the prefetch and is the author-presented door, not an invisible stall.
- **"Every level entry passes from install into Settling, never straight to Running"** (`boot_sequence.md` §1). Settling also drives the network revealed term, the reveal edge, accumulator re-arm and level audio at reveal. U3's pre-commit settle moves that settling before the swap; the swap frame must still publish everything Settling's exit publishes, including the host's reveal of the new level, or classic-path clients never re-promote. Rewritten at U3 promotion.
- **"Participation is a predicate, not a pair of transitions"** (`networking.md` §Slot lifecycle); a level change demotes every participating slot. U4 must keep the predicate: the outcome it owes is no loading screen and a bounded held window, not a new lifecycle path.
- **"Seam" already names a residency concept.** `streaming_seam_volume` (`build_pipeline.md` §Custom FGD) marks a best-effort load-hide point at a portal. A map-transition seam often sits at the same place. The U2 brief either relates the two or renames one.

**Alternatives rejected.**
- **Grow one map instead of joining two.** Large-map residency could in principle make a whole campaign one PRL. Rejected as this epic's vehicle: it streams baked lighting only today, everything else loads whole, and one PRL per campaign makes every edit a whole-campaign compile. Maps as units match how mods author and catalog content.
- **Metroid Prime's door-press load.** Start loading when the door is activated. Simple, and it is the hesitation the owner wants gone. Kept only as the fallback when a prefetch has not finished (D7).
- **Stop after U1 and U2.** A Half-Life-style landmark change keeps pose and loadout, and U1 shortens its loading screen. Rejected: it keeps a visible break, and removing that break is an owner decision (2026-10-28). It remains the fallback (D8).
- **Two pipelines, seamless and classic.** Each would own an install order, failure path, cancellation and parity publication, and they would drift. Rejected for D1.
- **Swap check at compile time only.** prl-build could mark blind cells once. Doors move, so a cell's blindness depends on runtime mover state, and the runtime walk already folds that in. The compiler keeps a warning (D6), not the decision.

## Cross-cutting decisions

- **D1 — One load pipeline, two commit orders.** Loading-screen order: unload A → load B → commit → settle → reveal. Seam order: load B while A runs → hold B ready and unpublished → swap commit (unload A, commit B) on one frame. U1 builds the pipeline; it must not assume the previous level is gone before its stages run.
- **D2 — One active level.** Exactly one level simulates and draws on any frame. A prefetched level is CPU and GPU data held off the frame path until its swap commit (I1, I2).
- **D3 — At most one prefetched level.** A second seam's prefetch supersedes the first, which is released. Bounds peak residency at two levels.
- **D4 — Seam arrival is the seamless-teleport transform.** Body offset from the landmark (vertical included), view yaw and every velocity channel rotate onto the matching landmark in the next map, through the same `foundation` function teleports and ghost links use. A blocked arrival or a missing matching seam falls back to player starts with one warning.
- **D5 — The engine moves every player.** Players inside the seam arrive landmark-relative; anyone outside arrives at the next map's player starts through ordinary placement. Whether to wait for everyone is mod policy: the seam fires on its activation, and mods can gate activation on occupancy.
- **D6 — Blind swap.** A seamless seam swaps only when, for every player inside it, an all-directions walk from the eye under current door state reaches no cell outside the seam. A walk that falls back to frustum culling counts as not blind. prl-build warns when no cell of a seam can ever be blind with its sealing movers closed. U3's brief pins the walk's form and its cost on the host.
- **D7 — The door waits, and says so.** Activating a seamless seam before its prefetch is ready, or while it is not blind, holds the swap. The seam fires a **waiting** event once per activation and a **ready** event when the swap can proceed. Presentation is the author's: a pulsing light is two reactions through the shipped `setLightAnimation`. Wiring nothing means the door simply waits. Waits are counted in diagnostics.
- **D8 — A failed prefetch never strands the player.** A prefetch that fails, or a seam activated with no prefetch started, takes the classic path with a loading screen and one warning.
- **D9 — Defaults for beginners, tools for advanced authors.** The engine and tools do something sensible when an author places only the minimum, and give advanced authors explicit overrides:
  - **Prefetch region.** prl-build generates one by portal-graph distance from the seam; an authored region replaces it. U3's brief sizes the default distance from measured background load time and sustained movement speed.
  - **Landmark.** Defaults to the seam volume's bounds centre at yaw 0; an explicit landmark point overrides it.
  - **Blind check.** Automatic (D6); the compiler warning tells a beginner their seam cannot swap seamlessly.
  This principle lands in `context/lib/` at hub promotion; no library doc states it today.
- **D10 — Seam modes mirror teleporter modes.** One seam entity carries `classic` or `seamless`. Classic is U2; seamless is U3. A seamless seam degrades to classic under D8.
- **D11 — Arrival latches the seam.** A player arriving inside the next map's matching seam is latched against it until their body point leaves, as teleporter arrival latches (`movement.md` §6). A two-way seam therefore never bounces a player straight back.

## Units

**Brief inputs.** Each unit's brief author reads, besides the unit's own section: Goal, Direction, Cross-cutting decisions, the full Acceptance criteria, Invariants, Pinned orderings, Boundary inventory and `research.md`.

### U1 — Off-main level load

**Purpose.** Move a level's CPU preparation and GPU creation off the main thread and shrink the install to a short commit. Every level change benefits; U3 cannot ship without it, because a one-second commit at a seam is the hitch the epic removes.

**Contents.** `drafts/level-load-pipeline/` as amended alongside this hub: decisions reveal-gate already delivered are dropped; unload-first becomes the loading-screen order; stages may run before an unload; "one level's world data resident at a time" is gone.

**Depends on.** Nothing. Runs concurrently with U2.

**Key acceptance.** AC 1–2.

**Brief.** `context/plans/drafts/level-load-pipeline/` (resumable).

### U2 — Landmark level change

**Purpose.** The seam author surface and the transition contract, proven with a loading screen before any background loading exists.

**Contents.**
- Seam brush entity with a target map (catalog id), a seam name shared across both maps, an activation of touch or use as `trigger_volume` has, a mode (D10), and an optional landmark point (D9).
- Activation runs a classic level change carrying the seam name; arrival applies D4 and D5; carried state is unchanged (I3).
- Arrival latch (D11).
- Co-op: the host's relevel carries the seam; every client's pawn arrives by D4 or D5 from the host's placement.
- A dev-mod two-map pair joined by a seam, catalogued, playable both ways.
- Whether `loadLevel` gains an optional seam argument for scripted transitions: brief decides.

**Depends on.** Nothing. Runs concurrently with U1.

**Key acceptance.** AC 3–8.

**Brief form.** Compact brief (`/draft-brief`). The FGD surface is modder-facing; its names are pinned by the brief and reviewed before code.

### U3 — Seamless seam

**Purpose.** The seam swap, single-player first.

**Contents.**
- Prefetch region: generated by prl-build, authored override (D9). Entering it starts the next map's load in seam order (D1). D3 bounds it.
- Pre-commit settle: the arrival view's SH and lightmap settle set is read into the prefetched level before the swap, so the first frame after the swap draws settled light.
- Blind check (D6) and the prl-build warning.
- Swap commit on one frame: unload A, commit B, arrival by D4 and D5, latch by D11.
- Door wait and **waiting** / **ready** events (D7); failure fallback (D8).
- Diagnostics: prefetch start and ready times, waits, peak resident bytes with two levels.
- `boot_sequence.md` §8 rewritten.
- Co-op: clients take the classic relevel (loading screen) at a host swap until U4.
- Dev-mod content: a door airlock pair and a bent-hallway pair, plus a reference door that pulses a light while waiting.

**Depends on.** U1 (off-main pipeline), U2 (seam surface and arrival).

**Key acceptance.** AC 9–18.

**Brief form.** Resumable brief. The swap lifecycle crosses loader, renderer, sim and streaming seams; checkpoint the prefetch, the swap commit and the blind check as stages.

### U4 — Co-op seamless seam

**Purpose.** Co-op clients cross a seam with no loading screen and no visible hold.

**Contents.**
- The host tells participating clients to prefetch when it does; a client prefetch is speculative and commits only on the host's swap (I6).
- The host's swap waits for every participating client to report ready, bounded by a timeout after which a late client takes the classic relevel.
- Participation stays the one predicate (`networking.md` §Slot lifecycle). A swap demotes and re-promotes like any level change; what U4 owes is player-visible: no loading screen and a bounded hold that prediction hides.
- The client reports readiness for its prefetched level separately from its installed-level declaration, so the report never fails the predicate while the host is still on the old map. With every report in hand, the host demotes at unload and re-promotes at commit on its swap frame.
- The entry rule's pawn spawn takes the seat's carried arrival pose (D4 or D5) instead of its assigned `player_spawn` when one exists. Network identity re-baselines; the client predicts its pawn from the arrival pose until the host's baseline lands, with a teleport-style cut.
- A dedicated swap path that keeps slots participating is reopened only if measurement at realistic latency shows the hold is visible.
- The relevel-at-request non-goal in `drafts/level-load-pipeline/` (a failed host parse would strand clients) is answered by the speculative prefetch: a client never commits a level the host did not.

**Depends on.** U3.

**Key acceptance.** AC 19–23.

**Brief form.** Resumable brief; it changes the slot lifecycle.

## Acceptance criteria

### Automated

**U1**
1. The `level-load-pipeline` brief's own acceptance list, as amended.
2. A load's stages can run while a level is installed and running, and nothing they create reaches the frame path before commit.

**U2**
3. On a fixture pair, a player crossing a classic seam arrives in the next map with the same offset from the landmark (vertical included), the same view yaw relative to it, and every velocity channel rotated by the landmark yaw delta. The result matches the seamless-teleport transform for the same poses.
4. Carried state crosses a seam exactly as it crosses `loadLevel` today.
5. Touch activation and use activation each fire the transition; use without overlap does not.
6. A destination with no matching seam, and a blocked arrival pose, each fall back to player starts with one warning. A seam with no explicit landmark uses its bounds centre.
7. An arriving player is latched against the destination seam until leaving it; re-entering fires it.
8. Co-op: a client pawn inside the seam arrives landmark-relative, and one outside arrives at a player start.

**U3**
9. Entering a generated prefetch region starts the next map's load; an authored region replaces the generated one, and a map that places neither still gets one.
10. Activating a seamless seam after its prefetch is ready swaps on that frame with no loading screen and no Loading state, and the next frame draws the next map.
11. The swap frame and the frames around it stay under the frame budget the brief pins on the reference fixtures.
12. The first frame after a swap draws the arrival view with SH and lightmap settled; no ambient-floor or miss fallback is drawn on it.
13. Activation before the prefetch is ready fires **waiting** once and holds the swap; **ready** fires once when the prefetch completes, and the swap follows. Activation after ready fires neither.
14. With the seam's door open, a swap waits; with it docked closed, the swap proceeds. A bent-hallway seam swaps from its blind cells and not from a cell that sees past a bend.
15. A walk that falls back to frustum culling never permits a swap.
16. prl-build warns on a seam that can never be blind and stays silent on one that can.
17. A failed prefetch, and an activation with no prefetch started, each take the classic path with one warning.
18. Peak resident CPU and GPU bytes with two levels are reported per `testing_guide.md` §Resource bounds and stay within the 2 GB VRAM target on the reference fixtures.

**U4**
19. Clients prefetch when the host does, and a client prefetch never commits without the host's swap.
20. At a host swap, a ready client crosses with no loading screen. At the reference simulated latency its input is held for no more than the tick bound the U4 brief pins, and its view never freezes: its pawn is predicted from the arrival pose (D4 or D5) until the host's baseline lands.
21. A client not ready by the timeout takes the classic relevel; the others still swap seamlessly.
22. A host whose prefetch fails takes D8, and clients follow the classic relevel.
23. A ready client's slot leaves and re-enters participating on the host's swap frame, through the same predicate as any level change. Its readiness report never demotes it while the host is still on the old map.

### Manual

- Playtest the airlock pair and the bent-hallway pair, single-player and co-op at simulated latency. Watch for a lighting or audio pop at the swap; the seam's two bakes can differ (Open questions).
- Outrun a prefetch on purpose and confirm the door's wait reads as the door powering up.
- Metal: swap-frame timing and peak memory on the reference pairs.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| I1 — Exactly one level simulates and draws per frame | U3 swap commit | Prefetch install; streaming installs for the prefetched level | AC 2, 10 |
| I2 — No frame mixes two levels' world data | U1 commit; U3 swap | Pre-commit settle writing into shared pools; renderer resources keyed globally rather than per level | AC 2, 12 |
| I3 — A seam carries the session-state ledger plus the landmark pose, nothing more | U2 | U3 swap path diverging from the classic harvest | AC 4, 20 |
| I4 — Arrival pose comes from the one landmark → target transform | U2 | A seam-specific copy of the math | AC 3 |
| I5 — At most one prefetched level | U3 | A second prefetch region; a level change during prefetch | AC 9, Pinned orderings |
| I6 — The host owns every swap; a client never commits a level the host has not | U2 (relevel), U4 | Client prefetch completing first | AC 19, 21 |
| I7 — A seam never strands a player | U3 | Prefetch failure, cancellation, suspend | AC 17, 22 |
| I8 — Participation changes only through the one predicate | Existing slot lifecycle | U4 readiness report; any seam-specific swap path | AC 23 |

## Pinned orderings

| Scenario | Ordering | Expected outcome |
|---|---|---|
| Activation before prefetch ready | activate → (load) → ready | **waiting** once, then **ready** once, then swap |
| Prefetch completes on the activation frame | ready, activate same frame | Swap; no **waiting** |
| Two activations on one tick | activate × 2 | One swap |
| Player leaves the prefetch region before activating | enter → leave | Prefetched level stays until superseded or a level change |
| Second seam's region entered | prefetch A→B ready → enter A→C region | B released, C prefetched (D3) |
| Level change during prefetch | prefetch → `loadLevel` / `restartLevel` / return to frontend | Prefetch cancelled; the requested change proceeds |
| Suspend during prefetch | prefetch → suspend | Prefetch cancelled; resume does not restore it |
| Prefetch fails after activation | activate → **waiting** → failure | Classic path with one warning (D8); **ready** never fires |
| Door opened while waiting | activate → **waiting** → door opens | Swap holds until blind again |
| Arrival inside the return seam | swap → arrival inside B→A seam | Latched (D11); B→A prefetch may start, the swap cannot fire until the player leaves and re-enters |
| Player dies inside a waiting seam | activate → **waiting** → death | Open (Open questions) |

## Boundary inventory

Leanings, pinned by the U2 brief.

| Concept | FGD | Notes |
|---|---|---|
| Seam volume | brush entity, e.g. `level_seam` | keys: target map catalog id, seam name, activation (`touch` / `use`), mode (`classic` / `seamless`), optional landmark name |
| Landmark | point entity; reuse `teleport_destination` as ghost links do, or a new name | absent → bounds centre, yaw 0 |
| Prefetch region | brush entity naming its seam | absent → generated by prl-build |
| Events | `waiting`, `ready`, and an arrival event on the destination seam | reach reactions through the trigger event path |

## Open questions

- **Lighting parity across a seam.** D6 checks that a seam is blind to sight, not to light: a bend unsealed against outside light pops at the swap. Lean: a per-map prl-build warning when any light outside the seam reaches its cells, directly or through SH; two identical copies that both pass bake alike. U3 brief.
- **Death while a seam waits.** Whether a dead or downed pawn (E18 player life states) crosses, blocks the swap, or arrives at a player start — U2 brief, against the life-states spec.
- **Cost of the blind walk on the host.** One all-directions walk per player in a seam per tick while activated. Measured in U3.
- **Commit-frame cost after U1.** The script VM and registry stages stay on the main thread. If they exceed the swap budget, U3 needs its own lever.
- **How long a prefetched level stays.** Pinned orderings keep it until superseded, so the two-level peak can last the rest of the map. A release on leaving the prefetch region, with hysteresis, is the alternative — U3 brief, against AC 18.
- **Two-level peak memory.** Unmeasured. World textures alone are about 80–96 MiB per level (`drafts/level-load-pipeline/research.md`).
- **Audio across the swap.** Level sounds reload per level; ambient loops cut at a swap unless carried or crossfaded — U3 brief.
