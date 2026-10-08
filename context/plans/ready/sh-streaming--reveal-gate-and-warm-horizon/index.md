# sh-streaming--reveal-gate-and-warm-horizon

Brief · resumable · reads: `context/lib/rendering_pipeline.md` §Cluster SH residency, §4 (Lightmap cell-block residency), `context/lib/build_pipeline.md` §Cell residency set (id 51), `context/lib/boot_sequence.md` §1 (Loading screen), §3, §4, `context/lib/networking.md` §Two-gate handshake, §Admission and content parity, §Slot lifecycle, §Session-state ledger, `context/lib/testing_guide.md` §Resource bounds · `context/plans/large-map-spatial-residency.md` (Reach bound) · read at ef855f247

**Sequence: 3 of 4.** `E23--preferences-comfort-floor` and `animated-lightmap-compact-atlas` (1 of 4) have landed. It shares install and host-net files with `coop--spawn-placement-occupancy` (2 of 4) but not behavior; rebase on it if it lands first. `coop--client-spawn-view-handoff` (4 of 4) and `level-load-pipeline` build on this brief's Settling state, presented-pose chokepoint, settle chokepoint and revealed hold.

## Problem
The owner saw SH lighting fill in on screen, both on level entry and in play. Basis: an observed defect. On entry, the level is revealed as soon as CPU install finishes. SH residency starts empty, so the first frames show the ambient-floor fallback. Lightmap blocks the spawn view draws outside the cell's baked set also miss. In play, SH prefetch is a small fixed count of clusters from an id-46 walk, so walking a few rooms ahead, or returning after the hysteresis window, brings a cluster into view cold. The goal is the first *complete* frame, not the earliest frame. A world shown before it is fully realized breaks immersion, and load time is not bought with it (owner, 2026-10-08). When done, a level is revealed only once every streamed resource its first frame draws is resident, or a generous timeout has run out. In co-op, a client's pawn spawns only once both it and the host have revealed. In play, SH holds what lightmap already holds: everything visible from the cells within the movement lead L.

## Decisions
- **Settling boot state.** A new state sits between Loading and Running. The sim does not tick, no world frame is presented, and no level sound starts; level sounds start at reveal. Unanchored music and UI sounds that began before the load keep playing, as they do through Loading. The sim-not-ticking rule is load-bearing, because nothing may act on the player while they can't see. Settling frames run visibility and the level's streaming from the settle pose until the settle check passes. Precedent: the first-launch hold is also its own boot state, not a pause flag (`boot_sequence.md`).
- **Settling counts as installed.** Request draining, parity, script hot-reload and observe-live treat a Settling level as installed, as in Running. Only the sim tick, the world present, level audio, UI input, the system-command drain and the world net poll wait for reveal; Settling polls world-less.
- **One presented pose (owner, 2026-10-08).** One chokepoint resolves the pose the first Running frame presents: position, yaw and pitch. Whenever the frontend menu is present it is the menu pose. That covers a backdrop install and a co-op client following a Relevel with the menu pushed, which install cannot tell apart. Otherwise it is the spawn pose. Install, Settling and every settle target read it. Whether a relevel should dismiss the menu belongs to `coop--client-spawn-view-handoff` or the frontend hub.
- **Settling is a Loading frame to the player.** The loading tree stays drawn, never the bare splash. The E23 rule that drops UI input pressed on a non-UI frame covers Settling. The flash limiter's clamp ages its window across a Settling stretch, as it does across Loading. `loading.progress` keeps rising through Settling, never decreases, and reaches 1.0 before reveal. A bar that stalls through the settle would read as a hang.
- **One settle chokepoint.** One named lifecycle check decides whether Settling may end. It asks each streamed resource whether the settle pose's set is resident: SH Sampleable, lightmap installed. A resource that does not stream for the level answers settled. A streamed one that has not yet updated demand from the settle pose does not.
- **The settle set is what the first frame draws, plus the lead.** SH contributes its Visible and Pinned targets with their owner closure, plus its lead tier. Lightmap contributes its mandatory set and every block the settle pose draws, on any visibility path. During Settling, drawn blocks are requested even where play only holds them. That is a one-off read bounded by the timeout, as capture already does. The lead is in the set so the first steps after reveal are not cold.
- **Settling replaces the synchronous spawn lightmap preload.** Otherwise two mechanisms would guarantee the same thing. The lightmap plan says "whichever lands second adopts the other's seam" (`plans/done/spatial-residency--lightmap-cell-blocks`).
- **Every level entry settles.** This covers the boot map, a catalog load, `restartLevel`, a network relevel, and the frontend backdrop, which the menu is drawn over.
- **Timeout, never hang.** Settling releases after a fixed timeout, default 10 s. It logs one warning naming the unsettled target count per resource, then reveals with each resource's existing miss fallback. A timed-out reveal is a reveal for every purpose, co-op included. The timer starts fresh on each entry to Settling. A load or unload request during Settling follows the ordinary Unload→Loading path, and no reveal edge fires for the abandoned settle.
- **In-play misses still never stall.** The gate applies only at level entry. Doors never wait on residency. In play, an SH miss still falls back to ambient floor and a lightmap miss to SH-only (`plans/done/sh-probe-streaming` §Open questions).
- **Promotions on the reveal frame start whole.** A baked light promoted on the reveal frame starts at full weight, since an entry has nothing to fade from. Later promotions crossfade as today.
- **SH reach is id 51, the lightmap's reach (owner, 2026-10-08).** This replaces the cluster-count warm horizon.
  - SH's mandatory tier holds Visible, Pinned, owner closure, and the clusters of id-51 cells within lead L. Mandatory is never refused: the pool grows past the budget to hold it.
  - The optional tier holds band clusters (lead between L and the baked maximum) and seam-warm targets. Budget pressure trims it, farthest lead first.
  - On every visibility path, SH still requests drawn clusters as Visible. Only the lead and band tiers follow id 51.
  - A level without a usable id 51 has no lead or band tier, at entry or in play.
  - Warrant: id 51 is keyed by cell so that "a later streamed resource can map the same cells to its own units" (`build_pipeline.md`), and the epic decided reach is visibility-based (`large-map-spatial-residency.md`, Reach bound).
