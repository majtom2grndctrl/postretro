# `_bounce_fill` A/B captures (fill 0 vs 1, AC 38)

These are headless captures of the owner A/B in `../ab/AB.md`, taken at the poses where the SH base (id 34) changes most between `_bounce_fill` 0 and 1. The goal is to tell whether the difference is invisible or just hard to find in-engine.

**Contact sheets:**
- `contact-full.png`: full lighting.
- `contact-indirect.png`: indirect only.

Each row is one pose. The columns are fill 0, fill 0.5 (campaign only), fill 1, and |fill1 − fill0| ×4 as a heat ramp (black, red, yellow, white). Each row's label carries the diff numbers. `stats.json` holds every figure, including fill 0.5 vs 0 under `half_vs_0`.

## Inputs

- **Engine:** working tree at `efd369696` (`feature/surface-radiance-cache`). Built with `cargo build -p postretro --features capture,dev-tools`, dev profile. The uncommitted edits in that tree are all in `level-compiler` and docs, which the engine does not link.
- **Adapter:** AMD Radeon Pro 5300M, Metal.
- **PRLs:** `content/dev/maps/ab-campaign-fill-{0,0.5,1}.prl` and `ab-whiteroom-fill-{0,1}.prl`, built with `--release` at `194113525` (see `../ab/AB.md`). They are gitignored, so they exist only locally.
- **Settings shared by every capture:** 960×540, fov 100, `POSTRETRO_SH_STREAMING=sync-proof`, default indirect scale. The two frames in a pair differ only in their PRL.
- **Modes:**
  - `full`: no `light_term_mask`, so every term at default settings.
  - `indirect`: `light_term_mask: 2`, which is `INDIRECT_STATIC` only. Ambient floor, direct, specular and depth AO are all off.

## Pose selection (`select_poses.py`, writes `poses.json`)

1. `sh34.py` decodes id 34 (v11) from fill 0 and fill 1:
   - It reads the container section table, the probe metadata and the BC6H mode-11 atlas.
   - It reconstructs each valid probe's mean irradiance: from its L0 tile, by scale-aware L1 trilinear over the node's valid corners, or from the L2 node mean.
   - It converts that mean to Rec. 709 luminance.
   - The storage levels differ between the two fills, so the comparison is per probe, not per tile.
2. **Campaign-test candidates:** probes 1–2 cells (1 m spacing) above solid, i.e. eye height over a floor. That leaves 10,628 of 63,418 valid probes.
   - Four spots are ranked by `Δ / (fill0 + 0.005)` among probes whose Δ is above the near-floor 75th percentile. This favours big lifts where fill 0 is dim, i.e. indirect-dominated areas. These are `c1-rel` to `c4-rel`.
   - Two more spots are ranked by absolute Δ: `c5-abs` and `c6-abs`.
   - Spots are kept at least 10 m apart.
3. **Camera placement:** each camera sits 2–4 m from its spot.
   - All eight probes around the camera, and every sample on the camera-to-spot segment, must be valid, so the camera is in open space.
   - The solver prefers a direction with a surface 1–5 m beyond the spot.
   - The camera looks at the spot, pitched down 12°.
   - All six frames were checked visually: none is inside a brush.
4. **White room:** two hand-placed poses.
   - `w1`: looks at the pillar face that is edge-on to both lights. Both lights sit on the pillar's map-x axis, so this face gets almost no direct light, along with the floor around it.
   - `w2`: the far wall corner, about 15 m from both lights.

## Probe-space result (id 34)

| | fill-0 luminance p50 / p90 | Δ luminance p50 / p90 / p99 / max | relative Δ p50 / p90 |
|---|---|---|---|
| campaign-test | 0.0021 / 0.024 | 0.0018 / 0.0068 / 0.0137 / 0.023 | +37% / +99% |
| white room | 0.136 / 0.164 | 0.254 / 0.265 / 0.273 / 0.297 | ×2.9 median (fill 1 p50 0.389) |

## Frame diffs (fill 1 vs fill 0)

