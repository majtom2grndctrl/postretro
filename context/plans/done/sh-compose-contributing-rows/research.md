# sh-compose-contributing-rows — research

## Basis (source trace at b9d0a4645)
- `ShResidencyState::compose_row_membership` (`sh_streaming/row_refs.rs`) sets the following:
  - indirect `contributing`: the row is in `indirect_delta_row_refs`;
  - Pass B `contributing`: the row is in `direct_animated_row_refs`;
  - Pass B `upstream`: the row is in `direct_promotion_row_refs`.
- `allocate_sparse_rows` (`sparse_install.rs`) increments those refs for every installed sparse row, whatever its entry count. The compiler emits a sparse row for every brick the cluster covers (`cluster_directory.rs`, AffinityCell domain; `pack/cluster_sh_payloads/sparse.rs` writes `count = entry_count(row)`, which may be 0).
- `plan_frame_into` (`compose_plan.rs`) computes:
  - `indirect_changed = frame.indirect_active || self.indirect_was_active`;
  - `animated_changed = animated_direct_active || animated_direct_was_active || animated_weights_changed || static_weights_changed`;
  - `contributing: per_source && <changed>`, where `per_source = !controls_changed`.
- The `*_active` flags mean "any animated light active" (`renderer_pre_scene.rs`). They are true every frame on maps whose lights all animate.
- The spike's `RowCountWindow` counts rows from `plan.rows()`, and entries as `affinity_offsets[row+1] − affinity_offsets[row]` of the whole-level section. At the arena, 2129 rows per pass compose and 70 of them carry entries.

## Zero-entry row output
| Pass | Output for a row with no entries | Changes only when |
|---|---|---|
| Indirect | base atlas (if the static bit is set), alpha = stored-slot validity | base install or dirty, mask change |
| Pass A (static direct) | base × static bit, no promotion subtraction | base install or dirty, mask change |
| Pass B | Pass A's intermediate, clamped ≥ 0 | Pass A rewrote the row, mask change |

The composed atlases persist across frames, and only compose writes them, apart from the growth copies (`sh_streaming/gpu/growth.rs`). A skipped current row keeps its last value.

## Prior commitments
- `plans/done/perf-sh-compose-sampled-row-gating` set "today's pass-level trigger, restricted to gated contributing rows". Its spike findings (`spike-findings.md`, Q3) defined the measured "scoped" set as "resident rows with ≥1 CSR entry for an active light". `rendering_pipeline.md` §4 and §7.1 say compose filters rows by contribution. The landed membership uses sparse-row refs, which cover every brick in the domain, including empty ones (cluster directory AC3). So this brief repairs drift from a committed contract. It keeps the "≥1 CSR entry" half and drops "for an active light" (see Rivals).
- That plan also deferred *per-light* row scoping, which needs per-light dirty tracking and has no dirty signal for curve samples. This brief uses static entry presence and needs no dirty signal.
- The spike projected the filter alone at the arena: indirect 6.71 → 0.52 ms and Pass B 8.00 → 0.86 ms. That is `c + r · rows_f + entry share`, with `c` = 0.24 / 0.26 ms, `r` = 2.99 / 3.46 µs, and `rows_f` = 70. It extrapolates below the fitted row range (730–2579).

## Exemption
The exemption is not lifted here. What the research found:
- `ScriptMutableDescriptorSlots` (`delta_drop_policy.rs`) marks every `is_animated` light and every `LightMembershipManifest` target as mutable. Mutable lights keep zero-payload entries and cube reach.
- Its stated rationale (`build_pipeline.md`, id 45): "later curve replacement can make an at-rest zero record nonzero". The cone-cull exemption followed from it for byte identity (`plans/done/lighting-scale--sh-delta-cone-reach-cull/research.md`).
- Payload is unit-radiance transport, and the only script surface is `setLightAnimation`. So a zero payload looks geometric and curve-invariant. That is an inference; no plan states it.
- Unchecked: whether an animated radius curve (`eval_animated_radius`) or entity follow (`cached_follow_positions`) can move a baked light's reach, and how `sh_runtime_envelope.rs` uses the mutable mask.
- Lifting the exemption would shrink entry rows further at the kinematic poses (780 entry rows). It needs its own grounding.

## Rivals
- **Filter `plan.rows` after planning.** Zero-entry rows would stay stale forever, which breaks the staleness bookkeeping the oracle checks.
- **Per-light active scoping:** a row contributes if it has an entry for a light active this frame or last. The active flags are already on the CPU, so it needs no curve dirty signal. Rejected: membership becomes dynamic and needs per-light epochs, and every light on the target maps animates continuously, so it saves nothing there.
- **Nonzero-payload membership, decided at install:** runtime-only, and it would capture part of the exemption-lift saving without a compiler change. Rejected for now: it relies on retained records being exactly zero (unverified), and its yield at the kinematic station is unmeasured.
- **Compiler drops empty rows:** it breaks the cluster directory's wire contract (AC3) and forces a re-bake. Residency needs empty rows for base seeding and eviction.