- **One cell-demand stage (owner, 2026-10-08).** A level-scope stage owns L. It turns id 51 and the frame's visibility path into per-cell classes, and each resource maps cells to its own units. Lightmap's id-51 demand moves onto it. With one stage, "one L, one reach" holds by construction, including when the lightmap does not stream or is declined.
- **Departure eviction stays (owner).** Hysteresis departure still evicts an untargeted Sampleable cluster, even on a map that fits its budget. This differs from the epic's stance that a miss fallback cannot stand in for residency on interior maps. The wider mandatory tier makes such a departure rarer, and backtracking is uncommon in some retro-FPS styles.
- **Measure before building the reach.** SH bytes held by the lead tier are unmeasured. The first slice reports SH mandatory bytes per camera cell at the default L, on the largest stress map and on campaign-test. If the worst cell's SH mandatory bytes, with fixed metadata, exceed the 256 MiB SH floor, the executor stops for the owner. The fallback is id 51 within L for the entry settle set only, keeping the cluster-count warm horizon in play.
- **Pop-in evidence.** Diagnostics count clusters read again after being evicted. The SH miss counter puts each visible miss in exactly one bucket per cause, each pointing at the lever a later change should pull: outside the reach, trimmed by pressure, read in flight, held by the per-drain budget, awaiting compose, or failed (owner, 2026-10-08). Settling frames count no misses. Line C's first-level-frame mark is the reveal frame.
- **Co-op: Settling polls; parity is content identity.** The endpoint keeps polling through Settling, so a long hold drops no peer. Level parity, Relevel and the join seed publish at install, as today, so a content-divergent client learns its cause during its settle.
- **Revealed is a participation term.** A slot participates iff parity matches and both peers have revealed the host's installed level. The client declares on Control the level identity it has revealed, and none at unload or suspend. The host records its own reveal locally and clears it at unload and at suspend, since a resumed install runs no unload. The name is "revealed", never "ready", which `context/research/coop-session-lobby.md` §2 reserves for mod-authored meaning.
- **The host half is load-bearing.** A Settling poll's promotion handler spawns no pawn. A restarted host must therefore not promote a revealed client before it reveals itself.
- **The pawn hold is admitted, not a new stage.** A parity-matched slot that has not revealed stays admitted, held with a revealed holding cause. The four stages and "any entry to participating spawns its pawn" stand. Rival shapes: `research.md`.
- **Revealed lifetime.** A slot's revealed record survives parity changes and host level changes. Only the client's own declaration or the slot closing changes it. A mod-digest demotion therefore re-promotes without a fresh reveal, and a restarted host re-promotes a Running client at its own reveal. Neither peer runs a timer: the Settling timeout bounds a live client, and keepalive bounds a silent one.
- **Reveal at settle, not at arm.** The client declares revealed on its Settling→Running frame. The pawnless window that follows is inferred, not measured, at about one round trip plus a snapshot interval. During it the client sends no input, and nothing acts on its player. Holding the loading tree until arm would need snapshot apply during Settling, which the world-less poll excludes.
- **Co-op layer placement.** The net crate owns the revealed record and the predicate term. It carries level identities as opaque strings and stays registry-blind. The engine publishes each peer's reveal from the Settling→Running edge.
- **Non-goals.**
  - Budget tiers and laptop defaults: they wait for a resource-neutral budget (`large-map-spatial-residency.md`, Decisions still open).
  - Particle emitter prewarm, for a later brief not yet drafted. Rate emitters start with no particles on the reveal frame. Fixing that is a modder-facing descriptor choice, separate from streaming.
  - Lightmap drawn-block demand on non-portal paths in play: the frustum-all fallback can draw the whole map, and that policy belongs to lightmap residency.
  - Changing the per-drain budget or read coalescing in play, unless measurement implicates them. Settling's budget is an open question.
  - The co-op client's view at its real spawn, owned by `coop--client-spawn-view-handoff` (4 of 4). A client learns its spawn only from the first baseline after promotion, so it settles at the first `player_spawn`. Until 4 of 4 lands, its first armed frame can jump to a cold view.
  - A waiting overlay for a held co-op client, which is UI policy. Accepted: after a host restart, a client sees the level and cannot act for up to the host's timeout, with only the holding diagnostic.
  - Shortening the load itself: `level-load-pipeline` owns that.

