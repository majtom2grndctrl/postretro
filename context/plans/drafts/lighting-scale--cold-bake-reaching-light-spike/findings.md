# Cold-Bake Reaching-Light Cull — findings

Build-to-learn spike. Deliverable is a decision, not a shippable feature. See
`index.md` for the contract and `context/lib/experimental_spikes.md` for the
findings-note form.

## TL;DR

On `stress-warren-lit` at the ship (cold `--no-cache`) config, a typical bake
receiver is reached by only **~4–6 % of the static light set** (≈6–7 of 157
lights). The cull is worth doing **for the cold SH bake** — but the measurement
also shows the right mechanism is **not** the affinity-cell reaching-light index
the spike anticipated. It is the **per-receiver falloff-range early-out the cold
lightmap bake already performs and the cold SH bake is missing.** That one change
cut the cold SH stage **20.9× (245.9 s → 11.8 s)** on the fixture, byte-for-byte
identical output. The cold lightmap bake already gates its shadow rays this way,
so it has no comparable win.

Recommendation: **promote to an implementation spec — a small one** (add the
missing falloff early-out to the cold SH bake), and **defer** any affinity-cell /
portal reaching-light index for the cold bakes as not worth the hardening.

## What was measured, and how

Instrumentation lives in `crates/level-compiler/src/spike_reach.rs` plus small
call-site hooks in `sh_bake.rs`, `lightmap_bake.rs`, `affinity_grid.rs`, and
`pipeline.rs`. It is gated entirely behind env vars and defaults **off**, so
ordinary and shipped bakes are byte-for-byte unaffected. Re-run with:

```bash
# distributions (both cold bakes) — logs "SPIKE …" lines at RUST_LOG=info
RUST_LOG=info POSTRETRO_SPIKE_REACH_STATS=1 prl-build \
  content/dev/maps/stress-warren-lit.map -o /tmp/out.prl \
  --sh-probe-spacing 10.0 --lightmap-density 0.25 --no-cache

# byte-identical range cull (for wall-clock / byte-identity), no stats overhead
POSTRETRO_SPIKE_REACH_CULL=1 prl-build … --no-cache
```

Per receiver we record two counts, each as a full histogram keyed on
reaching-light count (exact min/median/p95/max/mean, no sampling):

- **in-range** — lights within falloff range of the receiver's **exact point**
  (a shadow ray hit point for the SH bake, a texel world position for the
  lightmap). This is the set a byte-identical per-receiver range early-out keeps.
  Mirrors `falloff()` exactly: a light contributes zero iff `dist > falloff_range`,
  for every falloff model. Directional lights are always in-range.
- **mechanism** — the count the shipped affinity-cell cull (falloff-sphere AABB ∩
  portal-reachability flood, the same `decompose_affinity_for_lights` the direct
  SH bake consumes) would keep for the receiver's affinity cell.

Both honor the spike's correctness constraints: receivers are attributed to the
hit point / texel position, never the probe's cell; directional lights are never
range-culled and are kept in every affinity cell.

Fixture: `content/dev/maps/stress-warren-lit.map`, the bake-stress warren — 157
rooms, **157 static baked lights** (all `light` / `light_spot`; this committed
map has no dynamic or directional lights). Ship config `--sh-probe-spacing 10.0
--lightmap-density 0.25 --no-cache`.

## Measured finding — reaching-light fraction

`stress-warren-lit`, ship config, 157 static lights. Fraction = count / 157.

### Cold SH bake (indirect) — 421,933 shadow-ray hit-point receivers

| set | min | median | p95 | max | mean |
|-----|-----|--------|-----|-----|------|
| **in-range** (exact per-point range) | 1 (0.6 %) | 7 (4.5 %) | 10 (6.4 %) | 14 (8.9 %) | 6.77 (4.3 %) |
| **mechanism** (affinity cell) | 1 (0.6 %) | 6 (3.8 %) | 33 (21.0 %) | 35 (22.3 %) | 9.81 (6.2 %) |

### Cold lightmap bake — 10,036,267 texel receivers

