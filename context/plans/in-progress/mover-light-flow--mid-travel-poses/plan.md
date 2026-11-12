# mover-light-flow--mid-travel-poses — plan of record

mode: resumable
status: approved
read at: 83dfb5317 (no source change since the brief's 1f24e7926; every cited symbol re-read)

## Corrections

- **Fraction → position.** Brief: the compiler can reuse the runtime's arc-length rule. → Physics exports only `MoverPathArcLengths` and `mover_path_fraction` (phase → t). `set_mover_phase_at_path_fraction` needs an entities component and positive speed, and `cumulative` is private. Planning around it by adding one pure public helper in `postretro_physics::kinematic_mover` (arc lengths + t → position, same segment rule). `forced_phase` then calls it, so the bake and the runtime share one function. Physics becomes a normal dependency of level-compiler. The only cycle runs through dev-dependencies, which Cargo allows. Termini keep today's f64 offsets (0, and last − first), so rest bytes cannot move. Mid poses use the helper on waypoints converted to f32 the way the loader converts them.
- **Swept volume.** Brief: the domain is the union of the brushes' swept volumes. → `AnalysedMover.swept` is one AABB over every waypoint's pose, which is loose on a diagonal or bent path. Planning around it with a new domain type: one box per (brush, path segment), the hull of the brush's bounds at both segment ends, widened by the gather lift plus one probe spacing. It is a conservative superset of the exact sweep, and exact for the axis-aligned box movers on both fixtures.
- **Bounce memo holds reduced states.** `bounce_memo` stores `MoverBounceStates` after `reduce_poses`, one key per mover. MP5 needs unskipped per-pose states with the reductions run after. Planning around it by memoizing each pose's raw `PoseStates` and moving `reduce_poses` and skipping after every memo read.
- **Terminus later shares need bounce one over the domain (MP7).** A terminus's full-state memo cannot hold a domain-gathered bounce one without keying on the domain, and that key would re-bake a terminus (~216 s on the hallway) whenever a middle waypoint moves. Planning around it with a second, cheap per-terminus memo: posed bounce one gathered over the domain, keyed on the pose and the domain. Each terminus's full state keeps its own key.
- **Mid-pose memos and fidelity.** A mid pose's own memo holds its bounce one over the domain and its depth. Neither depends on fidelity, so its key folds no fidelity and one entry serves both. Fidelity reaches a mid pose only through the termini's later shares, which are blended after the memo reads. This keeps the meaning of "per (mover, pose, fidelity)", since the cache keys on what changes the output.
- **TUI (MP6).** Brief: a `--no-cache` launch confirmed unchanged must not become a release bake. → `--no-cache` and `--release` set `quality_flag_supplied`, so today the config screen never shows on such a launch. The latent hazard is real, though. The form starts in `Production` when `args.release || args.no_cache`, and confirming `Production` writes `release = true`. Planning around it:
  - `--bake-fidelity` is a quality flag too.
  - The form starts in `Production` only from `release`.
  - `IncrementalBuild` keeps a CLI `no_cache`.
  - The form carries fidelity.
  - The `--release` × `--bake-fidelity` usage check reads the flags the author typed, before the TUI runs.
  - The AC is proved at `apply_form_to_args`, whether or not the screen shows.
- **A `None` cache is not release intent.** `construct_stage_cache` also returns `None` when the cache directory cannot be created. Confirms the Path: fidelity and the release intent reach the stages as explicit inputs.
- **Gate report columns.** Brief: the gate report's "per-terminus columns". → The Build Summary block (`mover_light_gate/summary.rs`) prints only four aggregate lines. The per-terminus structure is the `-v` line per (mover, terminus) and the tally rows behind it. Planning around it:
  - Those rows and lines stay terminus-only and unchanged.
  - The block gains one line per mover with mid-pose-only gated lights. The `-v` log gains matching lines.
- **C10 harness.** C10 refuses `enemy-movers`, because three movers meet at one probe and the harness supports two. AC 1 needs it, so the harness is generalised to any number of movers.
- **"Both build-flag tables"** are `docs/level_design.md` §Common options and `build_pipeline.md` §CLI flags. `docs/distribution.md`'s refused-args table also gains `--bake-fidelity`. `docs/level_design.md` :523 ("blends straight from the first waypoint's lighting to the last's") and :533 (bake cost per end of the path) are rewritten.
- **Knots reach the renderer, not the game.** Plan task 2 said the loader carries knots to mover runtime data. The game needs only the ramp (it hands t′ = ramp(t)), and the renderer reads id 57 itself, so knots stay in the section and `apply_light_ramps` hands the ramp alone.
- **Mid poses have no waypoint name.** `Pose.waypoint` is an authored name that reports print. A mid pose reports by its path fraction instead (`t 0.40`).

- **Enclosure seams (task 13).** Brief: geometry knots sit at each enclosure end, by the bake's own enclosing rule. → Where two of a mover's brushes touch, the bake's inset leaves a ~2 mm stretch at the seam that does not count as enclosed. Read strictly, each seam adds three knots, and the knot inside the seam stores dark in-solid values mid-enclosure, which brings back the sawtooth. Planning around it by joining enclosures whose gap lies inside the mover's solid (inset 0) for placement only; the bake's enclosing rule is unchanged. Strict counts would be wall 169, room 43, warren lifts 201 and 207.

- **A 178-pose room truth is not clean (task 23 scan).** Consensus and the handoff cite 178 as clean. The exact band scan puts poses 21, 84 and 172 in a layer's band. The consensus conclusion survives: 178's band poses fall away from the late-trip ceiling. Only 76 is cited as clean from here.

## Delegated answers

- **Band, for authored poses (task 23).** A pose holds a probe in the band when it neither encloses it (`Pose::encloses` at `enclosure_inset`) nor clears it (`clear_of_band`, lift band `RAY_EPSILON + enclosure_inset`). Seams count as band, since a probe in a seam sliver is in solid but enclosed by neither brush. Such a pose moves until one brush encloses it, matching the "Enclosure seams" correction.
- **No clean spot (task 23; owner to confirm).** When an authored pose in a band has no clean spot between its neighbours, the build fails, naming the mover, pose, probe and neighbours. It never drops, stacks or reorders a pose ("No silent fallback"). This only happens when authored poses sit within about 5 mm of travel of each other.
- **"Stored interval longer than one even step" (task 22; owner to confirm).** Read per probe: shipped knot intervals are all shorter than one even step, so read per knot the exception would cover nothing. A receiver counts as long when at least half its light, by the truth's weights, comes from probes whose stored interval (direct across marks, or bounce marks, where that term changes) is longer than one even step. The exception is not load-bearing: every receiver it reports would also pass the 3-or-1.5 clause.

- **Epoch-5 byte layout** — pinned in task 2 and recorded in the brief's Wire format section. Three refinements over the proposal below: mover records sit right after the cell index, since every block's lengths depend on N; bounce keeps its per-cell entry-count CSR and adds a per-entry byte-offset table; and a base mark must lie inside the entry's stored union, extending the tightness rule. Reason: it meets every Wire format constraint, keeps the direct and bounce GPU tables close to epoch 4's, and puts every variable length behind offsets the loader checks.
- **Where id 34's recorded bake time lives** (AC 26) — in a small cache entry, `"sh_base_bake_time"`, keyed on id 34's volume key. It is written whenever id 34 bakes, cold or warm, and a `--release` build uses its own measured time. Reason: a field inside the `sh_base_volume` entry would bump that stage's version and re-bake every author's id 34 once, for no change to bytes. A build with no record prints "—".

### Proposed epoch-5 layout

Little-endian, internal epoch 5 in the first byte, exact match. The header, cell index and placements are unchanged from epoch 4.

| Part | Epoch 5 bytes |
|---|---|
| Mover records (replaces ramp records) | Count u32. Then per mover, ascending by id: `mover_id u32`, `ramp_start f32`, `ramp_end f32`, `pose_count u32` (N ≥ 2), then N `f32` knots. Knots are finite and strictly ascending, knot 0 is exactly 0.0 and knot N−1 exactly 1.0. Exactly the movers with entries in any block, as today. |
| Direct | Record and mover-entry CSR unchanged. Under each mover entry, N `u8` q per masked probe, probe-major: byte `slot·N + k`. The record mask is the union over poses of q < 255. A mover storing 255 at every masked probe and pose is still refused. |
| Bounce | Per cell, an entry-offset CSR (u32 byte offsets, from 0, ascending) replaces the fixed 24-byte stride. Entry: `mover_id u32`, `reserved u32 = 0`, then N pairs `(stored u64, base u64)`. At a terminus, `base` must be 0: unmarked already reads the base. At a mid pose, `base` marks probes that read the base (enclosed or unchanged), and unmarked blends across. A probe set in both is refused. The union of `stored` over poses is the affected mask, which must be non-empty, tight and within the valid mask. Tiles follow, pose-major, then probe by `stored` rank, in id 27's compact format. |
| Depth | Records by mover, then probe: `mover_id u32`, `probe_index u32`, then N two-bit marks (bit 0 stored, bit 1 base), packed low-first and padded to 4 bytes. That stride is fixed per mover, because N is the mover's. Both bits set is refused. Base at a terminus is refused, as for bounce. A record storing no pose is refused. Tiles follow, in pose order. |
| Streaming | Unchanged: the direct q bytes plus own tiles, and the bounce tiles, stream past their 8 MiB bars. Records, masks, marks, knots and offsets stay resident. |

The renderer expands what the GPU needs at install: per-pose tile offsets and each mover's knots mapped through its ramp. The shaders then do no per-texel scan.

## Owner rulings after the first slice (2026-11-10)

Recorded in the brief's Decisions and Acceptance:
- **Enclosed mid poses blend across**, in bounce and direct, keeping the dark depth tile; termini keep reading the base. Direct gets per-(cell, mover) across marks so compose blends exactly after the ramp (epoch-5 wire addition).
- **Mid poses trace their own later bounces** over the near-field domain's patches; rays leaving the domain read the mover-absent bake's per-bounce radiance, computed once per build. Supersedes the termini-blended later shares (MP3, MP4).
- **Depth: per-interval through-base.**
- **Restated:** AC 7 (patch-grid exception, follow-up filed), AC 8 (traced mid-pose bounce, against a static-brush reference), AC 12 (enclosed mid pose blends across), AC 19 (sweeper t′ 0.30 only), AC 27 (low serves high where cached, bakes new mid poses low), M4 (east face against a static-brush reference).
- **Follow-up to file at landing:** cut static patches at posed mover faces, so a posed face off the patch grid matches a static reference (AC 7's exception).
- **M7 restated (owner, 2026-11-11):** the row now asks that mid poses add no frame cost over the pre-brief build; the 0.2 ms budget goes to a follow-up brief, since the pre-brief build already costs 1.7–2.7 ms per travelling mover.
- **Follow-up to file at landing:** bring per-mover recompose under 0.2 ms on the 5300M (M7 data: scratchpad `m7/runs/ab-table.md`, summarised in the M7 row). Leads: cost tracks rows, not probes (wall 3.4× probes at 1.55× cost; both movers sub-additive); options to weigh include recomposing on a t′ step rather than every change, spreading rows over frames, and narrowing rows to the probes a mover changes. The M7 harness (map variants, xctrace runner, parser) is the A/B tool.

## Owner rulings after the motion captures (2026-11-11)

These came from the owner's `--release` playtest on the Windows box:
- the rising room's floor and ceiling leak, pulse and step as it moves;
- em_sweeper shows "patches of light changes", worst on its east face.

Diagnosis (`captures/motion-results.md`):
- depth acts only at knots, so the ceiling flashes dark at each knot and leaks lattice dots between them;
- the east face has a value sawtooth at the knot period;
- the AC 7 patch-grid error is not the visible cause.

A Fable reviewer and I went four rounds, with offline evidence in `captures/advice-results.md`. Recorded in the brief's Decisions and Acceptance:
- **Geometry knots.** Knots go just outside and just inside each end of every domain probe's enclosure interval, joining the even poses. They are exact from geometry, shared per mover, and never dropped to meet a count. An authored `light_poses` places even poses only.
- **Depth blend.** Mean and variance on every interval except termini ↔ termini, which keeps raw through-base. Supersedes per-interval through-base.
- **IP.** The `ip-372` question 9 counsel wording is updated (owner: "Update it").
- **Fallback: none.** No silent fallback. Cost is priced by the existing gates, with named errors. Knot counts on every map and the hallway decide whether sparse per-probe storage joins (owner: "Carry on", accepting this recommendation).
- **New and restated ACs.** New: ACs 34–37 and M9. Restated: AC 4 and AC 5 (even poses plus geometry knots), AC 15 (depth mix rule) and AC 18 (MP8 wording).
- **Build here, this session** (owner: "once the pause is over, let's build it here").

## Open owner questions

- Amendment 2's two rulings (backface form; in-wall layer approach) are asked once its captures exist; see §Amendment 2.

## Owner answers on the Amendment 2 captures (2026-11-12)

- **Ruling A: leaning toward the recommendation** (surface-point ramp, 0.4-cell offset), but the owner wants to see it in action before committing. Next: A/B motion clips from the capture series, and an in-engine toggle (`POSTRETRO_PROTO_MOVER_SH`) for the Windows playtest.
- **Ruling B: prototype the dark value state first.** Option (ii) is built behind a switch and captured, sweeper faces included for the dark-sphere risk, alone and combined with Ruling A's ramp, before the owner rules.
- **Rulings made (owner, 2026-11-12, after the in-engine demo and the dark-state prototype):**
  - **A:** surface-point ramp with a 0.4-cell offset.
  - **B:** the dark state for enclosed mid poses, only with A. Across marks go, and the counsel line is restated.
  - **Re-light into rest:** fixed in this brief with a terminus-adjacent geometry knot.
  - All three are recorded in `index.md` Decisions and Acceptance.
  - The dark-state prototype: `POSTRETRO_PROTO_ENCLOSED_DARK`, `AMEND2-PROTO-B` in `mover_probe_channel/{mid_pose,direct,across}.rs`. Results are in `captures/amendment2/results.md` §Ruling B option (ii).
  - The wall truth at 267 is clean by the exact band predicate.
- **Tasks 22–23 calls: keep both.** A set count too dense to clear a band fails by name. The long stored-interval exception is read per probe.

## Owner rulings after the playtest diagnosis (2026-11-12)

The owner played a Windows `--release` rebake. Most flat edges no longer pulse. What remains: a pulsing leak on the rising room's ceiling above the door, a pulse on the wall door's edge, a patch on its upper-right white square, and the sweeper's crusher-side shimmer now "jumps from one frame to the next". An Opus agent, a Fable agent and the lead each diagnosed independently, then reached consensus (`captures/playtest-diagnosis/consensus.md`, write-ups, scripts and charts beside it).

**Findings:**
- **No new channel defect.** HEAD tracks dense truth bakes: the wall at 0.02 luma, the sweeper at 0.51 (pre-amendment 4.51), the room ceiling at ~0.05. The late-trip ceiling excess was a truth artifact. The 177-pose truth parks poses 123, 139 and 171 within 1 mm of layer z 122.96; a 178-pose truth matches HEAD (8.18 vs 8.20 at t′ 0.818).
- **Lattice hand-over.** This causes the sweeper jumps, the wall-edge pulse and part of the ceiling pulse. It predates the brief: the strip column shows it.
  - The mover backface cosine is taken from the lookup point (`sh_sample.wgsl` `sh_backface_weight`), 0.1 cell off the face.
  - When the lookup crosses a probe layer, which is where that layer's trilinear weight peaks, its weight falls to the 0.03 floor. Renormalisation then hands the face to the next layer in 1–3 frames.
  - The amendment removed the knot blinks and drift that hid it.
  - A larger offset only moves the flip.
- **In-wall layer.** Plane x = 846.93 lies 1.07 u inside the room's west wall. Its enclosed and above-roof probes carry arena bounce into the dark room through the 0.03 floors. Moving the lattice 32 u drops the band from 6–13.5 luma to under 2 until t′ 0.56.
- **White square.** It is static, not the amendment. The back-room spot `150 1420 290` lands a world-fixed footprint at probe resolution, and the upper-right square slides under it at t′ ≈ 0.2–0.5. Removing the spot removes it.
- **Seam hazard.** An authored `light_poses` count can park a pose's brush face or seam within the enclosure band of a probe layer, which stores an in-solid dark state. Room 41 has one such pose (pose 19, 0.24 mm), room 78 one (pose 71, 1.14 mm) and room 177 three. Room 76 and sweeper 113 are clean. The 78-pose per-knot failures (t′ 0.117, 0.805) are HEAD's 1.4 u windows, not band poses; t′ 0.831 is a genuine long-interval miss.
- **M9 ("no stepping") is not met in motion.** The cause predates the brief, and Amendment 2 takes it on.

**Rulings** (the owner agreed with the recommendations and asked for the hand-over and in-wall fixes as soon as possible, as an amendment to this brief):
- **AC 36:** option 1, with the room truth at **76** poses. The room's worst knot is reported, not gated. Time-weighted means are gated. Values are gated except at knots inside a stored interval longer than one even step, which go to the long-interval follow-up. The truth test asserts that no truth pose parks a mover brush plane within the enclosure band of a probe layer. Task 22.
- **Authored `light_poses` near probe layers:** fixed in this branch. An authored pose whose brush plane falls within the enclosure band of a domain probe layer moves the least distance out of it. Task 23.
- **Hand-over and in-wall fixes:** Amendment 2, below, in this branch. Work moves to the owner's Windows machine (`HANDOFF-windows.md`).

## Amendment 2 (owner, 2026-11-12): mover-receiver hand-over and in-wall probe layers

**Goal.** M9 met in motion: no per-layer stepping on mover faces and no pulsing band from a probe layer inside a thin mover wall. Rest frames may change; AC 1's per-probe channel comparison should still hold.

The executor drafts the amendment's Decisions and Acceptance rows into `index.md` (the "Geometry knots" amendment is the model) and asks the owner the two rulings below with captures in hand, before building.

- **Ruling A, backface form (ip-372 lever 5, counsel question 6).** Consensus candidate: measure the mover backface term from the *surface point*, not the lookup point, as a smooth signed-distance ramp along the receiver's geometric normal, scaled by the cell and floored at 0.03. It may combine with a 0.25–0.4-cell lookup offset.
  - Capture A/B on the sweeper east face, the wall's trailing end face and north face, and the room ceiling, against the current form.
  - Redo the eighth-power capture (rising room floor lattice dots) under the new form.
- **Ruling B, in-wall layer.** Measure and present the options:
  - (i) an authoring warning when a probe layer lies inside a mover slab or within the lookup offset of its face, plus a lattice-phase key;
  - (ii) a dark value state for enclosed mid poses. This reverses the 2026-11-10 enclosing rule and carries a dark-sphere risk;
  - (iii) the larger lookup offset alone, which cuts the above-roof share (0.49 → 0.19) but not the in-wall column's.

  The split between in-wall and above-roof probes is unmeasured; measure it first.

**IP.** Every candidate must keep every valid probe at a continuous weight of at least 0.03. Nothing may flag, zero or exclude a probe. The rules in `HANDOFF-windows.md` bind verbatim.

| # | Task | Depends | Status |
|---|---|---|---|
| 22 | AC 36 restated (option 1, room truth 76) in `index.md` and `pipeline/mover_motion_proof_tests.rs`; truth-band assertion; re-run `--ignored` | — | done (Windows, dev profile): `stay_within_the_motion_bars_of_the_fine_bake` 2 passed under `--ignored`, 236 s. Time-weighted share Δ: ceiling 0.0014, floor 0.0080, east 0.0095, west 0.0050 (bar 0.01; pre-amendment 0.09–0.12, fails). Worst knots reported: ceiling 0.0216, floor 0.590, east 0.384, west 0.149. Values: ceiling worst 2.17 and west 1.17 levels; the floor (t′ 0.72–0.93) and east (0.03–0.18, 0.91–0.98) pass some knots by the 1.5 clause alone. Long-interval receivers reported: floor 4, east 3 runs, all direct-dominated, and each would pass the 3-or-1.5 clause anyway. `assert_truth_poses_clear_every_band` checks the unmoved even poses and that the baked truth knots equal them |
| 23 | Authored `light_poses` poses moved out of probe-layer enclosure bands; unit test (a slab whose authored pose lands within 1 mm of a layer); `light_poses` Decision note | — | done: `mover_light_gate/layer_bands.rs` (`clear_authored_knots`, `poses_in_band`), called from `place_poses` for authored counts. Four tests in `geometry_knot_tests.rs` (face 0.5 mm past / 0.9 mm short, seam, clean count bit for bit, refusal by name); `mover_light_gate::` 64 passed, `--bin prl-build mover` 201 passed, clippy clean. Docs and FGD note the nudge. An exact scan of the real maps matches Fable's table: room 76 and sweeper 57/113 are clean; room 41, 77, 78 and 177 are not |
| 24 | Amendment 2 drafted into `index.md` (Decisions, ACs, M9 restated if needed); hand-over and in-wall measurements; owner rulings A and B | 22 | measured, waiting on the owner: `captures/amendment2/results.md` and charts (GTX 1660 Super, Vulkan, cold `--release` bakes, room truth 76, sweeper 113, wall 267, whose band check is pending). The prototype is switched by `POSTRETRO_PROTO_MOVER_SH` and marked `AMEND2-PROTO` in `sh_sample.wgsl`, `kinematic_brush.wgsl` and `kinematic_brush.rs`; it is uncommitted. Its attribution filter is a measurement tool, which breaks levers 2 and 7, and never ships. Recommendation: A, the surface-point ramp with a 0.4-cell offset; B, (i) a warning plus an author phase key, with (ii) held back. Then: ramp clips and an in-engine demo; (ii) prototyped. **Done, ruled:** A the ramp at 0.4, B the dark state with A, the terminus knot in this brief |
| 25 | Build the ruled fixes; capture A/B against dense truth (seam-free counts); rest baselines re-cut where the ruling changes rest frames; AC 1 re-run | 24 | in progress, split: **25a** renderer, the ramp at 0.4 as the backface form (prototype switch and attribution filter removed, floor/continuity test, frame baselines re-cut); **25b** compiler and format, the dark state as the rule (across marks removed, epoch decided by the implementer, terminus-adjacent knot, tests); **25c** after both, the `--release` capture A/B against dense truths under the new rule, proposed bars for the Amendment 2 rows, and the AC 1 re-run |
| 26 | Review round, fix pass, preflight; owner in-engine M9 check on Windows; M7 re-measured on the Mac when it is free, if the shader changed | 25 | todo |

Then land the plane (`HANDOFF-windows.md` §Land the plane).

## Superseded: AC 36 question (resolved above)

- **AC 36, room truth density (raised 2026-11-12, task 20).** The ruling names `light_poses 113` for the sweeper so the truth resolves each geometry-knot window. The room's truth is still 41, which samples none of its 1 u windows. At 78:
  - the time-weighted means pass (0.0074, 0.0069);
  - the room's per-knot share gate fails on single-knot spikes (ceiling 0.44, floor 0.52), which the GPU shows as at most 2.0 luma on one sample;
  - the floor values fail at t′ 0.831 (7.10 vs 5.35 pre-amendment, 0.25 over the 1.5 clause), the long-interval direct miss that the pre-amendment build shows at the same strength.

  Options:
  - **(1) Recommended:** room truth at 78; the room's worst knot reported, not gated, like the faces; time-weighted means gated; values gated, except knots inside a stored interval longer than one even step, which are reported and go to the long-interval direct-interpolation follow-up.
  - **(2)** Keep the room's truth at 41, with the GPU check recorded as the proof of its windows.

## Owner rulings after review round 4 (2026-11-12)

The owner agreed with all three recommendations ("I agree with your recommendations on the three questions!"):
- **IP wording.** `ip-372` question 9 is restored to the owner-approved text with one correction. Between termini, a live tile runs raw moments through the probe's static tile, reaching it at mid-travel; "raw-linear between termini" misstated the code. The brief's counsel line is corrected to match. The Piece 7 row in `ip-check.md` and the original counsel-packet line ("states per pose…") land at landing.
- **AC 36.** The faces' share is gated on a time-weighted trip mean, integrated over travel against a truth that resolves each geometry-knot window (`light_poses 113` on the sweeper). This replaces the unweighted mean over truth knots, under which a 1 u crossing counted as 1/56 of the trip (0.0102 vs 0.01). Fable's recommendation over the executor's "exclude the worst knot". Task 20.
- **AC 3, C10 per-probe.** The case runs twice: bit for bit on `--uncompressed-irradiance` builds, and within a bound derived from BC6H's mode difference through two f16 roundings (about 2 steps), pinned in the test, on BC6H builds. Task 21.

## Owner rulings, AC 7 and AC 36 (2026-11-11)

The owner agreed with both recommendations ("I think I agree with your recommendations on the two questions you asked"). Recorded in the brief:
- **AC 7:** the exception widens to probes within one patch of a plane along which the reference cuts the floor off the patch grid.
- **AC 36:** restated. The share bar gates the trip mean and the room layers' every knot, while the faces' worst knot is reported. Values pass within 3 levels of truth or within 1.5 of the pre-amendment miss at that knot.
- **Follow-up to file at landing:** direct interpolation over long intervals, and the skip bar's reach on dark surfaces. These are the misses the pre-amendment build shares (east t′ 0.982, floor t′ 0.825, ceiling bounce at 3.3–4.5 levels).
- **Test changes** land after task 18's gates. Task 19 implements them.

## Resolved questions

- **AC 7, em_sweeper bounce one (raised 2026-11-10, task 3; resolved by the restatement above).** At em_sweeper's mid poses, bounce one reads up to 4.69 display levels off the static-brush reference (bar 3), in one probe column 0.21 m before the slab's west face. Cause: the posed bake's existing patch handling. The slab's face lands part-way into a 0.5 m static floor patch, which the posed slab partly buries, while the reference cuts the floor at the face. The error follows each pose's phase in the patch grid (1.72–4.69 levels over mid poses 1–6). The termini read 1.78 and 2.01 only because their offsets land on patch boundaries. A fix in the posed bake would move terminus bytes, which AC 1 forbids. The test stays strict and `#[ignore]`d with the reason.

## AC-to-proof

Numbering follows the brief's Acceptance order: automated rows 1–33 (Rest 1–3, Placement 4–6, Bake 7–14, Compose 15–19, Fidelity and cache 20–31, Format and docs 32–33), manual rows M1–M8.

| AC | Proof | Status | Result |
|---|---|---|---|
| 1 Rest unchanged at high (three maps, t′ 0/1, C10-style, three-mover probes) | GPU harness `rest_channel_output_equals_the_pre_change_build` (`--ignored`, `POSTRETRO_REQUIRE_GPU`), pre-change PRLs rebaked fresh at 83dfb5317 | achievable as stated | pass — `rest_channel_output_equals_the_pre_change_build` bit for bit, both modes, against the parent compiler (loader refuses 83df's epoch 4; 83df and parent differ only in id 57); end to end 23 of 28 frame pairs identical, 5 at 1–2 px by 1 level (task 11) |
| 2 `light_poses 2` at `high` stores and composes today's blend | compiler: section equals the pre-change build; renderer compose test at sampled t′ | achievable as stated | pass — parity `every_mover_at_light_poses_two_packs_byte_identical…` 3/3; 0-pixel frames against the pre-brief engine at t′ 0, 0.25, 0.5, 1 |
| 3 K1, K2, K3, C7, C10 pass on `--release`; C9 renderer half against the piecewise mix | existing capture and renderer tests on `--release` PRLs; C9 expectation generalised | achievable as stated | pass — K1, K2, K3, C7, C9 both halves on `--release`; per-probe C10 bit for bit on `--uncompressed-irradiance` builds and within the derived BC6H bound (worst 0.968 of it; task 21); C10 frames per AC 3 restated (setpiece st3 mask 8: 21 vs bar 22) |
| 4 Automatic pose count, even arc length, unequal, degenerate and zero-length paths, positions match the runtime's t | compiler unit tests on placement, plus the physics helper round-tripping `mover_path_fraction` | achievable as stated | pass — placement and `point_at` unit tests (task 1) |
| 5 16 exactly, 17 → 16 + warning, `light_poses 20` → 20 and no warning | compiler unit tests with log capture | achievable as stated | pass — placement cap tests (task 1) |
| 6 `light_poses` parse accepts and rejects | `parse.rs` tests | achievable as stated | pass — `parse.rs` tests; past 16,777,217 refused at parse (review round 2) |
| 7 Mid pose matches static-brush reference (direct, bounce one, depth) on room and sweeper, with the patch-grid exception (restated 2026-11-10) | `the_rising_rooms_mid_pose_matches_its_static_brush_reference`, `the_sweepers_mid_pose_matches_its_static_brush_reference` (`--ignored`, 13 s); reference at `_bounce_fill 0` for bounce one | achievable as restated | pass — both reference tests (`--ignored`, 13 s): room bounce one 0.87, sweeper 2.30 with the patch-grid exception |
| 8 Mid bounce = bounces one..effective `light_bounces` posed, later bounces over the domain's patches with leaving rays reading the mover-absent bake, plus id 34's past them; matches the static-brush reference's full bounce; `low` mid = `low` terminus (restated 2026-11-10) | `later_bounce_tests.rs` unit tests; the two reference tests above at default fill; `low` half via `light_bounces 1` until task 9 | achievable as restated | pass — `later_bounce_tests.rs`; references full bounce room 1.92, sweeper 2.83; `low` half via fidelity tests |
| 9 Near-field domain in/out; direct still covers outside | compiler unit test | achievable as stated | pass — near-field domain unit tests (tasks 1, 3) |
| 10 Skipping within bars; adjacent skips bounded (MP2) | compiler unit tests on synthetic curves | achievable as stated | pass — `skip_tests.rs` (task 7) |
| 11 Ramp (0.6, 0.9) skips post-ramp; ramp edit re-decides; warm = `--no-cache` (MP9) | compiler tests plus `prl-build` integration | achievable as stated | pass — skip ramp tests and `a_ramp_edit_re_decides_skips_over_the_memoized_states_and_warm_equals_cold` |
| 12 Enclosed mid pose stores no value state and blends across (bounce and direct), keeps dark depth; enclosed terminus reads base; face probe keeps per-ray rule (restated 2026-11-10) | compiler unit tests plus GPU across compose test | achievable as restated | pass — enclosure compiler tests and GPU across compose tests (task 6a; high-word GPU test added in review) |
| 13 Mid-pose-only blocked light gated; report counts it; per-terminus columns unchanged | compiler test plus gate report comparison against the pre-change build | achievable as stated | pass — crossing-slab test plus report diff empty against `prl-build-83df` on three maps (task 10) |
| 14 Squeezed-out blocking pose loads and composes unblocked (MP11) | integration: build, load, renderer compose test | achievable as stated | pass — `mover_probe_channel_poses` bake-and-load half and renderer compose test |
| 15 Exact at stored knots, mix between, direct exact, out-and-back alike | renderer compose tests (GPU, skip without adapter) | achievable as stated | pass — renderer GPU compose tests |
| 16 Two movers, different pose counts and skips, combine as today | renderer compose test | achievable as stated | pass — renderer GPU compose tests |
| 17 Ramp edges read first and last pose; ungated without state (MP1, MP10) | renderer compose tests | achievable as stated | pass — renderer GPU compose tests |
| 18 Outside-domain and all-skipped depth compose through-base (MP8) | renderer depth unit test plus compiler skip test | achievable as stated | pass — `termini_to_termini_depth_runs_through_the_base`, `piecewise_depth_…` and the compiler all-skipped depth test |
| 19 em_sweeper t′ 0.30 and roof t′ 0.4, 0.6 within 3 levels of a static-brush reference; today fails (restated 2026-11-10: t′ 0.25 dropped) | compiler-side CPU compose against a reference bake (`pipeline/mover_mid_pose_compose_tests.rs`, now un-ignored) | achievable as restated | pass — sweeper t′ 0.30 1.07 / 0.00, roof 0.4 1.78 / 0.00, roof 0.6 1.80 / 0.00 levels (bounce / direct); two poses miss at 11–20 / 23–59 |
| 20 `low` caps at 1, `high` uses own, `--release` = interactive `high` on empty cache, `light_bounces 0` stores no bounce | `prl-build` integration | achievable as stated | pass — `tests/mover_probe_channel_fidelity.rs` |
| 21 `--release` + `--bake-fidelity` fails naming both; `--no-cache` and failed cache dir bake `low` | `main.rs` arg tests; integration with an uncreatable cache dir | achievable as stated | pass — `main.rs` arg tests and the uncreatable-cache-dir integration |
| 22 TUI carries fidelity; `--no-cache` confirmed unchanged never becomes release (MP6) | `tui_config` tests at `apply_form_to_args` (Corrections, TUI) | achievable as stated | pass — `tui_config` tests at `apply_outcome` |
| 23 Per fidelity, warm = `--no-cache`; two `low` builds alike | integration | achievable as stated | pass — `per_fidelity_warm_equals_no_cache_and_two_low_builds_over_one_cache_are_alike` |
| 24 `low` after `high` serves `high`, bakes edited mover `low`; `high` after `low` = `--release` | integration | achievable as stated | pass — `low_after_high_serves_high_and_high_after_low_rebakes_every_pose_and_equals_release` |
| 25 `light_poses` change hits both termini, bakes new poses, warm = cold (MP5) | integration, with memo hit lines asserted | achievable as stated | pass — `a_light_poses_change_keeps_both_termini_and_bakes_only_the_new_poses` |
| 26 Middle waypoint move changes domain; warm = cold (MP7) | integration | achievable as stated | pass — `moving_a_middle_waypoint_changes_the_domain_keeps_the_termini_and_equals_cold` |
| 27 After `high`, a `light_poses` change built `low` serves termini and unchanged mid poses from `high` and bakes new mid poses `low`; a `light_bounces` edit built `high` re-bakes every pose, warm = cold (restated 2026-11-10) | integration | achievable as restated | pass — fidelity low half and `a_light_bounces_change_rebakes_every_pose…` at `high` |
| 28 Low against high at rest, largest difference and sign, fails past owner's bound | `--ignored` gate test (`tests/mover_probe_channel_fidelity_rest.rs`); bound 12 display levels (owner, 2026-11-11, from M2) | achievable as stated | pass — worst 11.42 levels (setpiece wall t′ 0, low brighter) under the 12-level bound |
| 29 Build Summary: pose count, fidelity, projected time (termini × 2 + mid poses by domain share, restated 2026-11-11) or "—", approximate marking | compiler summary tests on the printed summary | achievable as restated | pass — summary tests; estimate vs measured high − low: setpiece 6.6 / 6.8 s, enemy-movers 4.6 / 4.6 s |
| 30 SH memory gate covers every pose; under-budget refuses by name | extend `a_budget_under_the_mover_priced_peak_refuses_before_any_bounce_ray` | achievable as stated | pass — gate test at `low` and `--release`; priced 124.5 MiB vs 100–102.5 MiB RSS on setpiece |
| 31 Ledger counts every stored pose's closed-pose tile; zero after unload | extend `tile_bytes_and_held_closed_pose_tiles_are_ledgered_and_leave_with_the_level` | achievable as stated | pass — ledger test (multi-pose) and the compiled-map ledger test |
| 32 Loader refusals for epoch 5; epoch 4 refused | level-format and loader tests | achievable as stated | pass — level-format and loader refusal tests, incl. across marks and `StreamedRowOverflow` |
| 33 `light_poses` in FGD and docs; `--bake-fidelity` in both flag tables; `dist` refuses it in a recipe | FGD property test; `manifest.rs` `rejects_reserved_bake_arguments`; docs review | achievable as stated | partial — FGD, `docs/level_design.md`, `docs/distribution.md` and `dist` refusal pass; `build_pipeline.md` §CLI flags row lands at landing |
| 34 Geometry knots just outside and inside each enclosure end, within tolerance (lattice slab, off-phase slab, two brushes, bent path, ramp); authored `light_poses` places none | compiler placement unit tests | achievable as stated | pass — `geometry_knot_tests.rs` (task 13's five cases plus seam, a grazing second brush, a 1:10 ramp, slant, bevel, band gap and a lift-wall graze; sampled oracle; mutation-checked, passes A and C); authored count places none |
| 35 Depth mean-and-variance blend: ends exact, translating occluder adds no variance, σ² ≥ 0 and f16 saturation, termini interval bit-identical to through-base, several movers; skip and renderer share it | level-format unit tests; renderer and `skip.rs` call sites | achievable as stated | pass — `depth_mix_tests.rs`, `depth_blend_tests.rs`, `the_tile_blend_is_the_texel_blend_texel_for_texel` (bit for bit); renderer `depth_blend.rs` and `skip.rs` both call `mix_depth_tile`; termini interval bit-identical (parity 3/3); mutation-checked (task 14, 14b) |
| 36 Motion proof against a fine bake: time-weighted far-side share within 0.01 trip mean, room per-knot within 0.05, faces' worst knot reported; values within 3 levels or within 1.5 of the pre-amendment miss; pre-amendment build fails (restated 2026-11-11 and 2026-11-12) | `--ignored` `pipeline/mover_motion_proof_tests.rs` (`…_stay_within_the_motion_bars_of_the_fine_bake`), sweeper truth `light_poses 113`, room truth 41 | achievable as restated | pass at these truths — sweeper east 0.0095, west 0.0050 (worst knot reported 0.384, 0.149); room ceiling 0.0018, floor 0.0002 (worst 0.0176, 0.0024); pre-amendment fails all four (0.08–0.10). Room truth density is an open owner question: at 78 the per-knot gate and one floor value fail on 1 u windows; GPU check shows ≤ 2.0 luma on the ceiling and the known long-interval floor drift (task 20) |
| 37 No geometry knot dropped; SH gate prices every geometry pose; Build Summary prints even and geometry knot counts | gate and summary tests | achievable as stated | pass — `admit_placed_poses` keeps every geometry knot (cap applies to even poses; warning names it); AC 37 SH-gate pricing in `mover_bounce_stage_tests.rs` via `PatchSet::try_heap_bytes`; Build Summary `N pose(s) (E even, G geometry)` summary tests |
| M1 Depth candidates: owner picks | owner, from first-slice stills | manual | done — owner ruled per-interval through-base (2026-11-10) |
| M2 Low vs high first measurement: owner sets bound | owner | **done 2026-11-11:** low − high at rest: closet door t′ 0 +10.75 levels, setpiece wall −11.23 / +11.42, room −7.84 / +4.21; owner set AC 28's bound at 12 levels, low being an iteration preview judged in-engine | done (see Status) |
| M3 Roof top t′ 0.3–0.70, `low` and `high`, within 3 levels, no pits | owner, in-engine and captures | manual | **outstanding: owner, in-engine** |
| M4 em_sweeper faces within 3 levels, no pits | owner | manual | **outstanding: owner, in-engine** |
| M5 Rising room floor, no dots or diamonds | owner | manual | **outstanding: owner, in-engine** |
| M6 Rising room mean luma against t′ | owner | manual | **outstanding: owner, in-engine** (captures: `t6a-luma.png`, `t6b-luma.png`) |
| M7 5300M trace: mid poses add no frame cost over the pre-brief build (restated 2026-11-11) | executor, Metal System Trace A/B | **pass, 2026-11-11.** Before the restatement it failed the 0.2 ms budget, and the cause predates this brief. 937ba607d `--release`, 2560×1440 windowed, clock 1228 MHz, async streaming, 3 valid rounds per config: room 1.73 ms, wall 2.68 ms, both 1.48 ms per mover (bounce compose ~1.0–1.4, Pass A ~0.6–1.2, uploads ~0.1). Same harness on HEAD at `light_poses 2`: 1.65 / 2.63; on pre-brief 83dfb5317: 1.69 / 2.69. Mid poses add nothing measurable to compose (< 0.1 ms, mixed sign); uploads possibly +0.05 ms (within noise). Cost does not scale with affected probes (wall 3.4× probes, 1.55× cost); stripped id 57 → 0. Owner ruled: follow-up brief. Harness and data: scratchpad `m7/` (`runs/ab-table.md`) | pass (see Status) |
| M8 Cold and warm bake times, id-57 bytes per block, hallway | owner judges; executor measures | manual | measured (`t6b`–`t9` results; hallway projected); **outstanding: owner judges** (re-measure after task 15) |
| M9 In motion: room floor and ceiling, sweeper faces, no flashes, dots, pulsing or stepping; dense motion series before and after; M7 A/B re-run | owner in-engine; executor captures | manual | captured (`captures/t15c-results.md`), at HEAD fff783e14 + 977db499a:
• Mean |final − truth| (pre-amendment): ceiling 0.09 (4.41), floor 0.10 (1.19), interior 0.05 (0.40), east 0.58 (3.87), west 0.08 (0.86).
• Ceiling knot flash: mean 0.05, worst 0.38 over 43 knots (was 5–6). Ceiling 12c peaks median 9 (pre-amendment 24, truth 1): a faint lattice remains.
• East peak-to-trough tracks truth; residual east +3.7 at t′ 0.16 and −2.0 at 0.98 from long-interval direct, shared with pre-amendment.
• **M7 re-run, interleaved with 83df:** per mover minus docked, HEAD/83df: room +1.722/+1.641, wall +2.570/+2.535, both +1.443/+1.534 ms. Wall compose 2.456–2.477 against 83df 2.509–2.608; room within 83df's range. Pass: no measurable cost; the wall's +0.13 ms is gone with the bracket.
• Bakes: setpiece 44.3 s, 12.03 MB (wall 71, room 45 poses); enemy-movers 14.5 s, 4.52 MB (sweeper 31).
• **Outstanding: owner, in-engine.** |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | **Poses.** Physics fraction → position helper (forced_phase uses it). Pose model widened from `Terminus` to N indexed poses, with termini first and last and today's offsets kept. Placement rule, cap and warning. `light_poses` parse, FGD line and its tests. Near-field domain type. ACs 4, 5, 6, 9 (domain half). | integrating executor | — | done: `point_at_lands_by_arc_length_and_reads_back_the_same_fraction` (physics); `automatic_pose_count_is_one_per_probe_spacing_plus_one_capped_at_sixteen`, `mid_poses_sit_at_even_arc_length_by_the_runtimes_rule_and_termini_are_unchanged`, `a_path_past_the_cap_spreads_sixteen_poses_and_warns_once_naming_the_mover`, `near_field_domain_is_the_swept_brushes_widened_by_lift_and_one_spacing`, `kinematic_mover_light_poses_*`, `fgd_kinematic_mover_declares_spin_properties`. Mid poses are placed but inert until task 3 (dead-code warnings on the domain until then) |
| 2 | **Epoch-5 format.** Wire layout above in level-format: types, `to_bytes`/`from_bytes`, assemble, streamed rows. Loader carries knots to mover runtime data beside the ramp. Every named refusal. AC 32. | integrating executor (delegated worker) | 1 | done: format 26, loader 43, compiler `mover` 137, renderer `mover` 107 (GPU required) passing; `mover_probe_channel_prl_parity` and placement/setpiece/bounce/cache/depth_cap integration passing. Refusal tests in `level-format/.../mover_probe_channel/tests.rs` (`a_mover_record_with_fewer_than_two_poses_is_refused_by_name`, `knots_*`, `a_bounce_entry_naming_a_pose_past_its_movers_count_*`, `a_probe_marked_both_stored_and_base_*`, `a_base_mark_at_a_terminus_*`, `placements_are_read_per_block_and_any_other_epoch_is_refused_by_name`) and loader. Compiler emits N = 2; renderer composes pose 0 and N−1 until task 4 |
| 3 | **Bake, first slice (high, no skip).** Mid-pose direct q at every reached probe, with a light gated when any pose blocks it. Mid-pose bounce one and depth over the domain. Termini's posed bounce one over the domain, and later shares blended to each mid pose. Enclosing rule at every pose. Interim memo: today's one key per mover, folding every pose, so warm still equals cold. ACs 7, 8 (high half), 9, 12, 13, 14. | integrating executor (delegated worker) | 1, 2 | done except AC 7's sweeper half (owner question below). `--bin prl-build mover` 143 passing; `mover_probe_channel_prl_parity` 3/3 against the parent compiler (`light_poses 2` packs byte-identical; every terminus state kept); AC 8 `mid_pose_tests`, AC 9 `a_mid_pose_gathers_its_domain_alone_and_direct_reaches_past_it`, AC 12 enclosing test over every pose, AC 13 `a_light_only_a_mid_pose_blocks_is_gated_at_that_pose_alone`, AC 14 `a_ramped_movers_squeezed_out_blocking_pose_still_bakes_and_its_section_loads` + `tests/mover_probe_channel_poses.rs`. AC 7: `the_rising_rooms_mid_pose_matches_its_static_brush_reference` passes; `the_sweepers_mid_pose_matches_its_static_brush_reference` misses B7's 3-level bounce-one bar by up to 4.69 levels at 34 of 304 domain probes |
| 4 | **Compose, first slice.** Renderer install expands knots through the ramp, with squeezed knots carrying the end states. Piecewise bounce in `sh_compose.wgsl`/`mover_bounce.wgsl` and direct in `mover_channel.wgsl`. Depth on the CPU with both candidates behind a test-only switch. ACs 15, 16, 17 (and 2's compose half). | integrating executor (delegated worker) | 2 | done: knot block and per-pose tables fold into binding 25 (no new binding); interval lookup once per (entry, probe); switch `POSTRETRO_MOVER_DEPTH_BLEND=lerp\|through-base` (default through-base). Renderer `mover` 122 and full renderer 1372 passing with `POSTRETRO_REQUIRE_GPU=1`; tests `knots_*`, `piecewise_*`, `two_posed_*`, `a_ramped_movers_*` (AC 14 compose half, 15, 16, 17), `termini_to_termini_depth_runs_through_the_base_under_either_rule` (AC 18 compose, AC 2 depth), `mid_pose_closed_pose_tiles_are_ledgered_and_leave_with_the_level` (folded into `tile_bytes_and_held_closed_pose_tiles_are_ledgered_and_leave_with_the_level` at task 10) (AC 31). Also fixed task 2's stale `ramps:` capture fixture in `postretro/src/capture/driver.rs` |
| 5 | **First-slice capture — owner gate.** Build the setpiece and enemy-movers at `--release`. Capture the roof (12d metric), sweeper faces and room floor (12c metric) at the brief's t′, as plain lerp against per-interval through-base depth, beside today's and stripped frames. Run AC 19's test, today's build failing and this one passing. Report bytes and bake times. **Stop: owner picks the depth rule (M1), and the Decision is restated in the brief.** | integrating executor (delegated worker) | 3, 4 | captured, awaiting owner: `captures/t5-results.md` and sheets. Residue gone on the roof (t′ 0.30–0.60 within 2.1 levels; today 6.9–17.8) and the sweeper west face (within 1; today 14.5–17.4 with pits). New: interior mean luma pulses with the pose spacing (`t5-luma.png`), east face t′ 0.15 pits, roof t′ 0.65 still 7.6 dark. AC 19 (`mover_mid_pose_compose_tests.rs`, ignored) fails all four groups for the new build (bounce 3.57–7.39, sweeper t′ 0.25 direct 15.84) and far worse for today's (11.09–65.06). Depth rules nearly identical |
| 5b | **Diagnosis of first-slice artifacts** (owner, 2026-11-10). `captures/t5b-results.md`. The interior pulse, the sweeper trailing-face leak and a leading-face brightening are one cause: the enclosing rule at mid poses (q 255 and Base for a probe layer the room's floor slab or the sweeper slab encloses at a pose, lerped against dark neighbours). `POSTRETRO_DIAG_MID_ENCLOSED=blend` removes the pulse (interior monotone, t′ 0.55 luma 16.79 → 3.31) and cuts AC 19 sweeper t′ 0.25 to 6.13 / 9.03. East-face t′ 0.15 pits match the static reference (stripped is not the truth there). No cheap later-share source passes every group. Diagnostic switches (`POSTRETRO_DIAG_*`, in `mid_pose.rs` and `direct.rs`) stay until the owner's ruling, then go | integrating executor (delegated worker) | 5 | done; owner rulings pending |
| 6 | **Depth rule lands.** Remove the switch and keep per-interval through-base; two-terminus intervals stay through-base. AC 18's compose half. | integrating executor | 5b | done: `POSTRETRO_MOVER_DEPTH_BLEND`, `DepthBlend` and `set_blend` removed; `depth_blend::segment` always D_lo → base → D_hi; `piecewise_depth_reads_each_stored_pose_exactly_and_blends_each_interval_through_the_base`, `termini_to_termini_depth_runs_through_the_base` and the two-mover and ramp depth tests assert through-base only |
| 6a | **Enclosed mid poses blend across.** Format: per-(cell, mover) across marks (pin bytes, refusals). Compiler: an enclosed mid pose marks bounce Unmarked and direct across, keeps the dark depth tile. Renderer: direct interval skips across poses. Remove every `POSTRETRO_DIAG_*` switch. AC 12 restated; recapture the interior luma series. | integrating executor | 6 | done: bytes pinned in the brief (block present only when some N > 2, so `light_poses 2` stays byte-identical to the parent: parity 3/3); 9 named refusals, one test each in `level-format/.../mover_probe_channel/tests.rs`; AC 12 `an_enclosing_pose_stores_no_bounce_state_and_keeps_its_dark_depth`, `a_probe_a_mid_pose_encloses_stores_q_255_and_is_marked_across`, GPU `direct_blends_across_a_pose_the_across_marks_name` (mutation-checked); memo versions bounce 8, direct 7; diag switches and `mover_mid_pose_diag_tests.rs` gone. Gates: fmt, workspace clippy `-D warnings`, format 37, loader 43, compiler 144 (+6 ignored), renderer GPU 123 (+5 ignored). Measured (`captures/t6a-results.md`, `t6a-luma.png`): id 57 +2,040 B setpiece, +792 B enemy-movers, rest byte-identical; interior luma peak 16.79 → 3.31, still 1–2.5 above reference mid-travel (later shares, 6b); AC 19 direct 0.00 at every held group, bounce 4.19 / 3.57 / 6.04 levels (6b); sweeper west face no pits. Open: knot dips of 0.2–0.5 luma at t′ 0.2, 0.4 undiagnosed; high-mask-word path CPU-tested only |
| 6b | **Mid poses trace their own later bounces.** Mover-absent per-bounce patch radiance once per build (its own memo on id 34's volume key); each mid pose iterates its domain's patches with rays leaving it reading that radiance. Replace the later-share machinery. Measure the cost (setpiece, enemy-movers; hallway projected). AC 8 restated, AC 19, AC 7. | integrating executor | 6a | done: `absent_bounces.rs` (`AbsentLaterBounces`, replaces `StaticTail`), `absent_memo.rs` (stage `mover_absent_bounces` v1, keyed on id 34's `sh_base_volume` key; payload id 34's unscaled later bounces per static patch, 48 B/patch), `near_field_bounce.rs`; `add_posed_later_bounces` takes `LaterReach::{Everywhere, NearField}`; bounce one posed everywhere, later bounces posed over domain static patches + posed sides, out-of-domain patches carry id 34's bounce k; share machinery deleted; `mover_probe_bounce` v9. Tests: `later_bounce_tests.rs` (domain = every patch is terminus bit for bit at 1..5 bounces; `light_bounces 1` = id 34 later bit for bit; outside = id 34, inside posed). AC 19 now passes (bounce 1.07 / 1.78 / 1.80, direct 0.00) and is un-ignored; AC 7/8 reference tests pass (room bounce one 0.87, full 1.92; sweeper 2.30 / 2.83 with the exception's 40 probes set aside). Gates: fmt, workspace clippy, compiler 148 (+4 ignored), integration bounce 6, cache 10, poses 2, setpiece 2, parity 3/3, references 2/2 `--ignored`. Cost (`captures/t6b-results.md`): cold setpiece 20.4 → 23.6 s, enemy-movers 6.8 → 7.7 s; mid pose ~0.25–0.33 s; absent pass free cold (kept from id 34's pass), 0.6 s on a memo miss; warm 0.94 s, room edit 4.3 s; hallway projected ~0.3 s per door mid pose, ~2.5 s per 50 m lift mid pose, absent memo ~84 MB. Interior bounce within 0.33 / 0.53 levels at knots; between knots at t′ 0.3 direct differs from reference by up to 49.6 levels at one interior probe (piecewise direct lerp, pose spacing). Open: §Build Cache line for the new memo at landing; knot dips undiagnosed |
| 7 | **Redundant mid-pose skipping and cross-pose reductions.** Per-pose raw states, with `reduce_poses` and skipping run after every state resolves. Bars are tested against what compose reads after the ramp and after earlier skips; the depth floor is judged against the interval compose will read. ACs 10, 11, 18. | integrating executor | 6b | done: `skip.rs` (`skip_redundant_mid_poses`), run once per build in `mover_bounce_stage::skip_mid_poses` after every memo read and before block layout; farthest-reach per probe from each stored or base-reading knot, so every skipped pose is judged against the interval compose reads once all skips apply; squeezed-out poses always skip. The knot and interval rule moved to `level-format/.../knots.rs` and is shared by the renderer, its depth blend and the compiler's CPU compose. Bounce memo unchanged (its key holds knots, never the ramp, so a ramp edit hits and re-decides). `reduce_poses` stays before the per-mover memo (ramp-free, per mover); moving it behind per-pose memos is task 8. Tests: `skip_tests.rs` (AC 10 ×4, AC 11 ×2, AC 18), cache `a_ramp_edit_re_decides_skips_over_the_memoized_states_and_warm_equals_cold`; renderer AC 18 half `termini_to_termini_depth_runs_through_the_base`. Gates: fmt, clippy, format 37, compiler 155 (+4 ignored), renderer GPU 123, cache 11, parity 3/3, references 2/2, AC 19 unchanged. `captures/t7-results.md`: id 57 setpiece 11.29 → 9.14 MB (−19%), enemy-movers 4.60 → 3.19 MB (−30.5%); closed-pose tiles 10,013 → 6,651 and 4,638 → 2,084; bake time unchanged (skip pass 0.04 s); two cold builds byte-identical. Open: skip counts not yet in the Build Summary (task 9) |
| 8 | **Memos per pose.** Full-state memo per (mover, pose), keyed by effective bounce count. Terminus bounce-one-over-domain memo. Mid-pose memo free of fidelity. Bump `mover_probe_bounce`'s stage version. Direct memo per light folds every pose. ACs 25, 26, 27 (cache half), 23. | integrating executor | 7 | done: `pose_memo.rs`, `pose_payload.rs`, `pose_lookup.rs` replace `bounce_memo.rs` and `state_memo.rs`. `mover_probe_bounce` v10, one entry per (mover, pose) of raw per-pose states, keyed on id 34's volume key and bar constants, then pose kind, effective bounce count, posed solid and side albedos, and for a mid pose its domain probe and static-patch lists. Never keyed on knot, pose index or count, ramp, fidelity or mover id. `reduce_poses` and skipping run after every pose resolves. The terminus bounce-one-over-domain memo is moot (since 6b a terminus never reads the domain), so it was not added. The direct memo already folds every pose (v7). For task 9: a `low` build probes `key(high)`, then `key(min(high, 1))`, and each pose slot must carry its own bake count. Tests: AC 25 `a_light_poses_change_keeps_both_termini_and_bakes_only_the_new_poses`; AC 26 `moving_a_middle_waypoint_changes_the_domain_keeps_the_termini_and_equals_cold`; AC 27 cache half `a_light_bounces_change_rebakes_every_pose_of_that_mover_and_warm_equals_cold`; AC 23 `an_unedited_warm_build_hits_every_pose_and_equals_cold`; key and payload unit tests. Gates: fmt, clippy, compiler 156 (+4 ignored), integration pose_memo 4, cache 11, depth_cap 2, bounce 6, parity 3/3. PRLs byte-identical to task 7's. `captures/t8-results.md`: setpiece `light_poses` edit 7.46 → 2.98 s, middle-waypoint move 5.53 → 2.60 s, warm no edit 0.99 → 1.10 s. Bounce memo on disk 8.6 → 19.1 MiB (setpiece), 3.9 → 14.0 MiB (enemy-movers). `build_pipeline.md` §Build Cache and step 12 memo lines updated |
| 9 | **Bake fidelity.** `--bake-fidelity` arg, usage error beside `--release`, quality flag. TUI field and the `no_cache` fix. Fidelity and release intent as explicit stage inputs. A `low` build probes the `high` key first. Build Summary pose count, fidelity, approximate marking and projected time (`sh_base_bake_time` entry). `dist` refuses the flag in a recipe. ACs 20, 21, 22, 24, 27, 29, 33 (dist half). | integrating executor | 8 | done: `bake_fidelity.rs` (`BakeFidelity`, `BakeMode::{Interactive, Release}` read from flags only); `--release` beside `--bake-fidelity` is refused at parse naming both; jobs carry `high_bounces` and `bounces`, and each `PoseSlot` its resolved count; on an id-34 hit a pose probes `key(high)` then `key(bounces)`; `sh_base_bake_time` entry (`surface_radiance/bake_time.rs`); Build Summary block (`pipeline/mover_fidelity_summary.rs`) with pose count, fidelity, approximate marking, `--release` estimate and task 7's skip counts; TUI field, Production only from `release`; `manifest.rs` owns `--bake-fidelity`. Tests: `tests/mover_probe_channel_fidelity.rs` (AC 20, 21, 23, 24, 27 low half), `main.rs` arg tests (AC 21), `tui_config` at `apply_outcome` (AC 22), summary tests (AC 29), `rejects_reserved_bake_arguments` (AC 33 dist half); AC 27 high half moved to `--bake-fidelity high`; parity and bounce integration now build `--release`. Gates: fmt, clippy, compiler 158 (+4 ignored), fidelity 6, pose_memo 4, cache 11, tool 163, parity 3/3, full compiler `--tests` 2,819. `captures/t9-results.md`: setpiece cold low 16.95 s / high 23.17 s, enemy-movers 3.65 / 7.87; every warm and served build byte-identical to its fidelity's cold or `--release`. Low − high at rest (feeds M2): closet door t′ 0 +10.75 levels, setpiece wall −11.23 / +11.42, room −7.84 / +4.21 (`tests/mover_probe_channel_fidelity_rest.rs`, ignored). Owner question: the ×2 projection undercounts mid poses (setpiece 2.2 s per mover projected, ~6.6 s measured gap) |
| 10 | **Gate report, memory gate and ledger.** Mid-pose-only gated-light count per mover. SH base gate prices every pose, with termini bounce-one layers over the domain. Closed-pose host row per stored pose. ACs 13 (report half), 30, 31. | integrating executor | 3 | done: `mover_light_gate/mid_pose_only.rs` and `mover_probe_channel/mid_pose_only.rs` count, per mover, lights gated at a mid pose and at neither terminus; one Build Summary line and a `-v` line per such mover; per-terminus lines and summary diff empty against `prl-build-83df` on all three maps (count 0 on every mover there; non-zero path by the crossing-slab test). "Termini bounce-one layers over the domain" is stale since 6b; the gate prices the per-pose raw states held one mover at a time, the mover-absent later bounces and the near-field patches, and fidelity never enters the price. Setpiece: priced SH peak 124.5 MiB against 100.0 / 102.5 MiB RSS (low / high) in the stage. Ledger host row 6,319,200 B (6,651 tiles) on setpiece, zero after unload. Tests: AC 13 `a_light_only_a_mid_pose_blocks_is_gated_at_that_pose_alone` plus 3 unit tests; AC 30 `a_budget_under_the_mover_priced_peak_refuses_before_any_bounce_ray` at low and `--release`, `the_sh_base_gate_covers_every_poses_bake_on_the_setpiece_at_either_fidelity`; AC 31 `tile_bytes_and_held_closed_pose_tiles_are_ledgered_and_leave_with_the_level` (now multi-pose), `--ignored` compiled-map `every_stored_poses_closed_pose_tile_is_ledgered_on_a_compiled_map_and_leaves_with_it`. Gates: fmt, clippy, compiler 162 (+4), renderer GPU 122 (+6), loader 52, fidelity 6, setpiece 2, parity 3/3. `captures/t10-results.md` |
| 9b | **Projection counts mid poses (owner, 2026-11-11).** Build Summary `--release` estimate = id 34's recorded time × 2 + Σ over mid poses of id 34's time × (domain static patches / all static patches). Check against measured high − low on setpiece and enemy-movers. AC 29 restated. | integrating executor | 10 | done: `pose_lookup.rs` carries each mover's domain and total static patch counts (point-in-box over patch centres, the mid-pose bake's own set; from the memo key on cached builds); `mover_fidelity_summary.rs` estimate T × (2 + mid poses × share). Estimate vs measured `--release` − `low` sums: setpiece 6.6 / 6.78 s, 6.7 / 6.81 s; enemy-movers 4.6 / 4.63 s, 4.2 / 4.38 s; per mover within ~0.7 s. AC 29 tests updated plus a `--release` vs cache-filled `high` share check |
| 11 | **Regression and rest gates.** Generalise the C10 harness to N movers. Rest-unchanged comparison on all three maps against fresh pre-change PRLs. K1–K3, C7, C9 renderer half, and C10 on `--release`. `light_poses 2` regression. Low-vs-high gate with the bound from M2. ACs 1, 2, 3, 19, 28. | integrating executor | 9, 10 | done except one capture gate (owner): `captures/t11-results.md`. AC 1: `rest_channel_output_equals_the_pre_change_build` (`MOVER_REST_PAIRS`), bit for bit in both modes against the parent compiler (the loader refuses 83df's epoch 4; 83df vs parent differ only in id 57 and 56 frame pairs at 0 pixels); end to end 23 of 28 frame pairs identical, 5 at 1–2 px by 1 level (hypothesis: taller depth-tile texture changes filtering; unconfirmed). AC 2: parity 3/3 plus 0-pixel frames against the pre-brief engine at t′ 0, 0.25, 0.5, 1. AC 3: K1, K2, K3, C7, per-probe C10 (N movers, mutation-checked), C9 both halves pass on `--release`; **frame-level `c10_streaming_off_and_sync_proof_compose_the_same_channel_output` fails** at setpiece station 3, mask 8, both movers at 0.5: mode gap 21 levels against bar 19 (pre-brief and parent 18; all builds 1 under `--uncompressed-irradiance`), the BC6H mode difference of `drafts/sh-streaming-mode-bc6h-parity.md`. AC 19 pass (two-pose misses 11–20 bounce, 23–59 direct). AC 28 gate at 12 levels: worst 11.42. Gates: fmt, clippy (incl. `--features capture`), compiler 162, renderer GPU 122 + 5 ignored gates, full compiler `--tests` 2,823 |
| 12 | **Docs.** `docs/level_design.md`: mover lighting table, error rows, Common options, and rewritten :523 and :533. `docs/distribution.md` refused args. FGD help. AC 33 (docs half). The "not built yet" flips in `context/lib/` happen at landing. | integrating executor | 9 | done: `docs/level_design.md` (Common options `--bake-fidelity` and `--release` rows; Movers → Lighting poses; `light_poses` key row and "How many poses"; ramp and bake-cost passages rewritten; "Bake fidelity"; "What the Build Summary shows"; five Compilation Errors rows) and `docs/distribution.md` (refused args). FGD help unchanged: it already describes `light_poses` and is pinned by `fgd_kinematic_mover_declares_spin_properties`. Gates: `coloured_bounce_source_gates` 4/4, FGD property 1/1, `help_and_context_docs_no_longer_describe_approximate_warm_sh` 1/1, `rejects_reserved_bake_arguments` 1/1. At landing: `build_pipeline.md` §CLI flags `--bake-fidelity` row (AC 33's second flag table) and `context/lib/index.md` router keys (`light_poses`, `--bake-fidelity`) |

| 13 | **Geometry knots and the knot count (riskiest premise: counts stay sane).**<br>• Exact enclosure intervals per (mover, domain probe) along the path, from the bake's own enclosing rule (convex brushes, piecewise-linear path, ramp).<br>• Outside and inside knots at each end; merge tolerance pinned below the failing slack; union with even poses behind one placement chokepoint.<br>• Authored `light_poses` places none.<br>• Build Summary prints even and geometry knot counts.<br>• Measure counts on closet, setpiece, enemy-movers and the stress hallway (placement only).<br>• **Stop for the owner** if a realistic mover's count makes bake or bytes blow up: sparse storage then joins.<br>ACs 4, 5, 34, 37 (count half). | integrating executor | — | done:
• Code: `mover_light_gate/geometry_knots.rs` (exact per-leg enclosure intervals, inside and outside windows, minimal merge, each knot verified against `Pose::encloses`), `travel.rs` (`PathTravel`), `place_poses` as the chokepoint.
• Ordering: the probe grid is laid out before placement (`pipeline.rs`); it casts no rays.
• Constants: outside 4.4/39.37 m, inside margin 0.01 m, merge 0.025 m, side guard 1 mm.
• Build Summary: `N pose(s) (E even, G geometry)`.
• Tests: `geometry_knot_tests.rs` (AC 34 ×5 cases plus seam, mutation-checked), AC 37 SH-gate pricing in `mover_bounce_stage_tests.rs`, `knot_counts_on_the_reference_maps` (ignored).
• Counts:
  - closet door 4 = 4 + 0;
  - setpiece wall 57 = 15 + 42, room 32 = 6 + 26;
  - enemy-movers lift 11, sweeper 25, crusher_open 21, crusher_wall 32;
  - hallway doors 18–28, warren lifts 137 and 139 (5 brushes).
• Hallway projection: about +150 s on a cold bake of more than an hour (lifts 0.48 s per mid pose at measured domains; +645 s at t6b's assumed 2.5 s).
• Bakes, `--release` cold:
  - setpiece 24.4 → 42.7 s; id 57 9.14 → 18.31 MB (depth 2.68 → 10.82 MB, direct 2.39 → 3.00, bounce 4.08 → 4.49); closed-pose tiles 6,651 → 26,979;
  - enemy-movers 8.3 → 12.6 s; id 57 3.19 → 4.53 MB.
• Verdict (executor): bake time and value bytes stay sane, so no stop. Depth bytes are the open cost: skips are still judged against the through-base blend, so re-measure after task 14 before deciding task 16.
• Gates: fmt, compiler clippy `-D warnings`, `--bin prl-build mover` 185 (+5 ignored), every `tests/mover_*`, parity 3/3. |
| 14 | **Depth blend.**<br>• Mean and variance in level-format beside `depth_leg`, one compose function given the interval's end poses.<br>• Renderer `depth_blend.rs` and compiler `skip.rs` call it.<br>• Termini ↔ termini stays raw through-base.<br>• id-57 epoch 6, since the depth rule changes meaning; C9 expectation updated.<br>ACs 15, 18, 35. | integrating executor | — | done, with two open items:
• Code: `level-format` `knots.rs` `depth_segment` and `mix_depth_moments` replace `depth_leg`. Termini ↔ termini stays raw through-base, bit for bit; every other interval mixes mean and variance, rebuilding E[d²] from the f16-rounded mean, clamping σ² at 0 and capping at 65504.
• Callers: renderer `depth_blend.rs` and compiler `skip.rs` call it.
• Format: id 57 epoch 6, with epochs 4 and 5 refused. Parity rewrites the parent's epoch byte and compares every other byte (3/3).
• Tests: C9's renderer half accepts the variance mix. `depth_mix_tests.rs` and `depth_blend_tests.rs` are new; AC 18's skip case now keeps pose 3. Mutation-checked.
• Bytes, `--release`:
  - setpiece id 57 18.31 → 11.39 MB (depth 10.82 → 3.90 MB), closed-pose tiles 26,979 → 9,695;
  - enemy-movers 4.53 → 4.35 MB.
• Gates: fmt, workspace clippy, format 49, loader 52, compiler mover 186, `tests/mover_*`, renderer GPU 124, parity, AC 19.
• **Open, CPU cost:** live depth tiles cost 0.53 → 0.70 ms per frame on the setpiece room and 0.26 → 0.29 on the wall, against M7's no-measurable-cost rule (task 14b).
• **Open, AC 7:** the sweeper reference fails, bounce one 4.01 levels at mid pose 12 of 25 (bar 3). The tested pose moved with task 13's knots; undiagnosed (task 14b). |
| 14b | **Task 14's open items.** (a) Diagnose the AC 7 sweeper reference failure at the new mid pose: patch-grid exception, or a new error at outside knots? (b) Bring the mean-and-variance CPU mix to the old rule's cost on the setpiece room, or report the floor reached and the levers left. | integrating executor | 14 | done:
• **(b) CPU cost.** Rebaked setpiece, per frame: room 0.53 (old rule) → 0.70 (task 14) → 0.36 ms; wall 0.26 → 0.29 → 0.18 ms.
  - How: CPU-held tiles stored as moment planes (means, then second moments; same bytes, same ledger); one per-tile `mix_depth_tile` in level-format, shared by the renderer and skip; branch-free f16 rounding and packing, so the loops vectorise.
  - Test: `the_tile_blend_is_the_texel_blend_texel_for_texel`, bit for bit.
• **(a) AC 7 sweeper.** Probe 2267 reads 4.01 levels at pose 12, an even pose; the test's `len/2` pick moved from t 4/7 to t 3/7 with task 13's knots. The probe sits 0.91 m ahead of the leading face.
  - Cause: the reference's static slab cuts the floor along its off-grid north end-face plane, extended 2+ m past the slab, into 0.060 and 0.190 m² slivers, while the posed bake keeps whole patches. The reference is the outlier, beyond the exception's "within one patch of a posed face".
  - **Owner question below**; nothing loosened.
• Gates: fmt, workspace clippy, format 50, loader 52, renderer GPU 124, C9 closet, compiler mover 186, `tests/mover_*`, parity 3/3, AC 19 2/2. |
| 15 | **Slice proof.**<br>• Bake setpiece and enemy-movers `--release` with tasks 13 and 14.<br>• GPU motion recapture (`captures/motion-harness/`) beside the pre-amendment build and the 41/57 truth bakes.<br>• Port the offline metric to the AC 36 `--ignored` test.<br>• Measure id-57 bytes, bake times and M7's A/B.<br>• From the data, decide whether inside knots become depth-only (task 16) and whether the GPU needs a per-mover bracket (task 17).<br>ACs 36, M8, M9 data. | integrating executor | 13, 14 | done (`captures/t15-results.md`):
• Bakes, `--release` cold:
  - setpiece 39.1 s, id 57 11.39 MB, 9,695 closed-pose tiles;
  - enemy-movers 13.6 s, 4.35 MB;
  - poses: wall 57, room 32, sweeper 25 (7 + 18: one even knot merged into a geometry knot).
• **Motion, GPU, 966 new frames.** Mean |X − truth| at the 41/57 truth knots, amend vs pre-amendment: ceiling 0.22 vs 4.41, floor 0.11 vs 1.19, interior 0.06 vs 0.40, east 0.58 vs 3.87, west 0.13 vs 0.86.
  - Ceiling knot flash: gone (worst 0.31; was 5–6).
  - Ceiling lattice peaks: median 9 (pre-amendment 24, truth 1).
  - East sawtooth: gone; peak-to-trough tracks the truth's own layer crossings.
  - Residuals: ~2-luma ceiling bumps at t′ 0.125, 0.35 and 0.575, one per layer crossing, in short geometry intervals; east +2.9 and +3.7 early in the trip; −1 to −2 in the long last even interval, shared with the pre-amendment build.
• **AC 36** (`pipeline/mover_motion_proof_tests.rs`, `--ignored`): **fails on HEAD** (pre-amendment fails 7–10× on shares).
  - Shares: mean within bar except east 0.0104 (bar 0.01); worst east 0.329 and west 0.069, single truth knots inside short geometry intervals while a face crosses a layer.
  - Values worst: ceiling 5.92 (bounce, t′ 0.575), floor 4.28 (direct), east 11.98 (direct in the last even interval, same as pre-amendment), west 4.33.
  - Mutation (raw depth on every interval) brings the shares back to pre-amendment levels.
• **M7, 83df re-run interleaved:** wall compose +0.13 ms over pre-brief (non-overlapping ranges, in `mover_bounce.wgsl`); room +0.13 within noise; both flat. Task 17 triggered.
• Task 16 not needed: bytes and bake time are fine.
• Hypothesis for the ceiling bumps and the face share spikes: values lerp across the whole outside-before → outside-after span, so in the 4.4 u window between an inside knot and its outside knot, the probe is out of the slab but still mid-lerp. Task 15b tunes the outside distance. |
| 15b | **Tune the geometry-knot constants.** Outside distance (and inside margin) against AC 36 and GPU motion frames on the ceiling and the sweeper faces: does a smaller outside distance, still clear of the gather-lift band, remove the per-crossing residuals? Then judge AC 36's bars with the owner if misses remain. | integrating executor | 15 | done (`captures/t15b-results.md`):
• **Chosen:** outside distance 4.4 u → **1 u**, plus `OUTSIDE_KNOT_CLEARANCE_M` 3 mm (the 1 mm lift, ~1 mm inset and 1 mm side guard), which caps the outside window short of the band; byte-identical at 4.4 u.
• Sweep (outside distance → room/wall/sweeper poses, setpiece id 57, cold bake, ceiling GPU |Δ truth| mean/max, AC 36 values ceiling/floor/east/west):
  - 4.4 u: 32/57/25, 11.39 MB, 39 s, 0.22/2.18, 5.92/4.28/11.98/4.33;
  - 2 u: 37/71/29, 11.84 MB, 41 s, 0.17/1.46, 4.57/5.77/11.98/2.35;
  - **1 u: 45/71/31, 12.03 MB, 47 s, 0.09/0.45, 4.52/5.33/11.98/1.13, west passes;**
  - 5 mm: 49/71/31, 12.03 MB, 0.08/0.23, little further gain.
• Ceiling bumps confirmed as the outside window, and gone at 1 u.
• Unmoved: east +3.7 luma at t′ 0.161, east direct misses, ceiling faint lattice (12c median 9–10).
• The east share spike at t′ 0.107 grows 0.329 → 0.384 (GPU ~−1 luma).
• **AC 36 at 1 u:** west passes; ceiling, floor and east fail on values (4.52, 5.33, 11.98). East direct misses all sit in long intervals and predate the brief (t′ 0.982: 11.98 vs 12.30 pre-amendment); ceiling bounce 3.3–4.5 vs 2.9–3.7 pre-amendment, one on a stored knot (skip bar). Restatement proposed (owner questions).
• Gates: fmt, clippy, `--bin prl-build mover` 186, `tests/mover_*` (shutter fixture 9 → 8 poses), parity 3/3. |
| 16 | **(Not needed per task 15) (If task 15's bytes or bake time call for it) depth-only inside knots.** A per-pose kind in the mover record; values blend across a depth-only pose for every probe; no direct q or bounce stored there; loader refusals; memos per pose kind; SH gate and Build Summary estimate count it. | integrating executor | 15 | |
| 17 | **(If M7 shows cost) per-mover knot bracket.** One word per mover slot with the bracketing value-knot index, so `mc_interval` and `mover_bounce.wgsl` skip the linear knot loop. M7 A/B re-run. | integrating executor | 15 | done; M7 re-run pending a quiet machine:
• T-block: 2 words per mover slot, t′ then bracket. `MoverKnots::bracket` is a binary search, computed once per mover per frame in `set_states`.
• Shaders: `mc_interval` (`mover_channel.wgsl`) and `mb_entry_blend` (`mover_bounce.wgsl`) start from the bracket; the across and unmarked walks are unchanged. CPU compose and depth `plan()` already binary-search.
• Test: GPU `bracket_tests.rs`, `the_knot_bracket_reads_what_the_linear_knot_scan_read`, bit for bit against the old linear WGSL over 1,000+ slots × 64 probes (70-knot, squeezed, MP1, MP10, degenerate ramp, t′ on and around knots). Mutation-checked in the CPU bracket and the bounce shader.
• Note: Metal flushes subnormal t′, as before; unaffected.
• Gates: fmt, workspace clippy (with and without capture), renderer GPU 126, loader 52, C9 on the epoch-6 closet PRL. |
| 19 | **AC 7 and AC 36 per the owner's rulings.** Widen the sweeper reference's set-aside to probes within one patch of a plane along which the reference cuts the floor off the patch grid, and pin it with a check that it catches probe 2267's case. Implement AC 36's restated bars (share mean, room per-knot, faces reported; values within 3 levels or within 1.5 of the pre-amendment miss). Both tests pass `--ignored`. | integrating executor | 18 | done, with one marginal miss for the owner:
• **AC 7/8 sweeper:** passes. The reference's floor cuts are derived from its own static-slab faces: off-grid vertical planes that also edge a floor face beyond the footprint (pose 15 of 31: x −11.379, x −1.626, z −35.299 m). Set aside 40 (posed face, worst 0.18) + 30 (floor cut, worst 4.01, includes probe 2267, asserted). Gated worst: bounce one 1.31, full 2.69, depth within f16. Room unchanged (0.87 / 1.51).
• **AC 36:** the room passes; **the sweeper fails by a hair: east trip-mean share 0.0102 against 0.01**. West passes; the faces' worst knot is reported (east 0.384 at t′ 0.107, west 0.016). The pre-amendment build fails the share bar on all four surfaces, asserted (0.08–0.10).
  - Knots passing only by the 1.5 clause (follow-up input): ceiling t′ 0.575–0.700 (3.3–4.5); floor 0.375, 0.725, 0.825; east 0.036, 0.161, 0.179, 0.768–0.839, 0.911–0.982 (up to 11.98 vs 12.30).
  - Reading: the clause applies at any knot, whether or not the pre-amendment build exceeds 3 there; ceiling t′ 0.600–0.700 pass only by it.
• **Mutations:** without the floor-cut rule the sweeper reference fails (4.01); without the 1.5 clause the room motion test fails on 9 knots.
• Gates: fmt, compiler clippy `-D warnings`. |
| 20 | **AC 36 time-weighted share (owner, 2026-11-12).** Bake the sweeper truth at `light_poses 113`; integrate the far-side share difference over travel (time-weighted) for the trip mean; keep the room's per-knot gate and the faces' reported worst knot; keep the 1.5-level value clause. The sweeper and room tests pass `--ignored`; the pre-amendment candidate still fails. | integrating executor | 19 | done; one room finding under GPU check:
• The trip mean is a trapezoid integral over the truth's knot t′ on every surface (the unweighted mean is printed beside it); the room keeps its per-knot gate; the faces report their worst knot; the values clause is unchanged; the pre-amendment candidate fails the share bar on all four surfaces (asserted).
• **Sweeper at truth 113:** passes. East 0.0095 (0.0104 time-weighted at 57), west 0.0050 (0.0037); worst knot reported east 0.384, west 0.149. 4 of 30 shipped intervals still hold no truth knot (12 at 57).
• **Room at truth 41:** passes (ceiling 0.0018, floor 0.0002; worst 0.0176, 0.0024).
• **Room at truth 78 (measurement only):** means pass (0.0074, 0.0069), but the per-knot gate fails (ceiling 0.44 at t′ 0.117, floor 0.52 at t′ 0.805), and floor values fail at t′ 0.831 (7.10 vs 5.35 pre-amendment). These are single-knot spikes inside 1 u crossing windows that 41 cannot sample.
• **GPU check of those windows (`captures/t20-*`):** against a 78-pose truth baked and rendered at HEAD (the epoch-5 truth reads 3–4 luma off between its knots under through-base depth; the two agree within 0.01 luma at knots). The room moves at 1 m/s, so one 60 fps frame is 0.0037 t′.
  - **Ceiling, t′ 0.0975–0.1375:** HEAD crop mean ≤ 0.80 luma, worst block ≤ 2.0, at the spike knot 0.1169; not visible. Pre-amendment reads 6–15 mean and 14–28 block on every sample.
  - **Floor, t′ 0.785–0.85:** HEAD crop mean ≤ 0.8. One local patch drifts smoothly over the long 0.806–0.892 interval: worst block 10.4 at 0.831, over 5 for about 110 ms (6–7 frames). Pre-amendment shows the same patch at 9.9, over 5 for 200+ ms. This is the known long-interval direct-interpolation miss (the landing follow-up), not a flash.
• **Open owner question:** the room's truth density (below). |
| 21 | **C10 per-probe split (owner, 2026-11-12).** Run the per-probe case on `--uncompressed-irradiance` builds, bit for bit in every class, and on BC6H builds within a derived, pinned bound (two f16 roundings of BC6H-differing bases weighted by w). Both pass on the setpiece and closet; mutation-check the derived bound. | integrating executor | 19 | done (`renderer_tests/mover_channel_modes.rs`):
• `c10_uncompressed_builds_compose_bit_for_bit_in_both_modes` (`MOVER_C10_UNCOMPRESSED_PRLS`): bit for bit on closet, setpiece and enemy-movers.
• `c10_off_and_sync_proof_compose_the_same_channel_tiles_at_every_probe` (BC6H) is judged against the derived `mode_gap_bound`.
  - Derivation: O = a + c·a₀ is mode-invariant in exact arithmetic, so the gap is (e_a − e_b) + c·(e_a₀ − e_b₀). WGSL's f32→f16 picks either neighbour (under 1 ulp each). One store rule means same-sign pairs err alike, so |e_a − e_b| < max ulp. The bound is under 3 steps, and at most 2 where |c| is a power of two.
  - Round-to-nearest alone fails enemy-movers probe 1255 (1.81 vs 1.59 ulp); round toward zero fits.
  - Worst fraction of the bound: setpiece 0.968, enemy-movers 0.927, closet 0.660.
• Mutations: the bound at 0.9× fails (1.074); one f16 step injected fails (1.90).
• The frame-level C10 is unchanged and passes (21 vs 22).
• Gates: fmt, clippy (with and without capture). |
| 18 | **Regressions, docs and IP.**<br>• Re-run AC 1, 2, 3 (C9, C10), 7, 8, 19 and 28, plus parity.<br>• `ip-372` question 9 wording.<br>• `docs/level_design.md` and the FGD: `light_poses` places even poses only, geometry knots are automatic.<br>• `build_pipeline.md` and `rendering_pipeline.md` updates queued for landing. | integrating executor | 15, 16, 17 | done, with one owner question:
• **Docs and IP:** `ip-372` question 9 rewritten; `docs/level_design.md` (`light_poses` table row, extra poses, a set count places even poses only, Build Summary split); FGD help and its pinned test.
• **Leak fix:** four fixtures held a `TempDir` in a `static OnceLock` (bounce `closet`/`setpiece`, setpiece `setpiece`, `mover_cache_support` `base`/`high_base` plus fidelity `low_cold`/`release`, light-gate cache `base`), leaking 566 dirs (~25 GB) since Nov 10. Now `fixture_dir` under `$CARGO_TARGET_TMPDIR/<binary>/<name>`, recreated at init (`prl_build_support`). The leaked dirs are left for the owner.
• **C10 harness:** `compile_release_using` rewrites the parent's epoch byte to 6, as task 14 did for parity.
• **Gates:**
  - AC 1: pass, bit for bit, 3 maps, both modes.
  - AC 2: parity 3/3.
  - AC 3: K1, K2, D2, K3, C7, C9 both halves and C10 frames pass (setpiece st3 mask 8: 21 vs 22). **C10 per-probe fails**: 'one mover reading id 34' class at t′ 0.5, setpiece, 1.28 f16 steps against a 1-step bar. Bit for bit under `--uncompressed-irradiance`, so it is the BC6H base difference across two f16 roundings; it surfaced as base-reading pairs grew with geometry knots. Owner question.
  - AC 7/8: room passes (0.87 / 1.51); sweeper fails at probe 2267 (bounce one 4.01, full 3.51), task 19.
  - AC 19: pass (1.72; 2.06; 1.94).
  - AC 28: pass (11.42 ≤ 12).
  - AC 36: fails as before, task 19.
• **Preflight:** fmt, clippy (workspace and capture), `cargo test --workspace` GPU 15,849 passed, 1 failed (`line_c_names_staged_marks_and_adds_up`, a timing flake under suite load, passes 3/3 alone, untouched by this branch), 133 ignored; release check; crate graph. |

Execution is sequential on the main checkout's warm `target/`. Every task crosses the compiler ↔ format ↔ renderer seam, so worktree workers would each build the engine cold for little parallelism. Tasks 3 and 4 touch disjoint crates once task 2 lands, and may run as one worktree worker if disk stays above 15 GB plus a debug build.

Hot paths: compose is still one dispatch per moved row per pass. The interval lookup runs once per (entry, probe), never per texel, from install-expanded tables. Live-tile writes allocate nothing per frame. The 0.2 ms budget is proved manually (M7).

## Review panel, round 1 (2026-11-11, read-only; fixes wait for a free machine)

Panel: 7 depth (opus: 2 contract verifiers, 4 correctness tracers, 2 adversarial testers — format, renderer) + 4 hygiene (sonnet) over 83dfb5317..eda72b376. Verdict: request changes. No IP-rule findings.

Fix list:
- 🔴 `level-format` `streamed.rs:56` `expect("one cell's payload fits u32")` is reachable from a ~290 KB crafted PRL (N = 65,536, 1,024 streamed records); refuse a cell's streamed direct or bounce row over u32 with `SizeOverflow` at decode; test a large-N streamed cell and N = u32::MAX.
- 🟡 Bake: tile-range stop judged before skipping but the failure check after (`bake.rs:260-280`, `mover_bounce_stage.rs:202`); a squeezed-out ramp can silently drop movers past `stopped_at` from id 57. Fail by name whenever the bake stopped or cut a mover.
- 🟡 `light_poses` up to u32::MAX allocates all poses in placement before the SH gate (OOM, not the named error; knots collide past ~2^24). Refuse an over-large count before placement allocates, by name; docs errors table to match. Also bounds the shader's linear knot scan.
- 🟡 Absent memo not marked used on warm builds that don't read it (`surface_radiance/stage.rs:270`): evictable, costing an id-34 later-bounce pass on the next mover edit. Mark it used on every volume hit where some job reads the layers.
- 🟡 Stale contract comments: an enclosed mid pose "reads the base" in `level-format poses.rs:19` (`PoseMark::Base`), `states.rs:199`, `posed_scene.rs:103`, `posed_solid.rs:146`; `level_geometry.rs:100` "two termini"; `gpu.rs:191` "ramps"; `pose_bake.rs:28`; `cost.rs:139`; `path_fraction.rs:2`; `mover_light_gate.rs:1`; `wire/across.rs:3` ("tight"); `mover_probe_channel.rs:195` (q scale).
- 🟢 Mechanical: drop owner/date and AC/MP tags from code comments; "12c"/"today's" wording; `mid_pose.rs:19` reflow; WGSL bounce entry field constants; `depth.rs` shadowed `stated`; renderer `knots.rs` pass-through; `debug_assert` in compiler `across.rs` parts that marked poses hold no bounce state; mid-pose key gates patches on `bounces > 1`; id-34 bake time excludes mover-cache I/O; progress units per slot count; `pose_lookup.rs:66` comment (id-34 miss ignores stored poses); loader accepts −0.0 first knot (compare bits); subnormal ramp span (runtime and `ramped_knot`); GPU test for a probe ≥ 32 at pose ≥ 2; ignore-reason commands; split test files past 800 lines where touched.
- Landing (context/lib): `build_pipeline.md` id-57 paragraph and section-table row rewritten to epoch 5 (direct, bounce, depth, mover records, placement, every epoch-5 load error), step 12 Poses/Bounce/Depth per pose, "too large" `light_poses`, §CLI flags; `rendering_pipeline.md` §4 Poses, Direct/Bounce/Depth compose, Not reached, IP masks wording; skip doc note on id-59 trim.
- Open hypothesis: the 0.2–0.5 luma knot dips are the per-interval through-base depth returning to the base mid-interval (owner rule); check with a capture.
- Pending: C10 frame bar per AC 3 restated (partial edit saved at scratchpad `c10-partial.diff`).

### Round 1 fix pass (2026-11-11, after merging origin/main at 5aa17c2a4)

- 🔴 fixed: streamed id-57 rows past u32 are refused at decode (`StreamedRowOverflow`, `wire.rs` `check_streamed_row_lens`); tests for a ~290 KB crafted direct cell, a streamed bounce row and `pose_count = u32::MAX`.
- 🟡 fixed: a bake the tile range stopped or cut fails by name before any skip (`refuse_cut_short`; `a_bake_cut_short_past_the_tile_range_fails_by_name_before_its_skips`).
- 🟡 fixed: authored `light_poses` past 16,777,217 (knot precision) or whose poses alone pass `--working-set-max-size` is refused by name before placement allocates (`MoverPoses::admit_pose_counts`); docs and step 12 updated.
- 🟡 fixed: the mover-absent memo is marked used on warm volume hits that don't read it (`a_volume_hit_that_reads_no_absent_bounces_still_marks_their_memo_used`).
- 🟡 fixed: stale contract comments (enclosed mid pose blends across; N poses), and the owner/date and plan tags the panel named (the "(owner, date)" pattern elsewhere is a repo-wide convention and stays).
- 🟢 fixed: mid-pose key folds domain patches only past one bounce (`mover_probe_bounce` v11); id-34 bake time excludes mover-cache I/O; progress per slot; `debug_assert` that across marks hold no bounce state; −0.0 first knot refused; subnormal ramp span reads 1 past its end in `ramped_knot` and the runtime's `ramped_travel_fraction`; WGSL bounce field constants; GPU test `direct_blends_across_marked_poses_for_probes_in_the_high_mask_word`.
- C10 frame bar per AC 3 restated: bar = max(stripped gap + 2, parent-build gap + 4); setpiece st3 mask 8 reads 21 against 22 (parent 18); needs `POSTRETRO_MOVER_CHANNEL_PARITY_BASELINE_BIN`.
- Skipped: splitting test files already past 800 lines before this branch (churn on files only grown here); the renderer `knots.rs` pass-through (its tests call it); remaining AC tags in test comments (a repo-wide convention).
- Gates: fmt; clippy `-D warnings` (with and without `--features postretro/capture`); full `cargo test --workspace` (GPU) 15,820 passed, 0 failed; release check; crate graph; parity 3/3; AC 7/8 references 2/2; C10 frames pass.

### Round 2 (review of 5aa17c2a4..25b0e0277, then fixes)

Panel: correctness tracer + adversarial tester (opus) + hygiene (sonnet), read-only. No 🔴.
- 🟡 fixed: `light_poses` admission sums every authored mover's floor (knot plus the placed pose and its posed-solid copy) against `--working-set-max-size` and names every contributor, largest first; parse refuses counts past 16,777,217 citing the reason (one "too large" docs row).
- 🟡 fixed (rule made order-independent): the tile range is judged before skips on the distinct probes the movers carry; a mover exactly one past the range is refused whatever the job order, as before skipping existed. Owner may prefer judging after skips — flagged in the report.
- 🟢 fixed: compiler refuses a streamed id-57 row past u32 by name before packing (`mover_rows.rs`); `light_ramp_start "-0"` stored as +0.0; comment rewraps; `level_geometry.rs` residency list.
- Noted, unchanged: the recorded id-34 time includes layer-cache reads on a volume miss with cached layers (the projection is labelled an estimate).
- Gates: fmt; clippy `-D warnings` (workspace, and `postretro --features capture`); full `cargo test --workspace` (GPU) 15,827 passed, 0 failed; parity 3/3; AC 7/8 references 2/2; release check; crate graph.

### Round 3 (review of 25b0e0277..870d41b40, then fixes)

One reviewer (opus, tracer + adversarial), read-only. No 🔴; round-2 fixes confirmed (parse boundary, ramp −0, order-independent tile-range verdict with no false refusal and warm = cold, compiler row sum = `row_lens`).
- 🟡 fixed: authored `light_poses` are priced per pose from the mover's first terminus as the bake actually allocates it before the SH base gate (knot, placed pose, `PosedSolid` with its sides' geometry, `PosedScene` patches; `cost.rs` `pose_held_bytes`): a unit box costs 4,784 B a pose, so `MAX_POSES` on one is refused at the 16 GiB default. The refusal is worded as the mover poses' own working set, before the SH base gate.
- 🟢 fixed: `finish_bounce_half`'s tile-range refusal documented as an unreachable backstop.
- Gates: fmt; clippy (workspace and `postretro --features capture`); full `cargo test --workspace` (GPU) 15,828 passed, 0 failed; parity 3/3; release check; crate graph. Review loop closed: round 3 raised no new defect class.

## Review panel, round 4 (2026-11-12, over ebb65f313..e2758fd33)

**Panel:** 9 read-only reviewers plus a Fable check of the build against the consensus.
- 3 correctness tracers (placement; depth compose and skip; the knot seam compiler → pixel);
- 1 contract verifier (id 57 epoch 6, `light_poses`, IP wording);
- 2 adversarial testers (placement, GPU bracket);
- 3 hygiene and drift sweeps (compiler, format and renderer, tests).

**Verdict:** request changes. The core is faithful (Fable): interval rule, f16, tile = texel, multi-mover, the skip judging compose, and the bracket ≡ linear scan. The seam and bracket are clean.

**Findings:**
- 🔴 `ip-372` question 9 is not the owner-approved wording (expanded, the Lever 7 sentence dropped), and "raw-linear between termini" misstates the code (raw moments through the probe's static tile). Owner question.
- 🔴 Drift in `context/lib`: `rendering_pipeline.md` §4 and `build_pipeline.md` step 12 describe the old rules. Landing work.
- 🟡 Outside-knot clearance was measured in travel, not along the face normal, so oblique faces can put an outside knot in the lift band or inside the brush. Raised by 3 reviewers.
- 🟡 The sliver fallback places in-band knots near termini and in narrow gaps. Seam join used exact float compares.
- 🟡 Geometry poses were allocated before admission.
- 🟡 Scale was unmeasured at the new counts (skip roughly K², placement roughly legs²).
- 🟡 Fixture-dir cross-process race; harness relabel without a two-pose assert.
- 🟡 Skip against an id-59 tile the trim could drop (predates this branch).
- 🟡 The cap warning hid that an authored count turns geometry knots off; "extra" vs "geometry" naming.
- 🟢 A duplicated `F16_MAX`, the WGSL stride literal, the bounce bracket parameter, a missing construction assert, ignore-reason commands, test names, `copy_dir`, and about a dozen comments narrating removed code or citing plan paths, dates or "epoch-5 layout".

### Fix pass A (compiler)
**Geometry knots**
- Interval ends carry the binding face's rate, so the margin, outside distance and guard are measured along the face normal.
- Outside knots stay out of a band span (brushes grown by lift + inset + 1 mm) and are verified by `clear_of_band`, failing by name.
- A clear side with no room is held by its terminus, and gaps that never leave the band join; the slivers are gone.
- The seam join goes through the band, so a few-ulp gap on a sloped seam joins.
- Each (leg, brush) is prefiltered by its swept hull.
- New tests: diagonal slab, bevelled face, diagonal two-wedge seam, faces 1.5/2.5 mm off a layer at both termini, brushes 2 mm apart. The band invariant is checked on every knot. Mutation: reverting the rate scaling fails the diagonal test, and the old placement fails 4 of 5.

**Admission:** `admit_placed_poses` runs after placement and before resolve, over every mover's final count, naming the movers largest first. Tested.

**Scale**
- Counts: hallway doors 18–23, warren lifts 140 and 142; others unchanged.
- `skip_marks` walks farthest-first and stops at the first holding end, proven identical to the old walk over 3,000 random cases (3–140 poses, ramps).
- Setpiece skip: wall 0.48 → 0.10 s, room 0.74 → 0.17 s.
- At 140 poses × 8k probe-states: linear 0.76 s (was ~41 s), crossings 2.0 s (was ~15.5 s), zigzag worst case 40.6 s (was ~62 s), still K² with no exact pruning found.

**Tests and harnesses**
- Fixtures sit in a per-process dir under an advisory lock, with dead dirs reaped.
- Parity asserts two knots per record, baseline named as 27c92488f (task 2's epoch-5 two-pose compiler; the binary was built 9 minutes after it).
- Ignore reasons carry commands; the motion tests are renamed `…_stay_within_the_motion_bars_of_the_fine_bake`.
- The floor-cut doc says it is narrower than the ruling; plan tags are out of test comments; `copy_dir` is shared.

**Id-59 trim:** `reserve_mover_tiles` keeps carrying probes' id-59 tiles first. When they can't fit, the lowest drop as before and the build warns, naming `--probe-depth-tile-cap`. A refusal broke `mover_probe_channel_depth_cap`, and the bake's own significance choices also read the pre-trim id 59. Tested.

**Wording:** the cap warning says an authored count places no geometry poses; the docs define "geometry poses"; stale comments fixed.

**Result:** `--release` rebakes are byte-identical to HEAD's on setpiece and enemy-movers.

**Gates:** fmt, clippy, `--bin prl-build mover` 194, `tests/mover_*`, parity 3/3, AC 7/8 references, AC 19 (now un-ignored).

**Left:** the zigzag skip worst case (follow-up candidate); the trim warning instead of a refusal; the `build_pipeline.md` trim note at landing; and old test names cited in `captures/`.

### Fix pass B (format, renderer, engine tests)
- **One f16 ceiling:** `PROBE_DEPTH_MOMENT_MAX` in level-format `probe_depth_tiles.rs`, used by `knots.rs` and the renderer's `depth_blend.rs`.
- **f16 rounding:** pinned on both sides through `f32_to_f16_bits`, with a shared edge list (subnormal ties, smallest normal, 65519.996, 65520, ∞, both signs). The docs say "written as selects"; `mix_depth_tile` zips its slices.
- **WGSL:**
  - `MC_T_SLOT_WORDS` and `MB_T_SLOT_WORDS`, pinned to `T_SLOT_WORDS` without a GPU (`the_wgsl_t_slot_stride_is_the_blocks`);
  - `mb_entry_blend` takes the bracket explicitly;
  - `debug_assert` on the slot count in `MoverTBlock::new`.
- **Harnesses:** the engine C10 harness and the renderer rest harness assert two poses per baseline record and point at the parity test's doc.
- **Comment drift** fixed; the renderer's `effective_knots` passthrough removed.
- **Gates:** fmt, clippy (with and without capture), renderer GPU mover 127 plus the full renderer suite 1377, level-format 764, loader 52, compiler mover 194 and `tests/mover_*`, parity 3/3, C10 frames.

### Re-review of the fix passes and the final preflight (2026-11-12)
- **Re-review** (1 adversarial reviewer over `e2758fd33..9a414d5a7`): 🔴 a brush whose band grazes a probe without enclosing it could take another enclosure's outside knot, and the build bailed. 🟡 Shallow faces scaled both tuned distances by the face rate (a 1:10 ramp put outside knots 10 u out). 🟡 A NaN or inf half passed task 21's BC6H bound; subnormal f16 steps were understated. 🟡 Task 20's truths don't resolve every 1 u window (owner question). 🟢 A long graze joined like a seam; a fixture-lock PID-reuse race.
- **Final preflight at `9a414d5a7`:** every gate passed (AC 1, 2, 3 incl. both C10 per-probe runs, 7/8, 19, 28, 36, knot counts; fmt, clippy, release check, crate graph). The full GPU suite had 15,858 passed and 1 failed: `a_warm_volume_hit_frees_the_gates_patch_set_before_the_delta_stage_begins`, an extra patch-set drop from `admit_placed_poses` pricing through a throwaway `PosedScene`.

### Fix pass C
- **Band clearing:** outside knots are cleared against the complement of **every** brush's band span, clamped into the nearest clear piece, or held by the terminus or neighbour. Test: `a_brush_grazing_a_probe_keeps_another_brushs_outside_knot_out_of_its_band`, with an independent sampled oracle.
- **Travel units:** outside and inside targets are the tuned distances in travel; the band clearance alone scales with the face rate; the inside knot is kept near termini. Test: a 1:10 ramp. Pass A's rule fails the ramp, slant, bevel and band-gap tests.
- **Joins:** spans join only across a gap inside the solid grown by 1e-4 m. Test: a lift-wall graze.
- **Pricing:** `PatchSet::try_heap_bytes` prices patches face by face without building a scene, still equal to a built scene. The preflight failure passes.
- **Task 21 bound:** non-finite halves fail by name; `f16_step` clamps at 2⁻¹⁴, so subnormals step 2⁻²⁴. Both mutation-checked.
- **Fixture locks:** reaping happens only while holding the dead process's lock; a new process relocks until its lock file is the one at the path.
- **Result:** setpiece and enemy-movers `--release` byte-identical to `9a414d5a7`; knot counts unchanged.
- **Gates:** fmt, clippy (with and without capture), compiler mover 197, `tests/mover_*` 64, `surface_radiance` + geometry knots + AC 19 86, parity 3/3, AC 7/8, knot counts, renderer C10 tests.
- **Disk:** the agent deleted 9.6 GB of `target/debug/deps` artifacts over 2 days old to stay above 15 GB.

