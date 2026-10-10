# sh-streaming--reveal-gate-and-warm-horizon — research

First read at 3f070e32d (`feat/preferences-comfort-floor`); refreshed at ef855f247 after lightmap cell-block streaming and the mod loading screen landed. Symbols, not lines. In the co-op sections, **V** means read in source and **I** means inferred, not executed.

## First-frame incompleteness survey (ef855f247)

What a first presented frame can show differently from the same view seconds later, camera unmoved.

| Source | Mechanism | Disposition |
|---|---|---|
| SH clusters | Ids 27/34/35/41/45 start empty; only capture preloads SH. A cluster installed in drain N samples from N+1. Consumers: world, movers, skinned meshes (id-35 direct base), billboards, fog ambient scatter | Settle target |
| Lightmap blocks drawn outside the baked set | `install_spawn_lightmap` → `preload_batch` reads Mandatory blocks only. A runtime walk that reaches a cell the id-51 lattice and dilation missed draws a Visible-class block the preload skips; `count_visible_misses` reports `drawn_outside_baked_set` | Settle target |
| Lightmap on non-portal paths | `PathDemand::CameraSet` holds drawn blocks and never demands them; persists in play | Settle target during Settling; in-play non-goal |
| Promoted-light crossfade | `PromotedBakedLightState::default()` weight 0, ramp over `PROMOTE_SECONDS` = 0.3 in `advance_promoted_baked_state`. Entities near a promoted light lack self and near-tier shadows on frame one | Reveal-frame promotions start whole |
| Particle rate emitters | `EmitterBridge::update` accumulator starts at 0; no prewarm exists | Non-goal; own brief |
| Ruled out | Shadow depth caches render cold layers same-frame; animated lightmap atlas is install-time compact pages; SDF atlas whole-resident; no TAA/SSR/exposure/fog history; no reflection probes; textures and models whole at install; palette cache resamples on miss | — |

## Pop-in sources (in play)

| Source | Mechanism | Addressed by |
|---|---|---|
| Past the warm horizon | Visible cluster outside `WARM_SET_CLUSTERS` (8, id-46 walk in `warm_set.rs`); first sight is a cold read. `record_visible_misses` counts it | id-51 reach within L as mandatory |
| Departure eviction | `departed_eviction_order` evicts any Sampleable, untargeted, unpinned cluster, never checking remaining capacity; runs every async drain | Unchanged (owner); wider mandatory makes departure rarer |
| Hysteresis expiry | `HYSTERESIS_SECONDS` = 2.0 (`targeting.rs` `unsuppressed_target_classes`) | Unchanged |
| Drain throttle | `MAX_INSTALL_DECODED_BYTES_PER_DRAIN` 8 MiB, now shared between SH and lightmap (`streaming/drain_budget.rs`) | Non-goal unless measurement implicates it |

Owner's "low VRAM" observation: the resident set is capped by what's targeted, not by the pool. The pool reserves physical capacity at install (`plan_initial_pool_floor` → `StreamingGpuPools::new`, each family `min(whole map, share)` via `dense_slots_for_share`) and grows on demand (`grow_dense` / `grow_sparse`). On a map that fits its share, evicting a departed cluster frees no VRAM.

## Budget today

- `DEFAULT_STREAMED_SH_POOL_FLOOR_BYTES` (`sh_streaming/floor.rs`) and `DEFAULT_GPU_FLOOR_BYTES` (`sh_streaming/budget.rs`) are both 256 MiB. Rationale: GTX 1660 6 GB floor, "a relief target, not a hard cap" (`plans/done/sh-probe-streaming/index.md` §Open questions).
- Fixed metadata and whole-resident billboard scatter (ids 47/48) are charged first; the rest splits dense/sparse by whole-map weight (`capped_proportional_shares`).
- `compare_pressure_keys`: Prefetch yields before SeamWarm, then lower effective priority, then higher `warm_rank` (farther). Under the id-51 reach, lead replaces warm rank.
- SH clusters and lightmap blocks share one read issuer and one per-drain budget, mandatory tier before optional across both (`rendering_pipeline.md` §4).

## Entry reveal today (ef855f247)

