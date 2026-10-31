# `_bounce_fill` A/B captures (fill 0 vs 1, AC 38)

These are headless captures of the owner A/B in `../ab/AB.md`, taken at the poses where the SH base (id 34) changes most between `_bounce_fill` 0 and 1. The goal is to tell whether the difference is invisible or just hard to find in-engine.

**Contact sheets:**
- campaign-test and white room: `contact-full.png` (full lighting) and `contact-indirect.png` (indirect only).
- movement-feel: `contact-movement-feel-full.png` and `contact-movement-feel-indirect.png`.
- kinematic movers (mover-setpiece): `contact-movers-full.png` and `contact-movers-indirect.png`.

**Phone-sized pairs** (fill 0 | fill 1, full lighting, 1608 px wide): `pair-mover-setpiece-v3-north-face.png`, `pair-movement-feel-m1-rel.png`, `pair-whiteroom-w1-pillar-shadow-face.png`.

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

## movement-feel and kinematic movers (second batch)

### Inputs

- **PRLs:** `ab-movement-feel-fill-{0,1}.prl` and `ab-mover-setpiece-fill-{0,1}.prl`. Each is `content/dev/maps/<map>.map` copied byte for byte with only `"_bounce_fill" "<f>"` added to worldspawn. They were built with `target/release/prl-build ... --release --no-tui` at `8319b126a` (release compiler rebuilt at that HEAD), from a scratch folder holding `maps/` plus `textures`, `scripts` and `models` symlinks, as `../ab/build.sh` does. They were deleted after capture, so rebuild them to re-run.
- **Engine:** `target/release/postretro`, built with `capture`. No engine-side commits land between its build and `8319b126a`.
- **Settings:** the same as the first batch (960×540, fov 100, sync-proof SH, default indirect scale, modes `full` and `indirect` = mask 0x2).

### Pose selection

**movement-feel:** `select_poses.py movement-feel`.
- Same probe method as campaign-test: two spots by relative change (`m1-rel`, `m2-rel`) and two by absolute change (`m3-abs`, `m4-abs`).
- Relative change picks the spots where fill 0 is dim, i.e. multi-bounce light arriving where direct light misses. Absolute change picks the brightest-walled areas.
- `m5-cor` is the typical in-play read. The camera sits at player height (about 1.5 m above a floor), with two open cells on each side and one above it, and looks down the longest open axis run (75 m, the east-west lane), pitched down 3°.

**mover-setpiece:** `select_poses.py mover-setpiece`.
- I chose mover-setpiece over kinematic-platform. All six of its baked lights are static, and every baked falloff crosses the sliding wall. kinematic-platform's movers sit under dynamic lights, promoted spots and `style`-animated spots.
- The poses frame the sliding wall (8 m × 5.7 m) at its authored closed pose:
  - `v1`: the README's station 1, 8.1 m south of the wall, as the typical read.
  - `v2` and `v3`: on the south and north faces, the near-floor probe within 1.5 m of the face with the largest relative change, seen square-on from 4.5 m away.
- No part of the wall is in a region direct light misses: every baked light reaches it. The rising room's interior is unlit by baked light, but at its rest pose it sits underground.
- **Mover position is fixed.** The capture scene format has no mover time or position field. Movers draw at their authored rest pose (§7.8), so each pose was captured at one mover position only.

### Probe-space result (id 34)

| | fill-0 luminance p50 / p90 | Δ p50 / p90 / p99 / max | relative Δ p50 / p90 |
|---|---|---|---|
| movement-feel | 0.0056 / 0.0099 | 0.0018 / 0.0023 / 0.0047 / 0.0125 | +33% / +51% |
| mover-setpiece | 0.118 / 0.182 | 0.050 / 0.061 / 0.071 / 0.093 | +41% / +53% |

### Frame diffs (fill 1 vs fill 0; same units as above)

