# sh-compose-contributing-rows

Brief · compact · reads: `context/lib/rendering_pipeline.md` §4 (Sampled-row compose, Animated SH delta volumes, Promoted static lights), §7.1, §12 · `context/lib/build_pipeline.md` §PRL section IDs (ids 27, 41, 45) · read at b9d0a4645 · evidence: `research.md`

## Problem
The `sh-compose-row-cost-spike` found that each streamed SH compose pass composes every gated resident row in its cluster domain on every frame while any animated light is active. Rows that carry no CSR entry are included. At the hallway arena that is 2129 rows per pass, of which 70 carry entries. On the Mac perf target each row costs about 3 µs, so the waste is several milliseconds per pass. The cause is the pass's per-source staleness trigger: it fires for every row whose brick the cluster's directory covers, and the compiler emits a sparse row for every covered brick whatever its entry count. A zero-entry row's output does not depend on animation: it is the base, Pass A's intermediate, or base × the static bit, plus control uniforms. When this is done, the per-source trigger stales only rows that carry at least one CSR entry in that pass's section. Every stored slot a consumer samples still equals what full-resident compose would write that frame.

## Decisions
- **Narrow per-source membership to entry-carrying rows: drift repair.** A row contributes to a pass when the pass's section has at least one CSR entry for it: id 27 for indirect, id 41 for static direct and Pass B's upstream bit, id 45 for Pass B. §4 and §7.1 already say compose filters by contribution, and `plans/done/perf-sh-compose-sampled-row-gating` measured its scoped set as entry-carrying rows. The landed membership drifted to the whole brick domain. Entry counts are static per level, so membership stays static per installed row. The predecessor's "for an active light" qualifier is not restored (`research.md` §Rivals).
- **The ref tables stay as they are.** Residency unions, dirty-row pruning and eviction read the per-brick ref tables, so membership derives from them plus entry count and does not replace them.
- **Exactness rests on triggers that ignore membership.** Install, slot reuse and partial eviction mark rows pending through the dirty patches. Control changes and force-full fire the resident epoch. Pass A's planned rows mark Pass B rows pending. A zero-entry row therefore recomposes whenever its inputs can change, and §4's full-resident equality still holds. The full-resident switch stays the reference.
- **Retained zero-payload entries count as entries.** Script-mutable lights keep zero-payload CSR records so a replaced curve finds them (`build_pipeline.md`, id 45). Their rows stay contributing. This brief depends on that retention and does not change it.
- **Renderer-only, no format change.** This is a planner change on the CPU side. Shaders, PRL sections and the compiler are untouched. The change removes work on every backend, so no 1660 gate applies.
- **Durable capture at promotion.** §4 "Sampled-row compose" defines a pass's contributing rows as entry-carrying rows, and names the membership-independent triggers that keep zero-entry rows exact. §7.1's dispatch wording follows.
- **Non-goals:**
  - Lifting the script-mutable zero-drop and cone-cull exemption. It is a compiler change, and its premise is unsettled: `build_pipeline.md` says curve replacement can make an at-rest zero record nonzero, and runtime radius and follow paths are unchecked (`research.md` §Exemption).
  - The array-free compose lever (`sh-compose-array-free`).
  - Rate limiting (`drafts/animated-light-update-rate`).
  - Per-light change scoping.

## Acceptance
### Automated
**Membership (each pass):**
- [ ] A resident, gated row with zero entries in the pass's section is not planned on a frame where only the per-source trigger fires.
- [ ] A resident, gated row with at least one entry is planned on that frame.
- [ ] A row whose only entries are retained zero-payload records is planned.
- [ ] Pass B upstream: on a frame where only promotion weights change, a row with no id-41 and no id-45 entries is not planned in Pass B (unless Pass A rewrote it). A row with id-41 entries and no id-45 entries is planned.
**Membership-independent triggers still reach zero-entry rows:**
- [ ] A zero-entry row is planned on its install frame and after slot reuse or partial eviction.
- [ ] A control or mask change, and force-full, plan every resident row including zero-entry rows.
- [ ] A Pass A rewrite of a row marks it pending in Pass B whatever its id-45 entries.
- [ ] A zero-entry row that leaves the gate while current and re-enters is not recomposed. An entry-carrying row that leaves while stale is recomposed on re-entry.
- [ ] The deactivation frame (activity returns to zero) still plans every entry-carrying row once.
**Exactness:**
- [ ] The planner's CPU oracle (`compose_plan_oracle`) agrees with the narrowed planner over its randomized frame sequences, with zero-entry rows present in every pass's domain.
- [ ] `sampled_row_gate_capture_matches_full_resident_at_stepped_times` (`--features capture`, `--ignored`) passes, and its report shows fewer rows composed than before the change at a pose with zero-entry rows in view.

### Manual
- [ ] Mac, hallway arena (`--start-pose=21.13,2.44,30.48,0,0`) and kinematic station (`--start-pose=-6.5,1.22,-27.94,0,0`): rows composed per pass equal the entry-carrying rows in view (arena 70 per pass, from 2129). Report per-pass ms per compose encoder before and after. Projections: arena 0.52 / 0.86 ms, station 2.77 / 0.88 ms, ind / B. Also report Pass A (`Streamed Direct SH Promotion`) ms at the kinematic station, before and after: its id-41 membership narrows too, and the spike measured it at 2.6–2.9 ms there.
- [ ] Mac atlas byte check: streamed vs force-full-resident composed atlases, both passes, at two stepped times at the arena and the kinematic station, are byte-equal. This uses the spike's atlas dump from a throwaway branch.

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