## Acceptance
Pins `P#` are in `research.md` §Ordering pins.

### Automated
**Settle predicate**
- [ ] Not settled while any SH target in the settle set (Visible, Pinned, owner closure, or a lead-tier cluster) is below Sampleable. Also not settled while any lightmap block in the settle set (mandatory, or drawn by the settle pose) is not installed.
- [ ] Settles with a band cluster, a seam-warm target, or a lightmap band block still cold.
- [ ] Drawn lightmap blocks outside the baked set block settling, and so do drawn blocks on a non-portal visibility path. Each settles once those blocks install.
- [ ] Settled immediately when nothing streams: SH off or no valid id-49/id-50 pair, and lightmap all-resident.
- [ ] A streamed level does not settle on a Settling frame that precedes its first demand update from the settle pose. "Nothing streams" is decided by the level's sessions, not by an empty target list (P1).
- [ ] A level with streamed SH and no usable id 51 settles on Visible, Pinned and owner closure alone. In play, it targets no lead or band tier.
- [ ] With a budget below the settle set's bytes, Settling still settles before the timeout; the settle set is admitted past the budget.
- [ ] Capture's SH preload waits on the settle chokepoint's SH answer, not on optional targets, and settles on a map whose band exceeds the SH budget.

**Settling lifecycle**
- [ ] Boot map, catalog load, `restartLevel`, frontend backdrop and network relevel each pass from install into Settling, never straight to Running.
- [ ] The sim tick count, game time and timed-reaction frame counter do not advance during Settling, and the system command queue is not drained. The first tick and the first drain come after the switch to Running (P12).
- [ ] No world frame is presented and no level sound starts during Settling. The loading tree is drawn on each redraw. Level sounds queued by install stay in the system command queue through Settling and are dispatched on the reveal frame.
- [ ] No Settling frame releases the level's streaming sessions. The SH and lightmap sessions install created are still current on the reveal frame, and the loading tree's active load is not ended before it.
- [ ] `loading.progress` never decreases across Loading and Settling, including when the settle set's total changes mid-Settling (P10). The last Settling frame's loading tree reads 1.0 before the loading state resets at reveal, including on a timed-out reveal.
- [ ] A UI input pressed during Settling activates nothing on the first Running frame. The channel clamp ages its flash window across Settling by the stretch's length.
- [ ] A cluster reaches Sampleable and a block reaches installed during Settling, without a switch to Running. Sampleable requires a submitted compose, so this proves the drain, compose and submit all run.
- [ ] Install performs no synchronous lightmap read: no production path calls the spawn preload (grep gate). The spawn set arrives through Settling's drains.
- [ ] Install, Settling and the first Running frame read one presented pose, orientation included. A backdrop install, and a client that follows a host Relevel while the frontend menu is pushed, settle at the menu pose. With no menu present, a client settles at its first `player_spawn` pose.
- [ ] On a reveal that settled (not timed out), the reveal frame records no SH visible miss and no lightmap drawn block that is not resident. This holds for a pawn spawn and for a frontend backdrop (P1, P9).
- [ ] The timeout releases to Running and emits exactly one warning, verified by log capture. A settle just before the deadline, or on the deadline frame itself, releases with no warning (P3).
- [ ] `restartLevel` after a timed-out entry gets a full, fresh timeout.
- [ ] A load request during Settling unloads and reloads, with no stale settle or stale timer carried over. A load or unload request drained on a frame whose settle check would pass unloads the Settling world first, and no reveal edge fires for it (P2).
- [ ] A suspend during Settling fires no reveal edge and leaves no settle state or timer for the next entry (P7).
- [ ] Settling entered while the previous level's streaming generation is still retiring issues no read until it joins, then settles without a further reload (P8).
- [ ] A staged script commit landing during Settling recomposes the installed level's reactions, as in Running (P13).
- [ ] A light promoted on the reveal frame renders at full weight on that frame. A light promoted on a later frame ramps over `PROMOTE_SECONDS`.
- [ ] Regression guard: after Running, a visible cluster that is not resident renders the ambient-floor fallback on that same frame. No frame waits for it.

