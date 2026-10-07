# sh-compose-contributing-rows

Brief · compact · reads: `context/lib/rendering_pipeline.md` §4 (Sampled-row compose, Animated SH delta volumes, Promoted static lights), §7.1, §12 · `context/lib/build_pipeline.md` §PRL section IDs (ids 27, 41, 45) · read at b9d0a4645 · evidence: `research.md`

## Problem
The `sh-compose-row-cost-spike` found that each streamed SH compose pass composes every gated resident row in its cluster domain on every frame while any animated light is active. Rows that carry no CSR entry are included. At the hallway arena that is 2129 rows per pass, of which 70 carry entries. On the Mac perf target each row costs about 3 µs, so the waste is several milliseconds per pass. The cause is the pass's per-source staleness trigger: it fires for every row whose brick the cluster's directory covers, and the compiler emits a sparse row for every covered brick whatever its entry count. A zero-entry row's output does not depend on animation: it is the base, Pass A's intermediate, or base × the static bit, plus control uniforms. When this is done, the per-source trigger stales only rows that carry at least one CSR entry in that pass's section. Every stored slot a consumer samples still equals what full-resident compose would write that frame.

## Decisions
- **Narrow per-source membership to entry-carrying rows: drift repair.** A row contributes to a pass when the pass's section has at least one CSR entry for it: id 27 for indirect, id 41 for static direct and Pass B's upstream bit, id 45 for Pass B. §4 and §7.1 already say compose filters by contribution, and `plans/done/perf-sh-compose-sampled-row-gating` measured its scoped set as entry-carrying rows. The landed membership drifted to the whole brick domain. Entry counts are static per level, so membership stays static per installed row. The predecessor's "for an active light" qualifier is not restored (`research.md` §Rivals).
- **Membership reads the installed row's entry count.** It uses the same value compose reads, so membership and compose cannot disagree. Residency unions, dirty-row pruning and eviction keep reading the per-brick ref tables; only compose membership narrows.
- **Exactness rests on triggers that ignore membership.** Zero-entry rows stay exact because install, slot reuse, partial eviction, control changes, force-full and Pass A rewrites trigger recompose independent of membership; the full-resident switch stays the reference.
- **Retained zero-payload entries count as entries.** Script-mutable lights keep zero-payload CSR records so a replaced curve finds them (`build_pipeline.md`, id 45). Their rows stay contributing. This brief depends on that retention and does not change it.
- **Renderer-only, no format change.** This is a planner change on the CPU side. Shaders, PRL sections and the compiler are untouched. The change removes work on every backend, so no 1660 gate applies.
- **The capture report counts entry-carrying composed rows per pass.** The capture test asserts the narrowing end to end, so the automated tier fails on the defect.
- **Durable capture at promotion.** §4 "Sampled-row compose" defines a pass's contributing rows as entry-carrying rows, and names the membership-independent triggers that keep zero-entry rows exact. §7.1's dispatch wording follows.
- **Non-goals:**
  - Lifting the script-mutable zero-drop and cone-cull exemption: a compiler change with an unsettled premise (`research.md` §Exemption).
  - The array-free compose lever (`sh-compose-array-free`).
  - Rate limiting (`drafts/animated-light-update-rate`).
  - Per-light change scoping. It needs per-light dirty tracking, and curve samples have no dirty signal; static entry presence needs neither.