| set | min | median | p95 | max | mean |
|-----|-----|--------|-----|-----|------|
| **in-range** (exact per-point range) | 1 (0.6 %) | 6 (3.8 %) | 10 (6.4 %) | 15 (9.6 %) | 6.46 (4.1 %) |
| **mechanism** (affinity cell) | 1 (0.6 %) | 6 (3.8 %) | 33 (21.0 %) | 35 (22.3 %) | 9.99 (6.4 %) |

Two reads on the distribution:

1. **The reaching fraction is small, with a modest tail.** Half of all receivers
   see ≤6–7 lights; 95 % see ≤10 by exact range. The p95/max blow up only for the
   *mechanism* set (33–35), because the coarse affinity cell (probe_spacing ×4 =
   40 m here, plus 0.5 m padding) over-keeps lights near cell boundaries. The
   exact per-point range set stays tight everywhere (p95 = 10, max ≤15).
2. **The exact per-point range set is *tighter* than the affinity-cell set**
   (mean 6.8 vs 9.8 for SH; 6.5 vs 10.0 for the lightmap). On this fixture the
   affinity grid's AABB looseness outweighs anything its portal-reachability half
   removes — so the coarser, more complex mechanism keeps *more* lights than a
   plain distance test, not fewer.

## Measured finding — wall-clock (prototype cull)

The prototyped cull is the byte-identical per-receiver falloff early-out applied
to the cold SH bake (skip the 32-sample soft-visibility shadow ray for a light
whose falloff is provably zero at the hit point). Cold lightmap already does this
before its shadow ray, so the flag is a no-op there. Per-stage wall-clock from
the pipeline's own stage timing, matched runs (`--lightmap-density 0.5` so the
run completes — see Caveats; the SH stage is independent of lightmap density):

| stage | baseline | with cull | delta |
|-------|----------|-----------|-------|
| **Cold SH bake** | 245.9 s | **11.8 s** | **−234 s, 20.9× faster, −95.2 %** |
| Cold lightmap bake | 36.6 s | 38.9 s | no change (within run-to-run noise) |

The SH speedup tracks the in-range fraction: casting ~4.3 % of the shadow rays
instead of 100 % is a ~23× reduction in ray work; the realized 20.9× is that,
less the residual per-light iteration and ray-setup overhead. The cold SH bake
is the dominant compile stage (the spike cites ~83 % on `campaign-test`), so this
is a large fraction of total compile.

## Correctness confirmation (honesty gate)

- **Receiver attribution.** Counts are read at the shadow-ray hit point (SH) and
  the texel world position (lightmap), not the probe's cell — implemented and
  exercised (see `sh_bake.rs` `sample_radiance_rgb`, `lightmap_bake.rs`
  `bake_face_chart`).
- **Directional lights always reaching.** `reaches_range` returns true for
  `Directional`, and the affinity decompose keeps them in every cell. Not
  exercised on this fixture (no `light_sun` present) but the path is correct.
- **Byte-identical cull.** Baseline vs. cull produced **SHA256-identical** `.prl`
  files (`f9c12dfc…`, full 16.6 MB output). A range-culled shadow ray is provably
  zero-contribution (`falloff` returns 0 for `dist > range` in every model), and
  the kept lights keep their global soft-visibility seed, so output is unchanged.

## Why the mechanism differs from the spike's premise

The spike framed the fix as "extend the affinity-cell reaching-light cull (already
used by the direct/delta/animated-direct SH bakes) to the cold bakes." The
measurement revises that in two ways:

1. **The cold *lightmap* bake needs nothing.** It already returns before the
   shadow ray for any out-of-range light
   (`light_texel_contribution_and_visibility`). Its shadow rays are already
   gated to the in-range set; a reaching-light index would only skip the *cheap*
   per-light contribution math for far lights, on an already-small stage. Not
   worth a cull.
