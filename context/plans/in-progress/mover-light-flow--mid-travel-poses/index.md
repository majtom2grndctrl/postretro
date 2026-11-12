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
- **Geometry knots (owner, 2026-11-11, after the motion captures).** Adds to the even placement above, without replacing it.
  - **What gets added.** With no `light_poses` key, a mover's knots are its even poses plus its geometry knots. Take each near-field domain probe that the mover's brushes enclose somewhere along the path, by the bake's own enclosing rule. At each end of each such enclosure interval, a pose goes just outside the interval and one just inside it.
  - **Exact placement.** The bake places geometry knots where its enclosure test changes, never on a grid. Knots closer than a merge tolerance merge; the tolerance is pinned by the build, below the slack the evidence shows fails.
  - **Kinds and limits.** An outside knot is a full pose. An inside knot may store depth only, with values blended across it (delegated, by measured bytes and bake time). The cap of 16 counts even poses only.
  - **No silent fallback.** No geometry knot is ever dropped to meet a count. Their cost is priced by the SH memory gate before any ray (overage is the named error) and by the Build Summary's projected time. The Build Summary prints each mover's even and geometry knot counts.
  - **Why.**
    - Dense motion captures showed two effects (`captures/motion-results.md`):
      - depth acts only within ±0.001–0.003 t′ of a knot, which gives a one-frame dark flash at each knot on the rising room's ceiling and a probe-lattice dot leak between knots;
      - em_sweeper's east face has a 5.7–10 luma value sawtooth at the knot period.
    - **Cause:** the probe layer behind a moving surface is enclosed by the thin slab mid-interval, so no even knot stores that state. Its values meanwhile switch from lit to shadowed across a whole interval while it is still in front of the face.
    - **Offline evidence** against a 41/57-pose truth bake (`captures/advice-results.md`):
      - ceiling far-side share 0.119 → 0.020 (truth 0.016);
      - east face 0.127 → 0.049 (truth 0.046);
      - east-face value error, worst, ~13 → ~0.5 display levels.
    - One knot per enclosure, or knots one fine step loose, fall short.
  - **Scale.** Knot counts are measured on every map and the stress hallway before the bake work lands. If a realistic mover's count makes the cost blow up, sparse per-probe storage and per-pose sub-domain bakes join this brief: the owner's check.
- **`light_poses` key (owner).** An optional `kinematic_mover` integer ≥ 2 that sets the pose count, past the cap if the author chooses; 2 means termini only. An authored count places exactly that many even poses and no geometry knots (owner, 2026-11-11): the author's explicit trade of exactness for cost. Blank reads as absent, and absent means automatic. Any other value fails the build naming the mover, like `light_bounces`. Documented in `docs/level_design.md` and the FGD.
  - **Authored poses clear probe-layer bands (owner, 2026-11-12, after the playtest diagnosis).** An authored pose whose brush plane falls within the enclosure band of a domain probe layer moves the least distance out of it, by the bake's own enclosing rule. Termini never move. Why: an even count can park a face or seam within about a millimetre of a layer, which stores an in-solid dark state there (room 41 pose 19, room 78 pose 71, room 177 poses 123, 139 and 171).
- **Termini are full states; mid poses are near-field (owner).**
  - Termini bake as today.
  - At a mid pose, direct q bakes at every probe a gated light reaches.
  - Bounce and depth gather only over the mover's near-field domain: the union of its brushes' swept volumes along the path, widened by the gather lift plus one probe spacing (owner).
  - A probe outside the domain stores no mid-pose bounce or depth and blends across the pose.
  - Why: a mid pose's gather over every valid probe costs ~85 s on the hallway (`probe-channel/research.md` §M1), whatever the mover's size.
- **Mid poses trace their own later bounces, near-field (owner, 2026-11-10, after the first slice).** Supersedes "Mid-pose later bounces blend from the termini".
  - A mid pose re-bakes bounces one through its effective `light_bounces` with the mover posed, as a terminus does, and keeps id 34's bounces past them.
  - Its later bounces iterate patch to patch only over the patches inside the near-field domain: the static patches whose centre lies in it, and the mover's posed sides. A gather ray that leaves the domain reads the mover-absent bake's radiance for that bounce, computed once per build and shared by every mid pose.
  - Why: the first slice measured the termini blend 3–7 levels short on the roof top and sweeper, and lending 5–12 levels of open-air share to a sealed room's interior. id 34's own later share lit that interior ~16 levels too bright. No cheap share source passed every group (`plan.md` task 5b, `captures/t5b-results.md`). Cost: not yet measured; the mover-absent per-bounce radiance costs about one id-34 later-bounce pass per cold build (~130 s on the hallway), and each mid pose iterates its domain's patches.
