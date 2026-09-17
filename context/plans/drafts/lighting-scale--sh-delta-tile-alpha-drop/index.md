# lighting-scale--sh-delta-tile-alpha-drop

Brief · compact · Epic: lighting scale (on-disk + VRAM) · reads: `context/lib/build_pipeline.md` §PRL section IDs, §Build Cache · `context/lib/rendering_pipeline.md` §4 · read at 4f9e5c5

> **Build order (lighting-scale footprint track):** `sh-delta-cone-reach-cull` (Phase 1) has **shipped to `main`**, so this is now the **next** footprint item → then `adaptive-probe-spacing`. Re-baseline this brief's delta byte-identity ACs and the three delta stage-version bumps onto the **landed** post-cull format — cone-reach already bumped those versions, so this stacks on the new baseline (Open questions R6). Independent of adaptive on the wire (delta id-27/41/45 vs base id-34/35); coordinate only on the shared SH compose/sampler code. **Re-ground before build:** cone-reach's landing edited the delta bake code this brief cites (it was read at 4f9e5c5) — refresh symbols against `main` in the `/review-brief` pass.

## Problem
Developer-raised, from the lighting-scale size work. Baked `.prl` files run to multiple GB
on large maps, and the dominant payloads are the three sparse SH delta sections —
`DirectShDeltaVolumes` (id 41, the largest), `DeltaShVolumes` (id 27) and
`AnimatedDirectShDeltaVolumes` (id 45). Cause: every delta tile texel is stored as four f16
halves, and the fourth is a per-texel validity flag the bake writes as constant 1.0 for a valid
probe; no consumer reads it — validity comes from `valid_probe_masks` and the kept mask, and
every reader takes `.rgb` (audit in `research.md`). The renderer uploads the payload verbatim
into a storage buffer, so the dead channel is paid on disk and in VRAM. When done: each delta
tile texel is three f16 halves; section bytes and delta storage buffers shrink by the dropped
quarter of the payload, every valid probe's reconstructed tile and the composed atlas are
bit-identical to today's, and runtime steady-state RAM does not grow.

## Decisions
- **Drop alpha where the delta payload is serialized, not in the shared tile packer.** The
  base atlases (id 34/35) reuse `pack_octahedral_irradiance_tile` (`sh_bake.rs`), and their
  sampler reads stored alpha for L1 stored-corner presence (`sh_sample.wgsl`) — base tile
  layout is untouched. Alpha leaves at each delta writer: the id-41 sub-block bake
  (`bake_direct_delta_subblock`, `direct_sh_bake.rs`), the id-27 sub-block bake
  (`delta_sh_bake.rs`), the id-45 sub-block bake (`animated_direct_sh_bake.rs`), and the
  compaction-time L2 brick-mean encoder (`synthesize_l2_mean_tile`, `delta_sections.rs`),
  which re-encodes a tile after the bake and must follow the same layout.
- **One wire change across all three sections, stride in lockstep.** The texel becomes RGB
  f16 and `DELTA_TILE_TEXEL_F16_COUNT` (`delta_sh_volumes.rs`) becomes the single source of
  the stride, threaded through `delta_probe_f16_stride` and the compose grid uniform. Every
  site that computes a texel offset or a tile length moves in the same change; a site left
  at the old stride misreads the payload and is a defect, not a follow-up. The site
  inventory is `research.md` §Stride sites. Prove id 41 first; 27 and 45 follow from the
  shared format.
- **Output equivalence is the contract.** Section bytes change; decoded tiles do not. The
  composed atlas from the RGB payload is bit-identical to the one from the RGBA payload for
  the same bake, at every coarsening level. This is the equivalence proof because `.prl`
  byte-identity is impossible by construction.
- **Compatible with landed delta passes, unchanged in value.** `delta-sh-valid-probe-compaction`
  (done) is the closest predecessor: it owns the CSR-entry compaction and the stride-based
  compose resolver (`offset[entry] + within_cell_rank × stride`) that this change re-strides —
  the resolver reads the stride from the same constant, so its arithmetic follows the drop and
  its layout semantics are unchanged. `delta-entry-dropping` (done) already treats alpha as
  structural; its zero test chunks texels by the stride constant and keeps its semantics. The
  coarsening classifier (`sh_coarsen.rs`) and the runtime-safe envelope
  (`sh_runtime_envelope_scoring.rs`) read RGB only: their offset arithmetic follows the stride,
  their decisions do not change. Coarsening is orthogonal; the drop applies at L0, L1 and L2
  alike.
