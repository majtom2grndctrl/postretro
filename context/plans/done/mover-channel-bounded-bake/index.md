# mover-channel-bounded-bake

Brief · resumable · reads: `context/lib/build_pipeline.md` §CLI flags (`--working-set-max-size`, `--sh-delta-max-size`), §Compiler pipeline step 12 (Mover probe channel), §PRL section IDs (id 57) · `context/lib/development_guide.md` §1.4 · `context/lib/testing_guide.md` §Resource bounds · `docs/level_design.md` (Common options, errors) · research: `research.md` · read at 176d0bf7

## Problem
The owner, compiling `stress-env-volumes` on a 16 GB desktop, was refused by
the SH base working-set gate at the 16 GiB default: 21.8 GB predicted, where
the same bake run warm at a raised budget peaked near 2.2 GB in that stage.
Cause: the mover probe channel holds every finished mover's states until the
last mover finishes, so its peak scales with movers × the whole map's valid
probes, and the gate must price that worst case without a ray
(`research.md` §2).
When this is done, the channel bakes each mover to its encoded blocks and
drops it before the next, its output is capped, and layer resolution is
capped. The bake's states no longer grow with the mover count; only the
posed scenes and direct entries still do. The gate
prices partitions rather than worst-case accumulation: it stays sound,
refuses before any bounce ray, and admits `stress-env-volumes` at the
default at `low`.

## Decisions
- **Bounded by construction; the gate stays.** The primary lever is a bake
  whose working set is one partition at a time (`research.md` §7). The gate
  stays pre-ray and sound and prices those partitions. It costs seconds and
  turns a bake that would page or die hours in into a named refusal before
  any bounce ray.
- **Each mover finishes before the next bakes.** Its depth joins the
  running union the tile-range verdict reads before its own skip, so the
  verdict never depends on job order, and its states drop before the next
  mover bakes. Emitted bytes stay identical. What spans movers is in
  `research.md` §6.
- **One finish site; the cache only accelerates.** Every mover, cold or
  warm, reads or bakes its poses and finishes at its slot in the bake loop.
  The memo lookup plans which poses are cached and holds no states. A read
  that fails at the slot bakes that pose, building the layers, bounce one and
  id 34's later bounces it needs then; the gate prices them whatever the
  cache holds. Warm equals cold at each fidelity with no higher entries
  cached, and a bad entry costs time, never output.
- **Id 57's blocks are capped.** `--mover-payload-max-size`, default
  256 MiB, same byte syntax as `--cache-max-size`, counts every byte the
  bounce and depth blocks hold. The bake checks it as each mover's blocks
  join and fails the build by name past it, never dropping or coarsening.
  Like the existing
  tile-range refusal, it comes after that mover's rays: it guards output a
  ray-free price cannot bound. Where one mover passes the cap and another
  the tile range, job order picks which of the two names the build fails
  with.
- **The gate prices one mover, not all.** The SH base gate prices the base
  stage, one mover's working set at today's per-mover bound (every valid
  probe), and the blocks at the cap. Direct entries keep
  `direct_entry_bound`. The accumulated-states term goes. Fidelity still
  never enters the price.
- **Layers in flight are capped.** Layer resolution admits at most a fixed
  number of layers at once, with identical output. The layer phase prices
  every finished layer plus the scratch of the largest layers that can be in
  flight together, not every light's scratch.
- **The direct phase is priced in its own window.** That window comes after
  the SH delta and direct-SH stages and holds the blocks at the cap and the
  id-57 section at the cap plus its direct records at `direct_entry_bound`,
  beside every SH section still alive (`research.md` §3). Ids 27, 41 and 45
  are still dense there and take the SH-delta gate's dense bound, never
  `--sh-delta-max-size`; the rest take their own size.
- **Carried bytes join the SH-delta projection.** What the channel carries
  through the SH delta and direct-SH stages (`research.md` §3) is added to
  that gate's projection.
- **Non-goals:**
  - A locality bound on the probes a mover can change: not needed once
    movers do not accumulate; a later brief owns it if one mover alone is
    ever refused.
  - Ray-free tightenings of today's per-mover terms (pairs prefilter,
    compact other-terminus tile): per-mover finish alone brings the stress
    map under the default (`research.md` §5).
  - Pricing one side per layer entry: an area light lights both sides of a
    patch (`plans/ready/area-lights`).
  - Bounding the direct-SH and direct-SH-delta stages themselves: measured
    under the default (`research.md` §2), and not driven by mover count; a
    separate survey brief.
  - Pricing mid-pose skips: skips read baked states, so no ray-free price
    exists.
  - Moving the round-trip bar or perceptual floor: either changes emitted
    bytes.
  - The cache spared-set warning: the cache's own accounting, not the
    bake's peak (`research.md` §8).

