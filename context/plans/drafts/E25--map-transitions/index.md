# Epic 25 — Map Transitions (Epic hub)

> **Status:** draft — epic index. Records cross-cutting decisions and each unit's contract; no task decomposition. Each unit gets its own brief, drafted when the unit comes up. U1 is the existing `drafts/level-load-pipeline/`.
> **Research:** `research.md` — session-verified facts, the swap lifecycle, rejected shapes.
> **Related:** `context/lib/boot_sequence.md` §1, §2, §4, §8 · `context/lib/movement.md` §6 (Teleport) · `context/lib/entity_model.md` §5, §7, §7d · `context/lib/rendering_pipeline.md` §2, §4 · `context/lib/networking.md` §Session-state ledger, §Admission and content parity, §Slot lifecycle, §What gates · `context/lib/build_pipeline.md` §Custom FGD · `context/lib/scripting.md` §12 · `context/plans/large-map-spatial-residency.md` · `context/plans/done/E17--doors-as-occluders/` · roadmap Epic 15.

## Goal

Players walk from one map into the next as if into the next room. A **seam** is a stretch of level built identically in two maps, with a landmark at the same spot in each. The next map loads in the background while the current one plays. When no player in the seam can see anything outside it, the engine swaps maps and carries every player through the landmark. Doors, bends and airlocks are content that keep a seam blind; the engine needs no knowledge of which one an author built.

## Scope

### In scope

- **U1 Off-main level load** — the staged load pipeline of `drafts/level-load-pipeline/`, amended so it can run while a level plays.
- **U2 Landmark level change** — the seam author surface and a classic transition through it: loading screen, landmark-relative arrival, carried state, co-op arrival.
- **U3 Seamless seam** — prefetch, background load during play, blind swap, door wait and its events. Co-op clients take the classic path.
- **U4 Co-op seamless seam** — clients prefetch on host instruction and cross with no loading screen and a bounded hold.

### Out of scope

- **Drawing a non-active level.** A straight hallway or a vista that sees into the next map needs two worlds in one frame. No plan owns that capability today. Large-map residency (`context/plans/large-map-spatial-residency.md`) is its nearest home, but that plan streams baked resources inside one logical PRL and never draws a second level. This epic forbids it by I2, so a later spec that draws a vista revises I2; U1 and U3 leave it a second resident level to draw from. Planted in the roadmap (Future › Rendering and visual polish).
- **Revisited-map state.** Planted separately in the roadmap (Future › Gameplay systems).
- **Non-player entities crossing.** Enemies following through a seam, dropped items carried across. Players only. At the swap, a non-player entity inside the active seam vanishes with its level unless the U3 brief makes a hostile one hold the swap (Open questions), and the next map's seam shows that map's own placements; the blind check hides neither. The U3 reference pairs keep their seams free of placements that differ between copies, and the manual playtest watches for an entity pop.
- **Co-op players straddling a seam with both levels simulated.** One level simulates at a time; the swap moves everyone.
- **A second, seamless-only load pipeline.** One pipeline serves both orders (D1).

## Direction

**Problem.** The engine has one level change, and it is a hard cut: `drain_level_requests` unloads the current level before the next one starts loading, the install freezes the main thread for one frame of about 0.7–1.0 s, and the player always arrives at a player start. Nothing lets a level change preserve where the player is or hide that it happened.

