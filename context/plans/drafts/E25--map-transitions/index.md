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
- **U4 Co-op seamless seam** — clients prefetch on host instruction, and a ready client inside the seam crosses with no loading screen and no movement hold, predicting through the swap; any other client takes the classic relevel.

### Out of scope

- **Drawing a non-active level.** A straight hallway or a vista that sees into the next map needs two worlds in one frame. No plan owns that capability today. Large-map residency (`context/plans/large-map-spatial-residency.md`) is its nearest home, but that plan streams baked resources inside one logical PRL and never draws a second level. This epic forbids it by I2, so a later spec that draws a vista revises I2; U1 and U3 leave it a second resident level to draw from. Planted in the roadmap (Future › Rendering and visual polish).
- **Revisited-map state.** Planted separately in the roadmap (Future › Gameplay systems).
- **Non-player entities crossing.** Enemies following through a seam, dropped items carried across. Players only. At the swap, a non-player entity inside the active seam vanishes with its level unless the U3 brief makes a hostile one hold the swap (Open questions), and the next map's seam shows that map's own placements; the blind check hides neither. The U3 reference pairs keep their seams free of placements that differ between copies, and the manual playtest watches for an entity pop.
- **Co-op players straddling a seam with both levels simulated.** One level simulates at a time; the swap moves everyone.
- **A second, seamless-only load pipeline.** One pipeline serves both orders (D1).

## Direction

**Problem.** The engine has one level change, and it is a hard cut: `drain_level_requests` unloads the current level before the next one starts loading, the install freezes the main thread for one frame of about 0.6–0.8 s on lavapipe, and the player always arrives at a player start. Nothing lets a level change preserve where the player is or hide that it happened.

**Prior commitments.**
- `boot_sequence.md` §8 lists "multiple simultaneously resident levels or streaming" as a non-goal. U3 reverses it for exactly one prefetched, unpublished level beside the active one. The argument: the alternative to a resident next map is a load at the moment of crossing, which is the hitch this epic exists to remove. Rewritten at U3 promotion.
- `drafts/level-load-pipeline/` kept "unload-first" and listed spawn-before-unload as a non-goal. It is amended here to keep unload-first as the loading-screen order only, so its stages may run before an unload (D1).
- Large-map residency keeps "one authored level, one logical PRL". This epic keeps it: each map stays its own PRL and catalog entry.
- The session-state ledger (`networking.md`) says what survives a level change is an explicit, enumerable set. A seam swap carries exactly that set plus each seam occupant's landmark-relative arrival (I3, D4). Under U4 the host also carries, for a seamless slot, its queued commands for client ticks after the harvest tick with the slot's cursor and playout state, and the harvested teleport record the new pawn's record is seeded from. The client keeps its prediction ring, its local pawn's predicted state and the newest tuning payload across its own unload of A. `networking.md` §Session-state ledger is rewritten at U4 promotion to name each.
- Teleporters ship classic and seamless modes, and seamless mode's landmark → target transform is one pure `foundation` function shared with ghost links (`movement.md` §6, `entity_model.md` §7d). Seam arrival uses that transform with the source and target in different maps (D4).
- Doors already seal portals: `doors-as-occluders` bakes which portals each mover's closed brush seals, and the runtime walk blocks them while the mover is docked closed. The walk lives in `postretro-visibility`, a CPU crate with no renderer dependency, so the host can run it for any pawn. It has no all-directions entry: the camera entry and the seeded entry both clip to a caller view-projection. The blind check (D6) reuses the blocked-portal state; its all-directions form is a union of camera-entry walks or a new entry, pinned by the U3 brief. Door state reaches the walk from post-tick mover state. The renderer's blocked-portal buffer is rebuilt once per frame in the visibility stage, after Audio and the script drain, and from the active level's live movers only. That is too late for D6's evaluation and blind to the prefetched level, which has no spawned movers. So the evaluation builds its own blocked-portal state: for the active level from post-tick mover state by the same rule, and for the prefetched level from its loaded movers' baked sealed portals at their spawn phase, the reading the compiler's docked-closed-at-spawn warning makes.
- The runtime visible set is a conservative superset (`rendering_pipeline.md` §2). A cell absent from it is not visible, which is the direction the blind check needs (D6).
- Map authority is server-owned; a client never asks for a level change (`networking.md`). The host owns every swap (I6).
- **"In play nothing waits on residency"** (`boot_sequence.md` §1; `done/sh-probe-streaming/` "no door waits on async residency"). D7 diverges: the swap, a gameplay event, holds on the next map's load. The argument: residency misses have a fallback (the ambient floor), but a missing level has none short of a loading screen, which D8 keeps for failure. The wait is the author-presented door, not an invisible stall, and it is bounded: a swap held on its prefetch or pre-commit settle for the Settling timeout (10 s, `boot_sequence.md` §1) takes the classic path (D8); the time counts only while they read unready (D7's clock). A hold on the blind check alone has no bound; the player ends it by closing the door or leaving.
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
- **Join-time prediction rules** (`networking.md` §What gates, §Snapshot apply ordering, §Host input command queue, §Slot lifecycle). The client drops its tuning payload at demotion and at level unload; it predicts nothing before its pawn's first record merges (the joiner gate); no provisional client-spawned pawn exists alongside the host's; arming prediction on a new pawn identity clears the command ring and adopts the first record silently; a new pawn's teleport record starts at zero accumulated yaw; a held slot's commands are discarded, the exit sweep clears its command queue and a level change clears every queue; Input stamped with a retiring participation epoch is dropped; a Holding frame demotes the client, despawning its pawn and dropping its payload; a client sends `Revealed(None)` at unload. Each rule exists because a joining client holds nothing it can trust. U4 bends them for one case, a ready client's seam commit, where the client holds everything prediction needs from the host: the host's newest tuning payload for B, B's parity checked by the host, the spawn pose, the harvest tick, the seeded teleport record and the pawn's host-assigned `NetworkId` (U4, The exception). The purposes stay whole: prediction uses only host numbers, and the client's one pawn carries the host's identity and is refreshed in place. Owner decision (2026-10-28); rewritten at U4 promotion.
- **"Seam" already names a residency concept.** `streaming_seam_volume` (`build_pipeline.md` §Custom FGD) marks a best-effort load-hide point at a portal. A map-transition seam often sits at the same place. The U2 brief either relates the two or renames one.

**Alternatives rejected.**
- **Grow one map instead of joining two.** Large-map residency could in principle make a whole campaign one PRL. Rejected as this epic's vehicle: it streams baked lighting only today, everything else loads whole, and one PRL per campaign makes every edit a whole-campaign compile. Maps as units match how mods author and catalog content.
- **Metroid Prime's door-press load.** Start loading when the door is activated. Simple, and it is the hesitation the owner wants gone. Kept only as the fallback when a prefetch has not finished (D7).
- **Stop after U1 and U2.** A Half-Life-style landmark change keeps pose and loadout, and U1 shortens its loading screen. Rejected: it keeps a visible break, and removing that break is an owner decision (2026-10-28). It remains the fallback (D8).
- **Held movement across a co-op swap.** Keep the join-time rules and hold the client's movement from the host's swap until its pawn's first record merges, about half a round trip. No rule bends. Rejected by the owner (2026-10-28) in favour of prediction through the swap (U4); a client lacking what that prediction needs takes the classic relevel instead.
- **Two pipelines, seamless and classic.** Each would own an install order, failure path, cancellation and parity publication, and they would drift. Rejected for D1.
- **Swap check at compile time only.** prl-build could mark blind cells once. Doors move, so a cell's blindness depends on runtime mover state, and the runtime walk already folds that in. Baked visibility is also sampled, so it can miss cells, which is the unsafe direction. The compiler keeps warnings (D6), not the decision.

