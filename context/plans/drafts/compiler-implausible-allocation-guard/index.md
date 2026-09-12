# compiler-implausible-allocation-guard

Brief · compact · reads: `context/lib/build_pipeline.md` §PRL Compilation, §Build Cache · `context/lib/experimental_spikes.md` · `context/lib/development_guide.md` §2.5 · read at `867e2c1a`

## Problem

The owner's `--release` compile of `content/dev/maps/stress-warren-hallway-inspection.map`
at lightmap density 0.04 requested 42,865,923,486,912 bytes on a 16 GiB machine and the
process died. Which computation asked is unknown, and the compiler cannot say: its size
computations guard machine-word overflow and nothing else, so a length that is wrong but
representable passes every guard, reaches the allocator, and aborts with a byte count and no
site. That size is not legitimate work scaled up: it clears the format's own ceiling for
that path fifty-fold (`research.md`). When this is done, a computed or read length its
inputs cannot justify aborts the build naming the computation, the length, the bound, and
the values the bound came from, and a findings note records what that diagnostic attributed
on the failing config.

## Decisions

- **The bound is plausibility, not budget.** Each computation is checked against a bound
  that is a pure function of the inputs legitimately determining it — for a bake over the
  lightmap atlas: width, height, layer count, selected-light count. No host-memory term, no
  flag. `lighting-scale--compile-peak-ram`'s configurable gate answers whether a host can
  finish; this answers whether the length is derivable at all. A corrupt length must fail
  identically on a 1 TiB host.
- **Refuse at the format ceiling, not at a tuned figure.** The bound admits every length the
  caps permit, including ones no machine could satisfy. A tuned bound needs re-tuning per map
  and becomes a second budget. The observed request clears the ceiling wide enough that the
  loosest derivable bound still catches it.
- **One helper owns the arithmetic and the message; each site supplies its bound.**
  Legitimate lengths differ by orders of magnitude across stages, so a compiler-wide constant
  is wrong. The helper multiplies the factors itself, folding machine-word overflow and
  over-bound into one diagnostic — overflow aborts today with no computed size in its
  message. Mechanism in a compiler-level module, the bound expression at the bake stage.
- **The failure is an abort carrying the diagnostic, not a recoverable error.** An
  implausible length is a compiler-invariant violation, not an authoring error, and these
  sites already abort. Errors would make bake signatures fallible — the cost
  `lighting-scale--compile-peak-ram` cited when it rejected in-bake budget gating.
- **Adoption covers every size-determining computation that precedes an allocation.** The
  existing machine-word overflow guards are the inventory to walk: writing one already
  identified that computation as size-determining. A guarded computation that allocates
  nothing needs no bound, and an allocation bounded by bytes already in hand is not
  size-determining.
- **The helper lives outside any bake stage's module,** so
  `lighting-scale--shadowmask-cold-working-set` deleting the shadowmask stage's size
  computations cannot delete the guard.
- **Attribution is reported rather than gated, and landing does not wait on it.** Per
  `experimental_spikes.md` the re-run is a measured finding: a diagnostic that names the
  computation and one that still names nothing are both results, the second bounding where
  the request is not. Both sides of the guard are asserted on literals, because reaching that
  config depends on the in-progress lightmap branch clearing an earlier stage.

### Non-goals

| Not doing | Warrant |
|---|---|
| The shadowmask working-set redesign | `lighting-scale--shadowmask-cold-working-set` owns it. This brief shares its attribution decision and depends on none of its shape |
| A host-memory budget, or adaptive density | `lighting-scale--compile-peak-ram` owns the budget gate and names forward-predicted adaptive density as its successor |
| Changing which maps compile, or any emitted byte | Every legitimate length stays admitted |
| An in-process allocator hook | Seeing the raw request needs a global-allocator wrapper, which needs `unsafe`, which needs owner approval (`index.md` §2) |
| A peak-RSS or allocation-counter probe | The compiler has none, and out-of-band measurement is its standing precedent (`shadowmask-bake-scaling`) |
| Log-level and verbosity changes | `compiler-log-hygiene` owns those. This output is a build failure, not a log line |
| Loader- and runtime-side allocation limits | The loader already checks on-wire byte lengths before decoder allocation. The defect is compiler-side |
| Splitting the files the guard touches | Structural refactoring never rides with a behavior fix (`development_guide.md` §2.5), and replacing an overflow guard adds no functionality to them |

