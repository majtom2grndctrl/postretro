# Bounded planar + probe live findings — Windows, 2026-10-23

The owner's hybrid goal is supported by a useful performance lever: limit the reflected scene distance D and let the existing probes supply the rest. At the lake's standing across-water pose, half-scale D15 cuts mirror-walk time from 2.749 to 0.061 ms (97.8%) and live frame time from 7.830 to 5.070 ms (35.3%). SSR High is 4.828 ms here, so bounded planar is about 0.242 ms / 5.0% slower, rather than a demonstrated live FPS win. D15 is a candidate for visual testing, not a selected preset.

The distant handoff is intentional and is not itself a rejection criterion. The image must still provide useful local planar reflection. Quarter D15 is byte-identical to probes-only at this static pose, reproduced in an independent launch. Its small performance advantage cannot establish an equally useful planar image. Half D15 differs visibly, including a broad near-water, deck-textured region absent from quarter D15; the correctness/appearance of that difference and its behavior in motion remain unresolved.

## Method

GTX 1660 SUPER, driver 621.01; Vulkan, release opt-level 3 + thin LTO, features `dev-tools,capture`. Native window, VSync disabled through the existing diagnostic toggle, indirect validation release default off, ordinary SH/lightmap streaming, dev mod and fixed pool seed 1. Actual surface/scene extents are both 1600×900, divisor 1. The pawn starts at (-16.8656, 1.2, -48.768), yaw 90°, pitch −10°, stands on the deck, and settles to an eye near y=1.5. This differs from the earlier capture eye y=1.7.

Eight configurations, each launched for 20 seconds, then repeated in reverse order: 16 accepted runs. Each uses the final five complete 120-frame CPU windows and final three complete 120-readback GPU windows: 9,600 CPU frames and 5,760 GPU readbacks in the reduced matrix. An independent D15 pilot passed before the matrix and is retained separately, not averaged into it. GPU logs round to 0.01 ms; CPU/GPU tail windows are asynchronous. `planar_forward` is inside `planar_reflection`, so do not add them together. Frame entries are window means, not percentile pacing statistics or synchronized capture completion.

No builds, bakes or other experiment processes ran alongside the timed matrix. GPU clocks were not locked; five-second board telemetry includes desktop use. Reverse-order repeats reduce ordering effects but do not make this a controlled hardware benchmark. The Windows warm-development lake bake is unchanged from the earlier dataset (84,508,363 bytes; SHA-256 `7cce7c5f69397b20f2cab3740deecd97b5114d5f8985b737244752315b5f15f4`). It is not the Mac bake. Compare settings within this experiment; do not subtract timing figures from the previous dev/VSync or offscreen datasets.

Exact binary/map hashes, base commit, command arguments and policy are in `runs/manifest.json`; source hashes and image hashes are in `audit.json`. The release binary includes the uncommitted measurement controls described below, so the base commit alone does not reproduce it.

## Results

All fades are 4 m; unlisted D/R are unlimited. Scales are per axis (half has one quarter of scene pixels). Every mode retains probes; SSR and planar are alternatives for the additional reflection layer.

| Configuration | Frame means, ms (run 1 / 2) | Mirror CPU mean, ms | Reflection GPU mean, ms |
|---|---:|---:|---:|
| SSR High | 4.874 / 4.781 | absent | 0.947 |
| Half, unlimited | 7.855 / 7.804 | 2.749 | 0.210 |
| Half, D30 | 5.833 / 5.801 | 0.775 | 0.120 |
| Half, D15 | 5.071 / 5.068 | 0.061 | 0.090 |
| Quarter, D30 | 5.807 / 5.810 | 0.773 | 0.080 |
| Quarter, D15 | 5.018 / 5.037 | 0.060 | 0.070 |
| Half, D30 + R15 | 5.664 / 5.541 | 0.607 | 0.090 |
| Half, R8 | 7.804 / 7.805 | 2.773 | 0.060 |

Full work/submit and per-run data: [tables](tables.md), `summary.json`, `runs/results.json` and original logs.

D15 changes the mirror walk from the 20,000-step fallback / 126 drawable cells to 354 considered steps / 11 cells, using the seeded path. D30 is 5,106 steps / 30 cells. R8 alone retains 17,950 considered steps / 83 cells and nearly all the CPU cost even though its reflection GPU span is small. D30 + R15 reduces the walk to 3,915 steps, improving on D30 slightly, but does not approach D15's CPU cost. Mirror-only cells are zero at this lake pose; the previous e24/probe fixtures remain the separate union/residency evidence.

