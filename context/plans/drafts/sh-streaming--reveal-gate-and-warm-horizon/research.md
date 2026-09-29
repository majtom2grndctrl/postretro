# sh-streaming--reveal-gate-and-warm-horizon — research

Read at 3f070e32d (`feat/preferences-comfort-floor`). Symbols, not lines. In the co-op sections, **V** means read in source and **I** means inferred, not executed.

## Pop-in sources (in play)

| Source | Mechanism | Addressed by |
|---|---|---|
| Past the warm horizon | Visible cluster outside `WARM_SET_CLUSTERS`; first sight is a cold read. `record_visible_misses` counts it | Wider horizon |
| Departure eviction | `departed_eviction_order` evicts any Sampleable, untargeted, unpinned cluster. It never checks remaining capacity. Called from `take_drain_batch_with_eviction(true)`, i.e. every async drain | Wider horizon keeps more targeted; budget trims |
| Hysteresis expiry | `HYSTERESIS_SECONDS` = 2.0; after it, the departed cluster is evictable (`targeting.rs` `unsuppressed_target_classes`) | Unchanged; wider horizon makes departure rarer |
| Install throttle | `MAX_INSTALL_DECODED_BYTES_PER_DRAIN` 8 MiB per drain, `MAX_STREAM_PERMITS` 8. The io-contract notes some stress-warren-mini clusters exceed the drain budget | Non-goal unless measurement implicates it |

Owner's "low VRAM" observation: the resident set is capped by what's targeted, not by the pool. The pool reserves physical capacity at install: `plan_initial_pool_floor` → `StreamingGpuPools::new`, where each family gets `min(whole map, share)` via `dense_slots_for_share`. It grows on demand beyond that (`grow_dense` / `grow_sparse`). So on a map that fits its share, evicting a departed cluster frees no VRAM. Keeping departure eviction anyway is an owner decision (see brief).

## Budget today

- `DEFAULT_STREAMED_SH_POOL_FLOOR_BYTES` (renderer `sh_streaming/floor.rs`, `plan_initial_pool_floor`) and `DEFAULT_GPU_FLOOR_BYTES` (`sh_streaming/budget.rs`, `ShResidencyAccounting::effective_floor_bytes`) are both hardcoded at 256 MiB.
- Fixed metadata and whole-resident billboard scatter (ids 47/48) are charged first. The rest is split between the dense and sparse families by whole-map weight (`capped_proportional_shares`), and each family is floored at its minimum.
- The rationale for 256 MiB is the GTX 1660 6 GB floor, "a relief target, not a hard cap" (`plans/done/sh-probe-streaming/index.md` §Open questions).
- `compare_pressure_keys`: Prefetch yields before SeamWarm, then lower effective priority, then higher `warm_rank` (farther).

## Entry reveal today

- `finish_level_payload` → `install_level_payload` (player spawn and camera placed here) → `BootState::Running` → `clear_splash`.
- `ShStreamingSession::from_snapshot` starts with nothing resident. Async mode always runs in production. `read_one_sync_at_target_time` exists only for proofs.
- SH targets come from `update_targets`, called only from the Running per-frame block in main.rs, which `drive_boot_state_for_redraw` reaches for Frontend/Running. Frontend is then diverted to `render_frontend_frame`.
- No sim-pause mechanism exists. A capturing modal pauses neither sim nor audio (`boot_sequence.md`). `freeze_time` is dev-tools only.
- SH is the only asynchronously streamed data. Textures, meshes and audio load synchronously during install. Pipelines are built at full-init.
- Lifecycle primitives are `loadLevel`, `restartLevel` and `returnToFrontend`. Respawn in test content uses `restartLevel`. In-level teleports don't touch residency.

## Settling shape rivals

| Shape | Verdict |
|---|---|
| New Settling boot state | **Chosen** |
| Enter Running; suppress sim tick and world draw behind a flag | Rejected. Spreads a boot concern through the frame loop |
| Post-install sub-phase of Loading | Rejected, strongest rival. Once install runs a world exists, and request draining, parity and dev tooling must see it as installed |
| Synchronous preload at install, reusing capture's `preload_visible_sh` | Rejected. Blocks the event loop and the transport poll |
| Per-cluster SH fade-in | Rejected. Breaks the invariant that a sampled slot matches what a full-resident compose would write |

## Co-op participation today

- **V.** `install_level_payload` calls `endpoint.set_level_parity` during install, before any reveal. With a Settling hold and no other term, a peer would be promoted and its pawn ticked while the local player is still on the splash.
- **V.** `NetServer::reevaluate_parity` is the single predicate site. It runs on `set_mod_digest`, `set_level_parity`, and once per client after each Control batch. `parity_cause` returns the first `HoldingCause` in variant order; variant order is documented as diagnostic precedence.
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

- `sh-probe-streaming` epic: misses never stall, doors never wait. Kept for in-play misses. The entry gate adds to the existing load wait, which already exists.
- The io-contract lists warm count, coalescing caps, byte budget and permits as tuning, not contract. It deferred "cap warm set by bytes as well as count"; the single budget still plays that role via pressure.
- `large-map-spatial-residency.md` says lightmap-shaped data is the next resource and per-resource platform budgets are an open decision. Budget tiers are split out for that reason.
- `networking.md` §Slot lifecycle: four stages, participation as one predicate, "any entry to participating spawns its pawn". Kept; the predicate gains the revealed term. §Admission and content parity: parity stays content identity, published at install.