## Acceptance
### Automated
**Membership (each pass):**
- [ ] A row's membership in a pass equals whether its installed sparse row has at least one entry in that pass's section.
- [ ] A cluster installs whose sparse rows include zero-entry and entry-carrying rows in ids 27, 41 and 45. Every installed row is resident in each pass it belongs to. In each pass, only its entry-carrying rows are contributing. The membership rows below take membership from this install, not from a planner fixture.
- [ ] A resident, gated row with zero entries in the pass's section is not planned on a frame where only the per-source trigger fires.
- [ ] A resident, gated row with at least one entry is planned on that frame.
- [ ] A row whose only entries are retained zero-payload records is planned. This covers script-mutable records in ids 27 and 45 and the last promotion record id 41 keeps (R2).
- [ ] Pass B upstream: on a frame where only the static promotion weights change, a row with no id-41 and no id-45 entries is not planned in Pass B (unless Pass A rewrote it). A row with id-41 entries and no id-45 entries is planned. Outside the gate, that row lags in Pass B, and the no-entry row does not (P11).
- [ ] A pass's trigger fires while every gated resident row has zero entries in its section. The pass plans no row, dispatches nothing, and reports no lagging row (P9).
**Membership-independent triggers still reach zero-entry rows:**
- [ ] A zero-entry row is planned on its install frame and after slot reuse or partial eviction, including when the per-source trigger fires that same frame. After that plan commits, it is not planned on a later frame where only the per-source trigger fires (P1, P4, P14).
- [ ] A control or mask change, and force-full, plan every resident row including zero-entry rows (P2).
- [ ] A Pass A rewrite of a row marks it pending in Pass B whatever its id-45 entries.
- [ ] A zero-entry row leaves the gate while current. The per-source trigger fires on every frame it is outside. It re-enters and is not recomposed, and it never counts toward the pass's lagging rows. An entry-carrying row that leaves while stale is recomposed on re-entry (P3).
- [ ] The deactivation frame (activity returns to zero) still plans every entry-carrying gated row once and plans no zero-entry row. When that frame records no compose, the next recorded frame does the same (P5–P7).
- [ ] On a map with zero-entry and entry-carrying rows in every sparse section, every install, failed install, eviction and session clear queues each row whose membership changed, once. Re-observing the queue leaves the planner's membership equal to the residency state's for every row (P12, P13).
- [ ] A zero-entry sparse row joins its pass's resident union and dirty set on install, and leaves them on eviction. Resident unions still equal a rebuild from the ref tables.
**Exactness:**
- [ ] The oracle equivalence suite (`compose_plan_oracle`) still passes. This is a regression guard; it passes on main too.
- [ ] `sampled_row_gate_capture_matches_full_resident_at_stepped_times` (`--features capture`, `--ignored`) passes at a pose with zero-entry rows in view. Its report carries, per pass, the last measured frame's composed rows that carry an entry in that pass's section; for Pass B that is id-45 entries, or membership in that frame's Pass A plan. For each gated capture, the test first asserts the report shows no queued, ready or installed-uncomposed cluster and every target sampleable. It then asserts that every composed row in every pass is entry-carrying, and that the indirect pass and Pass B each compose at least one row. Pass A may compose none, because its promotion weights have settled by the final frame (R5).
**Scope:**
- [ ] The diff touches no `.wgsl` file and nothing under `crates/level-format`, `crates/level-compiler` or `crates/level-loader` (grep gate).

### Manual
- [ ] Mac, hallway arena (`stress-warren-hallway-inspection.prl`, `--start-pose=21.13,2.44,30.48,0,0`) and kinematic station (`kinematic-platform.prl`, `--start-pose=-6.5,1.22,-27.94,0,0`), each at the fixture SHA pinned in the spike's measurement README: rows composed per pass equal the entry-carrying rows in view (arena 70 per pass, from 2129). Report per-pass ms per compose encoder before and after, against the projections in R3. Also report Pass A (`Streamed Direct SH Promotion`) ms at the kinematic station, before and after: its id-41 membership narrows too. Pass A ms comes from `all_passes_per_frame_ms`, or per encoder after adding its label to `gpu_time.py`'s compose filter. Pass A rows and id-41 entry rows come from a third `RowCountWindow` on the static-direct plan against the id-41 metadata.
- [ ] At the kinematic station, Pass B's rows composed equal the gated rows with id-45 entries plus the rows Pass A composed that frame. Report both counts. R3's Pass B station projection assumed 75 rows and does not include Pass A's rewrites (P10, R1).
- [x] Mac atlas byte check: new-membership composed atlases (indirect and direct) are byte-identical to old-membership atlases, gated and force-full-resident, at two stepped times at the arena and the kinematic station. This uses the spike's atlas dump from a throwaway branch. (Restated by owner ruling on 2026-10-06. A whole-atlas gated-vs-full compare differs wherever rows outside the gate lag, which the contract allows.)

## Path
Non-binding.
- **Seam:** `ShResidencyState::compose_row_membership` (`sh_streaming/row_refs.rs`). Entry counts come from the whole-level CSR (`affinity_offsets` per section) or a per-row flag computed at install. The installed payload's `entry_count` (`sparse_install.rs`, `allocate_sparse_rows`) is the cheaper source.
- **Planner:** `StreamedComposePlanner::plan_frame_into` and `PassStaleness` (`compose_staleness.rs`) need no logic change. A membership change already carries lag through its flag. Check that `observe_membership` sees the narrowed value on install.
- **Oracle:** `compose_plan_oracle.rs` models contributing rows per pass; feed it the narrowed sets.
- **Row counts:** the spike's `RowCountWindow` (`measurements/sh-compose-row-cost-spike/probes.patch`) logs rows and entry rows per pass.
- **Shape chosen:** narrow membership at its source. **Rivals** (`research.md` §Rivals): filtering `plan.rows` after planning, per-light active scoping, nonzero-payload membership, and a compiler-side drop.
- **First slice:** indirect membership plus its oracle rows, then a capture at the arena to confirm 70 rows.

## Open questions
- None.
