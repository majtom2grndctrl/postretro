# Measurements — sh-compose-contributing-rows

The brief is `context/plans/in-progress/sh-compose-contributing-rows/index.md`. These runs answer its manual rows on per-pass composed rows and pass time.

## Pins

**Machine, display, settings.** These match `../sh-compose-row-cost-spike/README.md` §Pins.
- The machine is an Intel MacBook Pro with a Radeon Pro 5300M running Metal.
- The display runs exclusive fullscreen at 2688×1680 and 60 Hz, at half render resolution.
- Shadow and fog quality are low, Surface Depth is off, and vsync is on.
- GPU timestamps are unsupported, so pass time comes from Metal System Trace.

**Binary.**
- The binary was built from a release build of branch `sh-compose-contributing-rows-probes` at `6f326ef9b`, with SHA-256 `a40cf45a41507221…`. Build command:

  ```
  CARGO_PROFILE_RELEASE_STRIP=none cargo build --release -p postretro -p postretro-script-compiler --features postretro/capture
  ```
- That branch is feature-branch commit `00da7fe6d` plus `probes.patch`. The patch holds the spike's seven probe commits, with the `frame.rs` conflict resolved onto the new commit seam. It also adds one commit:
  - a Pass A (`static-direct`) `RowCountWindow`, counted against id-41;
  - for Pass B, the count of rows with id-45 entries or in that frame's Pass A plan (`rows with entries or upstream`);
  - `POSTRETRO_SPIKE_OLD_MEMBERSHIP=1`, which restores the whole-domain membership. One binary therefore runs both arms, and the engine logs the arm as `[SH spike] compose membership: …`.

**Fixtures.** The SHA-256 prefixes match the spike's:
- `stress-warren-hallway-inspection.prl` `a543d0e0291dbceb`, at the arena pose `--start-pose=21.13,2.44,30.48,0,0`;
- `kinematic-platform.prl` `8939e012bc683c0c`, at the station pose `--start-pose=-6.5,1.22,-27.94,0,0`.

## Protocol

- **Batches.** `batch.sh` is the spike's script with the arm switch replaced. Arms `old` and `new` alternate round by round, for 3 rounds per pose.
  - Each run waits 15 s idle first, then takes one 4 s Metal System Trace after 3 `[CpuTiming]` windows.
  - `caffeinate` is held throughout.
- **Validity.** `run.py` is an unchanged copy.
  - Every run passed the foreground, unlocked and no-screen-saver checks.
  - All six station runs fail only `rows_stable`. Pass A dispatches on about 39% of frames, when the moving platform changes the promotion weights. Its window, and Pass B's, therefore vary by design. Indirect rows are constant.
- **Metric.**
  - `gpu_time.py` reports time per compose encoder, as in the spike. It also counts Pass A's `Streamed Direct SH Promotion`.
  - Per-frame Pass A is `all_passes_per_frame_ms`.

## Results

Each table cell is the median of 3 runs, and each cell's 3-run range sits within ±0.06 ms of its median. All times are in ms.

| Pose | Pass | Rows old → new | Entry rows | ms/encoder old → new | R3 projection |
|---|---|---|---|---|---|
| arena | indirect | 2129 → 70 | 70 | 6.71 → 0.40 | 0.52 |
| arena | Pass B | 2129 → 70 | 70 | 8.01 → 0.43 | 0.86 |
| station | indirect | 2511 → 780 | 780 | 8.02 → 2.89 | 2.77 |
| station | Pass A | 2511 → 327 | 327 (id-41) | 9.06 → 1.55 | — |
| station | Pass B | 2511 → 75 or 340 | 75 (id-45) | 9.38 → 0.89 | 0.88 (floor) |

**Pass A per frame.** At the station, Pass A costs 3.37 ms per frame before (range 2.75–3.50) and 0.61 ms after. The brief's "2.6–2.9 ms before" was a per-frame figure.

**Pass B at the station.** On frames without Pass A, Pass B composes the 75 gated rows that carry id-45 entries. On Pass A frames it composes 340 rows.
- That is the union of the 75 id-45 rows and Pass A's 327 rows. They share 62 rows, so it is 340, not 402.
- All 96 count windows in the new arm check out: the last frame's composed rows equal `rows with entries or upstream`.
- Pass B's 0.89 ms per encoder averages both kinds of frame.

**Whole frame (labelled GPU passes).** Per frame, the sum of every labelled GPU pass drops at both poses:
- arena: 26.5 → 11.9 ms;
- station: 33.5 → 16.0 ms.

One run per arm reads 2.5–2.7 ms above the other two.

**Frames per trace.** Frames recorded in the 4 s trace rise at both poses:
- arena: 159 → 276;
- station: 132 → 266.

**Confound.** Under the lighter load, the GPU core clock in the new arena runs falls to 1115–1160 MHz, against 1232 MHz for old. New-arm times are therefore, if anything, overstated.

## Not covered

The atlas byte check (streamed vs force-full-resident, both passes, two stepped times per pose) is a separate manual row and has not run.