| pose | mode | mean | p99 | max | > 2/255 | signed | fill0 luma |
|---|---|---|---|---|---|---|---|
| movement-feel m1-rel | full | 1.73 | 5 | 11 | 30.0% | +0.82 | 37.8 |
| movement-feel m2-rel | full | 0.68 | 2 | 6 | 0.1% | +0.32 | 40.6 |
| movement-feel m3-abs | full | 1.42 | 5 | 7 | 13.7% | +0.68 | 30.1 |
| movement-feel m4-abs | full | 0.88 | 3 | 6 | 2.3% | +0.40 | 27.7 |
| movement-feel m5-cor | full | 0.67 | 2 | 4 | 0.1% | +0.31 | 5.3 |
| mover v1-station1-wall | full | 1.75 | 5 | 13 | 9.0% | +1.35 | 122.0 |
| mover v2-south-face | full | 1.92 | 7 | 14 | 16.9% | +1.51 | 105.6 |
| mover v3-north-face | full | 2.39 | 8 | 14 | 41.0% | +1.89 | 101.3 |
| movement-feel m1-rel | indirect | 3.17 | 6 | 12 | 71.0% | +1.52 | 7.7 |
| movement-feel m2-rel | indirect | 1.50 | 4 | 7 | 7.7% | +0.72 | 5.1 |
| movement-feel m3-abs | indirect | 2.83 | 6 | 7 | 56.5% | +1.36 | 4.5 |
| movement-feel m4-abs | indirect | 1.81 | 4 | 7 | 19.4% | +0.87 | 3.2 |
| movement-feel m5-cor | indirect | 0.81 | 3 | 6 | 2.3% | +0.38 | 0.8 |
| mover v1-station1-wall | indirect | 8.49 | 14 | 18 | 98.4% | +6.88 | 24.1 |
| mover v2-south-face | indirect | 7.91 | 15 | 18 | 95.2% | +6.51 | 22.8 |
| mover v3-north-face | indirect | 8.31 | 13 | 16 | 98.9% | +6.91 | 26.5 |

**Reading:**
- **movement-feel:** behaves like campaign-test. The indirect term is near black (3–8/255 in indirect-only frames), so fill 1 adds under 1 luma level in full lighting. Invisible.
- **mover-setpiece:** the indirect term is substantial (about 24/255), and fill 1 lifts it +7 levels on the mover and walls alike. Direct light dominates the full frame (about 100–120/255), though, so full lighting moves only +1.4 to +1.9 levels (p99 5–8). It is barely perceptible even flipping back and forth. The mover receives the same lift as the static walls around it.
- **Anomalies:** none. There are no patch-scale blotches on the mover in the indirect-only frames: the diff is flat per checker square and follows albedo. No leaks were seen in either map.
- **Pose note:** a first corridor pose sat on the grid's outer probe column. Half the frame showed the clear colour, so the camera was in or on the outer wall even though the probe there reads valid. That pose was replaced. The extra lateral-clearance rule above prevents it.

## Re-run

```bash
python3 -m venv /tmp/abvenv && /tmp/abvenv/bin/pip install numpy pillow
cargo build --release -p postretro --features capture,dev-tools   # run.py defaults to target/release/postretro
/tmp/abvenv/bin/python measurements/surface-radiance-cache/ab-captures/select_poses.py [section ...]   # optional: rewrites poses.json sections
/tmp/abvenv/bin/python measurements/surface-radiance-cache/ab-captures/run.py [main|movement-feel|movers ...] [--pair <map>/<pose>]
```

The first batch was captured with a dev-profile `--features capture,dev-tools` build, and the second with `target/release/postretro`. `run.py --skip-capture` rebuilds the diffs and contact sheets from `raw/`. `POSTRETRO_BIN` overrides the engine binary. Per-capture logs go to `$CAPTURE_LOG_DIR/ab-capture-logs` (default `/tmp`). Each `scenes/*.json` is also a standalone scene: run `<capture binary> --capture <scene>` from the workspace root with `POSTRETRO_SH_STREAMING=sync-proof`.