## Acceptance
A mover fixture is a map under `content/dev/maps` with an analysed mover. The
closet and its edits run routinely, on one worker and four; the rest run on
demand behind `#[ignore]` on four, `stress-env-volumes` among them. Pins:
`research.md` §9.

### Output identity
- [ ] Every emitted PRL section is byte-identical to the parent commit's on
      every mover fixture, at `low` and `high`, cold and warm, on one worker
      and four (I1).
- [ ] Reversing the movers' job order, on two movers of different reach,
      leaves every emitted section and the tile-range verdict unchanged, a
      mover past the range included. The verdict is admission or the
      tile-range error by type; its carried count may differ, as it does at
      the parent commit when one order stops the bake and the other does not
      (I2).

### Bounded bake
- [ ] At no point are two movers' raw or reduced states live at once, cold,
      warm over an empty cache and on a full hit, on the closet's two-door
      edit and the setpiece; the ledger reports each kind's high-water mark,
      and the states kinds' high-water stays at or below the largest one
      mover's (M1).
- [ ] On the closet's two-door edit, warm over a cache primed with one
      door's `light_bounces` changed, so one mover hits every pose and the
      other bakes, in both job orders, the held mover's states are never
      live during the other's pose bake and the mover-bake phase's price is
      at or above the ledger's peak (M2).
- [ ] A mover whose memo read fails at its slot, on a warm build whose
      lookup found every pose cached, bakes that pose there; at `low` and
      `high`, with no higher entries cached, every emitted section equals a
      cold build's, with every mover held that way and with one held and one
      pending (M3).
- [ ] At `low`, warm over a cache whose `high` entry for one pose is corrupt
      and whose `low` entry is whole, every emitted section equals the parent
      commit's on the same cache; where the `high` entry reads at lookup and
      fails at the slot, the pose bakes at the `high` count and the sections
      equal the same build with the read succeeding (M5).
- [ ] On the closet's two-door edit at `low`, warm over a full hit whose
      second mover's read fails at its slot after the first mover's blocks
      join, the layers, bounce one and id 34's later bounces built there,
      the reach index and that mover's poses read back are routed through
      the ledger by capacity, and the mover-bake phase's price is at or above
      the ledger's peak (M4).
- [ ] Layer resolution never has more layers in flight than its cap, on one
      worker and four, cold and warm, with the cap set below the worker
      count on a fixture with more planned layers than workers, and never
      stalls with the governor at one permit (L1, L2).
- [ ] A build whose bounce and depth blocks pass `--mover-payload-max-size`
      fails naming the flag, the blocks' bytes and the movers joined so far,
      and drops or coarsens nothing. A cap equal to the blocks' bytes admits
      them; one byte under fails. The flag rejects a value past `u64` and a
      malformed size (C1).
- [ ] A single mover whose depth passes the tile range and whose blocks
      would pass the cap fails by the tile range's name, before it skips or
      encodes (I4).
- [ ] Where one mover passes the cap and another the tile range, both job
      orders fail the build by name; which name may follow job order (I3).

### Sound price
- [ ] On every mover fixture, at `low` and `high`, cold, warm over an empty
      cache, on a full hit, on an id-34 hit whose movers miss their memos,
      and as a `low` build over a `high`-primed cache, each priced phase is
      at or above the ledger's peak inside that phase's window. The mover
      bake and the direct window are marked as ledger phases. Every
      sound-price row also runs with `--mover-payload-max-size` set to each
      fixture's bounce and depth blocks' bytes, so the cap's term carries no
      slack (P1, P2, W1, D1, D3).
- [ ] While a mover finishes, its skip's and encode's buffers, and the skip
      bars, mover records and probe cells the loop keeps, are routed through
      the ledger by capacity, and the mover-bake phase's price covers them
      beside the blocks already joined (P1).
- [ ] The direct window's ledger routes, by capacity, the encoded blocks,
      each id-57 section, the direct entries, the posed solids and every SH
      section alive; each section's bytes are at or below its own term in
      the price (D1, D2).
- [ ] With `--sh-delta-max-size` below ids 27, 41 and 45's dense bytes but
      at or above their post-drop payload, the build is admitted and the
      direct window's price still covers each delta section as the window
      holds it, dense (D1).