## Cross-cutting decisions

- **D1 — One load pipeline, two commit orders.** Loading-screen order: unload A → load B → commit → settle → reveal. Seam order: load B while A runs → hold B ready and unpublished → swap commit (harvest, unload A, commit B) on one frame. The swap's unload releases only A's level-scoped state; B's prefetched CPU and GPU data and its pre-commit settle residency survive it. Today's unload clears renderer level resources and level streaming state wholesale. U1 keys renderer level resources per load, not per catalog id, so a seam into its own map can hold two copies. An unload releases only its own level's resources. The swap's unload of A cancels nothing, unlike the queued unload request the `level-load-pipeline` brief's cancellation rule names. U3 keys level streaming state the same way for the pre-commit settle, which that brief leaves to U3. U1 builds the pipeline; it must not assume the previous level is gone before its stages run.
- **D2 — One active level.** Exactly one level simulates and draws on any frame. A prefetched level is CPU and GPU data held off the frame path until its swap commit (I1, I2).
- **D3 — At most one prefetched level.** A second seam's prefetch supersedes the first, which is released, except while U3's prefetch target table keeps it: its seam has an open wait or a queued swap, or its region is occupied while no other seam's wait is open. Bounds peak residency at two levels. A prefetch belongs to the seam whose region targeted it; another seam to the same map reads it as not live (Loading, D7) until its own region targets it, which supersedes the first like any other.
- **D4 — Seam arrival is the seamless-teleport transform.** Body offset from the landmark (vertical included), view yaw and every velocity channel rotate onto the matching landmark in the next map. Two shipped pieces split that work today:
  - `foundation`'s `SeamlessMapping` maps the point and turns vectors. It takes plain poses, so a landmark pose read in map A and a target pose read in map B feed it unchanged.
  - The rest lives only inside the physics teleport chokepoint: the channel list (base, knockback and push velocity, a dash or slide's banked boost), the vault abort, the arrival ground settle and the blocked-arrival fit (`movement.md` §6, Arrival state). The chokepoint fires from the current map's teleporter records within one movement tick, so it cannot run across an unload.

  The movement component holding that state despawns at unload, and carried state does not hold it. View yaw and pitch are not pawn state: the host's come from its camera, a client pawn's from that slot's host input command queue. Today's carried-state harvest runs midway through `unload_level`, after its net reset has replaced the host's command queues and after the level, collision world and trigger bindings are cleared, so it can read neither a client's facing nor the seam's landmark. The arrival harvest therefore runs before unload's first teardown step. For each seam occupant it reads the body offset from the landmark, the velocity channels and the movement state payload from the live pawn, and view yaw and pitch from the host's camera or the slot's latest input command. The next map's spawn applies it through arrival code U2 factors out of the chokepoint, so teleports and seams share one rule (I4). A blocked arrival or a missing matching seam falls back to player starts with one warning; the blocked test runs when the pawn spawns, not at harvest.
- **D5 — The engine moves every player.** Players inside the seam (D11's test) arrive landmark-relative; anyone outside arrives at the next map's player starts through ordinary placement. Whether to wait for everyone is mod policy: a seam has an armed state, as trigger volumes and teleporters do, and a mod arms it from an occupancy reaction, such as a `trigger_volume` covering the seam, since seam events publish no occupancy.
- **D6 — Blind swap.** A seamless seam swaps only when, for every player inside it, an all-directions walk from the eye under current door state reaches no cell outside the seam. The same walk from the mapped eye in the prefetched level, under that level's mover spawn state, must also reach no cell outside its seam, or a door the next map spawns open shows world that was hidden one frame earlier. B has no registry before its commit, and the blocked-portal rule reads live mover components. So the prefetch derives that spawn state from B's map data: a sealing mover's baked sealed portals count as blocked when its authored spawn phase is docked closed. Commit-time `levelLoad` reactions run after the check, so a reaction that starts a sealing mover in B's seam can open it on the swap frame's ticks. The U3 brief pins whether prl-build warns on such a reaction or B's first draw precedes its first tick. A walk that trips its step cap or falls back to frustum culling counts as not blind. Under U4 a remote player's walks in both levels start from its eye widened by its client's lead (U4, Client lead on an unsealed seam), so the walk form the U3 brief pins accepts a widened start as well as a point eye.
  - **Evaluation point.** Activation is a fixed-tick trigger event; the swap is a redraw-boundary commit. Once per frame, in Game logic after its last fixed tick and before the frame's post-tick event drains, the host builds blocked-portal state (Prior commitments, doors) and evaluates each seam activated that frame or holding an open wait (D7). It reads the walks from every inside player's eye at that tick's pose and mover state, the bundle state polled at this frame's redraw boundary, and the pre-commit settle's answer for U3's settle set. A passing evaluation queues the swap, which commits at the next redraw boundary's request drain, before that frame's Input stage. No fixed tick runs between evaluation and commit, so the commit carries exactly the poses and door state the check passed. A frame that runs no fixed tick still evaluates.
  - **Events from the evaluation.** The **waiting** and **ready** events an evaluation fires, and a D8 request it raises, drain with that frame's post-tick events, outside any tick. Their reactions and queued commands therefore run against the current map before the commit's unload discards undispatched commands. A script level request drained after them replaces a D8 request, since one load is queued at a time.
  - **Compiler warnings.** prl-build warns when no cell of a seam can ever be blind with its sealing movers closed, and when a seam's sealing mover does not spawn docked closed.
  - Mover and light state do not cross the swap: the destination installs its copy at its spawn state, and a light the **waiting** reaction set does not carry.
  - U3's brief pins the walk's form and its cost on the host.
- **D7 — The door waits, and says so.** Activating a seamless seam before its prefetch is ready, or while it is not blind, holds the swap in the seam's one **wait**. Each D6 evaluation applies the table below, first matching row first. Activations while a wait is open join it, so a wait fires **waiting** once and **ready** at most once, on the evaluation that ends it in a swap. Presentation is the author's: a pulsing light is two reactions through the shipped `setLightAnimation`. Wiring nothing means the door simply waits. Waits are counted in diagnostics.

  | Player inside | Prefetch | Blind | Wait open | Outcome |
  |---|---|---|---|---|
  | No, or the seam is disarmed | Any | — | Any | No swap, no event; an open wait ends; the next activation evaluates afresh |
  | Yes | Failed, or Loading with the wait's clock at the Settling timeout | — | Any | Classic path with one warning (D8); an open wait ends; no **ready** |
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

  "Ready" means the bundle is ready, as polled at the frame's redraw boundary, and the pre-commit settle answers settled for U3's settle set. "Loading" is every other prefetch state of a seam with a player inside: requested and not yet ready, ready with its settle pending, targeted on a tick this frame and not yet requested, or not live at all (never targeted, or released by a supersede (D3)), which the target table targets on the tick after its wait opens (U3). Suspend ends an open wait with no event and drops a queued swap with its prefetch. Under U4, client readiness joins the Ready column until U4's own timeout expires. U4's table (U4, Host wait) overrides how each evaluation reads this table's Prefetch column and the clock table below. An evaluation that reads Loading only for client readiness runs the clock as Prefetch Ready, so the Settling timeout counts only the host's bundle and pre-commit settle, and a hold on client readiness alone never takes D8.

  The Settling timeout runs on the wait's clock, which counts only evaluations that read the prefetch Loading. A wait held on its blind check with a ready prefetch therefore never times out because its settle briefly pends, and a later unready stretch, such as a shadow-tier restart, starts afresh:

  | Evaluation | Clock |
  |---|---|
  | Opens the wait, prefetch Loading | Starts |
  | Opens the wait, prefetch Ready | Stays stopped |
  | Prefetch Loading, clock stopped | Starts |
  | Prefetch Loading, clock running | Runs; at the Settling timeout, the second row applies |
  | Prefetch Ready | Stops and clears |
  | Wait ends | Clears |
- **D8 — A seam never strands the player.** A prefetch that fails and a swap held on its prefetch or pre-commit settle for the Settling timeout on the wait's clock (D7) each take the classic path with a loading screen and one warning.
  A classic path to the level a live prefetch holds keeps that prefetch. The loading-screen unload of A does not cancel it; the prefetch finishes and commits behind the loading screen, then settles as any entry does. This holds for the host at D8, and for a client given the classic relevel while its prefetch of that level stands (U4).
- **D9 — Defaults for beginners, tools for advanced authors.** The engine and tools do something sensible when an author places only the minimum, and give advanced authors explicit overrides:
  - **Prefetch region.** prl-build generates one by portal-graph distance from the seam; an authored region replaces it. U3's brief sizes the default distance from measured background load time and sustained movement speed.
  - **Landmark.** Defaults to the seam volume's bounds centre at yaw 0; an explicit landmark point overrides it.
  - **Blind check.** Automatic (D6); the compiler warnings tell a beginner their seam cannot swap seamlessly.
  This principle lands in `context/lib/` at hub promotion; no library doc states it today.
- **D10 — Seam modes mirror teleporter modes.** One seam entity carries `classic` or `seamless`. Classic is U2; seamless is U3. A seamless seam degrades to classic under D8.
- **D11 — One inside test; arrival latches.** A seam has one inside test: capsule overlap with its volume's axis-aligned bounds, the test its touch and use activations already apply as a `trigger_volume`, which keeps one AABB per volume however many brushes it holds. A bent or other non-box seam therefore counts its whole box as inside, including space beside the bend that belongs to neighbouring rooms. The U2 brief either requires a seam volume to fill its box or gives the seam a brush-union test, and every use below reads the test it pins. It decides activation, landmark arrival (D5), the eyes the blind check walks from (D6), prefetch occupancy and the latch. Teleporters fire and latch on the body point (`movement.md` §6); a seam activates as a trigger, so it latches as one, and the teleporter latch store, keyed to teleporter indices, does not hold it. Every player arriving inside the matching seam is latched against it when its pawn spawns, before any trigger tick sees that pawn, and stays latched until their capsule leaves it. A co-op client's pawn spawns at participation entry (U2): on the classic relevel, frames after the destination's first fixed tick; for a U4 seamless slot, on the host's swap frame, before B's first fixed tick. A two-way seam therefore never bounces a player straight back.

  | Player against the destination seam | Touch seam | Use seam |
  |---|---|---|
  | Arrives overlapping it (body point inside or outside) | Latched; the first trigger tick that sees the pawn finds no entry edge | Latched; use does not fire |
  | Latched, still overlapping | Does not fire | Does not fire, even on use |
  | Latched, stops overlapping | Latch clears; does not fire | Latch clears; does not fire |
  | Unlatched, starts overlapping | Fires | Does not fire |
  | Unlatched, overlapping, presses use | Does not fire again | Fires |
  | Arrives at a player start, not overlapping | Fires on later entry | Fires on use while overlapping |
  | Unlatched, overlapping, seam armed from disarmed | Fires on the next trigger tick, as an armed `trigger_volume` admits its standing occupants | Does not fire; the next use fires |
  | Latched, overlapping, seam armed from disarmed | Stays latched; does not fire | Stays latched; does not fire |
  | Any, seam armed while already armed | Nothing changes | Nothing changes |
  | Any, seam disarmed | Does not fire; latches kept | Does not fire; latches kept |

  Arming keeps latches, unlike a teleporter's arm, so an occupancy reaction that arms a two-way seam as players arrive never bounces them back.

## Units

**Brief inputs.** Each unit's brief author reads, besides the unit's own section: Goal, Direction, Cross-cutting decisions, the full Acceptance criteria, Invariants, Pinned orderings, Boundary inventory and `research.md`.

### U1 — Off-main level load

**Purpose.** Move a level's CPU preparation and GPU creation off the main thread and shrink the install to a short commit. Every level change benefits; U3 cannot ship without it, because a one-second commit at a seam is the hitch the epic removes.

**Contents.** `drafts/level-load-pipeline/` as amended alongside this hub: decisions reveal-gate already delivered are dropped; unload-first becomes the loading-screen order; stages may run before an unload; "one level's world data resident at a time" is gone; renderer level resources are keyed per load, and an unload releases only its own level's (D1).

**Depends on.** Nothing. Runs concurrently with U2.

**Key acceptance.** AC 1–2.

**Brief.** `context/plans/drafts/level-load-pipeline/` (resumable).

### U2 — Landmark level change

**Purpose.** The seam author surface and the transition contract, proven with a loading screen before any background loading exists.

**Contents.**
- Seam brush entity with a target map (catalog id), a seam name shared across both maps, an activation of touch or use as `trigger_volume` has, a mode (D10; until U3 lands, a `seamless` seam runs classic), a start-armed key with script `arm` / `disarm` (D5), and an optional landmark point (D9).
- The seam's name against `streaming_seam_volume`: the brief relates the two or renames one (Prior commitments).
- Activation runs a classic level change carrying the seam name; arrival applies D4 and D5; carried state is unchanged (I3).
- Arrival code factored out of the teleport chokepoint and shared with it (D4, I4).
- The inside test and arrival latch (D11).
- Seam events, host-only as teleport events are. U2 adds an arrival event on the destination seam; it dispatches inside the first fixed tick after the arriving pawn spawns, as teleport events dispatch inside their tick. U3 adds **waiting** and **ready**, which drain with the post-tick events after D6's evaluation, outside any tick. Trigger events publish only `enter` and `exit`, so seams get their own event source, as teleporters did.
- A seat's arrival is written at every harvest, from the inside test at the harvest rather than at activation: the landmark-relative arrival (D4) for a seat whose pawn is inside the seam, none for a seat outside it, with no bound pawn, at a level change that is not a seam's, or at the harvest suspend runs. The seat's first spawn in the next level consumes it, as does U4's swap-frame split for a seamless slot whether or not that slot then spawns, and that level's unload clears an unconsumed one. A respawn never reads it.
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

  | Current prefetch | Regions occupied this tick, or another seam's wait | Target |
  |---|---|---|
  | Its seam has an open wait or a queued swap | Any | Current kept |
  | Any | Another seam has an open wait | That seam, first in map order; supersedes the current (D3) |
  | Its region is occupied | Any | Current kept |
  | Any | Only regions whose prefetch failed since install | Current kept; a failed seam never retries before a level change |
  | None, or its region unoccupied | One or more | First occupied region in map order, skipping failed ones; supersedes the current (D3) |
  | Any | None | Current kept |

  The frame's last tick's target is requested at the next redraw boundary, so a frame starts at most one prefetch. An activation on the tick that first targets its seam waits (D7) and never takes D8.
- Player options during a held prefetch. U1 snapshots tiers at dispatch and keeps a Running-frame change out of the bundle. A shadow-tier change cancels and restarts the prefetch. A Surface Depth change applies live to the active level only, because its live path rewrites installed materials' uniforms and B's are not installed. The swap commit compares the tier B's materials were written under with the current one. When they differ, it rewrites B's material uniforms in place, by that same rewrite, before B's first frame draws.
- Pre-commit settle: the next map's SH and lightmap settle set is read into the prefetched level before the swap, so the first frame after the swap draws settled light. The set is every view this machine draws on that frame: every cell of the next map's seam, plus, for each local player outside the seam, the all-directions view from the player start ordinary placement gives it. It is asked from the frame the bundle is polled ready. A swap needs every arrival eye blind in the next map (D6), so no arrival view from inside the seam reaches a cell outside the set, however the player turns or moves; and a touch seam's activation frame finds the set already answered once the prefetch has stood long enough. A remote pawn's eye joins the blind check but never this machine's settle set; under U4 each client settles its own. A pose the swap frame's own ticks carry out of the seam streams its new cells as in play. The prefetched level's streaming runs beside the active level's, which today's streaming does not allow: SH worker-pool generations never overlap (`boot_sequence.md` §4), and SH and lightmap reads share one issuer and drain budget (`rendering_pipeline.md` §4). The U3 brief decides whether the two levels share that pool and issuer or each run their own; either way, the active level's in-play drains keep their budget.
- Blind check, its evaluation point and the prl-build warnings (D6).
- Swap commit on one frame: harvest, unload A, commit B, arrival by D4 and D5, latch by D11, Settling exit effects per the Prior-commitments table.
- Door wait and **waiting** / **ready** events (D7); failure and timeout fallback (D8). A prefetched level with no seam matching its seam's name fails at bundle-ready, because D6's walk in it has no mapped eye. The activation takes D8, and the classic arrival falls back to player starts (D4) under that one warning.
- Diagnostics: prefetch start and ready times, waits, peak resident bytes with two levels.
- `boot_sequence.md` §8 rewritten.
- Co-op: clients take the classic relevel (loading screen) at a host swap until U4.
- Dev-mod content: a door airlock pair and a bent-hallway pair, plus a reference door that pulses a light while waiting.

**Depends on.** U1 (off-main pipeline), U2 (seam surface and arrival).

**Key acceptance.** AC 10–21.

**Brief form.** Resumable brief. The swap lifecycle crosses the loader, renderer, sim and streaming boundaries; checkpoint the prefetch, the swap commit and the blind check as stages.

### U4 — Co-op seamless seam

**Purpose.** Co-op clients cross a seam with no loading screen and no movement hold: the swap reaches a ready client as a cross-map teleport its prediction carries through.

**Contents.**
- **Instruction.** The host tells participating clients to prefetch when it does, and tells a slot that enters participating while the prefetch stands. A client prefetch is speculative and commits only on the host's swap (I6). Each instruction names the level and a prefetch generation and carries the host's tuning payload for that level; the client stages it and never installs it before its seam commit. A supersede (D3) sends the next generation; a cancel, and a host prefetch that fails (D8), send a release.
- **Readiness report.** A new Control message, separate from the client's installed-level declaration and its revealed declaration. It names its generation, carries the prefetched level's parity values, and says the client's bundle is ready and its pre-commit settle is complete for its own settle set (U3). The host keeps each slot's last report and counts it only for the current generation. A client whose prefetch is cancelled, fails or is released after reporting sends a retraction; suspend retracts too. A sent report stands while the client moves: the client keeps settling its own view through the swap, and a block its later movement needs takes its miss fallback for the frames it is short, as in-play streaming does.
- **Host wait.** The host's swap waits, inside D7's wait, for the report of every participating client whose pawn is inside the seam: **waiting** and **ready** cover client readiness. A client outside the seam would arrive at a player start the host assigns only at re-promotion, a view no report could have settled; it takes the classic relevel, and the host does not wait on it. A current report whose parity differs from the host's prefetched level ends the wait for that slot: the slot stays participating on A, and at the swap takes the classic relevel and is held on B with the diverged content cause. The timeout, pinned by the U4 brief, starts on the wait's first evaluation that passes every term but client readiness, keeps running while later evaluations fail other terms, and clears when the wait ends. An evaluation reads D7's tables as follows:

  | Host bundle and settle | Every waited-on client reported | U4 timeout | D7's Prefetch column reads | Wait's clock |
  |---|---|---|---|---|
  | Ready | Yes | Any | Ready | As Prefetch Ready |
  | Ready | No | Not started or running | Loading | As Prefetch Ready |
  | Ready | No | Expired | Ready | As Prefetch Ready |
  | Loading | Any | Any | Loading | As Prefetch Loading |
  | Failed | Any | Any | Failed | — |

  On the swap frame the host splits the slots once, before the commit's unload. Each participating slot takes the first matching row:

  | Slot's report | Pawn at the harvest | Split |
  |---|---|---|
  | None current: never sent, retracted, of a superseded generation, or not yet arrived | Any | Classic relevel |
  | Current, parity differing from the host's prefetched level | Any | Classic relevel; held on B with the diverged content cause; no swap notice |
  | Current, parity matching | Outside the seam | Classic relevel |
  | Current, parity matching | Inside the seam | Seamless: swap notice; its arrival spent |

  The split spends each seamless slot's arrival, so a retraction read later that frame leaves it at its assigned `player_spawn`. A report arriving later is not read. With no participating client inside the seam the swap does not wait.
- **Predicate input.** The predicate stays one comparison; only its client-side input widens. A current-generation report counts as the client's declaration and revealed record for the level it names only when the host installed that level by the swap commit of the report's generation and the report arrived before that swap frame. A classic path to the same level, D8 or a script level request included, never reads it, even while the prefetch it names stays live. A later message from the client supersedes the report term by term: a parity declaration replaces its parity, a revealed message replaces its reveal, and a retraction withdraws both. On its swap frame the host's unload re-evaluates every slot (demote), and the re-evaluation after its commit and reveal reads the reports (re-promote). That re-promotion runs in the swap frame's net poll, after the poll's Control drain and before B's first fixed tick, not at the redraw boundary where the commit publishes the reveal. A retraction that poll reads therefore holds the slot before it spawns, so the host skips its spawn and sends no swap message. `levelLoad` commands dispatched at the commit have run before re-promotion re-resolves the tuning payload. `networking.md` §Slot lifecycle is rewritten at U4 promotion to name the report as an input.

  | Host installed | Client's newest Control fact, per term | Verdict |
  |---|---|---|
  | A | Declaration and reveal of A | Participating |
  | A | Report or retraction for B, after its A declaration | Participating: it names a level the host has not installed and is not read |
  | B, by the report's swap | Current report for B, arrived before the swap frame, parity matching | Participating: the report stands in for declaration and reveal |
  | B, by the report's swap | Current report for B, arrived before the swap frame, parity differing | Held with the diverged content cause; the client takes the classic relevel |
  | B, by the report's swap | Report for B, arrived after the swap frame, or of a superseded generation | Not read; held until the client declares and reveals B through the classic relevel |
  | B, by the report's swap | Matching declaration of B after its own commit; reveal still the report's | Participating: the declaration replaces the parity term only |
  | B, by the report's swap | Reveal of B after its own commit; parity still the report's | Participating: the reveal replaces the revealed term only |
  | B, by a classic path | Current report for B | Not read; held until the client declares and reveals B |
  | B | Declaration of B whose parity differs | Held with the diverged content cause |
  | B | Declaration and reveal of A, no report | Held; the client takes the classic relevel |
  | B | `Revealed(None)` or a retraction after a report for B | Held |
  | B | Declaration and reveal of B after its own swap | Participating |
- **Host swap frame.** The harvest tick is the slot's resolved client-tick cursor, the last client tick the host processed for it, not the host's own tick. Re-promotion spawns each seamless slot's pawn at its stored arrival (U2's entry rule) under a `NetworkId` the host assigns then, and both peers seed that pawn's teleport record from the harvested one: accumulated yaw plus the landmark yaw delta, wrapped by the one shared function, the same generation, no last teleporter and no latches. That seed is what lets stamped rotation (`networking.md` §Host input command queue) turn a command sampled on A by exactly the landmark yaw delta and a command sampled on B by none. Re-promotion also re-resolves the tuning payload from the spawned pawn and sends it on Control ahead of the swap message when it differs from the staged one, so a weapon picked up or switched on A after the instruction still composes. Before teardown the host lifts the slot's queued commands for client ticks after the harvest tick, with its cursor and playout state, and hands them to the new pawn. Until the slot's first frame of the new participation epoch arrives, it also accepts input commands and switch declarations still stamped with the retiring epoch for ticks after the harvest tick; Input is reliable-ordered, so that bound holds.
- **Control order.** The split sends each seamless slot a swap notice naming the prefetch generation, so the notice precedes the Holding frames the unload sends and the relevel the install sends. Re-promotion's marker and payload follow, then the swap message. The swap message carries the pose the spawn actually used (the arrival, or the player start a blocked arrival fell back to), the pawn's `NetworkId`, the harvest tick, the seeded teleport record, the prefetch generation, B's catalog id, and, for each other slot the split made seamless, the `NetworkId` its pawn had on A and the one it has on B. That pairing is how the client knows which teammate presentations to carry and which first B record ends each one (Teammates across the commit). A client that reads the notice while its prefetch of that generation is ready and reported defers the engine effects of the Holding frames and the relevel until the swap message: the Holding frames still retire its epoch, but its pawn, prediction ring, held input, weapon state and installed payload stay on A, and the relevel queues no load. A deferred client resolves its deferral at each redraw boundary by the first row matching what it has seen since the notice, so a relevel or release read after the swap message but before the commit still wins:

  | Seen since the notice, by its next redraw boundary | Outcome |
  |---|---|
  | Its own suspend | Applies the deferred effects at once; suspend cancels the prefetch and sends `Revealed(None)`; the deferred relevel stands |
  | A relevel naming another level | Applies the deferred effects, releases the prefetch, discards held traffic and follows that relevel |
  | A release of the prefetch | Applies the deferred effects, discards held traffic and follows the deferred relevel as a classic relevel |
  | The swap message, its prefetch ready | Commits (Client commit) |
  | The swap message, its prefetch not ready | Follows the deferred relevel as a classic relevel (Client commit) |
  | None of these | Stays deferred |

- **Client commit.** A client whose current-generation prefetch is ready when the swap message arrives commits it at its next redraw boundary with no Settling, then declares B's parity and its reveal of B. The commit applies the Prior-commitments Settling exit table as the host's swap frame does, those declarations standing in for the host's net reveal: held movement and look carry across, the accumulator re-arms after the commit, and interpolation endpoints hold at the replayed eye. Its unload of A releases A's registry, A's `NetworkId` mappings and A's level-scoped state, and keeps the prediction ring, the local pawn's predicted state and the newest tuning payload. It sends no `Revealed(None)` and no cleared declaration. Until the commit, the client applies no traffic of the new participation epoch: snapshot records, the re-resolved tuning payload and host messages on Input are held aside and applied after it, Presentation payloads received meanwhile are dropped, and the newest held payload replaces the staged one. A client whose prefetch is not ready follows the deferred relevel as a classic relevel; it keeps that prefetch to commit behind the loading screen (D8), discards the held traffic, and arrives at its assigned `player_spawn`. A client whose spawn the host skipped, because a retraction landed in the swap frame's own poll, gets no swap message and follows the relevel the same way.
- **Predicted arrival.** At the commit the client registers the host-assigned `NetworkId` to its local pawn in B, marks that pawn its local player, and arms prediction with the first record already adopted and the ring kept, so the host's first record reconciles as any later record does rather than being adopted silently. The pawn starts at the pose the swap message carried, with the carried state and an inventory composed from the newest payload, under the seeded teleport record. Its ring commands for client ticks after the harvest tick replay on it from that pose. The host runs the same commands from the same pose under the same payload and record, which, not the seam's identity, keeps the first record's correction small. The seam's identity bounds a different difference: between the eye predicted on A, carried through the landmark, and the eye replayed on B at the commit, which also carries any misprediction in flight at the harvest tick. The commit presents that difference as a reconcile correction: smoothed below the teleport floor, snapped at or past it. View yaw turns by the landmark yaw delta from wherever look has taken it, never back to the harvested yaw.
- **Client lead on an unsealed seam.** The host's blind check walks from its pose for the client's pawn, which trails the client's eye by its input lead, and the swap message reaches the client half a round trip later. A client walking toward a seam's unsealed end, as along a bent hallway, can therefore see past the blind cells before its commit. The host widens each remote eye's walk, in A and from the mapped eye in B (D6), by the travel at sustained movement speed over that input lead plus half a round trip and one frame, the span the client's eye runs ahead of the walked pose until its commit. The swap therefore waits a little longer on a bent hallway in co-op rather than showing A's world turning into B's.
- **Teammates across the commit.** A client presents each teammate who crossed seamlessly at the teammate's A presentation carried through the landmark until that teammate's first B record presents, so a group crossing together never blinks out. B's other entities appear as their first records apply; the blind seam hides those outside it.
- **The exception, and its condition.** This bends `networking.md` rules written for joins (Prior commitments, "Join-time prediction rules"). It applies only when every input to the prediction came from the host or from a level whose parity the host checked: the newest tuning payload and the report's parity for the current generation, the swap notice and swap message, the host-assigned `NetworkId`, the spawn pose, the harvest tick and the seeded teleport record. Missing any of them, the client takes the classic relevel. A client never composes from its own descriptors or a previous level's payload.
- **Wire.** The instruction, release, swap notice, swap message, report and retraction are appended variants of `ServerControlMessage` and `ClientControlMessage`, advancing the application protocol from PRLC to PRLD; a change to a shipped message's layout also advances `WIRE_VERSION` from 35 (`networking.md` §Version gates).
- The relevel-at-request non-goal in `drafts/level-load-pipeline/` (a failed host parse would strand clients) is answered by the speculative prefetch: a client never commits a level the host did not.

**Depends on.** U3.

**Key acceptance.** AC 22–30.

**Brief form.** Resumable brief; it widens the participation predicate's inputs, adds Control messages and bends the join-time prediction rules for one case, but adds no lifecycle edge.

## Acceptance criteria

### Automated

**U1**
1. The `level-load-pipeline` brief's own acceptance list, as amended.
2. A load's stages can run while a level is installed and running, and nothing they create reaches the frame path before commit. Unloading the running level then releases nothing the ready bundle holds and cancels nothing, and the bundle commits.

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
14. The wait follows both of D7's tables on every row: **waiting** at most once per wait, **ready** at most once and only on the evaluation that swaps, nothing for activations during a wait, nothing when the swap can already proceed, and no swap or **ready** for a wait whose seam empties. Diagnostics count each wait.
15. With the seam's door open, a swap waits; with it docked closed, the swap proceeds. A bent-hallway seam swaps from its blind cells and not from a cell that sees past a bend. A seam whose next-map copy spawns its door open never swaps.
16. A walk that trips its step cap or falls back to frustum culling never permits a swap.
17. prl-build warns on a seam that can never be blind, and on a sealing mover that does not spawn docked closed, and stays silent on a seam with neither.
18. A failed prefetch and an activation whose prefetch or pre-commit settle stays unready for the Settling timeout on the wait's clock each take the classic path with one warning. A wait whose settle pends only after its prefetch read ready starts its clock at that pend, not at activation. A seam held only on its blind check never times out.
19. Peak resident CPU and GPU bytes with two levels are reported per `testing_guide.md` §Resource bounds and stay within the 2 GB VRAM target on the reference fixtures. When a second seam's prefetch supersedes the held one (U3's table), the held one is released before the new one uploads, so no more than two levels are ever resident.
20. A shadow-tier change during a prefetch restarts it, and a Surface Depth change made then is live on the first frame after the swap.
21. Co-op, until U4 lands: at a host's swap each client takes the classic relevel and arrives by D4 or D5, and the host publishes its reveal of the next map on the swap frame, so every client re-promotes.