**Co-op: a pawn spawns only once both peers have revealed**
- [ ] Both peers publish level parity on the install frame. A content-divergent client receives its divergence cause while still in Settling.
- [ ] A parity-matched client in Settling is held: no pawn, no tuning, no snapshot, no epoch marker. Its pawn spawns on the host poll that drains its revealed declaration. This holds when its parity and revealed declarations land in one host batch: one promotion, one pawn.
- [ ] A client whose Settling times out still declares revealed on its reveal frame. A host whose Settling times out records its reveal and promotes a revealed, parity-matched client (P6).
- [ ] Revealed before promotion: a client that declares revealed while parity mismatches is promoted the moment parity matches, with no second declaration.
- [ ] Revealed never arrives: the slot stays held with the revealed cause, stays connected past the Settling timeout, and receives no entity state.
- [ ] A duplicate revealed declaration for the current level causes no second participation entry, pawn or epoch. A declaration naming a level other than the host's installed one does not promote.
- [ ] Client unload or suspend retracts revealed (P7). A re-install of the same level is held until its own reveal, even when the host's Control drain collapses the unload and the install into one batch.
- [ ] While the host is in Settling, a parity-matched revealed client is not promoted, no pawn spawns for it, and it stays connected across a hold of at least the timeout. It is promoted exactly once, on the host's reveal frame, and that promotion spawns its pawn (P4). This holds for a fresh host and after a host `restartLevel` of the same level identity (P5).
- [ ] A host suspend clears the host's own reveal. After resume reinstalls the same level identity, a revealed, parity-matched client is not promoted until the host's new reveal (P15).
- [ ] Host restart: a Running client is re-promoted at the host's reveal without re-declaring. A client still settling is promoted exactly once, after both reveals, in either order. This holds when the host's Relevel reaches the client during its own Settling: the client does not reload (P14).
- [ ] A mod-digest demotion and recovery re-promotes a Running client without a new revealed declaration.
- [ ] A client that disconnects while held spawns no pawn, and its revealed record is gone. Its rejoin is held until it declares revealed again.
- [ ] A revealed-only demotion sends a Holding diagnostic and retires the client's epoch.
- [ ] The revealed declaration and both revealed holding causes survive an encode/decode round trip. A peer on the previous wire constants is refused at gate 1.