- **Version bumps make the old format unloadable and the old cache unservable.** Bump the
  three section-internal versions (`DELTA_SH_VOLUMES_VERSION`,
  `DIRECT_SH_DELTA_VOLUMES_VERSION`, `ANIMATED_DIRECT_SH_DELTA_VOLUMES_VERSION`) and the three
  delta stage versions (`INDIRECT_DELTA_SH_STAGE_VERSION`, `DIRECT_SH_DELTA_STAGE_VERSION`,
  `ANIMATED_DIRECT_DELTA_SH_STAGE_VERSION`): the stage cache stores raw sub-blocks whose
  length is the stride, and its key does not fold the length (`delta_sh_cache.rs`), so the
  bump — not the length check — is what makes a pre-change warm cache miss cleanly.
- **Layer placement.** Format crate (layout, length identity, versions), compiler
  serialization and compaction, render-cpu data logic, and the WGSL readers. The loader
  changes only through `from_bytes`; the renderer's upload stays a verbatim copy of the
  section, so the VRAM saving is the disk saving with no new staging copy.
- **Non-goal — sub-f16 numeric compaction of the RGB** (fixed-point, R11G11B10, per-entry
  scale). Gated, not free: ids 27/45 are multiplied at runtime by `animated_light_scale`,
  clamped only below at zero, with no bounded amplitude contract — the reason they are pinned
  uniform L0 (`plans/done/lighting-scale--sh-adaptive-coarsening-v2` Non-goals;
  `mutable_animated_direct_cost`, `sh_runtime_envelope.rs`); a fixed-range encoding under an
  unbounded multiply amplifies absolute quantization error. On id 41 it is a separate fidelity
  increment into an envelope that is f16-exact today. Deltas ring negative, so unsigned packed
  formats need an offset encoding first. Owed to a future bounded animated-amplitude plan.
- **Non-goal — BC6H on the delta.** The delta is a sparse CSR storage buffer read
  arithmetically; block compression is texture-only. Would need a storage→texture
  restructure.
- **Non-goal — section 48 billboard direct scatter.** A sibling dense RGBA16F payload with
  alpha reserved zero and its own reader (`billboard_direct_scatter_compose.wgsl`,
  `F16_PER_SAMPLE`). Not a tile section and not on the id-27/41/45 stride; a separate brief
  may apply the same drop.
- **Non-goal — container compression, selection tightening, clustering, the
  scalar/shadowmask representation, and retuning `--sh-delta-max-size` or the working-set
  gate default.** Separate levers on separate seams; the two byte-denominated gates shrink
  their measurements through the stride and keep their defaults.

## Acceptance

### Automated
- [ ] Each of id 27/41/45 round-trips a section whose payload is three halves per texel; a
  payload of the old length is rejected as a delta sub-block length error, and a `.prl`
  written at the previous section version fails with the named "recompile" error
  (research.md R1).
- [ ] Composed-atlas identity, id 41: on a fixture that emits id 41, every valid probe's
  reconstructed tile from the RGB payload has the same f16 bit pattern as from the RGBA
  payload of the same bake, at L0, at an L1 kept corner, at an L1 dropped-valid probe, and
  at L2 (research.md R2).
- [ ] Composed-atlas identity, id 45 and id 27: the same row on a fixture that emits id 45
  and one that emits id 27.
- [ ] Stride lockstep: a fixture tile whose R, G and B differ per texel and per probe decodes
  correctly through the format length identity, the compaction offset tables, the emitted
  section view, the runtime-envelope dense view, the CPU reference decoder and the entry-drop
  zero test; a reader advancing by the old stride reads a neighbour's channel and fails. The
  three compose shaders' texel multiplier is pinned to the format constant by a source guard
  (research.md R3).
- [ ] Odd-half alignment: a texel at an odd f16 offset — every odd texel index once the
  stride is three — reads back its own R, G, B through the word-packed reader, for the
  first texel of a probe at every kept rank and the last texel of a tile (research.md R4).
- [ ] Entry dropping and coarsening decide identically: on the id-41 and id-45 fixtures the
  retained entry set, `cell_levels` and `valid_probe_masks` are equal before and after.
- [ ] Working-set projection and raw-payload cap summaries report the reduced per-entry size
  for 6×6 tiles; `--sh-delta-max-size` and the working-set default are unchanged.