**U4**
22. Clients prefetch when the host does, including a slot promoted during the prefetch, and a client prefetch never commits without the host's swap. Each instruction carries the host's tuning payload for that level, and the client installs a payload only at its seam commit. A report of a superseded generation, and a retracted report, count as not ready; a current report whose parity differs never holds the swap. A host held only on client readiness fires **waiting** once and **ready** on the evaluation that swaps, and reads D7's tables per U4's table on every row.
23. At a host swap at the reference simulated latency, a ready client crosses with no loading screen and no movement hold: its input applies on every fixed tick across the commit, and its frame after the commit runs no catch-up ticks for the commit's duration. With its arrival unblocked, its view moves only by the landmark mapping and a smoothed correction that decays as reconcile's does. With its host arrival blocked at spawn, it replays from the player start the swap message carries and its view snaps once, at its commit. Either way the host's first record refreshes its pawn in place, with no second pawn, reconciles as any later record does, and its correction stays below the teleport floor on the reference fixtures.
24. A client not ready by the timeout, one whose report arrives after the host's swap frame, and one whose pawn is outside the seam each take the classic relevel; the others still swap seamlessly. A client given the classic relevel while its prefetch of that level stands commits that prefetch rather than loading the level again. The host never waits on a client outside the seam, and with no participating client inside it the swap does not wait.
25. A host whose prefetch fails takes D8, sends every client a release, and clients follow the classic relevel.
26. A ready client's slot leaves and re-enters participating on the host's swap frame, through the same predicate as any level change, with its current report standing in for its declaration and reveal; the predicate follows U4's table on every row, including a declaration and a reveal of B that land in different host polls. Its report never demotes it while the host is still on the old map, and a classic path to the same level never reads it.
27. The client's seam commit sends no `Revealed(None)` and no cleared declaration, keeps its prediction ring and the newest tuning payload, and applies no new-epoch record, payload or Input host message to A's registry; Presentation payloads received before it are dropped. No client composes from its own descriptors or a previous level's payload.
28. The host hands a seamless slot's queued commands for client ticks after the harvest tick to the new pawn, and accepts that slot's retiring-epoch commands for later ticks until its first new-epoch frame. Commands sampled on A turn by exactly the landmark yaw delta and commands sampled on B by none, including after an earlier teleport in A, and the first record's teleport record matches the prediction's.
29. A ready client that read the swap notice keeps its pawn, ring and payload on A through the Holding frames and relevel that follow, and queues no load. A client lacking any input U4's exception names, one whose prefetch is not ready when the swap message arrives, one whose spawn the host skipped, and one that suspends before its commit each take the classic relevel and arrive at their assigned `player_spawn`; one sent a relevel to another level releases its prefetch and follows it.
30. On the bent-hallway pair in co-op, a client walking toward the open end at sustained movement speed never sees past the seam before its commit, and a teammate crossing with it never disappears from its view.