**SH reach, cell demand and diagnostics**
- [ ] SH mandatory targets equal Visible, Pinned and owner closure, plus the clusters of the camera cell's id-51 entries with lead ≤ L. A cluster whose cells all lie past L is not mandatory.
- [ ] One L drives both resources. A change of L changes both mandatory sets. SH's lead tier uses L when the level streams no lightmap, and is unchanged when the lightmap is declined mid-level or mid-Settling (P11).
- [ ] On a non-portal visibility path in play, SH still requests every drawn cluster as Visible. Its lead and band tiers come from the cell-demand stage's classes for that path.
- [ ] The Streaming tab's lead slider, capture's reported `lead_metres` and the walk measurement read and set the level-scope L. The slider is present on a level that streams SH only, and after a lightmap decline.
- [ ] Over budget, mandatory SH targets are still admitted (the pool grows), while band clusters yield farthest lead first.
- [ ] On a map within budget, a Sampleable cluster that leaves every target class is still evicted after the hysteresis window.
- [ ] No production path reads `WARM_SET_CLUSTERS` or the id-46 warm walk (grep gate).
- [ ] The re-read counter increments when an evicted cluster is read again, and does not increment on a cluster's first read.
- [ ] A visible SH miss is counted in exactly one bucket: outside reach, trimmed by pressure, read in flight, held by the drain budget, awaiting compose, or failed. Each bucket is reached by at least one fixture. Settling frames add nothing to the SH or lightmap visible-miss counters. A cluster still not Sampleable on a timed-out reveal frame counts one miss on that frame (P9).
- [ ] Line C's first-level-frame mark lands on the reveal frame. The Settling hold appears as its own mark between install and reveal.

### Manual
- [ ] Measurement, before the reach is built: SH mandatory bytes per camera cell at the default L on `stress-warren-hallway-inspection` and `campaign-test`, beside lightmap mandatory bytes. Report the worst cell and p95. If over the threshold in Decisions, stop for the owner.
- [ ] Visual: entering campaign-test and the largest available stress map, by boot, `loadLevel`, `restartLevel` and a frontend backdrop, never shows a frame of ambient-floor SH or SH-only lightmap in the first view.
- [ ] Resource run (`testing_guide.md` §Resource bounds): a pinned walking route on the largest stress map.
  - Record, before and after: misses by bucket, evictions, re-reads, peak resident SH and lightmap bytes, mandatory overshoot, and Settling hold duration.
  - Pin the fixture, route, machine class, cache mode, and baseline (main before this brief).
  - Rebuild stale stress PRLs first, since some predate ids 49–51.
- [ ] The route's in-play SH misses fall compared with baseline. Peak resident SH bytes past the budget are mandatory only.
- [ ] Co-op loopback: a host and a client load the same level. Both settle, and neither sees the other's pawn move before its own reveal. Across a host `restartLevel` with a long settle, the client never sees its own pawn act before the host reveals.