- `finish_level_payload` → install (spawn and camera placed) → `install_spawn_streaming` → `BootState::Running`.
- `install_spawn_streaming` resolves the first frame's eye through `spawn_eye_position`: the frontend camera pose for a backdrop install, else the held spawn eye. It then runs `install_level_streaming`, which preloads the spawn cell's mandatory lightmap set synchronously and warns if it is not settled. SH gets no preload.
- Loading frames draw the mod loading tree (`boot_sequence.md` §1 Loading screen), with progress capped at 0.85 and one painted frame held at 0.85 before install. The bare splash is only the fallback.
- `drive_boot_state_for_redraw` drops UI input on Booting, Splash and Loading frames (`drop_ui_input_on_non_ui_frame`).
- The frontend backdrop runs through Running with the menu camera pose held, so its world is visible behind the menu.
- Production drains go through `frame_loop/mod.rs` → `Session::prepare_streaming_drains` → `LevelStreaming::prepare_drains`.
- No sim-pause mechanism exists. A capturing modal pauses neither sim nor audio. `freeze_time` is dev-tools only.

## SH reach rivals

| Shape | Verdict |
|---|---|
| id-51 cells within L as SH mandatory, band as optional, one L with lightmap | **Chosen** (owner, 2026-10-08). One reach for both resources; id 51 was built cell-keyed for this; the epic decided reach as visibility-based |
| id 51 for the entry settle set only; cluster-count warm horizon in play | Fallback if the measurement shows SH mandatory bytes past the floor |
| Cluster-count warm horizon raised by measurement, entry warm count mandatory during Settling (the first draft) | Superseded. Two definitions of "resident" across resources, and an entry count tuned apart from the in-play count |
| Path-distance horizon | Rejected by the epic: a portal-path distance never bounded what is seen (Reach bound) |

## Cell-demand seam rivals

| Shape | Verdict |
|---|---|
| One level-scope stage owns L, turns id 51 and the visibility path into per-cell classes; each resource maps cells to its units | **Chosen** (owner, 2026-10-08). "One L, one reach" is structural; L survives a lightmap decline or a level that streams SH only |
| SH mirrors `BlockDemand::recompute` over `ClusterHints::cell_to_cluster`; L moved to a level lever | Rejected. Two copies of path classification and the lead split kept in step by convention |

L today: `LightmapLevers` in the lightmap session, `DEFAULT_LEAD_METRES` = 16 clamped to id 51's `max_lead`; a declined session drops out of `LevelStreaming::lightmap()`. Id 51 is omitted when a level has no portals or the loader rejects them (`cell_residency_bake.rs`); id 49 is always packed.

## Ordering pins

| Id | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| P1 | First Settling frame | settle check vs. the frame's demand update from the settle pose | The chokepoint never answers settled on a frame before each streamed resource has updated demand from the settle pose. An empty target list means "not yet asked", not "nothing streams" (`settled()` and `all_targets_sampleable` are `all()` over possibly empty sets) |
| P2 | Load or unload request queued; this frame's settle check would pass | request drain runs before the settle check | The request wins: the Settling world unloads, and no reveal edge fires. No revealed declaration, host reveal record, sound start, progress 1.0 or Line C reveal mark |
| P3 | Settle set completes on the frame the timeout expires | settle check before the deadline test | Releases as settled, with no warning |
| P4 | Host Settling→Running frame | host reveal record vs. that frame's world-less poll | The reveal is recorded after the frame's Settling poll, so the promotion it causes is consumed by a world poll, which spawns the pawn |
| P5 | Host `restartLevel`, same level identity | host unload → install publishes parity → host Settling | The host's own reveal is cleared at unload, before install publishes parity. A revealed client is not promoted until the host's new reveal |
| P6 | Host Settling times out | timeout release → host reveal record | The timeout edge is a reveal edge: the host records its reveal and promotes revealed, parity-matched clients |
| P7 | Suspend during Settling or Running | suspend → resume → next entry | Suspend retracts the client's revealed declaration with its parity. No settle state, target set or timer survives to the next entry, and no reveal edge fires from the interrupted Settling |
| P8 | Settling entered while the previous generation retires | `poll_retirement` → read start → reads | No read is issued until retirement joins. The timer runs through the wait. Settling settles once reads start, and the wait shows in the hold duration |
| P9 | Settling frames drain with nothing presented | miss recording vs. Settling | Settling frames record no visible miss in any bucket. The reveal frame counts its misses fresh, so a cluster still cold on a timed-out reveal counts once on that frame (`prior_visible_misses` dedup) |
| P10 | Settle set changes during Settling (L change, lightmap decline, failed read) | total recomputed vs. progress write | Progress never decreases: a growing total holds the bar, a shrinking one advances it |
| P11 | Lightmap declined mid-Settling, or the level streams SH only | L source vs. SH demand | SH keeps L from the cell-demand stage. Lightmap answers settled, and the settle set's SH lead tier is unchanged |
| P12 | `levelLoad` enqueues a wait, a sound and a level command during install | Settling frames vs. scheduler counter and system-command drain | Neither advances during Settling. Both first advance on the reveal frame, in today's install-frame order |
| P13 | Script hot-reload commits during Settling | commit vs. has-installed-level | Settling counts as installed: the commit recomposes the Settling level's reactions as it would in Running |
| P14 | Host `restartLevel`; Relevel reaches a client in Settling for the same catalog id | Relevel vs. client settle | The client does not reload (`relevel_is_already_selected`). It finishes its settle, declares revealed, and is promoted once, after both reveals |