### Manual

- Playtest the airlock pair and the bent-hallway pair, single-player and co-op at simulated latency. Watch for a lighting, audio or entity pop at the swap; the seam's two bakes can differ (Open questions).
- Outrun a prefetch on purpose and confirm the door's wait reads as the door powering up.
- Co-op: watch for any correction or pop as a client crosses at realistic latency, walking the bent-hallway pair toward its open end and crossing as a group.
- Metal: swap-frame timing and peak memory on the reference pairs.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| I1 — Exactly one level simulates and draws per frame | U3 swap commit | Prefetch install; streaming installs for the prefetched level | AC 2, 11 |
| I2 — No frame mixes two levels' world data | U1 commit; U3 swap | Pre-commit settle writing into shared pools; renderer resources keyed globally rather than per level; the swap's unload releasing B | AC 2, 13 |
| I3 — A seam carries the session-state ledger plus each seam occupant's landmark-relative arrival, nothing more: body offset, view yaw and pitch, the velocity channels and the movement state payload, all read before unload; under U4 also, for a seamless slot, the host's queued commands for client ticks after the harvest tick with the slot's cursor and playout state, the harvested teleport record the new pawn's record is seeded from, and on the client its prediction ring, its local pawn's predicted state and the newest tuning payload | U2; U4 | U3 swap path diverging from the classic harvest; unload resetting the host's command queues, despawning the pawn and resetting the camera before the arrival is read; a stale arrival surviving into a later level; a level change clearing a seamless slot's queue or the client's ring | AC 3, 4, 11, 27, 28 |
| I4 — Arrival comes from the one landmark → target transform and the one arrival rule teleports use | U2 | A seam-specific copy of the math or of the arrival-state rules | AC 3 |
| I5 — At most one prefetched level | U3 | A second prefetch region; a level change during prefetch | AC 10, 19 |
| I6 — The host owns every swap; a client never commits a level the host has not | U2 (relevel), U4 | Client prefetch completing first | AC 22, 24 |
| I7 — A seam never strands a player | U3 | Prefetch failure, cancellation, suspend, a prefetch or settle that never completes | AC 10, 18, 25 |
| I8 — Participation changes only through the one predicate | Existing slot lifecycle; U4 adds the readiness report as one of its inputs | A report read while the host is on another level; any seam-specific swap path | AC 26 |
| I9 — A client predicts before its pawn's first record only at a seam commit, and only from host-sent or host-checked inputs | U4 | A payload staged for a superseded generation; a local descriptor fallback; a second pawn under another identity; a Holding or relevel handler acting before the swap message | AC 22, 23, 27, 29 |