Half-to-quarter changes D15 frame cost by only 0.042 ms (under 1%) and D30 by 0.008 ms. The main portal walk still consumes about 3 ms, with its existing step-limit fallback. Lower reflection GPU spans do not translate directly into equal frame savings because CPU/GPU work overlaps and other stages dominate. This experiment does not measure a GPU-bound live 4K view.

## Image interpretation

D limits reflected geometry's distance from the mirrored camera; R limits receiving water's distance from the real eye. With D15 and a 4 m band, an eligible reflected hit has full coverage through 11 m, then fades to zero at 15 m. Pixels without an eligible planar hit retain the existing probe contribution. R unlimited keeps all selected water eligible; it does not force a planar hit at every pixel.

Six fresh stills use the same release binary and a fixed capture eye y=1.5: [SSR High](handoff-images/ssr-high.png), [half unlimited](handoff-images/half-unlimited.png), [half D30](handoff-images/half-d30.png), [half D15](handoff-images/half-d15.png), [quarter D15](handoff-images/quarter-d15.png), [probes-only / Low](handoff-images/probes-only.png). Each has 120 warmup + 120 measured frames and a report. Capture uses sync-proof preload and no game VM/manifest fluid tuning; these are structural illustrations, not exact native gameplay frames.

Unlimited planar reflects the far walls/colored panels and room features clearly. D15/D30 replace much of that image with the stretched/smeared probe representation, as intended by the owner. That is a fidelity tradeoff to assess against the hybrid goal, not proof that the strategy failed. Half D15's broad foreground reflection differs substantially from quarter D15. Both independent D15 repeats are byte-identical to their respective originals; quarter D15 is also byte-identical to Low/probes-only. Thus the scale difference is reproducible, and no useful quarter-D15 local contribution is established at this pose. Root cause is not established by these captures; do not label it fixed, a validation error, or accepted image quality.

The next image-focused check should follow a nearby post/object while moving along the water and turning until it leaves the main screen, using half D15/D30 and probes-only controls. Judge local reflected-object presence and the D fade/probe transition. That moving-camera/entity acceptance remains open. The present experiment establishes stationary performance and repeatable stills only.

## Tooling and verification

Minimal dev-tools launch seeds expose the existing scale/D/R/band sliders and seed the existing VSync toggle off. No player settings are persisted. There are no visibility algorithm, composite shader, GPU binding or protected reference changes. The only existing test edit is the mechanical `chunks_exact(8)` → `as_chunks::<8>().0` lint correction.

Renderer checks with and without dev-tools, Clippy all targets with dev-tools and warnings denied, formatting, 10 planar tests (including two new parser tests and six GPU proofs), and 100 unchanged fluid tests passed. Source review approved the measurement controls; `review.md` and individual logs preserve details. The native pilot and all 16 runs confirm effective settings, VSync off, expected Vulkan adapter/extents, complete timing windows, planar/SSR exclusivity, zero logged lightmap read/install failures and zero visible lightmap misses. No accepted run logged SolidCellFallback, panic or renderer validation error. Release indirect validation was off; these runs do not replace the earlier validation-on GPU proofs.

Eight accepted stills, including the two D15 repeats, have complete 120-readback windows, zero malformed samples, correct reflection pass counts, clean visible lightmap/read/install counters and verified 1600×900 PNG dimensions. `audit.py` revalidates all matrix reductions and these reports/images. Known Vulkan overlay-loader startup messages remain in logs, as in the earlier dataset.

The first release build failed in MSVC `lib.exe` packing Luau (exit 1114). The successful retry used `-j 2` and worktree-local TEMP/TMP, taking 16m53s. Both logs are retained. The first illustration attempt omitted the GPU timing environment flag; its one image/report remain under `handoff-images-no-timing/`, excluded from the accepted eight. It was rerun with timing enabled. All launched engine/telemetry processes were terminated by their runners.

## Recommendation

Continue the planar-plus-probe approach with **half scale, D15, R unlimited and 4 m fade as the first motion-test candidate**, with D30 as the comparison when more reflected-scene reach is useful. This recommendation is about the next visual evaluation, not a shipping default or a measured FPS improvement over SSR. Probes make the distant simplification compatible with the design goal. D15 largely resolves the extra traversal cost without changing visibility algorithms; R-only limits do not do so here. Quarter scale offers little additional frame saving in this workload and currently lacks a visible D15 contribution at the tested pose.

Mac live bounded behavior, moving camera/entities, near-water scale behavior, and owner visual acceptance remain separate open questions. Keep the spike dev-only until those are understood.