- **The enclosing rule, per pose kind (owner, 2026-11-10, after the first slice).** A terminus whose brushes enclose the probe stores no value state there and reads the base, and keeps the dark depth tile, as today (`build_pipeline.md` step 12, An enclosing terminus reads the base). A mid pose that encloses the probe stores no value state there and blends across, in bounce and in direct, and keeps the dark depth tile. *(Mid-pose half superseded by Amendment 2's Ruling B, 2026-11-12: an enclosed mid pose stores its own posed state, which is dark.)* Direct marks each such (mover, probe, mid pose) in the section so compose blends across it after the ramp, exactly. Why: an enclosed mid pose reading the base lit one probe layer at full light between dark neighbours, so the rising room's interior pulsed up to ~11 luma between poses and the sweeper's faces leaked light (`captures/t5b-results.md`). A light any pose blocks is gated. So a mover whose middle pose alone blocks a light now gates it, reversing the foundation's three-waypoint row wherever a pose reaches that block.
- **Mover light gate report (owner).** Its per-terminus columns stay as they are: they size the world-surface channel, which picks its own spacing. Each mover gains one count: the lights only a mid pose gates.
- **Redundant mid poses are skipped (owner).** A mid pose stores no bounce or depth state for a probe where the blend of its neighbouring states is already within that probe's bars: the round-trip bar and perceptual floor for bounce, the significance floor for depth. A skipped mid pose blends across; a terminus with no stored state still reads the base. Direct q is one byte per pose and is never skipped.
- **Compose is piecewise between stored states.** Each consumer finds the stored or base-reading states around t′ and blends them as it blends termini today, several movers included.
- **Depth between poses: mean and variance (owner, 2026-11-11, after the motion captures).** Supersedes "through the base, per interval" (2026-11-10).
  - **Mixed intervals.** An interval between the two stored or base-reading states around t′, unless both are termini, blends each texel's mean and variance linearly. It then rebuilds E[d²] = σ² + μ² from the f16-rounded mean, with σ² clamped at 0, and is exact at either end.
  - **Termini-only intervals.** An interval whose ends are both termini keeps today's raw through-base blend, carrying 12c's fix. That covers a two-pose mover, and a probe where every mid pose skips.
  - **Several movers.** Each mover blends this way, then the existing difference sum combines them.
  - **One function.** The rule is a single compose function beside the format's interval rule. Skipping judges exactly what compose reads.
  - **Why.**
    - Lerping raw moments adds s(1−s)·Δμ² of variance, so Chebyshev passes everything just past a knot (`captures/motion-results.md`).
    - A hit distance on a translating surface moves linearly in t, so the mean lerp is exact wherever a texel sees the same surface at both ends.
    - Running through the base under this rule would slide every occluder out to the base each midpoint.
- **The author ramp applies first (owner).** Knots map through the mover's ramp: t′_k = ramp(t_k). `MoverTravel` is unchanged. `docs/level_design.md`'s "until the mover passes `light_ramp_start`, the first waypoint's lighting holds" stays true for every probe. Knots squeezed onto t′ = 0 or 1 carry the first or last pose's state, so lighting is continuous across both ramp edges; poses squeezed out are never read, and the bake may skip them.
- **Bake fidelity: a build-time cap on `light_bounces` (owner).**
  - `prl-build --bake-fidelity low|high`, default `low`, for interactive builds.
  - `low` caps every mover's `light_bounces` at 1 for the build, so every pose bakes bounce one posed and keeps id 34's later bounces. `high` uses each mover's `light_bounces`. `light_bounces 0` stays 0.
  - `--release` always bakes `high`. Naming `--bake-fidelity` beside it is a usage error, so a ship bake never reads as low. The bake keys on fidelity and the `--release` intent, never on whether a cache is present.
  - Why here: mover lighting is what designers iterate on with these set-pieces, and this brief already re-keys the mover bounce memos per pose. It wires only mover bounce.
- **The cache serves the best fidelity it holds (owner).**
  - Bounce and depth memos are per (mover, pose, fidelity).
  - A `low` build uses a matching `high` entry when one exists, and bakes `low` only on a miss. A `high` build reads only `high`.
- **Low is approximate, and says so (owner).** Safeguards like those of the approximate warm path retired in 5209e6931.
  - The exception is named. A `low` build's bytes are a function of its inputs plus the `high` entries cached: same inputs and same cache give the same bytes. It diverges from `build_pipeline.md` §Build Cache's "both modes bake the same bytes" and "`--release` bypasses the cache exactly like `--no-cache`", from its Determinism invariant, from `development_guide.md`'s "cold `--no-cache` is the exact ship source of truth" and "`--release` only bypasses the cache", and from step 12's warm-equals-cold rule for the probe channel. Each now holds per fidelity with no higher entries cached, and promotion restates them in `context/lib/`. Interactive `high` equals `--release`.
  - An automated gate compares `low` against `high` at rest and reports the error's sign. Its bound is set by the owner from the first measurement.
  - The Build Summary marks a mover approximate only where what it shows differs from `high`: its effective bounce count (`light_bounces`, clamped to id 34's) exceeds 1 and some state was baked `low` rather than served from `high`. It projects each mover's `--release` bounce-bake time as id 34's last recorded bake time on the map × the mover's two termini, since one terminus costs about one id-34 bake (§M1), plus each mid pose's domain-sized trace: id 34's time scaled by the share of static patches the pose's domain holds (owner, 2026-11-11: × 2 alone projected 2.2 s per setpiece mover against ~3.3 s measured, the mid poses uncounted). It is labelled an estimate and reads "—" until a build has recorded that time. The gate covers the same movers.
- **Mover-receiver hand-over and in-wall layers (Amendment 2, owner, 2026-11-12, after the playtest diagnosis).** Ruled by the owner with Windows captures in hand (`captures/amendment2/results.md`), after an in-engine demo of the ramp and a prototype of the dark state.
  - **Goal.** M9 met in motion. A mover face shows no per-layer stepping as it crosses probe layers. A probe layer inside a thin mover wall pulses no band into the room behind it. Rest frames may change. AC 1's per-probe channel comparison still holds, because it compares the channel, not the frame.
  - **Cause (hand-over).** The mover backface cosine is taken from the lookup point, 0.1 cell off the face (`sh_sample.wgsl` `sh_backface_weight`). When the lookup crosses a probe layer, where that layer's trilinear weight peaks, the weight falls to the 0.03 floor and renormalisation hands the face to the next layer in 1–3 frames. The cause predates this brief: the strip column shows it. A larger offset alone only moves the flip.
  - **Cause (in-wall).** On `mover-setpiece`, plane x = 846.93 lies 1.07 u inside the rising room's west wall. Its enclosed and above-roof probes carry arena bounce into the dark room through the 0.03 floors. Shifting the lattice 32 u drops the band from 6–13.5 luma to under 2 until t′ 0.56.
  - **Ruling A, backface form (owner, 2026-11-12; `ip-372` lever 5, counsel question 6).** The backface term is taken from the surface point, not the lookup point. It is a smooth signed-distance ramp along the receiver's geometric normal, 0.4 cell wide and floored at 0.03, with the lookup offset at 0.4 cell. It uses only the fragment's own position and normal, as today's cosine does. Why: the largest per-frame step fell from 11.2 to 4.3 on the sweeper's east face and from 19.9 to 8.7 on the wall's end face, with error against truth equal or better. The ramp at today's 0.1 offset was worse than today, and the larger offset alone only moved the flip. Cost: rest frames shift, by up to 8–12 levels on parts of the sweeper and the closed wall, so frame baselines are re-cut. AC 1's per-probe bytes are unchanged.
  - **Ruling B, in-wall layer (owner, 2026-11-12): the dark state, with Ruling A.** A mid pose whose brushes enclose a probe stores the state its own posed bake gives: zero bounce (every lifted ray starts in solid), its traced direct q, and the dark depth tile. This supersedes the mid-pose half of "The enclosing rule, per pose kind". Termini still read the base. The direct block's across marks go, since no enclosed mid pose blends across any more. Why: with the ramp, the west-wall band falls from a mean of about 10.9 (peaks 15.8) to 0.8–2.3, within 0.02 mean of its own dense truth and steadier than shifting the lattice 32 u. Dark blotches on mover faces stay at 2.4 levels or less. With today's backface the dark state paints blotches of up to −17 levels on the sweeper, so it never ships without Ruling A. Rest bytes and frames are unchanged, id 57 grows 2–4 %, and bake time does not change. Options (i), an authoring warning and lattice-phase key, and (iii), the larger offset alone, were not taken.
  - **Re-light into rest (owner, 2026-11-12: fixed in this brief).** Because termini read the base, the band re-lights over the last ~4 % of travel into rest (step 8.7 at t′ 0.98, against the truth's 4.6). A geometry knot goes just inside each terminus where an enclosure interval reaches it, so the last stored interval before a base-reading terminus is short.
  - **IP.** Every candidate keeps every valid probe at a continuous weight of at least 0.03. Nothing flags, zeroes or excludes a probe. No candidate tests against posed mover geometry at runtime (lever 6).
  - **Not here.** Probes that ride the mover, the root fix for lattice hand-over, go to a follow-up if Amendment 2 leaves residue.
- **IP rules bind unchanged.** N absolute states blended by mover position is the shape accepted for two (`ip-check.md` §Piece 7, §Tencent US12293451B2). Depth blends moment sets, the domain and masks are storage sparsity, and nothing sets a per-probe flag, zeroes a weight or excludes a probe (`ip-372` levers 2, 7). The counsel packet gains one line: states per pose, not per terminus; an enclosed terminus reads the base; an enclosed or skipped mid pose blends across. The direct block's across marks are storage sparsity: compose blends between neighbouring stored states and never zeroes a weight.
  - **Counsel line, restated by Amendment 2 (owner, 2026-11-12).** "States per pose, not per terminus; an enclosed terminus reads the base; an enclosed mid pose stores the state its own posed bake gives (zero bounce, its traced direct q, the dark depth tile); a skipped mid pose blends across." The across-marks sentence is dropped. A dark probe keeps its weight at the 0.03 floor or above and darkens the result; nothing excludes it, so its neighbours never fill in for it (levers 2, 7). Question 9 is unaffected. Question 7 gains a note that probes at the floor now carry dark values mid-travel. The backface ramp is the lever-5 form the owner chose, and it is the signed distance counsel question 6 asks about.
  - **Counsel wording, updated (owner, 2026-11-11).** `ip-372` question 9 reads: a live tile is a blend, by mover position, of the two neighbouring stored tiles' mean and variance, and between termini, raw moments through the probe's static tile, reaching it at mid-travel (corrected by the owner, 2026-11-12: the earlier "raw-linear between termini" misstated the code). The mover's knots, shared by every probe, are chosen at compile time from geometry. Probes now sit at the 0.03 floor mid-travel as well as at rest, which bears on question 7. Lever 7 is unchanged: the blend still runs between moment sets, with no per-probe flag and no zeroed weight.
- **Memory.** Every pose's resident bake output is charged to the SH memory gate before any ray. Overage is the named error, never fewer poses (`build_pipeline.md` step 12; `development_guide.md` §1.4).
- **Per-frame cost unchanged (owner, 2026-11-11).** Mid poses add no measurable frame cost over the pre-brief build. Triggers stay per mover on t′. The 0.2 ms per travelling mover budget (`rendering_pipeline.md` §4) is a follow-up brief's: M7 measured the pre-brief build at 1.7–2.7 ms per travelling mover on the 5300M, so the cost predates this brief.
- **Non-goals.**
  - Spinners: still not analysed.
  - Poses at authored waypoints or rest stops. A mover resting mid-path reads the blend of poses about one step apart; the stored knots leave room for exact rest poses later.
  - Any runtime geometric or half-space test against the posed mover: `ip-372` lever 6, and per-probe GPU cost.
  - Per-probe knot sets. Knots stay per mover, shared by every probe. Per-probe sparsity is the existing skip and marks.
  - Animated lights' bounce (id 27).
  - Far-field bounce changes mid-travel: they follow the blend. A mid pose's later bounces past its near-field domain read the mover-absent bake.
  - The world-surface channel. It picks its own state spacing: lattice spacing suits probes, which already blur over one step, not dense lightmap texels.

## Acceptance

### Automated

**Rest is unchanged at high fidelity**
- [ ] On `mover-closet`, `mover-setpiece` and `enemy-movers`, built `--release`, at t′ = 0 and t′ = 1: every probe's channel contribution equals the pre-change build's, compared build against build with the base taken out as in C10, including `enemy-movers`' three-mover probes. Cells mid poses newly pin are judged within the base's round-trip bar. A probe whose live tile comes only from a mid pose reads its base. Regression guard.
- [ ] A mover with `light_poses 2`, built `high`, stores today's terminus states and composes today's blend at every t′: linear for bounce and direct, through the base for depth. Regression guard.
- [ ] K1, K2, K3, C7 and C10 pass on `--release` builds. C10's frame-level capture case is judged per station against the pre-brief build's own streaming-mode gap at the same pose plus 4 levels, since BC6H already separates the modes there (owner, 2026-11-11; `drafts/sh-streaming-mode-bc6h-parity.md`). C9's renderer half passes, with each moved half checked against its piecewise mix. C10's per-probe case runs twice (owner, 2026-11-12): on `--uncompressed-irradiance` builds it composes bit for bit in every class; on BC6H builds every class stays within a bound derived from BC6H's mode difference through two f16 roundings and pinned in the test. Regression guard.

**Placement and key**
- [ ] With the key absent, a mover's even poses number min(16, max(2, ceil(L / s) + 1)) at even arc length, and its geometry knots join them. Covered: a path of unequal segments, a path with a degenerate segment (it adds no length), and a zero-length path (two coincident termini, as today). Pose positions match the runtime's t at each knot. (Restated 2026-11-11 for geometry knots.)
- [ ] A path that needs exactly 16 even poses bakes 16 and does not warn. One that needs 17 bakes 16 spread over the full path and warns, naming the mover, its spacing and `light_poses`. Geometry knots are not counted against the cap. An authored `light_poses` of 20 bakes exactly 20 even poses and no geometry knots, and does not warn.
- [ ] `light_poses` accepts integers ≥ 2 and blank (as absent). It rejects 0, 1, negative, fractional and non-numeric values, and values too large for the field, naming the mover.

**Bake**
- [ ] At one mid pose on the setpiece room and one on em_sweeper, these match a static-brush reference bake at that pose: direct q at every reached probe, within B7's direct bar; bounce one at probes inside the domain, against the reference's first bounce alone, within B7's base bar, except at probes within one patch (0.5 m) of a posed mover face that lands off the static patch grid, where the posed bake's partly buried patch differs from the reference's cut floor (owner, 2026-11-10; follow-up filed), and at probes within one patch of a plane along which the reference cuts the floor off the patch grid, where the reference's static slab splits the floor past its own footprint (owner, 2026-11-11); and depth tiles at probes inside the domain, within f16, as in B4.
- [ ] A mid pose's stored bounce equals bounces one through its effective `light_bounces` re-baked with the mover posed, later bounces iterated over the domain's patches with rays leaving the domain reading the mover-absent bake, plus id 34's bounces past them. At the setpiece room's and em_sweeper's mid poses it matches a static-brush reference's full bounce at domain probes within B7's base bar, with AC 7's patch-grid exception. At `low`, a mid pose bakes bounce one posed and keeps id 34's later bounces, as a `low` terminus does. (supersedes research pin MP3)
- [ ] Near-field domain: a probe inside the widened swept volume gathers at a mid pose, and a probe just outside it stores no mid-pose bounce or depth and blends across. The direct half still covers a reached probe outside the domain.
- [ ] Skipping: a mid pose whose neighbour blend is within the bars stores nothing for that probe and composes the blend; one that misses the bars by a step stores its state. Where adjacent mid poses skip, compose at each skipped pose's t′ stays within that probe's bars, so a probe whose states curve gently keeps enough of them. (research pin MP2)
- [ ] On a mover with ramp (0.6, 0.9), each skip is tested against what compose reads after the ramp, with squeezed knots carrying the first or last pose's state. At every skipped pose's t′, compose stays within that probe's bars. Changing the ramp keys re-decides skips, and the build equals a `--no-cache` build, byte for byte. (research pin MP9)
- [ ] A probe enclosed at a mid pose stores the state its own posed bake gives: zero bounce, its traced direct q, and the dark depth tile (restated by Amendment 2's Ruling B, 2026-11-12). No across marks are written. A probe enclosed at a terminus reads the base, as today. A probe on a face at either pose kind keeps the per-ray rule.
- [ ] A light that only a mid pose blocks is gated: q < 255 at that pose and 255 at both termini. The mover light gate report counts it for that mover, and its per-terminus columns equal the pre-change build's.
- [ ] A ramped mover whose only pose blocking a light is squeezed out builds a PRL that loads, and that light composes as if unblocked at every t. (research pin MP11)

**Compose**
- [ ] At each stored pose's t′, bounce, direct and depth read that state exactly. Between stored states each reads the mix: bounce within one f16 step, direct exactly. Depth reads the mean-and-variance blend, or the raw through-base blend between termini. Out and back to the same t′ reads alike.
- [ ] Two movers at one probe, with different pose counts and skipped poses, combine as today (direct multiplies, bounce and depth add differences from the base) at t′ between knots for both.
- [ ] With ramp (0.6, 0.9), every probe reads the first pose for t < 0.6 and the last for t > 0.9. Knots between map to ramp(t_k). On a mover with mid poses before 0.6 and between 0.9 and 1, t just past 0.6 reads within one step of the first pose, and t just under 0.9 within one step of the last. A mover with no supplied state stays ungated. (research pins MP1, MP10)
- [ ] On a mover with mid poses, a depth probe outside the near-field domain, and a probe where every mid pose skips its depth, compose today's through-base depth at every t′. Each such skipped pose's tile is within the significance floor of the blend compose reads at its t′. (research pin MP8)
- [ ] On em_sweeper at t′ 0.30, the probes just west of the slab's west face compose bounce and direct within 3 display levels of a static-brush reference bake at the mover's position. So do the probes just above `mover-setpiece`'s roof top at t′ 0.4 and 0.6. Today's build fails this. (Restated by the owner, 2026-11-10: t′ 0.25 is dropped, because those probes sit inside the slab until t 0.232 and pose states one lattice step apart cannot describe that crossing.)

**Fidelity and cache**
- [ ] `low` bakes every mover's termini as `light_bounces` capped at 1. `high` uses each mover's `light_bounces`. `--release` equals an interactive `high` build on an empty cache, byte for byte. `light_bounces 0` stores no bounce at either fidelity.
- [ ] `--release` with `--bake-fidelity` (either value) fails before any stage, naming both flags. `--no-cache` alone bakes `low`, and so does a build whose cache directory cannot be created.
- [ ] The TUI's build mode carries fidelity. A launch with `--no-cache` and no `--release`, confirmed unchanged, bakes `low` or the fidelity it names, never a release bake. Production names no fidelity. (research pin MP6)
- [ ] Per fidelity, with no higher entries cached, a warm build equals a `--no-cache` build byte for byte. Two `low` builds with the same inputs and the same cache are byte-identical.
- [ ] After a `high` build, a `low` build serves `high` states for unchanged movers and bakes `low` for an edited mover. A `high` build after a `low` one re-bakes every mover at `high` and equals `--release`, byte for byte.
- [ ] Changing a mover's `light_poses` hits both of its termini memos and bakes only the new poses, and, with no higher entries cached, the build equals a `--no-cache` build of the same fidelity, byte for byte. (research pin MP5)
- [ ] Moving the middle waypoint of a three-waypoint mover, with both ends unchanged, changes its near-field domain. The build equals a `--no-cache` build of the same fidelity, byte for byte. (research pin MP7)
- [ ] After a `high` build, changing a mover's `light_poses` and building `low` serves the termini and the unchanged mid poses from `high` and bakes the new mid poses `low`. Built `high`, changing a mover's `light_bounces` re-bakes every one of its poses, and warm equals cold. (supersedes research pin MP4)
- [ ] Low against high at rest, on `mover-closet` and `mover-setpiece`: the largest per-probe display-level difference and its sign are reported, and the test fails past the owner's bound.
- [ ] The Build Summary prints, per analysed mover, its pose count, its fidelity, and its projected `--release` bounce-bake time: id 34's recorded bake time × 2 for its termini plus, per mid pose, id 34's time scaled by its domain's share of static patches, labelled an estimate, or "—" with none recorded (restated by the owner, 2026-11-11). A mover is marked approximate only where what it shows differs from `high`; one served entirely from `high` entries prints `high`.
- [ ] The SH memory gate's prediction covers every pose's resident bake on the setpiece, the termini's bounce-one layers over the domain included, and a budget below it refuses with the named error.
- [ ] The renderer's ledger counts every stored pose's closed-pose depth tile on the setpiece and reads zero after unload.

**Format and docs**
- [ ] The loader refuses an epoch-5 section by name for each of: a pose count below 2; knots out of order, outside 0–1, not finite, or not running from 0 to 1; a block naming a pose at or past its mover's count; a probe marked both stored and base at one pose. Epoch 4 is refused.
- [ ] `light_poses` appears in the FGD and in `docs/level_design.md`'s mover lighting table. `--bake-fidelity` appears in both build-flag tables. `postretro-tool dist` with a recipe naming `--bake-fidelity` fails, naming the recipe and the flag.

**Motion (owner, 2026-11-11, after the motion captures)**
- [ ] **Geometry knots.** For every domain probe a mover's brushes enclose along its path, a pose lies just outside and one just inside each end of each enclosure interval, within the pinned tolerance, by the bake's own enclosing rule. Covered: a slab moving along a lattice axis; a slab off the lattice phase; a two-brush mover; a bent two-segment path; and a ramped mover, whose knots map through the ramp. An authored `light_poses` places none.
- [ ] **Depth blend.** Unit tests on the shared compose function cover:
  - an interval reads each end exactly;
  - one occluder translating along a texel's ray composes its linearly moved mean with no added variance;
  - σ² never goes negative and saturated f16 texels return exactly;
  - an interval between two termini equals today's through-base blend bit for bit;
  - several movers combine by the difference sum.

  Skipping and the renderer call the same function.
- [ ] **Motion proof.** CPU check against a fine bake on `mover-setpiece`'s rising room ceiling and floor layers and on em_sweeper's east and west faces, at every knot of the truth. The truth is `light_poses 76` on the room and `113` on the sweeper (owner, 2026-11-12, option 1). No truth pose parks a mover brush plane within the enclosure band of a probe layer; the test asserts it on the truth's unmoved even poses. Bars (restated by the owner, 2026-11-11 and 2026-11-12):
  - far-side share: the trip's time-weighted mean, integrated over travel, within 0.01 of the truth's. The worst knot is reported, not gated, on every surface;
  - composed values: at every knot, within 3 display levels of the truth, or within 1.5 levels of the pre-amendment build's miss there. A receiver that reads through a stored interval longer than one even step is reported, not gated, and goes to the long-interval follow-up. Its interval counts as long when at least half its light, by the truth's weights, comes from probes whose stored interval there is longer than one even step.

  The pre-amendment build fails the share bar. Misses shared with the pre-amendment build go to a follow-up on direct interpolation over long intervals and the skip bar's reach on dark surfaces. Run under `--ignored`.
- [ ] **No silent fallback.** No geometry knot is ever dropped to meet a count. The SH memory gate prices every geometry pose and refuses by name below it. The Build Summary prints each mover's even and geometry knot counts.

**Authored pose bands (owner, 2026-11-12)**
- [ ] An authored `light_poses` pose whose brush plane falls within a domain probe layer's enclosure band, by the bake's own enclosing rule, moves the least distance out of it. Termini never move. An authored count with no such pose places exactly its even poses, at the same positions as before. Covered: a slab whose authored pose lands within 1 mm of a layer.

**Amendment 2 (owner, 2026-11-12; rulings A and B made). Numeric bars marked *(proposed)* are set from task 25's captures and go to the owner.**
- [ ] **Hand-over.** Dense t′ series, built `--release`, on em_sweeper's east face, the wall door's trailing end face and north face, and the rising room ceiling. The largest frame-to-frame step stays within *(proposed)* of a seam-free dense truth's step at the same t′. Today's form fails this.
- [ ] **Floor.** Across a sweep of surface distances on both sides of a mover face, the backface ramp is continuous and every valid probe's weight stays at least 0.03. Nothing flags, zeroes or excludes a probe.
- [ ] **In-wall layer.** On `mover-setpiece`, with the ramp and the dark state, the rising room's west-wall band stays within *(proposed)* of a dense truth baked under the same rule, over the whole trip. Mover faces show no dark blotch past *(proposed)* levels against a dense truth.
- [ ] **Re-light into rest.** Where an enclosure interval reaches a terminus, a geometry knot sits just inside the terminus by the bake's own enclosing rule, within the pinned tolerance. The band's largest step over the last 5 % of travel stays within *(proposed)* of the dense truth's.
- [ ] **Rest.** AC 1's per-probe channel comparison still passes, and terminus states are byte-identical with and without the dark state. Rest-frame baselines are re-cut where the ramp changes rest frames, with the change reported per view.

### Manual

- [ ] **Amendment 2 captures (owner, 2026-11-12).** A/B charts of each Ruling A form and each Ruling B option against the current form and a dense truth, plus the rising room floor's eighth-power capture redone under the chosen form. Owner rules A and B.
- [ ] Depth candidates, captured in the first slice: the rising room floor at t′ 0.50–1.00 (12c metric) and the sweeper faces, plain lerp against per-interval through-base. Owner picks.
- [ ] Low against high at rest, first measurement: the owner sets the automated gate's bound.
- [ ] Roof top on `mover-setpiece`, at t′ 0.3, 0.4, 0.5, 0.6, 0.65 and 0.70, at `low` and `high`: within 3 levels of the id-57-stripped frame on the 12d metric, with no pits.
- [ ] em_sweeper's west face at t′ 0.20–0.32: within 3 levels of stripped, with no pits. Its east face at 0.70–0.85 and 0.00–0.15: within 3 levels of a static-brush reference at the mover's position, with no pits (restated by the owner, 2026-11-10: the slab really shades those probes, so stripped reads ~7 levels too bright there). At `low` and `high`.
- [ ] Rising room interior floor (12c metric) at t′ 0.50–1.00: no lattice dots or diamonds, and no worse than today at 0.80–1.00.
- [ ] Rising room interior mean luma against t′, before and after, at `low` and `high`, for the owner's eye. The expectation is darker mid-travel.
- [ ] 5300M, Metal System Trace, passes grouped by label, every mover travelling minus all docked. On `mover-setpiece`'s room and wall, a travelling mover's compose passes plus its live-tile writes cost no more than on the pre-brief build under the same harness, within run-to-run noise. (Restated by the owner, 2026-11-11: both builds miss the 0.2 ms budget by 8–13×, and a follow-up brief owns it.)
- [ ] Cold and warm bake times, and id-57 bytes per block, on `mover-closet`, `mover-setpiece` and `enemy-movers` at `low` and `high`, plus the hallway measured or projected. Owner judges.
- [ ] **In motion (owner, 2026-11-11).**
  - In-engine on a `--release` bake, watch the rising room's floor and ceiling and em_sweeper's faces in motion. There should be no knot flashes, no lattice dots, and no pulsing or stepping that reads as unnatural.
  - After Amendment 2 (owner, 2026-11-12): also the wall door's edge and the room ceiling above the door, on the Windows machine. The back-room spot's static white square on the wall door is not this brief's.
  - The dense t′ motion series before and after, and charts against the truth bake, are there for the owner's eye.
  - M7's frame-cost A/B is re-run with geometry knots.

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

**Epoch 6 (plan task 14)** is epoch 5's byte layout under the mean-and-variance depth rule; the loader refuses epoch 5 by name, as it does epoch 4.

| Part | Epoch 5 bytes |
|---|---|
| Header, cell index | Unchanged from epoch 4. |
| Mover records | Right after the cell index, before the direct block, because every block's lengths depend on N. u32 count, then per mover ascending by id: `mover_id u32, ramp_start f32, ramp_end f32, pose_count u32 (N ≥ 2)`, then N `f32` knots. Exactly the movers with entries in some block. |
| Direct | Record and mover-entry CSR unchanged. N `u8` q per masked probe per mover entry, probe-major (`slot·N + k`). |
| Bounce | Per-cell entry-count CSR unchanged, then `u32 × (E+1)` entry byte offsets from 0, ascending (an entry spans `8 + 16N` bytes). Entry: `mover_id u32, reserved u32 = 0`, then N `(stored u64, base u64)`. Base is 0 at a terminus, and every base bit lies inside the entry's stored union. Tiles pose-major, then probe by `stored` rank. |
| Depth | u32 record count. Records by (mover, probe): `mover_id u32, probe_index u32`, then `4·⌈N/16⌉` bytes of two-bit marks low-first (bit 0 stored, bit 1 base), padding bits 0. Tiles per record, its stored poses in pose order. |
| Streaming | Unchanged; lengths come from each mover's N. |
| Across marks (task 6a) | In the direct block, right after the mover entries and before the q payload; always resident. Present only when some mover record has N > 2, so a section of two-pose movers packs byte-identical to before. `u32` A, then per entry, ascending by (listed cell, mover) with no repeats: `listed_cell_position u32, mover_id u32, mid_pose_count u32 (= N − 2)`, then N − 2 `u64` masks of the local probes mid poses 1 … N − 2 enclose. Only pairs with some mark. The q stored at a marked (pose, probe) is 255 and never read; the bounce entry neither stores nor reads the base there. |

**Across marks (owner, 2026-11-10; pinned at plan task 6a).** Enclosure is geometric: it depends on (mover, probe, mid pose), never on the light. The section records it once per (cell, mover), not per direct record: which probes each mid pose encloses. Compose blends direct across a marked pose, after the ramp. A mark at a terminus is refused, as is a mark on a probe no direct record or bounce entry of that mover names at that cell.

The loader also refuses a base mark at a terminus, a base mark outside the entry's stored union (it would only restate the blend), a mover record that no block names, and a block naming a mover with no record.

## Boundary inventory

| Name | Rust | Wire | FGD KVP / CLI | Docs |
|---|---|---|---|---|
| Pose count key | parsed `Option<u32>` on the map's kinematic mover, beside `light_bounces` | id-57 per-mover pose count and knots | `light_poses(integer)` in `sdk/TrenchBroom/postretro.fgd` | `docs/level_design.md` mover lighting table |
| Bake fidelity | a two-value enum on the compiler's args and stage inputs | not stored | `--bake-fidelity low\|high` on `prl-build` and its TUI build mode; `postretro-tool dist` passes `--release`, so a recipe naming it fails | `docs/level_design.md` build flags table; `build_pipeline.md` flags table |