Per pixel, the diff is the max over RGB of |Δ|, in 8-bit sRGB units. "Signed" is the mean change in luma. "fill0 luma" is the mean luma of the fill-0 frame.

| pose | mode | mean | p99 | max | > 2/255 | signed | fill0 luma |
|---|---|---|---|---|---|---|---|
| campaign c1-rel | full | 1.44 | 4 | 10 | 13.1% | +1.13 | 2.6 |
| campaign c2-rel | full | 1.32 | 4 | 6 | 13.6% | +1.08 | 10.0 |
| campaign c3-rel | full | 1.65 | 5 | 6 | 19.0% | +1.42 | 3.4 |
| campaign c4-rel | full | 1.14 | 4 | 6 | 8.5% | +0.88 | 7.2 |
| campaign c5-abs | full | 0.95 | 3 | 7 | 3.4% | +0.68 | 44.7 |
| campaign c6-abs | full | 1.00 | 4 | 6 | 4.2% | +0.72 | 47.0 |
| whiteroom w1-pillar | full | 21.65 | 41 | 59 | 100% | +16.55 | 125.0 |
| whiteroom w2-corner | full | 25.29 | 31 | 34 | 100% | +19.20 | 97.9 |
| campaign c1-rel | indirect | 1.49 | 4 | 10 | 14.6% | +1.18 | 1.4 |
| campaign c2-rel | indirect | 1.42 | 4 | 6 | 15.0% | +1.19 | 3.4 |
| campaign c3-rel | indirect | 1.67 | 5 | 6 | 19.7% | +1.44 | 3.1 |
| campaign c4-rel | indirect | 1.17 | 4 | 6 | 9.1% | +0.92 | 4.3 |
| campaign c5-abs | indirect | 2.15 | 6 | 8 | 39.3% | +1.90 | 8.6 |
| campaign c6-abs | indirect | 2.29 | 5 | 7 | 44.6% | +2.04 | 9.5 |
| whiteroom w1-pillar | indirect | 46.78 | 53 | 60 | 100% | +38.10 | 44.6 |
| whiteroom w2-corner | indirect | 45.55 | 49 | 50 | 100% | +36.72 | 43.1 |

Fill 0.5 vs fill 0 on campaign-test is about half of that: mean 0.6–1.3, p99 2–3.

**Reading:**
- **Campaign-test:** fill 1 roughly doubles an indirect term that is already near black. Indirect-only frames average 1–10/255 luma. On screen the change is about +1 to +2 levels, with p99 at most 6, even at the six spots where the probes change most. That is below what you can see on a flip.
- **White room:** the change is large and uniform, at +17 to +19 luma in full lighting. The lift is finite (no runaway brightness), and the pillar's unlit face gets soft fill.

## Anomalies

- **Black seam pixels in the white room, identical in both fills (not caused by fill):**
  - Indirect-only frames show 56–61 pixels with luma below 8 along the floor/wall and floor/pillar seams. There the SH indirect term reads 0, apparently because no probe survives the lookup.
  - In full lighting, 46 pixels stay black along the pillar base in `w1`.
- **No patch-scale blotches or new light leaks** are visible in either map's indirect-only frames. The blockiness on the campaign walls follows the texture.

## Re-run

```bash
python3 -m venv /tmp/abvenv && /tmp/abvenv/bin/pip install numpy pillow
cargo build -p postretro --features capture,dev-tools
/tmp/abvenv/bin/python measurements/surface-radiance-cache/ab-captures/select_poses.py   # optional: rewrites poses.json
/tmp/abvenv/bin/python measurements/surface-radiance-cache/ab-captures/run.py            # captures + diffs + sheets (~1.5 min)
```

`run.py --skip-capture` rebuilds the diffs and contact sheets from `raw/`. `POSTRETRO_BIN` overrides the engine binary. Per-capture logs go to `$CAPTURE_LOG_DIR/ab-capture-logs` (default `/tmp`). Each `scenes/*.json` is also a standalone scene: run `target/debug/postretro --capture <scene>` from the workspace root with `POSTRETRO_SH_STREAMING=sync-proof`.
