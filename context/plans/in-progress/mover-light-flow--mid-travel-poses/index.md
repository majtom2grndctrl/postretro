# mover-light-flow--mid-travel-poses

Brief · resumable · Lighting and GI · reads: `context/lib/rendering_pipeline.md` §4 (Mover state lighting) · `context/lib/build_pipeline.md` §Compiler pipeline step 12 (Mover light gate, Mover probe channel), §PRL section IDs (MoverProbeChannel), §Build Cache (Warm vs cold builds) · `context/lib/development_guide.md` §1.4 · `context/research/gi-rtx-lite/ip-check.md` §Piece 7, §Tencent US12293451B2 · `context/research/gi-rtx-lite/ip-372-direction-aware-visibility.md` §Design levers · `context/plans/done/mover-light-flow--probe-channel/` (index Decisions; research §M1, §Swept-face probe blobs, §Task 12c, §Task 12d) · read at 1f24e7926

## Problem

Owner playtest residue on the mover probe channel (id 57). The channel stores each probe's state only at the mover's two termini and blends them linearly by t′. A probe the mover's brushes sweep past switches state when the brush passes it, so the linear blend spreads that switch over the whole trip. On `mover-setpiece` the rising room's roof top dims at t′ 0.3–0.65, with pits at 0.70. The probe layers just above the roof top are still in open air but already lerped toward their dark raised-room state. On `enemy-movers` em_sweeper's west face reads 7.5–8.5 levels dark at t′ 0.25–0.30, from probes the slab shadowed at t = 0 that it has already passed. When this is done, each analysed mover also bakes states at poses spaced about one probe-lattice step apart along its path. Compose blends each probe between the two stored states around the mover's position, so a swept probe's switch spans about one lattice step of travel. At high fidelity, every probe's channel contribution with the mover at either terminus is unchanged.

## Decisions

- **Mid-travel poses (owner, 2026-11-10).**
  - A pose is the mover's brushes translated to the point at arc-length fraction t, by the runtime's own arc-length rule, so the bake and the hand-over share one t.
  - It reverses the foundation's "unit is (mover, terminus)" (`plans/done/mover-light-flow--foundation/`) and the probe channel's non-goal of intermediate states.
- **Placement: lattice-step automatic, capped (owner).**
  - With no `light_poses` key, a mover bakes min(16, max(2, ceil(L / s) + 1)) poses, evenly spaced in arc length. L is its path length and s the level's probe spacing.
  - A path past the cap spreads its 16 poses over the full trip at a coarser spacing. The build warns, naming the mover, its spacing and `light_poses`.
  - Termini are the first and last poses, so a path with no length bakes its two coincident termini, as today.
  - Why the cap: `--release` always bakes cold, long lifts are ordinary in the genre, and a coarser spacing still shrinks the residue against today's whole-trip blend.
- **`light_poses` key (owner).** An optional `kinematic_mover` integer ≥ 2 that sets the pose count, past the cap if the author chooses; 2 means termini only. Blank reads as absent, and absent means automatic. Any other value fails the build naming the mover, like `light_bounces`. Documented in `docs/level_design.md` and the FGD.
- **Termini are full states; mid poses are near-field (owner).**
  - Termini bake as today.
  - At a mid pose, direct q bakes at every probe a gated light reaches.
  - Bounce one and depth gather only over the mover's near-field domain: the union of its brushes' swept volumes along the path, widened by the gather lift plus one probe spacing (owner).
  - A probe outside the domain stores no mid-pose bounce or depth and blends across the pose.
  - Why: a mid pose's gather over every valid probe costs ~85 s on the hallway (`probe-channel/research.md` §M1), whatever the mover's size.
- **Mid-pose later bounces blend from the termini (owner).**
  - A mid pose's bounce is bounce one with the mover posed, plus the termini's later shares, blended to the pose's t.
  - A terminus's later share at a probe is its state there (the base, where it stores none) minus its posed bounce one at that probe, gathered over the near-field domain. A terminus that encloses the probe lends no share, and the other terminus's share holds across. Where neither lends one, the mid pose stores no bounce state and blends across.
  - The rule is the same at every fidelity: a `low` terminus's later share is id 34's.
  - Why: later bounces are low-frequency and the termini stay exact. Re-tracing them costs ~130 s a pose on the hallway (§M1), and id 34's own later-bounce layer is not live on a warm build.