| P15 | Host suspends and resumes into the same level identity | suspend → Splash → boot destination → install, with no `unload_level` | The host's own reveal is cleared at suspend, so it is unset through the resumed host's Settling; a revealed client is promoted only at the host's new reveal |

The first-launch hold needs no pin: it ends before any load starts and drains no level requests.

## Settling shape rivals

| Shape | Verdict |
|---|---|
| New Settling boot state | **Chosen** |
| Enter Running; suppress sim tick and world draw behind a flag | Rejected. Spreads a boot concern through the frame loop |
| Post-install sub-phase of Loading | Rejected, strongest rival. Once install runs a world exists, and request draining, parity and dev tooling must see it as installed |
| Synchronous SH preload at install, beside the lightmap preload | Rejected. Removes the co-op revealed term and wire additions, but blocks the event loop and transport poll; `level-load-pipeline` measured a 0.7–1.0 s install freeze, and hallway clusters reach 17 MiB. Settling retires the lightmap preload instead |
| Per-cluster SH fade-in | Rejected. Breaks the invariant that a sampled slot matches what a full-resident compose would write, and is still an incomplete frame |

## Co-op participation today

- **V.** `install_level_payload` calls `endpoint.set_level_parity` during install, before any reveal. With a Settling hold and no other term, a peer would be promoted and its pawn ticked while the local player is still on the loading tree.
- **V.** `NetServer::reevaluate_parity` is the single predicate site. It runs on `set_mod_digest`, `set_level_parity`, and once per client after each Control batch. `parity_cause` precedence is its check order, not variant order: a missing host mod digest or a missing declaration returns `HostLevelAbsent` before `ModDigest`. The revealed checks run after every parity check.
- **V.** `SlotTable::transition` requires a holding cause for participating → admitted. A revealed-only demotion therefore needs a cause, which is why the hold adds `HoldingCause` variants instead of holding silently.
- **V.** `send_divergence` deduplicates per slot by cause; `NetClient::drain_control` retires the active epoch on any `Divergence(Holding)` frame carrying an epoch.
- **V.** Control is drained per poll to the last retained declaration. An unload (`None`) and a same-level re-install (`Some`) that land in one host batch leave parity unchanged, so parity alone cannot see a client that re-installed and is settling again. The revealed declaration's retraction at unload is also retained-last, so the batch ends not-revealed until the reveal arrives.
- **V.** `NetEndpoint::poll_world_less` never applies snapshots. `poll_world_less_transport` handles `SlotEvent::Participating` by applying the join seed only; `host_handle_lifecycle` ignores the edge. A promotion consumed in a world-less poll leaves a participating slot with no pawn. This is the reason for the host half of the revealed term.
- **V.** The client sends its join seed only alongside a parity send (`set_level_parity` resets `join_seed_sent`). Parity at install keeps the seed's install timing.
- **V.** On a host `restartLevel`, the client does not reload: `relevel_is_already_selected` returns early for the active catalog id. It is demoted (the `Holding` arm of `client_drain_control` calls `demote_client_state`) and then re-promoted in place.