## Acceptance

### Automated

- [ ] A computed length above its bound aborts the build, naming the computation, the
  computed length, the bound, and the input values the bound was derived from. Asserted on
  literals.
- [ ] A length exactly at its bound is admitted; the next representable length above it is
  refused. Both sides asserted on literals.
- [ ] Factors whose product exceeds a machine word are refused as a bound violation, with the
  computed length reported. That case aborts today with no size in the message.
- [ ] The largest length derivable at the maximum atlas dimension, the maximum layer count,
  and the selected-light count is admitted — a request no machine can satisfy, and still
  legitimate. Asserted on literals.
- [ ] The bound is unchanged across cache state, worker count, verbosity, and every
  byte-budget flag the compiler accepts.
- [ ] The refusal is reached before the allocation it guards, on every path that reaches that
  computation, cached and uncached alike.
- [ ] A length read from a payload, in a different stage from the computation above, is
  bounded by the same rule: a header declaring a count larger than its inputs permit is
  refused, and the refusal cites that bound rather than a machine word. The existing coverage
  there cannot reach the branch it names, so this row replaces it.

Regression guards — these pass today:

- [ ] A zero-valued factor yields a zero length, is admitted, and allocates nothing.
- [ ] Every dev map that compiles today compiles to byte-identical output.

### Manual

- [ ] A `--release` compile of the stress map at density 0.04 is attempted on the owner's
  machine. Record what the diagnostic named: the computation, or that the abort still named
  no site. Either outcome is the result.
- [ ] The findings note is delivered: each size computation routed through the guard with its
  bound, each size-determining computation left unguarded with the reason, and the
  attribution result with the candidate set it narrows to.

## Path

- Nearest precedent is `gate_delta_working_set` in `pipeline.rs` — a pre-bake projection with
  a refuse/permit split and a per-light histogram. Borrow its diagnostic register and its
  acceptance shape; not its budget semantics, and not its plan-phase placement.
- The inventory to walk is the existing `checked_mul` guards: `shadowmask_bake.rs`
  (`texel_plane_len`, `collect_layer_membership`, `shadowmask_data_offset`'s caller, and the
  `data_len` in both the fill and the empty-section constructor), `delta_sections.rs`,
  `sh_runtime_envelope.rs`, `sh_runtime_envelope_scoring.rs`, `chunk_light_list_bake.rs`,
  `cell_visibility_bake.rs`, `pack.rs`, `lightmap_layer.rs`. Several allocate nothing.
- The bound's inputs already exist as constants: `MAX_ATLAS_DIMENSION`, `MAX_ATLAS_LAYERS`,
  `MIN_ATLAS_DIMENSION`, `round_atlas_dim`'s power-of-two contract in `lightmap_bake.rs`, and
  the `size_of` const assert pinning `LayerTexel`.
- `LightmapLayer::from_bytes` is a found defect, not a candidate site — its allocation is
  bounded by the payload in hand, and its overflow guard cannot fire on a 64-bit target, so
  `from_bytes_rejects_overflowing_texel_count` passes through the length check instead. Fix it
  under the same helper; `research.md` has the derivation.
- First slice: the helper plus one refuse/permit pair on literals at the shadowmask stage's
  plane computation. It falsifies the riskiest assumption — that a bound at the format ceiling
  is both derivable and loose enough to leave every legitimate build untouched.

## Open questions

- What follows a diagnostic that still names no site — **delegated**: the executor records the
  negative result and the narrowed candidate set in the plan of record. An out-of-band capture
  is a follow-on.
- Which guarded computations actually precede an allocation — **delegated**: the executor
  walks the inventory and records the list and every exclusion in the findings note.
- Whether the failing config is reachable on the owner's machine during this build —
  **delegated**: it depends on the in-progress lightmap branch, and the manual row records the
  attempt either way.