- [ ] Cache: each delta stage's key changes with its version; a cache directory populated by
  the pre-change binary serves no entry; two warm builds under the new format emit
  byte-identical `.prl` (research.md R5).
- [ ] Runtime RAM: no new owned copy of a delta payload is introduced — the loader's section
  and the renderer's staging bytes remain the only CPU copies.

### Manual
- [ ] Size delta, recorded in `research.md`: for one real id-41 map and one real id-45 map,
  `.prl` size, id 27/41/45 payload bytes from the compiler's delta cap summary, and the
  delta storage-buffer bytes from the dev-tools compose footprint log, before and after; each
  delta payload is three quarters of its previous size.
- [ ] E20 capture of one scene with promotion active and an animated light playing is
  pixel-identical on the before and after `.prl`.

**Regression guards**
- [ ] Base atlases id 34/35 are byte-identical before and after on the same fixtures; the
  base sampler's L1 corner-presence read is unaffected.
- [ ] Section 48 is byte-identical before and after.

## Path
- **Seams.** Two kinds of stride site, both in `research.md` §Stride sites: those that already
  derive from `delta_probe_f16_stride` / the compose grid uniform (compaction meta, drop
  policy entry stride, working-set projection — they follow the constant), and those with a
  literal texel multiplier that must be edited: `reconstruct_delta_probe_tile`,
  `EmittedDeltaSectionRef::decode_interior`, `DenseDirectView::decode_valid_entry`,
  `rgb_payload_is_zero`, and `read_delta_texel` in the three compose shaders.
- **Shape.** RGB triplets, tile-contiguous, kept-rank order unchanged. Rivals: strip alpha at
  load or at upload — no disk win and an extra load-time copy, against the RAM constraint.
  Defer and fold the drop into the future sub-f16/BC6H delta re-encode instead of spending a
  version bump and review gate now — rejected: that re-encode is gated (unbounded
  `animated_light_scale` multiply on 27/45; a storage→texture restructure for BC6H), while the
  alpha drop is the certain, unconditional floor available today, mirroring
  `sh-base-atlas-at-rest-slimming`'s split posture of taking the free at-rest win first.
- **Word packing.** The storage buffer is `array<u32>`, two halves per word. Entry and probe
  bases stay even (a 6×6 tile is 108 halves); odd texels start mid-word, so the reader loads
  two words and selects by parity. Do not pad texels back to four — that is the status quo.
- **First slice.** id 41 end to end — constant, `bake_direct_delta_subblock`, format identity,
  CPU reference, `direct_sh_compose.wgsl` — and the id-41 identity row on the
  `cache_cross_bake_tests.rs` fixture before touching 27/45.
- **Files past ~800 lines.** `delta_sections.rs`, `direct_sh_bake.rs`, `delta_sh_bake.rs`,
  `render-cpu/src/sh_compose.rs`: point edits, not extensions; new identity tests go in a
  sibling test file.
- **Measurement.** Compiler delta cap / drop summary logs (disk), dev-tools
  `ComposeStorageFootprint` log (VRAM storage buffers), `ls -l` for `.prl`.

## Wire format
Changed, not added; all three sections mirror each other and keep header, table order,
endianness and sentinels. Only the payload texel changes:

| Field | Before | After |
|---|---|---|
| `delta_subblocks` texel | 4 × f16 LE (R, G, B, validity) | 3 × f16 LE (R, G, B) |
| probe tile stride (halves) | `tile_dimension² × 4` | `tile_dimension² × 3` |
| payload length identity | `Σ kept tiles × stride` (unchanged form) | same, with the new stride |
| section version byte | 5 / 3 / 3 (id 27 / 41 / 45) | each +1 |
| WGSL texel multiplier | `texel_index * 4u` | derived from the same constant |

Validity stays in `valid_probe_masks` + kept mask; alpha of the composed atlas stays derived
from stored-slot validity, never from payload.

## Open questions
- Which real map carries id 45 for the size row (campaign-test, or a stress-warren variant
  with animated lights that compiles under the current working-set gate) — **delegated**: the
  executor picks and records it.
- Parity-select vs. branch in the WGSL packed read — **delegated**: the executor picks and
  reports it in the plan of record.
- Landing order against `lighting-scale--sh-delta-cone-reach-cull` (ready; bumps the same
  stage versions and carries its own byte-identity proof) — **delegated**: the second to land
  re-baselines its proof at the first's format version; the bumps are not merged
  (research.md R6).