## Why revealed needs both halves

- **Client.** The client-revealed term, not parity timing, stops promotion during client Settling. Parity at install gives the client its divergence cause during its settle rather than after reveal.
- **Host.** The host-revealed term stops promotion during host Settling. Without it, a host restart would promote a Running, revealed client during the host's settle, in the world-less-style poll, with no pawn.

## Revealed shapes

| Shape | Slot lifecycle | Cost | Verdict |
|---|---|---|---|
| Revealed term in the participation predicate; hold = admitted with a revealed cause | Unchanged stages; predicate gains a term | Two holding causes, one client message, one host setter | **Chosen** |
| New stage between admitted and participating | Adds a transition pair beside the predicate | Duplicates what an admitted-with-cause slot already expresses | Rejected: `networking.md` warns against lifecycle as transition pairs |
| Participating with the pawn withheld | Amends "any entry to participating spawns its pawn" | Epoch, tuning and snapshots to a client that cannot apply them in Settling | Rejected |
| Revealed folded into parity: both peers publish parity at reveal | Parity means installed and revealed | No new message | Rejected. Double meaning; a settling host reads as `HostLevelAbsent`; a divergent client learns its cause only after reveal |
| Host publishes parity at reveal; only the client declares revealed | Host parity means installed and revealed | One client message | Rejected. Same double meaning and mislabel on the host side |

Revealed is keyed to level identity, not reset by parity arrivals. Resetting on any parity arrival would strand a client after a mod-digest change: it re-declares parity but never re-reveals, so it would never re-send its declaration.

## Co-op timing

- **I.** The pawnless window after client reveal is the declaration's one-way trip, the host's next poll, and the epoch marker, tuning and first baseline's one-way trip: about one round trip plus up to one snapshot interval. Not measured.
- **I.** Between client reveal and the first snapshot, movers show their install phase and remote pawns are absent; the first snapshot corrects both. Today's join has the same window.
- **I.** On a host restart, a Running client's pawnless window is the host's whole settle, up to the Settling timeout. Accepted: the client sees the level, cannot act, and nothing acts on it.

## Budget tiers (split out)

Findings for the later resource-neutral budget brief:
- wgpu-hal Metal reports `IntegratedGpu` whenever the device has unified memory. A tier derived from device type would put every Apple Silicon Mac on the lowest tier.
- `ShadowQuality` has an options slot and a dev frontend-menu control ("Applies after reload"). It is not settings-file only.
- On `campaign-test`, whole-resident lightmap-shaped data far outweighs streamed SH (`large-map-spatial-residency.md` §Pre-planning measurements). An SH-only tier does little for laptops.
- Owner closure alone covers most clusters on `stress-warren-mini`. A low budget below the mandatory set is mostly overshoot.

## Prior commitments touched

- `sh-probe-streaming` epic: misses never stall, doors never wait. Kept for in-play misses. The entry gate adds to the existing load wait.
- The io-contract lists warm count, coalescing caps, byte budget and permits as tuning, not contract. Retiring the warm count is within that.
- `large-map-spatial-residency.md`: reach is decided visibility-based (Reach bound), adopted here for SH. The owner stance that a miss fallback cannot stand in for residency on interior maps is not adopted for departure eviction (owner lock). Per-resource platform budgets stay an open decision, so budget tiers are split out.
- `build_pipeline.md` §Cell residency set: id 51 is cell-keyed so a later streamed resource maps the same cells to its own units. SH is that resource.
- `plans/done/spatial-residency--lightmap-cell-blocks`: "whichever lands second adopts the other's seam". This brief lands second, so Settling absorbs the synchronous spawn preload.
- `networking.md` §Slot lifecycle: four stages, participation as one predicate, "any entry to participating spawns its pawn". Kept; the predicate gains the revealed term. §Admission and content parity: parity stays content identity, published at install.
- `level-load-pipeline` (draft, builds after): its "Spawn lightmap is a settle target" and "Loading screen stays drawn through Settling" decisions are delivered here.
