# mover-light-flow--mid-travel-poses — research notes

Read at 1f24e7926. Grounding for the brief's Decisions; the brief concludes, this argues.

## Residue mechanism

The probe channel's diagnosis (`plans/done/mover-light-flow--probe-channel/research.md` §Swept-face probe blobs, "Not fixed by it"; §Task 12d, Residue) traces both residues to probes whose terminus state is dark from the far side of a face the mover sweeps past.

Swept intervals, from the map brushes and waypoints. t is linear in path: neither mover has ramp keys, and both have two waypoints and no spin.

| Mover | Map | Brush | Travel |
|---|---|---|---|
| `setpiece_room` | `content/dev/maps/mover-setpiece.map` | roof slab x 832–1152, y 160–480, z −16…0 at t = 0 | +z 176 u |
| `em_sweeper` | `content/dev/maps/enemy-movers.map` | slab x 1280–1312, y 64–448, z 0–96 | +x 256 u |

| Probe layer | Inside a mover brush for t |
|---|---|
| Room z 4.9 | 0.03–0.12 |
| Room z 44.2 | 0.25–0.34 |
| Room z 83.6 | 0.48–0.57 |
| Room z 123.0 | 0.70–0.79 |
| Room z 162.4 | 0.92–1.0 |
| Sweeper x 1300.0 | 0–0.078 |
| Sweeper x 1339.4 | 0.107–0.232 |
| Sweeper x 1378.7 | 0.260–0.385 |
| Sweeper x 1536.2 | 0.876–1.0 |

The room's interior columns lie inside the roof's x/y footprint, and the sweeper's x 1300 layer lies inside the slab's y/z extent. That the other sweeper layers share its y/z was assumed, not checked.

**Roof band, t′ 0.3–0.65 (inference, not measured).** The roof top sits at z 53–114 over that band. The probe layer just above it, z 83.6 until t 0.48 and then z 123 until t 0.70, is still in open air, but its state is already lerped toward its dark t = 1 state inside the room. A research pass read the band as matching no crossing interval. That reading looks at the layer being crossed, not the one above. The first slice's capture settles it.

**Sweeper west face, t′ 0.25–0.30.** The face sits at x 1344–1357. The x 1339 probe was last enclosed at 0.232 and sits just west of the face, now lit, but it lerps from q₀ = 0 (shadowed by the slab at t = 0). Medium confidence: it fits the diagnosis, but no capture isolated it.

## Candidates

| Candidate | Fixes roof band | Fixes sweeper | Cost and risk | Verdict |
|---|---|---|---|---|
| (a) Values through the base, S₀ → id 34 → S₁ | Partly: z 123 still dims over 0.5–0.65 | Partly: x 1339 reads q = 0.5 at t 0.25 | Lights the room interior mid-travel as if the shaft were open; changes the Bounce and Direct Decisions | Rejected |
| (b) Per-probe crossing window: the bake stores [a, b] per (mover, probe) from the swept enclosure interval, and values blend over it | Yes | Yes | Epoch change; small resident block; IP-clear only if confined to values (no weight or depth); cannot describe a sealed moving room's interior | Rejected by owner for mid-travel poses |
| (c) Mid-travel poses at lattice spacing, full map | Yes (inference) | Yes, switch spread over one step | ~216 s a pose on the hallway (§Cost), whatever the mover's size; bytes per pose; epoch change; widens `Terminus` | Rejected after direction review |
| (c′) Mid-travel poses, near-field bounce one and depth, later bounces blended from the termini | Yes (inference) | Yes | Mid pose costs a domain-sized gather plus bounce one at every fidelity; far-field and later-bounce changes mid-travel follow the blend | **Chosen** (owner, 2026-11-10) |
| Runtime per-probe side test against the posed mover | Yes | Yes | GPU cost; a runtime per-probe half-space test, close to `ip-372` lever 6 | Rejected |

The IP reading of (b), for the record: it changes no weight, sets no flag and excludes no probe, so it stays clear of the '372 levers while confined to stored values. The owner chose (c), which needs no new IP argument.

## Grounded seams