- **The enclosing rule holds at every pose.** A pose whose brushes enclose the probe stores no value state there and reads the base, and keeps the dark depth tile (`build_pipeline.md` step 12, An enclosing terminus reads the base). A light any pose blocks is gated. So a mover whose middle pose alone blocks a light now gates it, reversing the foundation's three-waypoint row wherever a pose reaches that block.
- **Mover light gate report (owner).** Its per-terminus columns stay as they are: they size the world-surface channel, which picks its own spacing. Each mover gains one count: the lights only a mid pose gates.
- **Redundant mid poses are skipped (owner).** A mid pose stores no bounce or depth state for a probe where the blend of its neighbouring states is already within that probe's bars: the round-trip bar and perceptual floor for bounce, the significance floor for depth. A skipped mid pose blends across; a terminus with no stored state still reads the base. Direct q is one byte per pose and is never skipped.
- **Compose is piecewise between stored states.** Each consumer finds the stored or base-reading states around t′ and blends them as it blends termini today, several movers included.
- **Depth between poses: chosen by capture (owner).** Plain lerp between stored tiles, or each interval through the base, D_i → base → D_{i+1}, carrying 12c's fix. The owner picks from first-slice stills, and the result is restated here before the depth work lands. An interval whose ends are both termini, as on a two-pose mover or where every mid pose skips for a probe, keeps today's through-base blend either way.
- **The author ramp applies first (owner).** Knots map through the mover's ramp: t′_k = ramp(t_k). `MoverTravel` is unchanged. `docs/level_design.md`'s "until the mover passes `light_ramp_start`, the first waypoint's lighting holds" stays true for every probe. Knots squeezed onto t′ = 0 or 1 carry the first or last pose's state, so lighting is continuous across both ramp edges; poses squeezed out are never read, and the bake may skip them.
- **Bake fidelity: a build-time cap on `light_bounces` (owner).**
  - `prl-build --bake-fidelity low|high`, default `low`, for interactive builds.
  - `low` caps every mover's `light_bounces` at 1 for the build, so termini bake bounce one posed and keep id 34's later bounces. `high` uses each mover's `light_bounces`. `light_bounces 0` stays 0.
  - `--release` always bakes `high`. Naming `--bake-fidelity` beside it is a usage error, so a ship bake never reads as low. The bake keys on fidelity and the `--release` intent, never on whether a cache is present.
  - Why here: mover lighting is what designers iterate on with these set-pieces, and this brief already re-keys the mover bounce memos per pose. It wires only mover bounce.
- **The cache serves the best fidelity it holds (owner).**
  - Bounce and depth memos are per (mover, pose, fidelity).
  - A `low` build uses a matching `high` entry when one exists, and bakes `low` only on a miss. A `high` build reads only `high`.