## Pinned orderings

Each row is a test the named unit's brief carries.

| Unit | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| U2 | Latch clears on capsule exit | touch seam latched → body point leaves, capsule still overlaps → capsule leaves → re-enter | No fire while the capsule overlaps; re-entry fires |
| U2 | Level change before a co-op client arrives | seam crossing A→B → client still loading B → host changes B→C | Client arrives in C at a player start; its B arrival cleared |
| U3 | Touch seam entered with its prefetch ready and settled | bundle polled ready → settle answers settled → capsule enters the touch seam on a tick | Swap at the next boundary; no **waiting** |
| U3 | Prefetch completes during the activation frame's ticks | activate → completion → poll at the next boundary → settle drains | **waiting** on the activation frame; **ready** on the first evaluation at or after that poll whose settle answers settled; swap at the boundary after it |
| U3 | Blind on an early tick, not on the last | activate on tick 1 (blind) → door starts opening on tick 3 → boundary | No swap that frame; evaluation reads tick 3 |
| U3 | Two activations on one tick | activate × 2 | One swap |
| U3 | Activation repeated during a wait | activate → **waiting** → activate × N | No further **waiting**; one **ready**, then swap |
| U3 | Ready prefetch, seam not blind | prefetch ready → door open → activate | **waiting**; **ready** when the door docks closed; swap |
| U3 | Seam empties while waiting | activate → **waiting** → last player inside leaves → prefetch ready | Wait ends; no swap, no **ready** |
| U3 | Player leaves the prefetch region before activating | enter → leave | Prefetched level stays until superseded or a level change, unless the U3 brief adopts release on leaving (Open questions) |
| U3 | Second seam's region entered | prefetch A→B ready → every player leaves its region → enter A→C region | B released, C prefetched (D3) |
| U3 | Second seam's region entered while the first is occupied | prefetch A→B → one player stays in its region, another enters A→C region | B kept; C not prefetched |
| U3 | Second region entered during a wait | activate A→B → **waiting** → enter A→C region | B kept; C not prefetched |
| U3 | Prefetch and activation on one tick | a teleport arrival or a spawn inside a seam whose region holds no prefetch (a seam arrival is latched, D11) | Prefetch targeted first; the activation waits (D7), never D8 |
| U3 | Prefetch fails while its region stays occupied | prefetch → failure → player stays | No retry before a level change; activation takes D8 |
| U3 | Prefetch fails after activation | activate → **waiting** → failure | Classic path with one warning (D8); **ready** never fires |
| U3 | Prefetch never completes | activate → **waiting** → Settling timeout on the wait's clock | Classic path with one warning (D8); **ready** never fires |
| U3 | Level change during prefetch | prefetch → `loadLevel` / `restartLevel` / return to frontend | Prefetch cancelled; the requested change proceeds |
| U3 | Script level request on the swap's frame | evaluation passes; `loadLevel`, `restartLevel` or `returnToFrontend` dispatches the same frame, either order | The script request proceeds; swap dropped, prefetch released; **ready** fires only if the evaluation ended an open wait (D7), as when suspend falls between evaluation and commit |
| U3 | Prefetch target and level change on one frame | region entered; `loadLevel` dispatches the same frame | The change proceeds; the prefetch never starts |
| U3 | Two seams activated on one frame | A→B prefetched, A→C not, both activated | A→B evaluates; the A→C activation drops with no event and no D8 |
| U3 | Suspend during prefetch | prefetch → suspend | Prefetch cancelled; resume does not restore it |
| U3 | Suspend between evaluation and commit | evaluation passes → **ready** → suspend → resume | Queued swap and prefetch dropped; no swap commits; resume re-drives boot to its destination (`boot_sequence.md` §5), never into B |
| U3 | Door opened while waiting | activate → **waiting** → door opens | Swap holds until blind again |
| U3 | Arrival inside the return seam | swap → arrival inside B→A seam | Latched (D11) before B's first trigger tick; B→A prefetch targeted on that tick and requested at the next redraw boundary; the swap cannot fire until the player leaves and re-enters |
| U3 | First frame after a swap | commit → B's first fixed tick | `levelLoad` reactions run at commit; the arrival event fires on B's first fixed tick; their queued commands dispatch in that order |
| U3 | Player dies inside a waiting seam | activate → **waiting** → death | Open (Open questions) |
| U3 | Activation on a superseded seam | prefetch A→B → player enters the overlapping A→C region, C supersedes B → player enters the A→B seam | **waiting**; the next tick targets B; **ready** and swap once B is ready and settled; no D8 before the Settling timeout |
| U3 | Wait ends as another seam activates | A→B wait open → on one tick its last player leaves and another player enters the A→C seam | B's wait ends with no event; A→C takes D7's table, opens its wait and is targeted on the next tick |
| U3 | Dropped touch activation | A→B and A→C activated on one frame → A→C drops → A→B swaps or its wait ends, A→C player still inside | No A→C activation until that player's capsule leaves and re-enters (D11) |
| U3 | Prefetch restarts after Ready during a blind hold | activate with the door open → prefetch ready → hold past the Settling timeout → shadow-tier change → door docks closed | No D8 at the restart; the wait's clock starts at the restart's first unready evaluation; swap once ready again |
| U3 | **ready** reaction queues a command | **ready** → its reaction queues a sound and a script state write → commit | Both run on the evaluation frame against the current map, before the commit |
| U3 | D8 and a script level request on one frame | evaluation takes D8; `loadLevel` dispatches in the same frame's post-tick drain | The script request proceeds; the D8 load is replaced |
| U3 | Multi-tick swap frame | slow evaluation frame → commit → the swap frame runs several fixed ticks on B with held movement | No catch-up ticks for the commit's duration; the frame draws after those ticks, with seam cells settled; cells first seen outside the seam stream as in play |
| U4 | Report for a superseded prefetch | report for B arrives after the host supersedes with C | Not read; the slot is not ready for C |
| U4 | Client prefetch lost after reporting | report → client suspend, cancel or failure | Retraction; the slot is not ready |
| U4 | Slot promoted during a prefetch | instruct → slot enters participating → its pawn enters the seam → activation | Slot instructed on promotion and counted in the wait while its pawn is inside the seam |
| U4 | Report races the timeout | timeout expires → swap frame → report arrives | Slot already given the classic relevel; report not read; the client finishes its prefetch under the loading screen instead of loading B twice |
| U4 | Waited-on client closes | activate → **waiting** → close | The host stops waiting on it |
| U4 | Host prefetch fails after reports | reports in → host prefetch fails | D8 classic relevel; every client prefetch released |
| U4 | Ready client crosses | host swap frame (demote, commit, re-promote) → swap message → client commit → declaration of B → first record | Input applies every tick; the pawn is predicted from the arrival at the commit; the first record refreshes it in place |
| U4 | Client ticks on A after the host's harvest | host harvest → client keeps moving on A under its retiring epoch → swap message | Those commands reach the host's new pawn turned by the landmark yaw delta; the client replays them on B from the arrival |
| U4 | Commands queued before the host's swap frame | client commands for ticks after the harvest tick already in the host's queue → host swap frame | Kept and applied to the new pawn, turned; not cleared |
| U4 | Report arrives on the evaluation frame | client report → host net poll → that frame's evaluation passes | The report counts: the net poll precedes the evaluation, and none runs before the commit |
| U4 | Retraction arrives on the host's swap frame | evaluation counts the report → client retracts → host split spends the arrival → host net poll reads the retraction | Slot held; no spawn, no swap message; the client follows the install's relevel as a classic relevel and arrives at its assigned `player_spawn` |
| U4 | Swap message before the client's prefetch is ready | host swap → swap message → client bundle still loading | Classic relevel to the message's catalog id; prefetch kept to commit behind the loading screen; held traffic discarded |
| U4 | New-epoch traffic before the client's commit | host re-promotes → re-resolved payload, B records and Input host messages arrive with the swap message → client commit at its next boundary | Held aside until the commit, then applied; Presentation payloads dropped; no B traffic reaches A's registry |
| U4 | Client's own seam commit | client commit unloads A → declaration and reveal of B | No `Revealed(None)` sent; the newest tuning payload installed, the re-resolved one when re-promotion sent it; the slot stays participating; no second pawn spawn |
| U4 | Host arrival blocked at spawn | host's arrival blocked → pawn at a player start → swap message carries that pose | The client replays from the player start; the commit's correction is a teleport-class snap; the first record reconciles as any record does |
| U4 | Host timeout expires while the seam is not blind | evaluation passes every term but client readiness → door opens → timeout expires → door docks closed | Swap on the first blind evaluation; slots whose report arrived by then cross seamlessly, the rest take the classic relevel |
| U4 | Host timeout of zero | the U4 brief pins zero → activation with a client not yet reported | The first evaluation that passes every other term swaps; that client takes the classic relevel |
| U4 | Holding and relevel ahead of the swap message | swap notice → unload's Holding frames → install's relevel naming B → re-promotion marker and payload → swap message, all in one client drain | The ready client keeps its pawn, prediction ring, held input and weapon state on A until its commit; no load is queued; it commits at its next boundary |
| U4 | Retiring-epoch input after re-promotion | client sends commands for ticks after the harvest tick under its retiring epoch → host re-promotes → those frames arrive → the client's first new-epoch frame arrives | Retiring-epoch commands apply to the new pawn turned by the landmark yaw delta; retiring-epoch input after the first new-epoch frame is dropped |
| U4 | Facing across the commit after an earlier teleport | player teleports in A → harvest → commands sampled on A, then on B | Host and replay turn A-sampled commands by exactly the landmark yaw delta and B-sampled ones by none; the first record's teleport record matches the prediction |
| U4 | Host resolved the harvest tick differently | client's command for the harvest tick arrives late and the host resolves it held → harvest → client commit | The commit smooths the difference as a reconcile correction; no pop |
| U4 | Client at the harvest tick | the client's newest command is for the harvest tick when the swap message arrives | Pawn at the arrival; nothing replays; the first record reconciles as any record does |
| U4 | Payload changes after the instruction | instruct with B's payload → player switches weapon on A, or B's `levelLoad` calls `setGravity` → swap | The re-promotion payload replaces the staged one; the client's pawn composes from it and predicts with B's current gravity |
| U4 | Parity declaration and reveal in different host polls | client commit → its declaration of B lands in one host poll → its reveal of B in a later one | Participating throughout; no demotion, no second pawn |
| U4 | Classic path to the prefetched level | client reports B → host takes D8 on its settle timeout, or a script `loadLevel` of B → host reveals B | Report not read; the slot is held until the client declares and reveals B |
| U4 | Host leaves B before the client's commit | swap message → host changes B→C → the client reads both in one drain | The client never commits B: it releases the prefetch and follows C as a classic relevel |
| U4 | Client suspends before its commit | swap message read → client suspends | `Revealed(None)` sent; slot held; the deferred relevel stands, and the client reaches B by the classic relevel at its assigned `player_spawn` |
| U4 | Client outside the seam at the swap | evaluation passes with one client inside, another outside → swap | The host does not wait on the outside client; it takes the classic relevel and arrives at a player start |