**Prior commitments.**
- `boot_sequence.md` §8 lists "multiple simultaneously resident levels or streaming" as a non-goal. U3 reverses it for exactly one prefetched, unpublished level beside the active one. The argument: the alternative to a resident next map is a load at the moment of crossing, which is the hitch this epic exists to remove. Rewritten at U3 promotion.
- `drafts/level-load-pipeline/` kept "unload-first" and listed spawn-before-unload as a non-goal. It is amended here to keep unload-first as the loading-screen order only, so its stages may run before an unload (D1).
- Large-map residency keeps "one authored level, one logical PRL". This epic keeps it: each map stays its own PRL and catalog entry.
- The session-state ledger (`networking.md`) says what survives a level change is an explicit, enumerable set. A seam swap carries exactly that set plus each seam occupant's landmark-relative arrival (I3, D4).
- Teleporters ship classic and seamless modes, and seamless mode's landmark → target transform is one pure `foundation` function shared with ghost links (`movement.md` §6, `entity_model.md` §7d). Seam arrival uses that transform with the source and target in different maps (D4).
- Doors already seal portals: `doors-as-occluders` bakes which portals each mover's closed brush seals, and the runtime walk blocks them while the mover is docked closed. The walk lives in `postretro-visibility`, a CPU crate with no renderer dependency, so the host can run it for any pawn. It has no all-directions entry: the camera entry and the seeded entry both clip to a caller view-projection. The blind check (D6) reuses the blocked-portal state; its all-directions form is a union of camera-entry walks or a new entry, pinned by the U3 brief. Door state reaches the walk as the blocked-portal buffer the frame loop rebuilds from post-tick mover state once per frame.
- The runtime visible set is a conservative superset (`rendering_pipeline.md` §2). A cell absent from it is not visible, which is the direction the blind check needs (D6).
- Map authority is server-owned; a client never asks for a level change (`networking.md`). The host owns every swap (I6).
- **"In play nothing waits on residency"** (`boot_sequence.md` §1; `done/sh-probe-streaming/` "no door waits on async residency"). D7 diverges: the swap, a gameplay event, holds on the next map's load. The argument: residency misses have a fallback (the ambient floor), but a missing level has none short of a loading screen, which D8 keeps for failure. The wait is the author-presented door, not an invisible stall, and it is bounded: a swap held on its prefetch or pre-commit settle past the Settling timeout (10 s, `boot_sequence.md` §1), counted from activation, takes the classic path (D8). A hold on the blind check alone has no bound; the player ends it by closing the door or leaving.
- **"Every level entry — boot map, catalog load, `restartLevel`, frontend backdrop, network relevel — passes from install into Settling, never straight to Running"** (`boot_sequence.md` §1). U3's pre-commit settle moves that settling before the swap, and the swap frame enters Running directly. Some of Settling's exit effects exist because the player saw nothing during the hold; a seam player saw the whole time. The swap frame applies each effect as follows; rewritten at U3 promotion:

  | Settling exit effect | Swap frame |
  |---|---|
  | Host's net reveal of the new level | Published at commit, or classic-path clients never re-promote |
  | Install-queued `levelLoad` commands and level audio | Dispatch on the swap frame |
  | Baked light promoted on the entry frame starts at full weight | Applied |
  | Fixed-step accumulator re-arm | Applied after the commit: the frame's tick count is taken before the commit runs, so the commit's own duration never becomes catch-up ticks in the new level |
  | Interpolation endpoints | Both held at the arrival eye, never blended from A's camera or the unload's reset pose |
  | Input suspension and neutral look | Not applied: held movement and look carry across, or a walking player stops at the seam |
  | Line C `settle_hold` and `first_level_frame` marks; Settling entry log | Replaced by swap marks and one swap log line |
  | Loading screen end, splash clear | Nothing to end |
- **"Participation is a predicate, not a pair of transitions"** (`networking.md` §Slot lifecycle); a level change demotes every participating slot. U4 keeps one predicate and widens one of its inputs; it adds no lifecycle edge (I8).
- **"Seam" already names a residency concept.** `streaming_seam_volume` (`build_pipeline.md` §Custom FGD) marks a best-effort load-hide point at a portal. A map-transition seam often sits at the same place. The U2 brief either relates the two or renames one.

**Alternatives rejected.**
- **Grow one map instead of joining two.** Large-map residency could in principle make a whole campaign one PRL. Rejected as this epic's vehicle: it streams baked lighting only today, everything else loads whole, and one PRL per campaign makes every edit a whole-campaign compile. Maps as units match how mods author and catalog content.
- **Metroid Prime's door-press load.** Start loading when the door is activated. Simple, and it is the hesitation the owner wants gone. Kept only as the fallback when a prefetch has not finished (D7).
- **Stop after U1 and U2.** A Half-Life-style landmark change keeps pose and loadout, and U1 shortens its loading screen. Rejected: it keeps a visible break, and removing that break is an owner decision (2026-10-28). It remains the fallback (D8).
- **Two pipelines, seamless and classic.** Each would own an install order, failure path, cancellation and parity publication, and they would drift. Rejected for D1.
- **Swap check at compile time only.** prl-build could mark blind cells once. Doors move, so a cell's blindness depends on runtime mover state, and the runtime walk already folds that in. Baked visibility is also sampled, so it can miss cells, which is the unsafe direction. The compiler keeps warnings (D6), not the decision.

## Cross-cutting decisions