Source-read, by symbol.
- The ramp is game-side: `ramped_travel_fraction` (postretro `runtime_movers/travel.rs`). `MoverTravel` carries t′ only. `MoverTBlock::set_states` holds one f32 per mover slot, negative meaning no state. Change trackers key on that f32's bits.
- Bounce: `mb_entry_t` reads the slot's t′ per entry. Entries are 6 u32 (slot, m₀, m₁, tile offset), and tiles are found by mask rank. The lerp runs per (entry, probe, texel) with `local_probe` in hand, snapping at t ≤ 0 and t ≥ 1.
- Direct: `mc_record_visibility` reads t′ per mover entry (2 words: slot, q offset) and q₀/q₁ bytes at `slot * 2` in the masked-probe rank.
- Depth: `depth::blend_probe`, CPU, per `MoverDepthRecord`, through the base B at t′ = 0.5.
- Compiler poses only termini: `Terminus { First, Last }` (`mover_light_gate/pose.rs`), used in 17 compiler files. `MoverPoses::resolve` gives `AnalysedMover { poses: [Pose; 2], swept }`, and `PosedMover.solids` is `[_; 2]`. Spinners go to `not_analysed` (`SPIN_REASON`). Paths with more than two waypoints are admitted, with path[0] and path[last] as termini.
- Arc length: `postretro_physics::kinematic_mover::path_fraction` (`MoverPathArcLengths`, `mover_path_fraction`, the degenerate-segment rule). level-compiler takes postretro-physics only as a dev-dependency (review 1); the module is private and re-exported, and `forced_phase.rs` already maps a fraction to a position for a mover with positive speed.
- Probe spacing: `ProbeGridLayout.cell_size` (per axis, from `ShConfig.probe_spacing`). The pose step is the level's probe spacing.
- Entries come from terminus states only (`gather_posed_probes` → `states::reduce_poses`; `direct::bake_cell`). Mid-path poses are never tested today.
- `bounce_memo::key` folds the bounce count and every pose's solid and materials into one key per mover.
- Build mode: `construct_stage_cache` returns `None` for `--release` or `--no-cache`, and the pipeline reads `None` as the ship path. `build_pipeline.md` §Build Cache says the two modes bake the same bytes.
- Id 57 epoch 4: `MOVER_PROBE_CHANNEL_EPOCH` is an exact match. Direct and bounce are cell-keyed with probe masks, depth is a flat (mover, probe) record list, and the ramp record has one entry per mover. Only `visibility`/`own_tiles` and `bounce_tiles` stream with id 50.

## Cost

Measured per pose on the full hallway (`plans/done/mover-light-flow--probe-channel/research.md` §M1): bounce one 0.3–0.6 s, later bounces 128–131 s over every static patch, and the gather 84–86 s over every valid probe. About 216 s a pose, against id 34's 214 s, whatever the mover's size.

| Pose kind | Gather | Later bounces | Hallway per pose (est.) |
|---|---|---|---|
| Terminus, `high` / `--release` | every valid probe | posed, map-wide | ~216 s, plus a domain-sized bounce-one gather |
| Terminus, `low` | every valid probe | id 34's | ~86 s |
| Mid pose, any fidelity | near-field domain | blended from the termini | domain-sized gather plus bounce one, est. seconds |

- An earlier draft said `low` mid poses cost "a fraction" of an id-34 bake. That was wrong for a map-wide gather (~85 s, about 40% of a pose). The near-field domain is what makes `low` mid poses cheap.
- Re-tracing posed later bounces at a mid pose would cost ~130 s on the hallway, on every cold `--release` build. Mid poses blend the termini's later shares instead (owner, 2026-11-10), so a long mover's ship cost is its termini plus seconds per mid pose.
- Automatic placement caps at 16 poses; a longer path spreads them over the full trip and warns.

| Mover | L (u) | Poses at 1 m (39.37 u) | Against today's 2 |
|---|---|---|---|
| `setpiece_room` | 176 | 6 | 4 mid poses |
| `em_sweeper` | 256 | 8 | 6 mid poses |
| Short door, 64–128 u | — | 3–5 | 1–3 mid poses |
| 50 m lift | 1,969 | 16 (capped; ~3.3 m spacing) | 14 mid poses |

- Id 57 on the setpiece is 6.49 MB at 2 poses. Mid poses add direct q at every reached probe and near-field bounce and depth, less skipped poses. Not estimated; the first slice measures it. It streams past the 8 MiB bars as today.
- Live tiles stay one per depth probe, so the 15-bit tile range is unaffected. The closed-pose depth host row grows by the stored mid-pose tiles.

