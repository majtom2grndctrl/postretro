# Lightmap bake scaling — pre-change baseline measurement

Pre-change baseline of the static `LightmapSection` (PRL section 22) for the
stress-warren hallway-inspection fixtures. Captured to ground the spec's evidence
premise (the "direction blob is ~80% of lightmap bytes" claim and the 256-layer
array-cap question) and to seed before/after acceptance-criteria baselines. No
engine or compiler source was changed.

## Measured basis

**The 0.25 m/texel rows are the representative baseline.** These fixtures carry
no `_lightmap_density` KVP, but 0.25 m — not the `DEFAULT_TEXEL_DENSITY_METERS =
0.04` code default — is the *documented* production density for the
`--preset warren` family: `content/dev/maps/stress-warren.README.md:155-158`
("0.25 clears it and still stays under the 8192² atlas cap. This is why
`--preset warren` is documented to bake at 0.25.") and README:219-222 ("0.25 m is
the default here"). Every warren compile line in the README passes
`--lightmap-density 0.25`. So the 0.25 rows below are the real before-baseline.

The 0.04 rows are an **overflow probe, not a baseline** — a check of what the code
default does on these large-surface maps (see the 0.04 outcome and budget-axis
note below). They are marked incomplete because neither 0.04 bake finished within
budget.

| Fixture | Density | Role | Build profile | Direction blob (B) | Irradiance blob (B) | Direction % of lightmap blob | Array layers | dir dims | irr dims | dir_format | irr_format |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `stress-warren-hallway-inspection-mini.map` | **0.25** | **representative baseline** | cargo `--release`; `prl-build --no-cache` | 19,922,944 | 4,980,736 | 80.0000% | 19 | 512×512 | 512×512 | Rgba8Unorm octahedral (0) | Bc6hRgbUfloat (1) |
| `stress-warren-hallway-inspection.map` (full) | **0.25** | **representative baseline** | cargo `--release`; `prl-build --no-cache` | 37,748,736 | 9,437,184 | 80.0000% | 36 | 512×512 | 512×512 | Rgba8Unorm octahedral (0) | Bc6hRgbUfloat (1) |
| `stress-warren-hallway-inspection-mini.map` | 0.04 (code default) | overflow probe — did not complete | cargo `--release`; `prl-build --no-cache` | n/a (bake incomplete) | n/a | — | packed OK, count only in post-bake log (not reached) | — | — | — | — |
| `stress-warren-hallway-inspection.map` (full) | 0.04 (code default) | overflow probe — did not complete | cargo `--release`; `prl-build --no-cache` | n/a (bake incomplete) | n/a | — | packed OK, count only in post-bake log (not reached) | — | — | — | — |

"Direction % of lightmap blob" is `dir / (dir + irr)`, i.e. of the two texel
blobs; the 48-byte section header is excluded (negligible). Both representative
fixtures land at exactly 80.0000%, so the spec's "~80%" premise is confirmed and
is in fact structural (see below), not incidental to these fixtures.

Additional context per fixture (0.25, representative):
- mini: 381 static lights baked; `.prl` on disk 47,139,604 B; bake ~235 s; peak RSS not separately captured (well under the box's 16 GB).
- full: 763 static lights baked; `.prl` on disk 89,814,903 B; bake ~292 s; peak RSS ~1.56 GB (measured by polling `/proc/<pid>/status` VmRSS).

## Exact commands and how each number was obtained

Compiler built once:

```bash
cargo build --release -p postretro-level-compiler   # produces ./target/release/prl-build
```

Bakes (throwaway outputs under the scratchpad, never into `content/`):

```bash
# mini (ran first, per guardrails)
./target/release/prl-build content/dev/maps/stress-warren-hallway-inspection-mini.map \
  -o <scratch>/swh-mini.prl \
  --no-tui -v --sh-probe-spacing 10.0 --lightmap-density 0.25 --no-cache

# full
./target/release/prl-build content/dev/maps/stress-warren-hallway-inspection.map \
  -o <scratch>/swh-full.prl \
  --no-tui -v --sh-probe-spacing 10.0 --lightmap-density 0.25 --no-cache

# 0.04 overflow probes (same commands with --lightmap-density 0.04); neither completed
./target/release/prl-build content/dev/maps/stress-warren-hallway-inspection-mini.map \
  -o <scratch>/swh-mini-004.prl \
  --no-tui -v --sh-probe-spacing 10.0 --lightmap-density 0.04 --no-cache
./target/release/prl-build content/dev/maps/stress-warren-hallway-inspection.map \
  -o <scratch>/swh-full-004.prl \
  --no-tui -v --sh-probe-spacing 10.0 --lightmap-density 0.04 --no-cache
```

### 0.04 (code default) overflow-probe outcome

The code default is `DEFAULT_TEXEL_DENSITY_METERS = 0.04`
(`crates/level-compiler/src/lightmap_bake.rs:27`) — ~39× more texels than 0.25
(6.25× per axis). Probing what it does on these maps:

- **mini @ 0.04**: the atlas **packed successfully** — no `LayerOverflow`, no
  `ChartTooLarge`, no density-halving, no "overlapping atlas rects". The bake
  entered the per-texel stage and reached ~40% with an ETA of ~890 s (≈25 min
  total projected) before a 600 s watchdog killed it. Peak RSS ~2.97 GB. No cap
  error was produced; it is simply slow.
- **full @ 0.04 (definitive run to time budget)**: the atlas **packed
  successfully** (≤ 256 layers, ≤ 8192² dims — a real overflow errors *at pack*),
  then ran the per-texel bake for the full ~65-minute budget, reaching **93%**
  (ETA ~264 s remaining → ≈ 70 min projected total) before a **time** watchdog
  killed it. It was **not RAM-bound**: VmRSS/VmHWM climbed slowly and roughly
  linearly with charts baked, from ~3.33 GB at 0% (t=27 s) to ~7.82 GB at 93%
  (t=3929 s); **peak VmHWM = 8,006,932 KiB ≈ 7.64 GiB (~7.8 GB)**. System
  `MemAvailable` never fell below ~7.65 GB (floor was 600 MB), and VmRSS never
  approached the 15 GB kill line. Extrapolating the linear curve, completion
  (~100%) would peak around ~8.3 GB — still far under 15 GB. Monitored by polling
  `/proc/<pid>/status` (`VmRSS`/`VmHWM`) and `/proc/meminfo` (`MemAvailable`)
  every 60 s. Because it did not complete, the post-bake `log_stats` line never
  printed, so the exact 0.04 layer count / atlas dims / blob bytes remain
  uncaptured — but they are irrelevant to the RAM-vs-time question, which is
  settled.

  **Verdict — full stress-warren @ 0.04 is TIME-BOUND, not RAM-bound:** peak
  VmHWM ~7.8 GB with ~7.65 GB of headroom to spare, no OOM; the barrier is
  throughput (~70 min of per-texel bake), not memory. So peak-RAM is NOT the
  0.04 blocker for this map — bake time is.

**No overflow and no density-halving occurred at 0.04 for either fixture in the
current lightmap baker.** This is expected from the code: the multi-bin packer
"opens new array layers instead of failing on atlas area, so there is no
density-coarsening retry" (`crates/level-compiler/src/main.rs:648-650`); an actual
atlas-area or layer-count overflow *hard-errors* (`LightmapBakeError::{ChartTooLarge,
LayerOverflow}` in `lightmap_bake.rs`), it does not silently coarsen. The
README's "automatic density-halving fallback" phrasing (README:219-222) does not
match the current lightmap path — there is no such fallback in
`lightmap_bake.rs`. The separate "overlapping atlas rects" abort the README cites
at 155-158 lives in the **animated-weight-map** packer
(`crates/level-compiler/src/animated_light_weight_maps.rs:572`), a distinct stage
that runs after the main lightmap bake and was **not reached** in these 0.04
probes (both were killed during the main lightmap per-texel bake).

`--lightmap-density 0.25` is the documented density for the `--preset warren`
family (`content/dev/maps/stress-warren.README.md` §Animated lights / §Compile);
`--sh-probe-spacing 10.0` keeps the whole-world SH grid tractable. `--no-cache`
guarantees a full cold bake so the reported bytes are not served from a cache.
The bake blob bytes are independent of build profile and of the `prl-build
--release` (exact/shippable) flag — those affect speed/cache, not section-22
bytes — so cargo `--release` + `--no-cache` is a valid basis.

### Direction bytes, irradiance bytes, layer count, dims, density — from tool output

The `-v` (verbose) flag gates `lightmap_bake::log_stats`
(`crates/level-compiler/src/pipeline.rs:1084-1086`), which prints the section's
own fields (`crates/level-compiler/src/lightmap_bake.rs:604-615`). The emitted
lines were:

```
mini: Lightmap: 512x512x19 atlas, 0.25 m/texel, 381 static lights baked, irr=4980736 B, dir=19922944 B
full: Lightmap: 512x512x36 atlas, 0.25 m/texel, 763 static lights baked, irr=9437184 B, dir=37748736 B
```

`log_stats` prints, in order, `irr_width × irr_height × layer_count`, the
irradiance texel density, the static-light count, then `section.irradiance.len()`
and `section.direction.len()` (the two blob byte counts across ALL layers). So
the direction/irradiance byte totals, the atlas layer count, and the per-layer
irradiance dims + density come straight from tool output.

### Cross-check computed from the section header

`log_stats` does not print `dir_width/dir_height`, `dir_format`, or `irr_format`
separately, so those were derived from the header contract in
`crates/level-format/src/lightmap.rs` and confirmed against the printed byte
totals:

- Direction blob (`LightmapSection::direction`, Rgba8Unorm octahedral,
  `DIRECTION_TEXEL_BYTES = 4`, layer-major): `dir_width × dir_height × 4 × layers`.
  - mini: 512 × 512 × 4 × 19 = 19,922,944 B — matches the printed `dir` exactly.
  - full: 512 × 512 × 4 × 36 = 37,748,736 B — matches exactly.
  This is only consistent with `dir_width = dir_height = 512`; `encode_section`
  sets the direction dims equal to the atlas dims
  (`lightmap_bake.rs:249-260`), so direction dims are 512×512 (= the printed
  irradiance dims). `dir_format` is `DIRECTION_FORMAT_OCT_RGBA8` (0) — the only
  defined value (`lightmap.rs:134`).
- Irradiance blob: the printed sizes match the BC6H layout
  (`ceil(w/4)·ceil(h/4)·16` per layer, `IRRADIANCE_FORMAT_BC6H`), NOT RGBA16F:
  - mini BC6H: 128 × 128 × 16 × 19 = 4,980,736 B — matches printed `irr`.
    (RGBA16F would be 512×512×8×19 = 39,845,888 B, which it is not.)
  - full BC6H: 128 × 128 × 16 × 36 = 9,437,184 B — matches printed `irr`.
  So `irr_format = Bc6hRgbUfloat` (1) — BC6H is the bake default
  (`lightmap_bake.rs:443-450`; `--uncompressed-irradiance` was not passed).

### Why the 80% is structural

With BC6H irradiance and Rgba8 direction sharing identical dims and layer count,
per texel the irradiance costs 16 B / 16 texels = 1 B/texel-equivalent while the
direction costs 4 B/texel. Direction is therefore always 4× irradiance →
`4/(4+1) = 80%` for any BC6H bake, regardless of fixture. The spec should treat
"direction ≈ 80% of the lightmap blob" as a consequence of the BC6H-vs-RGBA8
format pairing, not a fixture-specific measurement. (Under
`--uncompressed-irradiance` the irradiance is RGBA16F at 8 B/texel and the split
inverts to direction ≈ 33%; the baseline above is the production BC6H path.)

## Budget-axis note

**Headline: at the documented representative density (0.25 m), NEITHER hard cap is
hit — the full fixture uses 36 of 256 array layers at 512×512 per layer (both caps
have large headroom). And the 0.04 probes did not hit either cap or any
density-halving either: they packed successfully and became bake-time-bound.** So
for these fixtures today, no atlas cap is the binding constraint on the lightmap —
the practical forcing function for the coarse 0.25 density is bake time / per-texel
cost, not a cap. (This diverges from the README's "8192² cap / automatic
density-halving fallback" wording, which does not match the current multi-layer
packer — see the 0.04 outcome above and the divergence note at the end.)

These are two distinct axes and must not be conflated:

- **(a) Per-layer 2D-dimension cap (`MAX_ATLAS_DIMENSION = 8192`,
  `lightmap_bake.rs:37`).** NOT hit at 0.25 (both fixtures pack each layer at
  512×512 — 1/16 of the cap; the packer sizes the per-layer square to host the
  largest single leaf, then spills to new layers rather than enlarging the
  square). NOT hit at 0.04 either — both 0.04 probes packed and entered the
  per-texel bake, which by construction requires ≤ 8192² per-layer dims (a chart
  or leaf exceeding a layer errors *at pack*, before any bake progress). The 0.04
  per-layer dim would be larger than 512² but stayed within the cap for these
  fixtures.
- **(b) 256-layer array cap (`MAX_ATLAS_LAYERS = 256`, `lightmap_bake.rs:43`).**
  NOT hit at 0.25:
  - mini: **19** array layers (of 256) — 237 layers of headroom (~7.4% of cap).
  - full: **36** array layers (of 256) — 220 layers of headroom (~14% of cap).

  NOT hit at 0.04 either — both 0.04 probes packed and entered the per-texel bake,
  which requires ≤ 256 layers (a `LayerOverflow` errors *at pack*). The exact 0.04
  layer counts were not captured (they print only in the post-bake `log_stats`,
  which the multi-minute bake never reached). These are genuine
  `texture_2d_array` layers from `LightmapSection.layer_count` (`log_stats` prints
  it as the third atlas dimension), NOT per-light incremental-cache layers — the
  per-light cache lives in `lightmap_layer.rs` and is bypassed here anyway because
  `--no-cache` was used, so nothing conflates the two.

At 0.25, layer count tracks leaf/chart count (which grows with geometry and
static-light count): 19 layers @ 381 lights vs 36 layers @ 763 lights — roughly
linear with fixture size. Reaching the 256-layer cap at 0.25 would take on the
order of ~7× the full fixture's geometry; the hallway-inspection fixtures do not
by themselves demonstrate either cap being approached. The spec's 256-layer
concern is real as a scaling ceiling but is not exercised by these fixtures at the
representative density — that gap is itself a useful piece of evidence.

### Divergence from the README / a caveat the spec should carry

The README ties the coarse 0.25 density to a hard "8192² atlas cap" and an
"automatic density-halving fallback" (README:201-204, 219-222). The current
lightmap baker has **no density-halving** — `main.rs:648-650` states outright
"the multi-bin packer opens new array layers instead of failing on atlas area, so
there is no density-coarsening retry," and an actual overflow *hard-errors*
(`LightmapBakeError::{ChartTooLarge, LayerOverflow}`). Empirically, 0.04 did not
overflow the main lightmap pack for either fixture. Two possibilities the spec
should not paper over: (1) the README's cap/halving language predates the
multi-layer packer and is stale for the main lightmap path; and/or (2) the true
forcing function for 0.25 is a *different* stage — most likely the
**animated-weight-map packer**, whose "overlapping atlas rects" abort
(`animated_light_weight_maps.rs:572`) is exactly what README:155-158 describes,
and which these probes did not reach (both were killed during the main lightmap
per-texel bake). Either way, the spec must not assert "the 8192² lightmap cap
forces 0.25" without verifying which stage actually binds; the measured behavior
of the main lightmap baker at 0.04 is "packs fine, then bakes for many minutes."

## OOM / did-not-complete notes

- **0.25 (representative) — both completed.** On a 16 GB box (no swap), contrary
  to the guardrail's caution about sibling stress fixtures OOM'ing at ~14 GB, the
  hallway-inspection bakes were memory-light: the full fixture peaked at ~1.56 GB
  RSS. A memory watchdog (kill if `MemAvailable` < ~900 MB, or at an 8-minute
  timeout) was armed for the full bake but never triggered. Bake times: mini
  ~235 s, full ~292 s.
- **0.04 (overflow probe) — neither completed; both killed by watchdog, not by
  OOM or by any cap error.**
  - mini @ 0.04: killed at 602 s (600 s timeout) at ~40% of the per-texel bake
    (~25 min projected total); peak RSS ~2.97 GB. No overflow/halving message.
    Consistent with the README's "bakes for many minutes."
  - full @ 0.04: ran the full ~65-minute time budget to **93%** of the per-texel
    bake (≈ 70 min projected total), then killed by the **time** watchdog — NOT by
    OOM. Peak **VmHWM ~7.8 GB** (8,006,932 KiB); `MemAvailable` never below
    ~7.65 GB; VmRSS never near the 15 GB line. **Definitively time-bound, not
    RAM-bound.** (An earlier ~9 s run was killed deliberately right after
    pack-success confirmation; this full-budget run supersedes it.)

All bakes used the cargo release build with `--no-cache` (cold bake).

## Files written by the bakes (throwaway; not committed, not in `content/`)

- `<scratch>/swh-mini.prl` (47,139,604 B) — 0.25, complete
- `<scratch>/swh-full.prl` (89,814,903 B) — 0.25, complete
- `<scratch>/mini-build.log`, `<scratch>/full-build.log` (verbose 0.25 bake logs)
- `<scratch>/mini-004-build.log`, `<scratch>/full-004-build.log` (0.04 probe logs; no `.prl` — both bakes were killed before completion)

where `<scratch>` is the session scratchpad directory.
