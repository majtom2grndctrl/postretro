# Extra arm: is the reflection-free fluid cost real? (5300M, 720p)

The reflection-probes comparison found the reflection-free arm (branch binary, `light_term_mask` 319) still added about 0.30 ms of fluid time on the 5300M, where the 1660 showed none. That arm also turned specular off. This arm separates the two: the reflection-probes binary on main's e24 PRL, which has no reflection probes, with every lighting term on.

Same capture and reduction as that comparison's 720p batch: 1280x720, fov 100, e24 pool pose (-4.8768, 1.6, -44.2976; yaw 0, pitch -89), `POSTRETRO_SH_STREAMING=sync-proof`, 240 warmup + 720 samples, Metal System Trace, two interleaved rounds (the second reversed), 15 s idle gaps. `fluid ms` is the median Fluid Surface Pass time over complete steady frames (Depth Pre-Pass, Textured Pass and Fluid Surface Pass rows all present), framed by the Candidate Cull Pass; at 720p the trace drops the Textured Pass row in part of the frames and shifts the fluid label onto forward's interval there.

Commits: main `b3322a2ef`; reflection-probes `3c57450ee` (code equals origin `de38e47ad`). Measured 2026-10-17. Idle before the batch: 766 MB VRAM in use, 2.95 GB free, 0% utilisation. Every run passed the lock, screen-saver and foreign-process checks.

| arm | binary | PRL | mask | fluid ms r1 | fluid ms r2 | mean | vs main | forward ms | complete/steady | clock MHz median |
|---|---|---|---|---|---|---|---|---|---|---|
| main | main | main | all | 6.006 | 5.971 | 5.99 | | 3.17 | 272, 319 /720 | 986, 994 |
| branch-mainprl | reflection-probes | main (no probes) | all | 6.323 | 6.258 | 6.29 | +0.30 ms (+5.0%) | 3.17 | 262, 294 /720 | 1011, 971 |
| branch-off | reflection-probes | reflection-probes | 319 | 6.258 | 6.258 | 6.26 | +0.27 ms | 2.87 | 296, 299 /720 | 975, 997 |
| branch | reflection-probes | reflection-probes | all | 7.263 | 7.249 | 7.26 | +1.27 ms | 3.17 | 283, 382 /720 | 1002, 1018 |

Reading:
- The reflection-free cost is real and belongs to the binary's fluid shader, not to probe data or the specular mask: branch-mainprl (no probes installed, the log shows no `ReflectionProbes` line) costs the same as branch-off.
- The mask touches forward, not fluid: branch-off's forward is 0.30 ms lower, its fluid equals branch-mainprl's.
- The earlier batch read main 5.99, branch-off 6.29, branch 7.27 at this pose; this one agrees within 0.03 ms.