- **D1 — One load pipeline, two commit orders.** Loading-screen order: unload A → load B → commit → settle → reveal. Seam order: load B while A runs → hold B ready and unpublished → swap commit (harvest, unload A, commit B) on one frame. The swap's unload releases only A's level-scoped state; B's prefetched CPU and GPU data and its pre-commit settle residency survive it. Today's unload clears renderer level resources and level streaming state wholesale, so U1 and U3 key them per level. U1 builds the pipeline; it must not assume the previous level is gone before its stages run.
- **D2 — One active level.** Exactly one level simulates and draws on any frame. A prefetched level is CPU and GPU data held off the frame path until its swap commit (I1, I2).
- **D3 — At most one prefetched level.** A second seam's prefetch supersedes the first, which is released, except while the first's region is occupied or its seam has an open wait or a queued swap (U3 prefetch target table). Bounds peak residency at two levels.
- **D4 — Seam arrival is the seamless-teleport transform.** Body offset from the landmark (vertical included), view yaw and every velocity channel rotate onto the matching landmark in the next map. Two shipped pieces split that work today:
  - `foundation`'s `SeamlessMapping` maps the point and turns vectors. It takes plain poses, so a landmark pose read in map A and a target pose read in map B feed it unchanged.
  - The rest lives only inside the physics teleport chokepoint: the channel list (base, knockback and push velocity, a dash or slide's banked boost), the vault abort, the arrival ground settle and the blocked-arrival fit (`movement.md` §6, Arrival state). The chokepoint fires from the current map's teleporter records within one movement tick, so it cannot run across an unload.

  The movement component holding that state despawns at unload, and carried state does not hold it. So the harvest reads each seam occupant's arrival from the live pawn before unload: body offset from the landmark, view yaw and pitch, the velocity channels and the movement state payload. The next map's spawn applies it through arrival code U2 factors out of the chokepoint, so teleports and seams share one rule (I4). A blocked arrival or a missing matching seam falls back to player starts with one warning; the blocked test runs when the pawn spawns, not at harvest.
- **D5 — The engine moves every player.** Players inside the seam (D11's test) arrive landmark-relative; anyone outside arrives at the next map's player starts through ordinary placement. Whether to wait for everyone is mod policy: a seam has an armed state, as trigger volumes and teleporters do, and a mod's occupancy reaction arms it.
- **D6 — Blind swap.** A seamless seam swaps only when, for every player inside it, an all-directions walk from the eye under current door state reaches no cell outside the seam. The same walk from the mapped eye in the prefetched level, under that level's mover spawn state, must also reach no cell outside its seam, or a door the next map spawns open shows world that was hidden one frame earlier. A walk that trips its step cap or falls back to frustum culling counts as not blind.
  - **Evaluation point.** Activation is a fixed-tick trigger event; the swap is a redraw-boundary commit. Once per frame, after its last fixed tick and after the frame loop rebuilds the blocked-portal buffer from that tick's mover state, the host evaluates each seam activated that frame or holding an open wait (D7): the walks from every inside player's eye at that tick's pose and mover state, the bundle state polled at this frame's redraw boundary, and the pre-commit settle asked for those eyes. A passing evaluation queues the swap, which commits at the next redraw boundary's request drain, before that frame's Input stage. Nothing simulates between evaluation and commit, so the commit carries exactly the poses and door state the check passed. A frame that runs no fixed tick still evaluates.
  - **Compiler warnings.** prl-build warns when no cell of a seam can ever be blind with its sealing movers closed, and when a seam's sealing mover does not spawn docked closed.
  - Mover and light state do not cross the swap: the destination installs its copy at its spawn state, and a light the **waiting** reaction set does not carry.
  - U3's brief pins the walk's form and its cost on the host.
- **D7 — The door waits, and says so.** Activating a seamless seam before its prefetch is ready, or while it is not blind, holds the swap in the seam's one **wait**. Each D6 evaluation applies the table below, first matching row first. Activations while a wait is open join it, so a wait fires **waiting** once and **ready** at most once, on the evaluation that ends it in a swap. Presentation is the author's: a pulsing light is two reactions through the shipped `setLightAnimation`. Wiring nothing means the door simply waits. Waits are counted in diagnostics.

  | Player inside | Prefetch | Blind | Wait open | Outcome |
  |---|---|---|---|---|
  | No | Any | — | Any | No swap, no event; an open wait ends; the next activation evaluates afresh |
  | Yes | Failed, none started, or unready past the Settling timeout from activation | — | Any | Classic path with one warning (D8); an open wait ends; no **ready** |
  | Yes | Ready | Yes | No | Swap; no event |
  | Yes | Ready | Yes | Yes | **ready**; swap; wait ends |
  | Yes | Ready | No | No | **waiting**; wait opens |
  | Yes | Loading | Any | No | **waiting**; wait opens |
  | Yes | Ready | No | Yes | Hold; no event |
  | Yes | Loading | Any | Yes | Hold; no event |

  When more than one seam evaluates in a frame, one seam takes the table and every other seam's activation that frame drops with no event and no D8:

  | Seams evaluating this frame | Takes the table | Every other seam |
  |---|---|---|
  | One | That seam | — |
  | Several; the seam holding the current prefetch is among them with a player inside | That seam | Activation drops; an open wait ends with no event |
  | Several; otherwise | First in map order with a player inside; if none has one, each seam takes the table | Activation drops; an open wait ends with no event |

  "Ready" means the bundle is ready, as polled at the frame's redraw boundary, and the pre-commit settle answers settled for the eyes this evaluation walks from (U3). "Loading" is every other live prefetch: requested and not yet ready, ready with its settle pending, or targeted on a tick this frame and not yet requested (U3). "None started" means no tick has targeted the seam since install. Suspend ends an open wait with no event and drops a queued swap with its prefetch. U4 adds client readiness to the Ready column under its own timeout (U4, Host wait); the Settling timeout in the second row counts only the host's bundle and pre-commit settle, so a hold on client readiness alone never takes D8.
- **D8 — A seam never strands the player.** A prefetch that fails, a seam activated with no prefetch started, and a swap held on its prefetch or pre-commit settle past the Settling timeout from activation each take the classic path with a loading screen and one warning.
- **D9 — Defaults for beginners, tools for advanced authors.** The engine and tools do something sensible when an author places only the minimum, and give advanced authors explicit overrides:
  - **Prefetch region.** prl-build generates one by portal-graph distance from the seam; an authored region replaces it. U3's brief sizes the default distance from measured background load time and sustained movement speed.
  - **Landmark.** Defaults to the seam volume's bounds centre at yaw 0; an explicit landmark point overrides it.
  - **Blind check.** Automatic (D6); the compiler warnings tell a beginner their seam cannot swap seamlessly.
  This principle lands in `context/lib/` at hub promotion; no library doc states it today.
- **D10 — Seam modes mirror teleporter modes.** One seam entity carries `classic` or `seamless`. Classic is U2; seamless is U3. A seamless seam degrades to classic under D8.
- **D11 — One inside test; arrival latches.** A seam has one inside test: capsule overlap with its volume, the test its touch and use activations already apply as a `trigger_volume`. It decides activation, landmark arrival (D5), the eyes the blind check walks from (D6), prefetch occupancy and the latch. Teleporters fire and latch on the body point (`movement.md` §6); a seam activates as a trigger, so it latches as one, and the teleporter latch store, keyed to teleporter indices, does not hold it. Every player arriving inside the matching seam is latched against it before the destination's first fixed tick and stays latched until their capsule leaves it. A two-way seam therefore never bounces a player straight back.

  | Player against the destination seam | Touch seam | Use seam |
  |---|---|---|
  | Arrives overlapping it (body point inside or outside) | Latched; B's first trigger tick sees no entry edge | Latched; use does not fire |
  | Latched, still overlapping | Does not fire | Does not fire, even on use |
  | Latched, stops overlapping | Latch clears; does not fire | Latch clears; does not fire |
  | Unlatched, starts overlapping | Fires | Does not fire |
  | Unlatched, overlapping, presses use | Does not fire again | Fires |
  | Arrives at a player start, not overlapping | Fires on later entry | Fires on use while overlapping |

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
- Seam brush entity with a target map (catalog id), a seam name shared across both maps, an activation of touch or use as `trigger_volume` has, a mode (D10), a start-armed key with script `arm` / `disarm` (D5), and an optional landmark point (D9).
- The seam's name against `streaming_seam_volume`: the brief relates the two or renames one (Prior commitments).
- Activation runs a classic level change carrying the seam name; arrival applies D4 and D5; carried state is unchanged (I3).
- Arrival code factored out of the teleport chokepoint and shared with it (D4, I4).
- The inside test and arrival latch (D11).
- Seam events, dispatched on the host as teleport events are. U2 adds an arrival event on the destination seam; U3 adds **waiting** and **ready**. Trigger events publish only `enter` and `exit`, so seams get their own event source, as teleporters did.
- A seat's arrival is written at every harvest: the landmark-relative arrival (D4) for a seat whose pawn is inside the seam, none for a seat outside it, with no bound pawn, or at a level change that is not a seam's. The seat's first spawn in the next level consumes it, and that level's unload clears an unconsumed one. A respawn never reads it.
- Co-op: a client pawn spawns at participation entry, not at install (`networking.md` §Slot lifecycle, entry rule). The entry rule's pawn spawn takes the seat's stored arrival instead of its assigned `player_spawn` when one exists; a seat with no stored arrival, or one whose arrival is blocked at spawn, takes the assigned spawn (D5). The relevel message is unchanged: the host places every pawn, so no client needs the seam. `networking.md` §Slot lifecycle (entry rule) and §Session-state ledger (the seat's arrival) are rewritten at U2 promotion.
- A dev-mod two-map pair joined by a seam, catalogued, playable both ways.
- Whether `loadLevel` gains an optional seam argument for scripted transitions: brief decides.

**Depends on.** Nothing. Runs concurrently with U1.

**Key acceptance.** AC 3–9.

**Brief form.** Compact brief (`/draft-brief`). The FGD surface is modder-facing; its names are pinned by the brief and reviewed before code.

### U3 — Seamless seam

**Purpose.** The seam swap, single-player first.

**Contents.**
- Prefetch region: generated by prl-build, authored override (D9). A player inside it, by D11's test, including one who spawns or arrives there, keeps its seam's next map loading in seam order (D1). Each fixed tick, before the trigger tick, the host picks the prefetch target, first matching row first:

  | Current prefetch | Regions occupied this tick | Target |
  |---|---|---|
  | Its seam has an open wait or a queued swap | Any | Current kept |
  | Its region is occupied | Any | Current kept |
  | Any | Only regions whose prefetch failed since install | Current kept; a failed seam never retries before a level change |
  | None, or its region unoccupied | One or more | First occupied region in map order, skipping failed ones; supersedes the current (D3) |
  | Any | None | Current kept |

  The frame's last tick's target is requested at the next redraw boundary, so a frame starts at most one prefetch. An activation on the tick that first targets its seam waits (D7) and never takes D8.
- Player options during a held prefetch. U1 reads tiers once at dispatch because options never apply on a Loading frame, but a prefetch runs on Running frames, where they do. A shadow-tier change cancels and restarts the prefetch. A Surface Depth change applies live to the active level and reaches the next map on the swap frame through the live path.
- Pre-commit settle: the SH and lightmap settle set of every arrival eye is read into the prefetched level before the swap, so the first frame after the swap draws settled light. Players move and turn through the seam, and the swap frame applies look after its commit, so the set is each inside player's eye mapped through D4, in all directions, re-derived every frame. The prefetched level's streaming runs beside the active level's, which `boot_sequence.md` §4 forbids today: SH worker-pool generations never overlap, and SH and lightmap reads share one issuer and drain budget. The U3 brief decides whether the two levels share that pool and issuer or each run their own; either way, the active level's in-play drains keep their budget.
- Blind check, its evaluation point and the prl-build warnings (D6).
- Swap commit on one frame: harvest, unload A, commit B, arrival by D4 and D5, latch by D11, Settling exit effects per the Prior-commitments table.
- Door wait and **waiting** / **ready** events (D7); failure and timeout fallback (D8).
- Diagnostics: prefetch start and ready times, waits, peak resident bytes with two levels.
- `boot_sequence.md` §8 rewritten.
- Co-op: clients take the classic relevel (loading screen) at a host swap until U4.
- Dev-mod content: a door airlock pair and a bent-hallway pair, plus a reference door that pulses a light while waiting.

**Depends on.** U1 (off-main pipeline), U2 (seam surface and arrival).

**Key acceptance.** AC 10–21.

**Brief form.** Resumable brief. The swap lifecycle crosses the loader, renderer, sim and streaming boundaries; checkpoint the prefetch, the swap commit and the blind check as stages.

### U4 — Co-op seamless seam

**Purpose.** Co-op clients cross a seam with no loading screen and a hold bounded by AC 23.

**Contents.**
- **Instruction.** The host tells participating clients to prefetch when it does, and tells a slot that enters participating while the prefetch stands. A client prefetch is speculative and commits only on the host's swap (I6). Each instruction names the level and a prefetch generation; a supersede (D3) sends the next generation, and a cancel sends a release.
- **Readiness report.** A new Control message, separate from the client's installed-level declaration and its revealed declaration. It names its generation, carries the prefetched level's parity values, and says the client's pre-commit settle is complete. The host keeps each slot's last report and counts it only for the current generation. A client whose prefetch is cancelled, fails or is released after reporting sends a retraction; suspend retracts too.
- **Host wait.** The host's swap waits for every participating client's report, inside D7's wait: **waiting** and **ready** cover client readiness. The timeout, pinned by the U4 brief, runs from the first evaluation that passes every term but client readiness; on expiry client readiness leaves the Ready column, and the next evaluation that passes the remaining terms, the blind check included, swaps. On the swap frame the host splits the slots once: a slot whose current report has arrived swaps seamlessly, every other participating slot gets the classic relevel, and a report arriving later is not read. With no participating client the swap does not wait.
- **Predicate input.** The predicate stays one comparison; only its client-side input widens. A current-generation report counts as the client's declaration and revealed record for the level it names only when that level is the host's installed level and the report arrived before the host's swap frame. A later declaration or revealed message from the client supersedes it. On its swap frame the host's unload re-evaluates every slot (demote), and the re-evaluation after its commit and reveal reads the reports (re-promote). `networking.md` §Slot lifecycle is rewritten at U4 promotion to name the report as an input.

  | Host installed | Client's newest Control fact | Verdict |
  |---|---|---|
  | A | Declaration and reveal of A | Participating |
  | A | Report or retraction for B, after its A declaration | Participating: it names a level the host has not installed and is not read |
  | B | Current report for B, arrived before the swap frame, parity matching | Participating: the report stands in for declaration and reveal |
  | B | Current report for B, arrived before the swap frame, parity differing | Held with the diverged content cause; the client takes the classic relevel |
  | B | Report for B, arrived after the swap frame, or of a superseded generation | Not read; held until the client declares and reveals B through the classic relevel |
  | B | Declaration and reveal of A, no report | Held; the client takes the classic relevel |
  | B | `Revealed(None)` or a retraction after a report for B | Held |
  | B | Declaration and reveal of B after its own swap | Participating |
- **Client across the swap.** Re-promotion on the host's swap frame spawns the pawn at the seat's stored arrival (U2's entry rule). The swap message follows on Control and carries that arrival. The client commits its prefetch at its next redraw boundary with no Settling, then declares B's parity and its reveal of B. The client's ticks on A after the host's harvest are discarded. Its view cuts once, teleport-style, to the arrival the swap message carried. Look applies throughout. Movement and actions are held until the pawn's first record merges, because the client has nothing to predict before then: demotion drops its tuning payload, prediction never falls back to the local registry, and a client pawn exists only once a baseline materializes it (`networking.md` §What gates, §Snapshot apply ordering). The first record merges with no further cut, because the host's pawn stood at that pose with no input.
- A shape that keeps the client's payload across the swap and predicts a provisional pawn from its own mapped pose would remove the movement hold, but diverges from `networking.md` §What gates. It is reopened only if measurement at realistic latency shows the hold is visible (Open questions).
- The relevel-at-request non-goal in `drafts/level-load-pipeline/` (a failed host parse would strand clients) is answered by the speculative prefetch: a client never commits a level the host did not.

**Depends on.** U3.

**Key acceptance.** AC 22–26.

**Brief form.** Resumable brief; it widens the participation predicate's inputs and adds Control messages, but adds no lifecycle edge.

## Acceptance criteria

### Automated

**U1**
1. The `level-load-pipeline` brief's own acceptance list, as amended.
2. A load's stages can run while a level is installed and running, and nothing they create reaches the frame path before commit.

**U2**
3. On a fixture pair, a player crossing a classic seam arrives in the next map with the same offset from the landmark (vertical included), the same view yaw and pitch relative to it, and every velocity channel rotated by the landmark yaw delta; a sliding or dashing player continues on the rotated boost, and a vaulting one arrives in `Normal`. The result matches a seamless teleport between the same poses.
4. Carried state crosses a seam exactly as it crosses `loadLevel` today. A seat's arrival is consumed by its first spawn: a later respawn, and the level after a non-seam change, place it at player starts.
5. Touch activation and use activation each fire the transition; use without overlap does not, and a disarmed seam does not fire. The destination seam fires its arrival event once per arriving player.
6. A destination with no matching seam, and an arrival pose blocked at spawn, each fall back to player starts with one warning. A seam with no explicit landmark uses its bounds centre.
7. The latch follows D11's table on every row, including an arrival whose capsule overlaps the seam with its body point outside.
8. Co-op: a client pawn inside the seam arrives landmark-relative, and one outside arrives at a player start.
9. On the dev-mod seam pair, a player crosses into the next map and back.

**U3**
10. Occupying a generated prefetch region, including by arriving inside it at a swap, starts the next map's load; an authored region replaces the generated one, and a map that places neither still gets one. The prefetch target follows U3's table on every row. A level change, a return to frontend, or suspend during a prefetch cancels it, and the requested change proceeds; resume does not restore it. Diagnostics log each prefetch's start and ready times.
11. Activating a seamless seam after its prefetch is ready swaps with no loading screen and no Loading or Settling state: the activation frame draws the current map, the swap commits at the next redraw boundary, and that frame draws the next map. A seam blind on one tick of a frame and not blind on its last tick does not swap that frame. The swap carries the same state as a classic crossing from the same pose (AC 3, AC 4), and the arriving player is latched (AC 7).
12. The swap frame and the frames around it stay under the frame budget the brief pins on the reference fixtures. The swap frame applies the Settling exit effects per the Prior-commitments table: held movement continues without a re-press, the next frame runs no catch-up ticks for the commit's duration, and a swap frame with zero ticks draws at the arrival eye.
13. The first frame after a swap draws every arrival view with SH and lightmap settled, including a player who turns on the swap frame; no ambient-floor or miss fallback is drawn on it. While the pre-commit settle runs, the active level's residency, miss counts and draws match a run with no prefetch.
14. The wait follows D7's table on every row: **waiting** at most once per wait, **ready** at most once and only on the evaluation that swaps, nothing for activations during a wait, nothing when the swap can already proceed, and no swap or **ready** for a wait whose seam empties. Diagnostics count each wait.
15. With the seam's door open, a swap waits; with it docked closed, the swap proceeds. A bent-hallway seam swaps from its blind cells and not from a cell that sees past a bend. A seam whose next-map copy spawns its door open never swaps.
16. A walk that trips its step cap or falls back to frustum culling never permits a swap.
17. prl-build warns on a seam that can never be blind, and on a sealing mover that does not spawn docked closed, and stays silent on a seam with neither.
18. A failed prefetch, an activation with no prefetch started, and an activation whose prefetch or pre-commit settle is still unready at the Settling timeout each take the classic path with one warning. A seam held only on its blind check never times out.
19. Peak resident CPU and GPU bytes with two levels are reported per `testing_guide.md` §Resource bounds and stay within the 2 GB VRAM target on the reference fixtures. When a second seam's prefetch supersedes the held one (U3's table), the held one is released before the new one uploads, so no more than two levels are ever resident.
20. A shadow-tier change during a prefetch restarts it, and a Surface Depth change made then is live on the first frame after the swap.
21. Co-op, until U4 lands: at a host's swap each client takes the classic relevel and arrives by D4 or D5, and the host publishes its reveal of the next map on the swap frame, so every client re-promotes.

**U4**
22. Clients prefetch when the host does, including a slot promoted during the prefetch, and a client prefetch never commits without the host's swap. A report of a superseded generation, and a retracted report, count as not ready.
23. At a host swap, a ready client crosses with no loading screen. At the reference simulated latency its movement is held for no more than the tick bound the U4 brief pins; look applies throughout; its view cuts once to the arrival and the first record merges with no further cut.
24. A client not ready by the timeout, or whose report arrives after the host's swap frame, takes the classic relevel; the others still swap seamlessly. With no participating client the swap does not wait.
25. A host whose prefetch fails takes D8, every client releases its prefetch, and clients follow the classic relevel.
26. A ready client's slot leaves and re-enters participating on the host's swap frame, through the same predicate as any level change, with its current report standing in for its declaration and reveal; the predicate follows U4's table on every row. Its report never demotes it while the host is still on the old map.

### Manual

- Playtest the airlock pair and the bent-hallway pair, single-player and co-op at simulated latency. Watch for a lighting, audio or entity pop at the swap; the seam's two bakes can differ (Open questions).
- Outrun a prefetch on purpose and confirm the door's wait reads as the door powering up.
- Co-op: judge whether the movement hold at the swap is visible at realistic latency.
- Metal: swap-frame timing and peak memory on the reference pairs.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| I1 — Exactly one level simulates and draws per frame | U3 swap commit | Prefetch install; streaming installs for the prefetched level | AC 2, 11 |
| I2 — No frame mixes two levels' world data | U1 commit; U3 swap | Pre-commit settle writing into shared pools; renderer resources keyed globally rather than per level; the swap's unload releasing B | AC 2, 13 |
| I3 — A seam carries the session-state ledger plus each seam occupant's landmark-relative arrival, nothing more: body offset, view yaw and pitch, the velocity channels and the movement state payload, all read before unload | U2 | U3 swap path diverging from the classic harvest; unload despawning the pawn and resetting the camera before the arrival is read; a stale arrival surviving into a later level | AC 3, 4, 11 |
| I4 — Arrival comes from the one landmark → target transform and the one arrival rule teleports use | U2 | A seam-specific copy of the math or of the arrival-state rules | AC 3 |
| I5 — At most one prefetched level | U3 | A second prefetch region; a level change during prefetch | AC 10, 19 |
| I6 — The host owns every swap; a client never commits a level the host has not | U2 (relevel), U4 | Client prefetch completing first | AC 22, 24 |
| I7 — A seam never strands a player | U3 | Prefetch failure, cancellation, suspend, a prefetch or settle that never completes | AC 10, 18, 25 |
| I8 — Participation changes only through the one predicate | Existing slot lifecycle; U4 adds the readiness report as one of its inputs | A report read while the host is on another level; any seam-specific swap path | AC 26 |

## Pinned orderings

Each row is a test the named unit's brief carries.

| Unit | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| U2 | Latch clears on capsule exit | touch seam latched → body point leaves, capsule still overlaps → capsule leaves → re-enter | No fire while the capsule overlaps; re-entry fires |
| U2 | Level change before a co-op client arrives | seam crossing A→B → client still loading B → host changes B→C | Client arrives in C at a player start; its B arrival cleared |
| U3 | Prefetch completion polled at the activation frame's redraw boundary | ready polled → activate on a tick of that frame | Swap at the next boundary; no **waiting** |
| U3 | Prefetch completes during the activation frame's ticks | activate → completion → poll at the next boundary | **waiting** on the activation frame, **ready** on the next, swap at the boundary after |
| U3 | Blind on an early tick, not on the last | activate on tick 1 (blind) → door starts opening on tick 3 → boundary | No swap that frame; evaluation reads tick 3 |
| U3 | Two activations on one tick | activate × 2 | One swap |
| U3 | Activation repeated during a wait | activate → **waiting** → activate × N | No further **waiting**; one **ready**, then swap |
| U3 | Ready prefetch, seam not blind | prefetch ready → door open → activate | **waiting**; **ready** when the door docks closed; swap |
| U3 | Seam empties while waiting | activate → **waiting** → last player inside leaves → prefetch ready | Wait ends; no swap, no **ready** |
| U3 | Player leaves the prefetch region before activating | enter → leave | Prefetched level stays until superseded or a level change, unless the U3 brief adopts release on leaving (Open questions) |
| U3 | Second seam's region entered | prefetch A→B ready → every player leaves its region → enter A→C region | B released, C prefetched (D3) |
| U3 | Second seam's region entered while the first is occupied | prefetch A→B → one player stays in its region, another enters A→C region | B kept; C not prefetched |
| U3 | Second region entered during a wait | activate A→B → **waiting** → enter A→C region | B kept; C not prefetched |
| U3 | Prefetch and activation on one tick | arrival inside a seam whose region holds no prefetch | Prefetch targeted first; the activation waits (D7), never D8 |
| U3 | Prefetch fails while its region stays occupied | prefetch → failure → player stays | No retry before a level change; activation takes D8 |
| U3 | Prefetch fails after activation | activate → **waiting** → failure | Classic path with one warning (D8); **ready** never fires |
| U3 | Prefetch never completes | activate → **waiting** → Settling timeout from activation | Classic path with one warning (D8); **ready** never fires |
| U3 | Level change during prefetch | prefetch → `loadLevel` / `restartLevel` / return to frontend | Prefetch cancelled; the requested change proceeds |
| U3 | Script level request on the swap's frame | evaluation passes; `loadLevel`, `restartLevel` or `returnToFrontend` dispatches the same frame, either order | The script request proceeds; swap dropped, prefetch released; **ready** fires only if the evaluation ended an open wait (D7), as when suspend falls between evaluation and commit |
| U3 | Prefetch target and level change on one frame | region entered; `loadLevel` dispatches the same frame | The change proceeds; the prefetch never starts |
| U3 | Two seams activated on one frame | A→B prefetched, A→C not, both activated | A→B evaluates; the A→C activation drops with no event and no D8 |
| U3 | Suspend during prefetch | prefetch → suspend | Prefetch cancelled; resume does not restore it |
| U3 | Suspend between evaluation and commit | evaluation passes → **ready** → suspend → resume | Queued swap and prefetch dropped; A stays installed; the seam needs a fresh activation |
| U3 | Door opened while waiting | activate → **waiting** → door opens | Swap holds until blind again |
| U3 | Arrival inside the return seam | swap → arrival inside B→A seam | Latched (D11) before B's first trigger tick; B→A prefetch targeted on that tick and requested at the next redraw boundary; the swap cannot fire until the player leaves and re-enters |
| U3 | First frame after a swap | commit → B's first fixed tick | `levelLoad` reactions run at commit; the arrival event fires on B's first fixed tick; their queued commands dispatch in that order |
| U3 | Player dies inside a waiting seam | activate → **waiting** → death | Open (Open questions) |
| U4 | Report for a superseded prefetch | report for B arrives after the host supersedes with C | Not read; the slot is not ready for C |
| U4 | Client prefetch lost after reporting | report → client suspend, cancel or failure | Retraction; the slot is not ready |
| U4 | Slot promoted during a prefetch | instruct → slot enters participating → activation | Slot instructed on promotion and counted in the wait |
| U4 | Report races the timeout | timeout expires → swap frame → report arrives | Slot already given the classic relevel; report not read; the client finishes its prefetch under the loading screen instead of loading B twice |
| U4 | Waited-on client closes | activate → **waiting** → close | The host stops waiting on it |
| U4 | Host prefetch fails after reports | reports in → host prefetch fails | D8 classic relevel; every client prefetch released |
| U4 | Ready client crosses | host swap frame (demote, commit, re-promote) → swap message → client commit → declaration of B → first record merge | Movement held until the merge; look live; one cut, none at the merge |
| U4 | Client ticks on A after the host's harvest | host harvest → client keeps moving on A → swap message | Those ticks discarded; one teleport-style cut to the host's arrival |

## Boundary inventory

Leanings, pinned by the U2 brief.

| Concept | FGD | Notes |
|---|---|---|
| Seam volume | brush entity, e.g. `level_seam` | keys: target map catalog id, seam name, activation (`touch` / `use`), mode (`classic` / `seamless`), optional landmark name, start armed |
| Landmark | point entity; reuse `teleport_destination` as ghost links do, or a new name | absent → bounds centre, yaw 0 |
| Prefetch region | brush entity naming its seam | absent → generated by prl-build |
| Events | `waiting`, `ready`, and an arrival event on the destination seam | a seam event source beside the trigger and teleport sources: trigger events accept only `enter` and `exit`, and teleporters keep a separate `teleportEvents` list accepting only `teleport`; FGD reaction keys follow `trigger_volume` `on_fire` and `trigger_teleport` `on_teleport` |

## Open questions

- **Co-op movement hold.** U4's held window runs about half a round trip plus the client's commit; the owner accepted it (2026-10-28). If the manual check finds it visible at realistic latency, the provisional-prediction shape (U4) is reopened with that measurement.
- **Lighting parity across a seam.** D6 checks that a seam is blind to sight, not to light: a bend unsealed against outside light pops at the swap. Lean: a per-map prl-build warning when any light outside the seam reaches its cells, directly or through SH; two identical copies that both pass bake alike. U3 brief.
- **Death at a seam.** Whether a dead or downed pawn (E18 player life states) activates or crosses a seam, given that teleporters bar dead pawns (`movement.md` §6), is for the U2 brief. Whether death inside a waiting seam blocks the swap or sends that player to a player start is for the U3 brief. Both are decided against the life-states spec.
- **Non-player entities inside a seam.** Whether a hostile actor inside the active seam holds the swap rather than vanishing with its level — U3 brief.
- **Cost of the blind walk on the host.** All-directions walks per player in a seam, once per frame while activated or waiting (D6 evaluation point). Measured in U3.
- **Commit-frame cost after U1.** The script VM and registry stages stay on the main thread. If they exceed the swap budget, U3 needs its own lever.
- **How long a prefetched level stays.** The Pinned orderings default keeps it until superseded, so the two-level peak can last the rest of the map. A release on leaving the prefetch region, with hysteresis, is the alternative — U3 brief, against AC 19.
- **Two-level peak memory.** Unmeasured. World textures alone are about 80–96 MiB per level (`drafts/level-load-pipeline/research.md`).
- **Audio across the swap.** Level sounds reload per level; ambient loops cut at a swap unless carried or crossfaded — U3 brief.
