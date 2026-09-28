# sh-streaming--reveal-gate-and-warm-horizon — research

Read at 0a7352039 (`feat/preferences-comfort-floor`). Symbols, not lines.

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

## Co-op parity

`install_level_payload` calls `endpoint.set_level_parity` during install, before any reveal. If that stays put, a Settling hold would let a peer be promoted and its pawn ticked while the local player is still on the splash.

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
