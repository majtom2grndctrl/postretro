# Cluster-residency synchronous proof checkpoint

Task 10, 2026-09-22. This is the **CPU/source proof gate**, not a GPU performance
finding. `POSTRETRO_SH_STREAMING=sync-proof` remains an explicit development mode;
unset/`async` names the not-yet-implemented bounded-async path until Task 11.

## Fixture and frame contract

`real_visibility_zero_one_many_targets_and_next_frame_promotion` constructs
tiny no-portal `LevelWorld` fixtures and obtains `VisibleCells` from
`VisibleRenderPreparation::for_level`, rather than inventing the visibility
result. The captured drawable cell/target ids are respectively `[]`, `[0]`,
and `[0, 1, 2]`. The zero case emits an empty target reset and no install.
The one case drains cluster 0 in frame N, records it as
`InstalledUncomposed`, and exposes it only at the frame-(N+1) promotion
boundary. The many case queues three real-visibility targets, drains at most
two in frame N, drains the third on the following bounded drain, and reaches
`Sampleable` only after their respective later promotion boundaries.

The real id-49/id-50 `sync_proof_manifest_reads_every_streamed_family_from_one_real_chunk`
loader fixture positionally reads cluster 0 and verifies block ids
`[27, 34, 34, 35, 41, 45]` (id-34 patch and isolated-atlas blocks are
separate). Its second cluster decodes as genuinely empty. Renderer CPU tests
`paired_direct_manifest_refuses_a_chunk_missing_id35` and
`all_family_writes_wait_for_both_next_compose_epochs_before_sampleability`
prove paired 34/35 admission is atomic and canonical dense/27/41/45 writes
require both compose epochs (42 and 72 in the fixture) before promotion.
`late_completion_after_hysteresis_is_dropped_before_renderer_install`
proves a completion arriving after target departure releases its permit,
leaves ready bytes at zero, and never reaches a renderer install batch. The session
submission-latch test keeps failed/skipped frames uncomposed and consumes a
successful submission exactly once at the next boundary.

The `stream_mode_keeps_large_sh_bodies_in_the_prl_and_retains_only_metadata`
loader fixture is the `off` parity proof: after validating the same id-49/id-50
pair, `off` selects `ShStorage::Legacy`, retains full id-34/id-35 atlas bodies
and present 27/41/45 bodies, and preserves whole-resident billboard scatter.
It is a loading/ownership assertion, not a GPU pixel-parity claim.

## Commands and results

| Command | Result |
|---|---|
| `cargo test -p postretro-level-loader --lib --quiet` | 206 passed, 0 failed. Includes real six-block sync-proof read and legacy-off body assertions. |
| `cargo test -p postretro-renderer sh_streaming --lib` | 46 passed, 0 failed. Includes coupled-family refusal and all-family compose-epoch gates. |
| `cargo test -p postretro-renderer --lib --quiet` | 569 passed, 1 ignored, 0 failed. Includes shader ABI/binding and WGSL validation tests. |
| `cargo test -p postretro --features capture sh_streaming --bin postretro --quiet` | 16 passed, 0 failed. Includes zero/one/many real-visible, late completion, and submission-latch tests. |
| `cargo test -p postretro --features capture --bin postretro --quiet` | 834 passed, 2 ignored, 0 failed. |
| `cargo run -p postretro-level-compiler -- content/dev/maps/test_animated_weight_maps_mixed.map -o /private/tmp/postretro-task10-mixed.prl --no-tui -j 4` | Completed in 1.20 s; produced a 1.3 MiB development PRL. This map is not the all-family fixture above. |
| `env -u POSTRETRO_SH_STREAMING cargo run -p postretro --features capture -- --capture /private/tmp/postretro-task10-mixed.scene.json` | Exited 1 with the named `[SH streaming] bounded async mode is not yet implemented at this checkpoint` error, before GPU initialization. |
| `POSTRETRO_SH_STREAMING=sync-proof target/debug/postretro --capture /private/tmp/postretro-task10-mixed.scene.json` | Exited 1 at offscreen renderer initialization: `No suitable graphics adapter found; ... metal found no adapters`. No PNG or timing report was published. |

## GPU finding and next measurement

GPU composition, seam/pop inspection, frame time, and whole-vs-streamed
requested GPU bytes are **not-yet-evaluable on this host**: Metal exposes no
usable adapter to the capture process. An ignored renderer adapter smoke test
also returns success when it skips for no adapter, so it is not cited as a GPU
pass. The layered loader/controller/renderer CPU assertions above support
continuing to bounded async (Task 11), but they do **not** prove rendered
pixels or sampled SH on hardware. Task 13 should inspect a mid-sized map on
this MacBook Pro if an adapter becomes available, or a large map on the GTX
1660 Super, and record frame-time windows and seam continuity separately.