## Path
- Settling needs visibility and the streaming drain without the sim tick or world draw. Today they run in the Running frame in `frame_loop/mod.rs`, which reaches `Session::prepare_streaming_drains` → `LevelStreaming::prepare_drains`; boot states dispatch through `drive_boot_state_for_redraw`. The Loading paint, `present_world_less_frame`, calls `clear_level_streaming` and submits an empty SH batch, and install calls `end_loading_screen`. Settling cannot reuse either as is. Isolate visibility → `prepare_streaming_drains` → drain, compose and submit as a call Settling makes from the presented pose, proving the split before the state exists. `has_installed_level` gates hot-reload recompose and observe-live on Running today. Rivals: `research.md`.
- Presented pose: `spawn_eye_position` / `followed_pawn_eye` (`lifecycle_spawn_residency.rs`) resolve position only. `apply_menu_camera_pose` and `render_aim_pose` supply the menu orientation per Running frame, and install sets yaw and pitch from the first player_spawn.
- Settle predicate: `ShResidencyController::all_targets_sampleable` / `ShStreamingSession::all_targets_sampleable` (behind `cfg(feature = "capture")`), and `LightmapStreamingSession::settled` / `lightmap_residency_settled`, which count only Mandatory-class blocks today. `capture/prepared.rs` `preload_visible_sh` is a bounded settle loop over today's warm targets. `update_capture_view` is the existing all-paths lightmap demand. `install_spawn_lightmap` and `LightmapResidencyController::preload_batch` are what Settling replaces.
- Cell-demand stage: lightmap's `BlockDemand::recompute` (`lightmap_streaming/demand.rs`) owns id-51 lookup, path classification and the lead split today; `LightmapLevers` owns L (`DEFAULT_LEAD_METRES`). `ClusterHints::cell_to_cluster` is SH's cell→cluster map. `sh_streaming/warm_set.rs` and `WARM_SET_CLUSTERS` are the retired walk; `compare_pressure_keys` ranks by `warm_rank` today. Departure eviction: `take_async_drain_batch` → `departed_eviction_order`. The re-read counter joins `ShStreamingLiveDiagnostics`, with its log line and the dev-tools Streaming tab.
- Revealed: the term joins `parity_cause` / `NetServer::reevaluate_parity`, after every parity check; precedence is check order. The client record mirrors `NetClient::set_level_parity` / `parity_sent` in `queue_control_messages`; `close_slot` clears the host record. `NetEndpoint::set_level_parity` is the dual-role precedent for the reveal setter, and `clear_net_level_parity` retracts both, on unload and on suspend. `client_drain_control` logs content-parity holds at warn; revealed holds should not. New settle state joins `reset_boot_state_after_suspend`'s list. The net half is provable in the in-memory harness before Settling exists.
- Promotion weight: `PromotedBakedLightState` / `advance_promoted_baked_state` (`renderer_light_slots.rs`), `PROMOTE_SECONDS`.
- Library capture at landing: `networking.md` §Admission and content parity and §Slot lifecycle gain the revealed term at promotion, and §Session-state ledger the host's per-slot revealed record. `rendering_pipeline.md` §Cluster SH residency and §4 gain the id-51 reach, the cell-demand stage and the retired preload. `boot_sequence.md` §1 and §3 gain Settling. `plans/done/level-load-perf`'s "first level frame" now means the reveal frame.
- Derivation, pop-in sources, the first-frame survey, ordering pins and the co-op participation trace: `research.md`.

## Wire format
Bitcode owns layout (`networking.md` §Wire/codec invariants); no manual endianness, prefixes or counts. Every addition is appended, following the `JoinSeed` and `SessionRoster` precedent.

| Addition | Enum, position | Channel, direction | Fields, in order | Absent value |
|---|---|---|---|---|
| Revealed-level declaration | `ClientControlMessage`, after `JoinSeed` | Control, client → host | revealed level identity `Option<String>` (the parity string) | `None` = not revealed |
| Host-not-revealed hold | `HoldingCause`, after `LevelDigest` | inside `Divergence`, host → client | host level identity `String` | — |
| Client-not-revealed hold | `HoldingCause`, after the host variant | inside `Divergence`, host → client | host level identity `String` | — |

The declaration is re-sent on change, like parity, and the host keeps the last one per slot. Both revealed causes rank below every content cause in `parity_cause`'s check order.

## Boundary inventory
| Name | Rust | Wire | Script | FGD |
|---|---|---|---|---|
| Revealed-level declaration | new `ClientControlMessage` variant; `NetClient` record and sent flag; `NetEndpoint` reveal setter for both roles | appended variant, Control | none | n/a |
| Revealed holding causes | two appended `HoldingCause` variants | inside `Divergence` | none | n/a |

## Open questions
- Whether Settling's frames raise the shared per-drain budget, since no world frame is at stake — **delegated**. Report the effect on hold duration.
- Whether the new variants bump `WIRE_VERSION`, as the `JoinSeed` precedent did, or also `PROTOCOL_ID`, as `networking.md` §Two-gate handshake words a vocabulary change — **delegated**.