## Ordering pins

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| MP1 | Ramp (0.6, 0.9) on a mover with six poses at t 0, 0.2, 0.4, 0.6, 0.8, 1. Knots t′ map to 0, 0, 0, 0, 0.667, 1. The mover crosses t = 0.6 | The game hands t′ = ramp(t), and compose then looks up the knot interval holding t′. Four knots are equal at t′ = 0 | Squeezed knots carry the first pose's state (owner, 2026-11-10). Just past 0.6 the probe reads within one step of the first pose, blending toward the pose at 0.8; the poses at 0.2, 0.4 and 0.6 are never read. The same holds at the ramp end |
| MP2 | Mid poses k and k+1 are adjacent. Each is within the probe's bars of the blend of its two immediate neighbours, and the probe's states curve gently, e.g. v = c·k² with c just under the bar | Skips are decided pose by pose. Compose then blends between the nearest states that are stored or read the base | Each skip is tested against the states compose will actually blend, after earlier skips are applied. At every skipped pose's t′, the composed value stays within that probe's bars. No run of skips adds up to an error larger than the bars |
| MP3 (superseded 2026-11-10: mid poses trace their own later bounces) | em_sweeper's x 1300 probe layer is enclosed at t = 0 (inside 0–0.078). Mid pose 1 at t = 1/7 is in open air | Terminus 0 reads the base under the enclosing rule, then the mid pose's later share is blended from the termini | Terminus 0 lends no later share, so terminus 1's share holds across. The mid pose never reads the dark baked layer. An unchanged terminus lends the base minus its posed bounce one (owner, 2026-11-10) |
| MP4 (superseded 2026-11-10; see the brief's restated row) | (a) A `high` build, then a `light_poses` edit, then a `low` build. The termini hit their `high` memos and the new mid poses bake. (b) A `light_bounces` edit while the mid poses' near-field inputs stay unchanged | The termini are resolved from memo or bake first. Only then is each mid pose's later-bounce share blended, and only then is skipping decided | (a) The new mid poses carry the `high` termini's later shares, and match a `high` build's mid poses. (b) Every mid pose's later share follows the new termini, and warm equals cold |
| MP5 | `light_poses` goes from 6 to 11 on one cache. Old mid poses at t 0.2, 0.4, 0.6 and 0.8 coincide with new ones and hit their memos | Memo reads happen first. The cross-pose reductions (skipping, and the depth significance floor) run after every pose has resolved | The memos hold each pose's unskipped states. A pose that hit its memo is stored or skipped against its new neighbours. The build equals a `--no-cache` build at the same fidelity, byte for byte |
| MP6 | `prl-build --no-cache map.map` (optionally with `--bake-fidelity low`) runs interactively. The author confirms the TUI form without touching the mode | The TUI writes its choices back to the arguments first. The `--release` × `--bake-fidelity` usage check and the fidelity choice follow | The build bakes `low` (or the fidelity named on the command line) and never becomes a release bake. It never fails on a flag pair the author did not type |
| MP7 | A mover with three waypoints. Its middle waypoint moves, and both end waypoints stay put. Its swept volume, which covers every waypoint, changes, so its near-field domain changes. Both termini's posed brushes are unchanged | Termini memos are read first. Each holds per-probe bounce-one layers gathered over the old domain. Mid poses then blend the termini's later shares | No mid pose reads a share gathered over the old domain. A probe newly inside the domain carries the termini's shares, as a cold build does. The build equals a `--no-cache` build at the same fidelity, byte for byte |
| MP8 | A mover with mid poses. Probe P has depth tiles at both termini. Either P lies outside the near-field domain, or every mid pose falls within the significance floor of its neighbours for P | Skips are decided pose by pose against a neighbour blend. When the last mid pose skips, P's interval becomes terminus to terminus, and that interval runs through the base whichever rule the capture picked | P composes today's through-base depth at every t′. Each skip is judged against the through-base blend it will actually compose. No pose is skipped against a plain lerp it will never read |
| MP9 | Ramp (0.6, 0.9) on a mover with poses at t 0, 0.2, 0.4, 0.6, 0.8 and 1. Knots t′ map to 0, 0, 0, 0, 0.667, 1. The bake decides whether to skip the pose at t 0.8 for a probe | In path t, that pose's neighbours are the poses at 0.6 and 1. After the ramp, compose reads it between the squeezed knot at t′ 0, which carries the first pose's state, and the last pose at t′ 1 | The skip test uses the knots and states compose reads after the ramp. At the pose's t′ 0.667, compose stays within the probe's bars. Changing the ramp keys re-decides skips, and the build equals a `--no-cache` build, byte for byte |
| MP10 | Ramp (0.6, 0.9) on a mover with `light_poses 21`, so poses at t 0.95 and 1 both map to t′ 1. The mover crosses t = 0.9 | The game hands t′ = ramp(t). Compose looks up the knot interval holding t′, with two knots equal at t′ 1 | Squeezed knots carry the last pose's state (owner, 2026-11-10). Just under t 0.9, the probe reads within one step of the last pose. The pose at 0.95 is never read, and nothing jumps at t 0.9 |
| MP11 | A ramped mover. Light L is blocked only at a mid pose that the ramp squeezes onto t′ 0 | The gate gates L because a pose blocks it. The direct bake may skip squeezed-out poses. The record mask is the union of probes with q < 255 at any pose | The PRL loads. L composes as if unblocked at every t: q reads 255 wherever compose reads. No mover entry stores 255 at every masked probe, because the loader refuses that |
