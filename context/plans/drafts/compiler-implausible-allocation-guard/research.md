# Research — implausible-allocation guard

Read at `867e2c1a`.

## The observed size is not scaled-up legitimate work

One `--release` compile of `stress-warren-hallway-inspection.map` at density 0.04 requested
**42,865,923,486,912 bytes**. Three independent arguments put it outside the space of
legitimate lengths in the lightmap-atlas bake path.

**It exceeds the format ceiling.** `MAX_ATLAS_DIMENSION` is 8192 and `MAX_ATLAS_LAYERS` is
256 (`lightmap_bake.rs`). The largest single allocation any per-light structure in that path
can reach is one light's texel vector over a full atlas: 8192 × 8192 × 256 × 48 B =
824,633,720,832 B, with 48 B pinned by the `size_of` const assert on `LayerTexel`
(`lightmap_layer.rs`). The observed request is **51.98×** that.

**Its 2-adic valuation is wrong for a plane multiple.** The size factors as
2⁶ · 3 · 199 · 1,121,909,639, so it is divisible by 64 and not by 128. `round_atlas_dim`
(`lightmap_bake.rs`) returns `raw.max(MIN_ATLAS_DIMENSION).next_power_of_two().min(max_dim)`,
with `MIN_ATLAS_DIMENSION` 64 and both bounds powers of two — so every atlas dimension is a
power of two ≥ 64, every plane has valuation ≥ 12, and every multiple of a plane inherits it.
A plane-derived length cannot land at valuation 6.

**It was requested at full size, not reached by growth.** Doubling from a small initial
capacity forces `final = 2^k · c₀ · elem`. For c₀ ≤ 8 and an element of 1, 4, 16, or 48
bytes, reaching ~4.3e13 needs k ≥ 40, hence valuation ≥ 40. Hashbrown table growth is ruled
out the same way: its allocation is a power-of-two bucket count times an odd per-entry
stride, so the valuation equals log₂(capacity), which would be ≈ 40 here. Observed valuation
is 6. The request therefore came from a single computed length, which is exactly what the
guard gates.

**What the factorization does not narrow.** 42,865,923,486,912 divides evenly by 4, 16, and
48, so element size discriminates nothing. Candidate shapes stay open: a count summed across
lights rather than derived from dimensions, or a count read from a payload.

## `from_bytes` is excluded, and its guard is dead

`LightmapLayer::from_bytes` (`lightmap_layer.rs`) was a candidate — a length read from bytes
driving a reserve. It is not:

- Its allocation is `pod_collect_to_vec` over the payload slice it already holds, so the
  request is bounded by the bytes read. It cannot reach 42.8 TB.
- Its overflow guard, `count.checked_mul(size_of::<LayerTexel>())`, cannot fail on a 64-bit
  target: `count` decodes from a `u32`, so the product tops out at 4,294,967,295 × 48 =
  206,158,430,160 — 208× *below* the observed size and far below `usize::MAX`.
- `from_bytes_rejects_overflowing_texel_count` therefore does not test the overflow branch.
  With `count = u32::MAX` the multiply succeeds and `payload.len() != expected` returns
  `None`. The test passes for the wrong reason and the branch it names is unreachable.

This is a real defect at a real site, and the right bound there is the atlas the layer claims
to cover, not a machine word.

## Every size guard in the compiler has the same shape

`checked_mul` appears at 53 sites across 13 modules — `cell_visibility_bake.rs`,
`chunk_light_list_bake.rs`, `delta_sections.rs`, `lightmap_layer.rs`, `pack.rs`,
`pipeline.rs`, `sh_analyze.rs`, `sh_density.rs`, `sh_group.rs`, `sh_runtime_envelope.rs`,
`sh_runtime_envelope_scoring.rs`, `shadowmask_bake.rs`, `size_options.rs`. Every one either
`expect`s or maps to an error naming overflow. Not one compares against a bound derived from
the inputs. The defect is uniform, which is why the mechanism is shared and only the bound is
per-site. Roughly 470 `with_capacity` / `vec![…; n]` sites exist in the crate; routing all of
them is not proportional, and the overflow guards are the author-marked subset that matters.

## Why not a budget

`lighting-scale--compile-peak-ram` (done) ships `gate_delta_working_set` in `pipeline.rs`: a
plan-phase projection over the three SH-delta bakes, compared against a CLI-configurable
budget (`--sh-delta-working-set-max-size`, parsed by `size_options.rs`), refusing with a
per-light histogram. Its acceptance splits refuse side and permit side, and one criterion
reads that compiling this very map "no longer OOMs" because it is over budget.

The map still died. That gate is scoped by construction to three bakes, and a budget is the
wrong instrument for a corrupt length anyway: a budget passes on a large enough host, while a
length the inputs cannot justify is wrong on every host. The two compose — the budget answers
"can this host finish", the bound answers "is this derivable".

That plan rejected *in-bake* gating because threading a running total makes the three bakes
fallible and leaves no single point seeing all three projections. Neither objection reaches a
plausibility bound: it has no cumulative term, needs no global view, and aborts rather than
returning an error, so no signature changes.

## Measurement posture

The compiler has no peak-RSS, allocation counter, or measured-memory instrumentation;
`reporter.rs` carries stages and timing only. `shadowmask-bake-scaling` (done) measured
process RSS out-of-band and reported it as a manual gate. `gate_delta_working_set`'s
projection is the only footprint number in-process, and it is a projection.

Seeing the raw allocator request in-process would need a `GlobalAlloc` wrapper, which needs
`unsafe impl`. `index.md` §2 forbids `unsafe` without owner consultation, so the in-process
route is closed by default. The guard's own diagnostic is the cheap substitute: it names the
computation before the request is made.

## Relationship to the shadowmask brief

`lighting-scale--shadowmask-cold-working-set` (draft) carries the same attribution decision —
"the restructure may delete the site, so the guard sits where a computed or read length
becomes an allocation, not at one call" — and one Acceptance row for it. That brief's guard
row cannot land until its equivalence gate decides between the analytic shape and the
bounded-membership fallback. This brief is that row lifted out, widened to the compiler, and
proved on literals so it lands first. Nothing here depends on which shape that gate selects.
