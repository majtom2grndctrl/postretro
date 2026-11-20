# camera-path-playback — Mac checkpoint results

These are the results of `mac-checkpoint.md` on the owner's Mac, at commit 0e586f7cf. The Mac is an Intel MacBook Pro with an AMD Radeon Pro 5300M; the engine ran on Metal on the discrete GPU. Run date: 2026-11-19.

## Answers

**1. Are `pr-frame <k>` marks in the export, and how do they nest with the drain encoders?**

Yes, but only on the CPU-side encoding rows. The GPU-execution rows do not carry them.

- **Where they appear:**
  - **`metal-application-intervals`:** 1,272 rows carry a mark. There are 1,247 distinct k values, contiguous from 724 to 1970: every playback frame inside the 20 s trace.
  - **`metal-object-label`:** the mark appears as a label `:pr-frame <k>`.
  - **`kdebug-strings`:** the raw string.
- **Where they do not appear:** `metal-gpu-intervals`, `metal-application-encoders-list` and `metal-command-buffer-completed` hold zero marks. Their labels stay `<command buffer>:Blit Command 0`, for example `Streamed Lightmap Drain:Blit Command 0`.
- **Nesting** (in `metal-application-intervals`):
  - The parent row is the command buffer, labelled `Streamed Lightmap Drain` or `Streamed SH Upload Copies`. Its event type is `Encoding` and its encoder-id is `0xffffffffffffffff`.
  - The child row sits at the parent's depth + 1, on the same `cmdbuffer-id`, with its own `encoder-id`. Metal labels it `Blit Command 0  -  :pr-frame <k>`. wgpu's command-encoder debug group surfaces as a suffix on the blit encoder's label, not as a separate group row.
  - Every drain and SH-copies command buffer in the trace has exactly one such child: 6 lightmap drains and 9 SH upload copies.
  - Every `Frame Encoder` command buffer also has one (1,247 of them). So do 10 `(wgpu internal) Signal` buffers.
- **Joining GPU time to a frame:**
  - Take the GPU row in `metal-gpu-intervals` (channel `SDMA0` or `GFX`).
  - Match its `encoder-id` to the child row in `metal-application-intervals`.
  - Read k from that row's label. The `cmdbuffer-id` matches as well.
  - `nesting-join.txt` lists every drain and SH-copies command buffer this way, with its GPU durations.
- **Don't use xctrace's own frame number.** Its `Frame N` column is the trace's frame count, not k; for example, xctrace Frame 335 is `pr-frame 986`.

**2. Does `Core Clock(MHz)` read non-zero during the route?**

Yes. Every sample in the route window is non-zero.

| Window | Readings | Non-zero | Clock min / median / max (MHz) | Utilization median | Temperature |
|---|---|---|---|---|---|
| Boot and load | 16 | 16 | 19 / 172 / 344 | 5% | 67–72 °C |
| Hold at pose 0 | 100 | 100 | 62 / 664 / 1088 | 48% | 70–82 °C |
| Route | 220 | 220 | 55 / 733 / 1222 | 56% | 80–96 °C |
| After exit (idle) | 4,616 | 4,610 | 0 / 58.5 / 535 | 0% | 63–90 °C |

- **Where the readings come from:** each poll gives four `Core Clock(MHz)` values, all from the AMD accelerator. `ioreg -r` lists its node twice, and each listing has `PerformanceStatistics` and `PerformanceStatisticsAccum`. The Intel iGPU reports utilization only.
- **How the windows are cut:** from the engine log's timestamps, which have 1 s resolution, and the playback record's frame intervals: hold 15.13 s, total 44.66 s.
- **The clock is not pinned.** It ranges from 55 to 1,222 MHz on the route. Utilization is about half, at vsync 60 with a 2560×1440 surface and a 1280×720 scene (divisor 2).
- **Temperature** reached 96 °C on the route.

## Run

- **Engine:** exit 0. It settled after 3 walk frames, then played 2,646 frames: 900 hold plus 1,746 route poses. The record is `cp-far.playback.json`. The tail of `cp-engine.log` shows one warning at shutdown: `[candidate-cull] counter map failed: BufferAsyncError`.
- **Trace:** 20 s, attached 8 s after `[Settling] entry`. It covers frames 724–1970, which are the last 176 hold frames and route poses 0 to about 1070.

## What the trace takes on this Mac (for the runbook)

- **Stale map:** `campaign-test.prl` was stale (`ClusterShPayloadsVersionMismatch: version 2, expected 3`), so step 2 rebaked it.
- **Locked screen:** a locked screen stalls the windowed engine at `[Engine] Window ready`. `cp_run.sh` now refuses to launch while the screen is locked.
- **Trace size and save time:** a 40 s Metal System Trace wrote about 4.7 GB of raw ktrace. xctrace then spent more than 10 minutes at 100% CPU in "Stopping recording". The runbook's 60 s watchdog, and later a 600 s one, killed it before it saved, and the export then failed with `Document Missing Template Error`. Twice it was the 40 s limit that stopped the recording rather than the engine exiting, so the save time comes from the trace size, not from the target exiting. The 20 s trace that succeeded took about 11 minutes to save and came to 436 MB. Recommendations:
  - Keep the trace at or under 20 s.
  - Give the watchdog at least 30 minutes.
  - Pass `--no-prompt` and `< /dev/null`, as the repo's earlier trace scripts do.
- **Paths:** results were written under the session scratchpad, not `/tmp`. Logs show it as `$OUT`.
- **Clock-poll timestamps:** BSD `date` has no `%N`, so the clock poll stamps with Perl's `Time::HiRes`.

## Files

| File | Contents |
|---|---|
| `cp-engine.log`, `engine-exit.txt` | full engine log (`RUST_LOG=info`) and its exit code |
| `cp-far.playback.json` | playback record |
| `cp-far.txt`, `route-gen.log` | generated route (40-step far-point, 1,746 poses) |
| `cp-ioreg.log` | clock poll, every 0.5 s, from before launch until after xctrace finished |
| `cp-far-toc.xml` | trace contents. The process environment, device name and UUID are removed. |
| `tables/*.xml.xz` | raw exported tables, xz-compressed (`xz -d` restores the raw XML) |
| `nesting-join.txt` | drain and SH-copies command buffer → `pr-frame` child → GPU rows |
| `xtrows.py` | xctrace export to TSV, resolving `id`/`ref` deduplication: `python3 -I xtrows.py <table.xml> [filter]` |
| `cp_run.sh` | the driver used: steps 3–7 with the changes above |