## Fact pins (review-brief)
| Id | Claim | Source | Confidence |
|---|---|---|---|
| R1 | At the kinematic poses the promotion weights move every frame. Narrowed Pass B therefore composes gated id-45 entry rows ∪ id-41 entry rows (upstream bit plus Pass A rewrites), not id-45 alone. The spike's B window counted id-45 entries only, so the 75-row / 0.88 ms station projection is a floor. Id-41 entry rows at the station are unmeasured. | `compose_plan.rs` `plan_frame_into` (`animated_changed`, upstream, Pass A → B pending); `done/sh-compose-row-cost-spike/findings.md` F1; `probes.patch` `RowCountWindow` (id-45 metadata) | med |
| R2 | Id 41 retains a second kind of zero-payload record: each promotion selection keeps its last canonical record even when all zero (loader/promotion representation contract), independent of script mutability. These count as entries, so their rows stay Pass A contributing and Pass B upstream. Exactness is unaffected: a zero record subtracts zero. | `delta_drop_policy.rs` `drop_direct_zero_entries`; `sparse.rs` count; `direct_sh_compose.wgsl` | high |
| R3 | Projected per-pass ms (indirect / Pass B): arena 0.52 / 0.86, station 2.77 / 0.88. Pass A at the station measured 2.6–2.9 ms before the change. | `done/sh-compose-row-cost-spike/findings.md` | high (as projections) |
| R4 | The persistent CPU copy of a row's installed entry count, and the value compose reads, is `sparse_pools[section].row_pairs[row]`: entries = `pair[1] − pair[0]`. The pool's sentinel makes a non-empty pair mean at least one entry; zero-entry and evicted rows read `[0,0]`. `ParsedSparseRow.entry_count` lives only for the drain. Each `(section, row)` has one owner, so delta refcounts are 0/1 and every flip already reaches `compose_membership_touched`. No new per-row state is needed. | `allocator.rs` `install_undoable`/`evict`/`new`; `payload.rs` header; `setup.rs` owner map; `install_journal.rs` `journal_add_row_refs`; `row_refs.rs` `release_row_refs`; `*compose.wgsl` `affinity_offsets` | high |
| R5 | The capture test's map `stress-warren-mini.prl` is a local, untracked bake. That indirect and Pass B gated entry rows are non-zero at the animroom pose rests on the spike's 1 m bake (112 / 15 cell-grain rows), not on a pinned fixture. Pass A fires only on static weight changes, and the promotion ramp settles in 0.3 s, before the sampled times. | `git ls-files content/dev/maps`; `done/perf-sh-compose-sampled-row-gating/spike-findings.md`; `compose_plan.rs` `plan_frame_into`; `renderer_light_slots.rs` ramp | med |

## Ordering pins (review-brief)
| Id | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| P1 | A zero-entry row installs on a frame where the pass's per-source trigger fires. | Membership observed as resident, not contributing; per-source epoch fires; row planned from pending. | Planned once that frame. Current after commit. Not planned on a later per-source-only frame. |
| P2 | A row installs on the frame a control or mask change fires. | Membership observed; resident epoch fires; full-repair scan runs. | Planned once in every pass that holds it, whatever its entries. Current after commit. |
| P3 | A zero-entry row is outside the gate while the per-source trigger fires for N ≥ 1 frames, then re-enters. | Per-source epoch fires N times; row re-enters on a frame with no trigger. | Not planned on re-entry. Never counted as lagging. |
| P4 | An entry-carrying row leaves the gate stale; its sparse row is evicted while base refs keep it resident (partial eviction), still outside the gate. | Membership goes contributing → not, lag carried; dirty patch marks the row pending. | Planned that frame regardless of the gate. Current after commit. |
| P5 | Activity goes 1→0 on a recording frame. | Tail fires the contributing epoch once. | Entry-carrying gated rows planned once; zero-entry rows not planned; next idle frame plans nothing. |
| P6 | Activity goes 1→0 on a non-recording frame; the next frame records. | Epoch advances on the skipped frame. | Next recorded frame plans entry-carrying gated rows only. |
| P7 | Activity goes 1→0→1 over three recorded frames. | Epoch fires each frame. | Each frame plans entry-carrying gated rows only; zero-entry rows never planned. |
| P8 | A level with no animated lights and no promotion. | Per-source trigger never fires. | Plans hold only pending and control rows, as before the change. (Preservation; no row.) |
| P9 | A pass's trigger fires while every gated resident row has zero entries in its section. | Epoch fires, then the pass plans. | Empty plan, no dispatch, no commit, no lagging row. |
| P10 | Static promotion weights change every frame while id-41 entry rows are gated (kinematic station). | Pass A plans its id-41 entry rows and marks them pending in Pass B; then Pass B plans. | Pass B rows = (id-45 entry rows ∪ rows Pass A planned) ∩ gate. |
| P11 | Static weights change while an id-41-entry row is outside the gate. | Pass A contributing epoch and Pass B upstream epoch fire; row re-enters later. | Both passes lag the row; on re-entry Pass A plans it, then Pass B. A row with no id-41 and no id-45 entries lags in neither. |
| P12 | An install fails after adding refs for a zero-entry and an entry-carrying row; the journal rolls back. | Refs undone newest-first; rows losing their last ref queued as touched; next plan re-observes. | Planner membership equals residency membership for every row. |
| P13 | A session clear and a reinstall in one drain. | Planner membership cleared; install queues touched rows. | Membership re-observed; zero-entry rows resident and not contributing. |
| P14 | A slot freed by an entry-carrying row's eviction is reused in the same drain by a zero-entry row. | Eviction releases the old row; install marks the new row dirty; plan runs. | New row planned from pending. Old row's lag leaves with its residency. Entry counts are static, so re-tenancy never flips a row's membership. |
| P15 | The dense atlas grows between frames while zero-entry rows are current. | Growth copies total, intermediate and direct-total atlases (`gpu/growth.rs`); following frames do not recompose those rows. | Zero-entry rows keep copied values, byte-equal to full-resident compose. Before this change every-frame recompose hid a missed copy. Covered only by the manual byte check. |
