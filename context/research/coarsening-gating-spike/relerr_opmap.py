#!/usr/bin/env python3
"""Relative-error (Weber-anchored) operating-point map for SH coarsening.

Consumes a --sh-analyze JSON that carries the per-brick `composed_magnitude`
field (added to prl-build). For each gate statistic and each RELATIVE threshold
(error as a fraction of local composed-irradiance magnitude), reports how many
bricks coarsen and the resulting delta density ratio.

Why relative: the raw gate error is an absolute max-per-channel irradiance diff
with no perceptual anchor. Dividing by the matching magnitude statistic yields a
Weber-style deviation (percent of local brightness), which is what a metric-only
threshold must be stated in to be defensible without a rendered A/B.

A darkness floor guards the near-black divide-by-zero: below the floor the
receiver is too dim for the error to matter, so the brick is treated as freely
coarsenable. The floor is expressed as a fraction of the map's p95 brick
magnitude, so it scales with the map's light level rather than being a fixed
irradiance guess.
"""
import json
import sys


def main(path, floor_frac=0.02):
    d = json.load(open(path))
    bricks = [b for b in d["bricks"]
              if b["composed_l1"]["evaluable"] or b["composed_l2"]["evaluable"]]
    NE = d["nonempty_bricks"]
    uniform_denom = d["brick_count"] * 64

    if not bricks or "composed_magnitude" not in bricks[0]:
        print("ERROR: JSON has no composed_magnitude — rebake with the extended "
              "--sh-analyze.")
        return

    # Darkness floor from the map's brightness distribution.
    mags = sorted(b["composed_magnitude"]["p95"] for b in bricks
                  if b["composed_magnitude"]["texel_samples"] > 0)
    map_p95_mag = mags[int(0.95 * (len(mags) - 1))] if mags else 0.0
    floor = max(floor_frac * map_p95_mag, 1e-6)
    print(f"map p95 brick magnitude = {map_p95_mag:.4f}; darkness floor = {floor:.4f} "
          f"({floor_frac:.0%} of p95)")

    def rel(b, level, stat):
        lvl = b[f"composed_l{level}"]
        if not lvl["evaluable"]:
            return None
        mag = b["composed_magnitude"][stat]
        return lvl[stat] / max(mag, floor)

    def choose(b, stat, thr):
        r2 = rel(b, 2, stat)
        if r2 is not None and r2 <= thr:
            return 2
        r1 = rel(b, 1, stat)
        if r1 is not None and r1 <= thr:
            return 1
        return 0

    def stored(b, lvl):
        vp = b["valid_probes"]
        if lvl == 0:
            return vp
        if lvl == 2:
            return 1 if vp > 0 else 0
        return max(1, round(8 * vp / 64)) if vp > 0 else 0

    compact_ratio = sum(stored(b, 0) for b in bricks) / uniform_denom
    thresholds = [0.05, 0.10, 0.15, 0.25, 0.40]
    for stat in ["max", "p95", "mean"]:
        print(f"\n=== gate on RELATIVE {stat} (error / composed_magnitude.{stat}) ===")
        print(f"{'rel_thr':>8} {'L0':>5} {'L1':>5} {'L2':>5} {'coarsen%':>9} "
              f"{'ratio':>7} {'vs_compact':>10}")
        for thr in thresholds:
            cnt = {0: 0, 1: 0, 2: 0}
            st = 0
            for b in bricks:
                lvl = choose(b, stat, thr)
                cnt[lvl] += 1
                st += stored(b, lvl)
            coarsen = 100 * (cnt[1] + cnt[2]) / NE
            ratio = st / uniform_denom
            vsc = 100 * (1 - ratio / compact_ratio)
            print(f"{thr:>7.0%} {cnt[0]:>5} {cnt[1]:>5} {cnt[2]:>5} "
                  f"{coarsen:>8.1f}% {ratio:>7.3f} {vsc:>9.1f}%")
    print(f"\n(compaction-only baseline ratio = {compact_ratio:.3f})")


if __name__ == "__main__":
    p = sys.argv[1] if len(sys.argv) > 1 else "mini-2m.sh-analysis.json"
    main(p)