2. **The cold *SH* bake has the whole problem — and a simpler fix than the
   affinity index.** It casts the 32-sample soft-visibility shadow ray
   *unconditionally* for every light, applying falloff only afterward
   (`sample_radiance_rgb`). Adding the same per-receiver falloff early-out the
   lightmap already has:
   - is a few lines, needs no portal graph, no cell decomposition, no reachability
     flood, no new section or wire change;
   - is **exact** (distance vs. hard falloff), so it is content-independent — no
     real-map validation gate is required for *correctness* (only the *magnitude*
     of the win is fixture-dependent);
   - is byte-identical with no cell-boundary hazard (the affinity cell's centroid
     portal test can disagree with an arbitrary hit point inside a straddling
     cell — a hardening cost the range test simply avoids);
   - is, on this fixture, **tighter** than the affinity-cell set (fewer shadow
     rays cast).

   The affinity cull's genuine advantages — amortizing the reach test across a
   region and skipping the O(N) per-light *iteration* — are marginal at these
   light counts (N = 157; the shadow ray dominates, and the loop already visits
   every light). They would matter only at much higher light counts. Its
   portal-reachability half could additionally skip in-range-but-occluded rays
   (a light within range but behind a wall), which the range test still casts and
   the shadow ray then zeroes; on this fixture that residual is not large enough
   to justify the machinery, since range alone already removes ~95 %.

## Recommendation

- **Cold SH bake → promote to a (small) implementation spec.** Add the missing
  per-receiver falloff-range early-out before the soft-visibility shadow ray in
  `sample_radiance_rgb`, matching what the cold lightmap bake already does. Large,
  content-independent win (~95 % of the stage on the fixture; the stage is the
  bulk of compile), byte-identical, no format/portal/cell work. This is a much
  smaller spec than "extend the affinity cull," and the two should not be
  conflated.
- **Cold lightmap bake → defer / no action.** Already gates shadow rays by range;
  no comparable win.
- **Affinity-cell / portal reaching-light index for the cold bakes → defer.** For
  the cold SH bake it is looser and more complex than the exact range test, and
  brings a cell-boundary byte-identity hazard. Revisit only if a future map
  pushes static-light counts high enough that skipping per-light *iteration*
  (not just the shadow ray) becomes material, or if in-range-but-occluded shadow
  rays are shown to dominate on real content.

### Prerequisites honored

Per `index.md`, the measured fraction is a **floor / mechanism bound on a
synthetic fixture**, not a production projection — `stress-warren-lit` is a
pressure probe, and the ~4–6 % number should not be projected onto real content.
What *is* content-independent is the mechanism conclusion: the cold SH bake casts
provably-zero shadow rays that the cold lightmap bake already skips, and closing
that gap is exact regardless of how many lights reach a receiver on a real map.
Real-map validation would refine the *magnitude* of the win, not its correctness.
The prototype and instrumentation were committed on this branch (env-gated, off by
default) and recorded here with their env flags (`POSTRETRO_SPIKE_REACH_STATS` /
`POSTRETRO_SPIKE_REACH_CULL`) and method, so re-measurement on a real map — once
one exists — is reproducible. The implementation spec that hardens this finding
(`lighting-scale--cold-sh-bake-falloff-early-out`) makes the SH early-out
unconditional and **removes the measurement harness in full**; re-measuring later
is then a `git restore` of the spike commit, not a live env flag in the shipped
compiler.

## Reproducibility notes

- Fractions measured at `--lightmap-density 0.25` (ship config); wall-clock at
  `--lightmap-density 0.5`. The cold SH stage is independent of lightmap density
  (same `--sh-probe-spacing 10.0`), so its baseline/cull timings transfer; the SH
  reaching fractions are density-independent by construction.
- The full-fidelity 0.25/0.5 runs were **OOM-killed by the container at the
  Shadowmask atlas bake tail stage** (well after the SH and lightmap stages this
  spike measures). That is an environment memory ceiling on a 157-light warren,
  not a bake failure, and does not affect any measured value — every SPIKE line
  and stage timing is emitted before the tail. The byte-identity check was run at
  `--lightmap-density 1.0`, which completes and writes the `.prl`.
- `POSTRETRO_SPIKE_REACH_STATS=1` adds non-trivial wall-clock (atomic histogram
  contention over ~10 M lightmap receivers); use it only for distributions, and
  measure wall-clock with stats off.