- [ ] Through the SH delta and direct-SH stages, the SH-delta gate's
      projection is at or above the ledger's peak with the carried bytes
      routed, and its carried term alone is at or above the carried bytes
      the ledger holds (D3).
- [ ] The layer phase's price covers the largest layers in flight together,
      a cache hit's payload included (L1).
- [ ] Edges: no analysed mover prices the channel at zero; with every mover
      at `light_bounces 0`, the per-mover states term prices no bounce state
      and the price holds no bounce-one copy; `_bounce_fill 0` prices no
      later-bounce buffer.
- [ ] On every mover fixture the SH base gate's projection is the same at
      `low` and `high`.
- [ ] One byte under the SH base peak refuses by the SH base gate's name
      before any bounce ray, through the pipeline's gate order; at the peak
      both gates admit (G1).
- [ ] `build_pipeline.md` gains a `--mover-payload-max-size` row, and its
      `--working-set-max-size` row and Mover probe channel *Memory*
      paragraph state what the gate prices, the "sidedness needs rays"
      reason replaced by area lights, and that every mover, held or baked,
      reads back and reduces at its slot. `docs/level_design.md` lists the
      flag, its default and its error.

### Admission
- [ ] `stress-env-volumes` is admitted at the 16 GiB default at `low`, by
      every gate. If a soundness term moves its refusal to another gate, the
      build stops at the findings note for an owner call.

### Measured findings
- [ ] The gate's estimated peak ÷ the largest cold measured window, with
      each phase's ratio, on `stress-env-volumes` and `mover-setpiece` at
      `low` and `high`; and whether `stress-env-volumes` is admitted at the
      16 GiB default with `--release`.
- [ ] The bounce and depth blocks' bytes against the 256 MiB cap on both
      maps at `low` and `high`.
- [ ] SH base stage time before and after: cold on the same two maps, layer
      resolution reported apart, and warm on a full hit on `mover-setpiece`
      at `low`.

### Manual
- [ ] Cold `--no-cache` bakes of `stress-env-volumes` and `mover-setpiece`
      at `low` and with `--release`, on the owner's 16 GB i5-9400F desktop at
      the default worker count, private bytes sampled every 30 s against
      stage starts. In every priced phase the prediction is at or above the
      measured private bytes less their value when the SH base gate ran. A
      phase shorter than the interval reports no sample, and the note says
      so. The owner launches these; nothing blocks on them.

## Path
- Accounting and the stop-at-gate harness: `research.md` §1–5.
- Per-mover finish: reduce, edge copy, memo write, join the depth union,
  skip, encode blocks, drop states. Plumbing and what spans movers:
  `research.md` §6. Check first whether the stage cache can tell a pose is
  present without reading it whole; if not, the lookup reads and discards,
  and the slot reads again. Layers and bounce one resolve lazily today only
  for pending jobs (`lookup.needs_radiance`), and id 34's later bounces only
  when `AbsentLaterBounces::needed_by`; a failed slot read must reach both. Direct-window section sizes: id 35 and the analyze
  clones from the probe grid, the entity-shadow section from its
  selection.
- Payload cap precedent: `enforce_payload_cap` (`delta_sections.rs`).
- Layer cap: batch the layers; no permit per layer, no wait inside a rayon
  task (`research.md` §3).
- Pricing seams: `MoverChannelCost`, `ShBaseWorkingSetProjection::price_movers`,
  the SH-delta projection (`pipeline/delta_working_set.rs`).
- First slice: per-mover finish with the byte-identity and job-order rows on
  the closet and setpiece. It is the riskiest change to output; pricing
  follows it, and the layer cap comes last so it never slows that slice.
- Rival: spill each mover's encoded blocks to a temp file and merge them by
  cell at id-57 assembly (id 50's spool is the precedent). Nothing would
  accumulate and the cap would go, but the PRL writer and read-back hold
  id 57 whole today, and later gates would still need a pre-ray figure for
  what is carried. The cap can retire if spilling lands.
- `pipeline.rs` reaches 2,600 lines and `mover_bounce_stage_tests.rs` 1,400.
  New tests go in a new module; split `pipeline.rs` first, in its own
  commit, if the wiring grows it.

## Open questions
- The layer cap's size — **delegated**.
- Whether the cap check runs before or after a mover's blocks join, and so
  whether the price adds one mover's blocks above the cap — **delegated**:
  the sound-price rows decide it.