## Boundary inventory

Leanings, pinned by the U2 brief.

| Concept | FGD | Notes |
|---|---|---|
| Seam volume | brush entity, e.g. `level_seam` | keys: target map catalog id, seam name, activation (`touch` / `use`), mode (`classic` / `seamless`), optional landmark name, armed on spawn (`enabled_on_spawn`, as `trigger_volume` and `trigger_teleport` spell it) |
| Landmark | point entity; reuse `teleport_destination` as ghost links do, or a new name | absent → bounds centre, yaw 0 |
| Prefetch region | brush entity naming its seam | absent → generated by prl-build |
| Events | `waiting`, `ready`, and an arrival event on the destination seam | a seam event source beside the trigger and teleport sources: trigger events accept only `enter` and `exit`, and teleporters keep a separate `teleportEvents` list accepting only `teleport`; FGD reaction keys follow `trigger_volume` `on_fire` and `trigger_teleport` `on_teleport` |

## Open questions

- **Prediction across the swap.** The tuning payload is built per spawned pawn: it carries the live inventory, the spawn active slot and the installed level's gravity (`networking.md` §What gates), so it differs between levels and goes stale after the instruction. The staged payload covers the environment; re-promotion's re-resolved payload replaces it before the commit. How far the widened remote walk delays co-op swaps on a bent hallway, and how a carried teammate presentation hands over to its first B record — measured and pinned in the U4 brief.
- **Lighting parity across a seam.** D6 checks that a seam is blind to sight, not to light: a bend unsealed against outside light pops at the swap. Lean: a per-map prl-build warning when any light outside the seam reaches its cells, directly or through SH; two identical copies that both pass bake alike. U3 brief.
- **Death at a seam.** Whether a dead or downed pawn (E18 player life states) activates or crosses a seam, given that teleporters bar dead pawns (`movement.md` §6), is for the U2 brief. Whether death inside a waiting seam blocks the swap or sends that player to a player start is for the U3 brief. Both are decided against the life-states spec.
- **Non-player entities inside a seam.** Whether a hostile actor inside the active seam holds the swap rather than vanishing with its level — U3 brief.
- **Cost of the blind walk on the host.** All-directions walks per player in a seam, once per frame while activated or waiting (D6 evaluation point). Measured in U3.
- **Swap-frame cost after U1.** The script VM and registry stages stay on the main thread, and the first frame after the swap pays the first light-bridge update, compose dispatches and a full-weight cull (lavapipe `first_level_frame` ≈ 0.46–0.81 s on a change, unmeasured on Metal). If commit plus that frame exceed the swap budget, U3 needs its own lever.
- **How long a prefetched level stays.** The Pinned orderings default keeps it until superseded, so the two-level peak can last the rest of the map. A release on leaving the prefetch region, with hysteresis, is the alternative — U3 brief, against AC 19.
- **Two-level peak memory.** Unmeasured. World textures alone are about 80–96 MiB per level (`drafts/level-load-pipeline/research.md`).
- **Audio across the swap.** Level sounds reload per level; ambient loops cut at a swap unless carried or crossfaded — U3 brief.
