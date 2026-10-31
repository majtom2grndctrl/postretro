# Seam-pixel check: base commit d2bda6032 vs branch (`_bounce_fill` 0), white room

Question: do the near-black seam pixels (luma < 8/255, Rec. 709, 8-bit) predate the branch?

Base PRL: `ab-whiteroom-fill-0.map` baked with the base `prl-build` (`--release`, single-bounce one-sample estimator). Scenes are copies of the existing fill-0 white-room scenes with only the map path changed. All captures here use `target/release/postretro` (the debug binary had been rebuilt without the `capture` feature during the run); the branch fill-0 poses were re-captured with the same binary. The original `raw/` fill-0 frames give identical counts.

| pose | mode | base | branch fill 0 | exact overlap |
|---|---|---|---|---|
| w1 pillar | full | 46 | 46 | 46 |
| w1 pillar | indirect (0x2) | 61 | 61 | 61 |
| w2 corner | full | 0 | 0 | 0 |
| w2 corner | indirect (0x2) | 56 | 56 | 56 |

Frames differ by at most 1-2 levels per channel anywhere. The black pixels are the same pixels.

Verdict: pre-existing. The seams are in the base bake too, so the branch did not introduce them.

Images: `compare-<pose>-<mode>.png`, 3 panels left to right: base, branch fill 0, overlay (branch frame dimmed; yellow = black in both, red = base only, cyan = branch only; marks dilated 1 px). `counts.json` has the figures; `raw/` the frames; `scenes/` the scenes.