- **Low is approximate, and says so (owner).** Safeguards like those of the approximate warm path retired in 5209e6931.
  - The exception is named. A `low` build's bytes are a function of its inputs plus the `high` entries cached: same inputs and same cache give the same bytes. It diverges from `build_pipeline.md` §Build Cache's "both modes bake the same bytes" and "`--release` bypasses the cache exactly like `--no-cache`", from its Determinism invariant, from `development_guide.md`'s "cold `--no-cache` is the exact ship source of truth" and "`--release` only bypasses the cache", and from step 12's warm-equals-cold rule for the probe channel. Each now holds per fidelity with no higher entries cached, and promotion restates them in `context/lib/`. Interactive `high` equals `--release`.
  - An automated gate compares `low` against `high` at rest and reports the error's sign. Its bound is set by the owner from the first measurement.
  - The Build Summary marks a mover approximate only where what it shows differs from `high`: its effective bounce count (`light_bounces`, clamped to id 34's) exceeds 1 and some state was baked `low` rather than served from `high`. It projects each mover's `--release` bounce-bake time as id 34's last recorded bake time on the map × the mover's two termini, since one pose costs about one id-34 bake (§M1) and mid poses cost seconds; it is labelled an estimate and reads "—" until a build has recorded that time. The gate covers the same movers.
- **IP rules bind unchanged.** N absolute states blended by mover position is the shape accepted for two (`ip-check.md` §Piece 7, §Tencent US12293451B2). The blended later bounce is a mover-position blend of absolute static-light states, never driven by a light. Depth blends moment sets, the domain and masks are storage sparsity, and nothing sets a per-probe flag, zeroes a weight or excludes a probe (`ip-372` levers 2, 7). The counsel packet gains one line: states per pose, not per terminus; an enclosed pose reads the base; a skipped mid pose blends across.
- **Memory.** Every pose's resident bake output is charged to the SH memory gate before any ray. Overage is the named error, never fewer poses (`build_pipeline.md` step 12; `development_guide.md` §1.4).
- **Per-frame budget unchanged.** At most 0.2 ms per travelling mover per frame on the 5300M, compose and live-tile writes together. Triggers stay per mover on t′.
- **Non-goals.**
  - Spinners: still not analysed.
  - Poses at authored waypoints or rest stops. A mover resting mid-path reads the blend of poses about one step apart; the stored knots leave room for exact rest poses later.
  - Any runtime geometric or half-space test against the posed mover: `ip-372` lever 6, and per-probe GPU cost.
  - Animated lights' bounce (id 27).
  - Far-field bounce changes and exact later bounces mid-travel: both follow the blend.
  - The world-surface channel. It picks its own state spacing: lattice spacing suits probes, which already blur over one step, not dense lightmap texels.

## Acceptance

### Automated

**Rest is unchanged at high fidelity**
- [ ] On `mover-closet`, `mover-setpiece` and `enemy-movers`, built `--release`, at t′ = 0 and t′ = 1: every probe's channel contribution equals the pre-change build's, compared build against build with the base taken out as in C10, including `enemy-movers`' three-mover probes. Cells mid poses newly pin are judged within the base's round-trip bar. A probe whose live tile comes only from a mid pose reads its base. Regression guard.
- [ ] A mover with `light_poses 2`, built `high`, stores today's terminus states and composes today's blend at every t′: linear for bounce and direct, through the base for depth. Regression guard.
- [ ] K1, K2, K3, C7 and C10 pass on `--release` builds. C9's renderer half passes, with each moved half checked against its piecewise mix. Regression guard.

**Placement and key**
- [ ] With the key absent, a mover bakes min(16, max(2, ceil(L / s) + 1)) poses at even arc length. Covered: a path of unequal segments, a path with a degenerate segment (it adds no length), and a zero-length path (two coincident termini, as today). Pose positions match the runtime's t at each knot.
- [ ] A path that needs exactly 16 poses bakes 16 and does not warn. One that needs 17 bakes 16 spread over the full path and warns, naming the mover, its spacing and `light_poses`. An authored `light_poses` of 20 bakes 20 and does not warn.
- [ ] `light_poses` accepts integers ≥ 2 and blank (as absent). It rejects 0, 1, negative, fractional and non-numeric values, and values too large for the field, naming the mover.

**Bake**
- [ ] At one mid pose on the setpiece room and one on em_sweeper, these match a static-brush reference bake at that pose: direct q at every reached probe, within B7's direct bar; bounce one at probes inside the domain, against the reference's first bounce alone, within B7's base bar; and depth tiles at probes inside the domain, within f16, as in B4.
- [ ] A mid pose's stored bounce equals its posed bounce one plus the termini's later shares blended to its t, a later share being the terminus's state minus its posed bounce one. A terminus that encloses the probe lends no share, and the other's holds across. At `low`, with `low` termini, a mid pose stores what a `low` terminus at that pose would. (research pin MP3)
- [ ] Near-field domain: a probe inside the widened swept volume gathers at a mid pose, and a probe just outside it stores no mid-pose bounce or depth and blends across. The direct half still covers a reached probe outside the domain.
- [ ] Skipping: a mid pose whose neighbour blend is within the bars stores nothing for that probe and composes the blend; one that misses the bars by a step stores its state. Where adjacent mid poses skip, compose at each skipped pose's t′ stays within that probe's bars, so a probe whose states curve gently keeps enough of them. (research pin MP2)
- [ ] On a mover with ramp (0.6, 0.9), each skip is tested against what compose reads after the ramp, with squeezed knots carrying the first or last pose's state. At every skipped pose's t′, compose stays within that probe's bars. Changing the ramp keys re-decides skips, and the build equals a `--no-cache` build, byte for byte. (research pin MP9)
- [ ] A probe enclosed at a mid pose stores no value state there, reads the base, and stores the dark depth tile. A probe on a face at that pose keeps the per-ray rule.
- [ ] A light that only a mid pose blocks is gated: q < 255 at that pose and 255 at both termini. The mover light gate report counts it for that mover, and its per-terminus columns equal the pre-change build's.
- [ ] A ramped mover whose only pose blocking a light is squeezed out builds a PRL that loads, and that light composes as if unblocked at every t. (research pin MP11)

**Compose**
- [ ] At each stored pose's t′, bounce, direct and depth read that state exactly. Between stored states each reads the mix: bounce within one f16 step, direct exactly, depth by the rule the owner picks. Out and back to the same t′ reads alike.
- [ ] Two movers at one probe, with different pose counts and skipped poses, combine as today (direct multiplies, bounce and depth add differences from the base) at t′ between knots for both.
- [ ] With ramp (0.6, 0.9), every probe reads the first pose for t < 0.6 and the last for t > 0.9. Knots between map to ramp(t_k). On a mover with mid poses before 0.6 and between 0.9 and 1, t just past 0.6 reads within one step of the first pose, and t just under 0.9 within one step of the last. A mover with no supplied state stays ungated. (research pins MP1, MP10)
- [ ] On a mover with mid poses, a depth probe outside the near-field domain, and a probe where every mid pose skips its depth, compose today's through-base depth at every t′, whichever rule the capture picks. Each such skipped pose's tile is within the significance floor of that through-base blend at its t′. (research pin MP8)
- [ ] On em_sweeper at t′ 0.25 and 0.30, the probes just west of the slab's west face compose bounce and direct within 3 display levels of a static-brush reference bake at the mover's position. So do the probes just above `mover-setpiece`'s roof top at t′ 0.4 and 0.6. Today's build fails this.

**Fidelity and cache**
- [ ] `low` bakes every mover's termini as `light_bounces` capped at 1. `high` uses each mover's `light_bounces`. `--release` equals an interactive `high` build on an empty cache, byte for byte. `light_bounces 0` stores no bounce at either fidelity.
- [ ] `--release` with `--bake-fidelity` (either value) fails before any stage, naming both flags. `--no-cache` alone bakes `low`, and so does a build whose cache directory cannot be created.
- [ ] The TUI's build mode carries fidelity. A launch with `--no-cache` and no `--release`, confirmed unchanged, bakes `low` or the fidelity it names, never a release bake. Production names no fidelity. (research pin MP6)
- [ ] Per fidelity, with no higher entries cached, a warm build equals a `--no-cache` build byte for byte. Two `low` builds with the same inputs and the same cache are byte-identical.
- [ ] After a `high` build, a `low` build serves `high` states for unchanged movers and bakes `low` for an edited mover. A `high` build after a `low` one re-bakes every mover at `high` and equals `--release`, byte for byte.
- [ ] Changing a mover's `light_poses` hits both of its termini memos and bakes only the new poses, and, with no higher entries cached, the build equals a `--no-cache` build of the same fidelity, byte for byte. (research pin MP5)
- [ ] Moving the middle waypoint of a three-waypoint mover, with both ends unchanged, changes its near-field domain. The build equals a `--no-cache` build of the same fidelity, byte for byte. (research pin MP7)
- [ ] After a `high` build, changing a mover's `light_poses` and building `low` gives mid poses that carry the `high` termini's later shares, equal to a `high` build's. Built `high`, changing a mover's `light_bounces` changes every one of its mid poses' later shares, and warm equals cold. (research pin MP4)
- [ ] Low against high at rest, on `mover-closet` and `mover-setpiece`: the largest per-probe display-level difference and its sign are reported, and the test fails past the owner's bound.
- [ ] The Build Summary prints, per analysed mover, its pose count, its fidelity, and its projected `--release` bounce-bake time: id 34's recorded bake time × 2, labelled an estimate, or "—" with none recorded. A mover is marked approximate only where what it shows differs from `high`; one served entirely from `high` entries prints `high`.
- [ ] The SH memory gate's prediction covers every pose's resident bake on the setpiece, the termini's bounce-one layers over the domain included, and a budget below it refuses with the named error.
- [ ] The renderer's ledger counts every stored pose's closed-pose depth tile on the setpiece and reads zero after unload.

**Format and docs**
- [ ] The loader refuses an epoch-5 section by name for each of: a pose count below 2; knots out of order, outside 0–1, not finite, or not running from 0 to 1; a block naming a pose at or past its mover's count; a probe marked both stored and base at one pose. Epoch 4 is refused.
- [ ] `light_poses` appears in the FGD and in `docs/level_design.md`'s mover lighting table. `--bake-fidelity` appears in both build-flag tables. `postretro-tool dist` with a recipe naming `--bake-fidelity` fails, naming the recipe and the flag.

### Manual

- [ ] Depth candidates, captured in the first slice: the rising room floor at t′ 0.50–1.00 (12c metric) and the sweeper faces, plain lerp against per-interval through-base. Owner picks.
- [ ] Low against high at rest, first measurement: the owner sets the automated gate's bound.
- [ ] Roof top on `mover-setpiece`, at t′ 0.3, 0.4, 0.5, 0.6, 0.65 and 0.70, at `low` and `high`: within 3 levels of the id-57-stripped frame on the 12d metric, with no pits.
- [ ] em_sweeper's west face at t′ 0.20–0.32, and its east face at 0.70–0.85 and 0.00–0.15: within 3 levels of stripped, with no pits, at `low` and `high`.
- [ ] Rising room interior floor (12c metric) at t′ 0.50–1.00: no lattice dots or diamonds, and no worse than today at 0.80–1.00.
- [ ] Rising room interior mean luma against t′, before and after, at `low` and `high`, for the owner's eye. The expectation is darker mid-travel.
- [ ] 5300M, Metal System Trace, passes grouped by label, every mover travelling minus all docked. On `mover-setpiece`'s room and wall, a travelling mover's compose passes plus its live-tile writes stay ≤ 0.2 ms per frame.
- [ ] Cold and warm bake times, and id-57 bytes per block, on `mover-closet`, `mover-setpiece` and `enemy-movers` at `low` and `high`, plus the hallway measured or projected. Owner judges.

## Path

- **Seams.**
  - Poses: `mover_light_gate::pose` (`Terminus`, `MoverPoses::resolve`, `AnalysedMover`) and `mover_probe_channel::posed_solid` (`PosedMovers`, `PosedMover.solids`). `PosedBrush::translated` takes any offset.
  - Arc length: `MoverPathArcLengths` and `mover_path_fraction` sit in a private physics module, re-exported. A fraction → position already exists in `forced_phase.rs` (`set_mover_phase_at_path_fraction`), but it needs a mover with positive speed. The compiler needs physics as a normal dependency; there is no cycle.
  - Gather: `gather::gather_posed_probes` iterates every valid probe; the domain filter goes there. `add_posed_later_bounces` (`bounce.rs`) folds later bounces into patch radiance before the one gather, so a terminus gathers its posed bounce one over the domain just before that call. The arc-length rule is `postretro_physics::kinematic_mover::path_fraction`.
  - States and memos: `states::reduce_poses`, `direct::bake_cell`, `bounce_memo::key` (one key per mover over every pose today). A pose memo keys on that pose's posed brushes and the domain, so a path-length edit moves every mid pose. Cross-pose reductions (skipping, the depth floor) run after memo reads (research pin MP5). The closed-pose depth host row grows per stored pose.
  - Renderer: `mb_entry_t` and `mb_entry_tiles` (`mover_bounce.wgsl`), `mc_record_visibility` (`mover_channel.wgsl`), `depth::blend_probe`, `MoverTBlock`.
  - Loader: `apply_light_ramps`.
  - Build mode: `construct_stage_cache` reads a missing cache as the ship path, so fidelity and the `--release` intent must reach the stage explicitly. The TUI's build-mode screen (`tui_config.rs`) gains fidelity.
- **Shape and rival.** Chosen: N absolute states at near-field mid poses, piecewise compose.
  - The strongest rival is a per-probe crossing window (`research.md` §Candidates, b): each probe blends over the span of t when the mover's brushes enclose it. It is cheaper, but poses cover what it cannot:
    - It cannot describe a room that passes fully through a probe within one trip, where both termini read open air.
    - It cannot gate a light that only a mid pose blocks.
    - It needs its own IP read; N absolute states needs none.
- **First slice.** Compiler poses plus mid-pose direct and near-field bounce for the setpiece room and em_sweeper, at `high`, and piecewise bounce and direct compose with both depth candidates behind a test switch. Capture the roof, sweeper and floor metrics before format polish, memos, skipping and fidelity. It falsifies the riskiest assumption: that one-lattice-step spacing removes both residues without new artifacts. The likeliest artifact is a pose enclosing a probe, which reads the base there and could brighten the slab's leading face early.
- **Large files.** Check `direct.rs`, `gather.rs`, `depth.rs` and `sh_compose.wgsl` against the ~800-line split rule before extending them.

## Open questions

- **Epoch-5 byte layout** within the Wire format constraints below; the plan shows the owner the pinned layout and updates the table. — **delegated**

## Wire format

Id 57 moves to epoch 5, exact match, little-endian, as epoch 4. Everything below is a constraint; the executor pins the bytes and updates this table at the format task.

| Part | Epoch 4 | Epoch 5 constraint |
|---|---|---|
| Per-mover record | Ramp record: mover id u32, start f32, end f32 | Adds the pose count N (unsigned, ≥ 2, no cap below the field's range) and N knots: f32 path positions, strictly ascending, first 0 and last 1, finite. Still exactly one record per mover in any block |
| Direct | (q₀, q₁) u8 per (record, mover, masked probe) | N u8 q values per (record, mover, masked probe), pose order. The record mask is the union of probes with q < 255 at any pose |
| Bounce | 24 B entry: mover, reserved, m₀, m₁ (u64 masks) | Per pose, per (cell, mover) entry: which probes store a state and which read the base. Unmarked at a terminus reads the base; unmarked at a mid pose blends across. Variable-length entries need offsets, not a fixed stride. Tiles pose-major, then probe by mask rank |
| Depth | 12 B record: mover, probe index, stored-termini bits | Per pose, the same three-way meaning as bounce, for any N; no fixed 32-pose bit field. Tiles in pose order |
| Streaming | Direct and bounce payloads stream past their 8 MiB bars | Unchanged; pose counts, knots and per-pose marks stay resident |

Loader rejects: a pose count below 2; knots out of order, out of [0, 1], non-finite, or not starting at 0 and ending at 1; any block naming a pose at or past its mover's N; a probe marked both stored and base at one pose; anything epoch 4 already rejects. Fidelity is not in the section: the bytes are the states.

**Pinned at the format task (plan task 2).** Little-endian, epoch 5 in the first byte, exact match.

| Part | Epoch 5 bytes |
|---|---|
| Header, cell index | Unchanged from epoch 4. |
| Mover records | Right after the cell index, before the direct block, because every block's lengths depend on N. u32 count, then per mover ascending by id: `mover_id u32, ramp_start f32, ramp_end f32, pose_count u32 (N ≥ 2)`, then N `f32` knots. Exactly the movers with entries in some block. |
| Direct | Record and mover-entry CSR unchanged. N `u8` q per masked probe per mover entry, probe-major (`slot·N + k`). |
| Bounce | Per-cell entry-count CSR unchanged, then `u32 × (E+1)` entry byte offsets from 0, ascending (an entry spans `8 + 16N` bytes). Entry: `mover_id u32, reserved u32 = 0`, then N `(stored u64, base u64)`. Base is 0 at a terminus, and every base bit lies inside the entry's stored union. Tiles pose-major, then probe by `stored` rank. |
| Depth | u32 record count. Records by (mover, probe): `mover_id u32, probe_index u32`, then `4·⌈N/16⌉` bytes of two-bit marks low-first (bit 0 stored, bit 1 base), padding bits 0. Tiles per record, its stored poses in pose order. |
| Streaming | Unchanged; lengths come from each mover's N. |

The loader also refuses a base mark at a terminus, a base mark outside the entry's stored union (it would only restate the blend), a mover record that no block names, and a block naming a mover with no record.

## Boundary inventory

| Name | Rust | Wire | FGD KVP / CLI | Docs |
|---|---|---|---|---|
| Pose count key | parsed `Option<u32>` on the map's kinematic mover, beside `light_bounces` | id-57 per-mover pose count and knots | `light_poses(integer)` in `sdk/TrenchBroom/postretro.fgd` | `docs/level_design.md` mover lighting table |
| Bake fidelity | a two-value enum on the compiler's args and stage inputs | not stored | `--bake-fidelity low\|high` on `prl-build` and its TUI build mode; `postretro-tool dist` passes `--release`, so a recipe naming it fails | `docs/level_design.md` build flags table; `build_pipeline.md` flags table |
