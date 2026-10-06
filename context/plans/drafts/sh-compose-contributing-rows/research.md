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
- `plans/done/perf-sh-compose-sampled-row-gating` set "today's pass-level trigger, restricted to gated contributing rows". There, "contributing" meant the section's domain: indirect scoped 4,214 of 8,127 resident rows. This brief narrows that definition; it does not repair drift. That plan deferred *per-light* row scoping, which needs per-light dirty tracking and has no dirty signal for curve samples. This brief uses static entry presence and needs no dirty signal.
- The spike projected the filter alone at the arena: indirect 6.71 → 0.52 ms and Pass B 8.00 → 0.86 ms. That is `c + r · rows_f + entry share`, with `c` = 0.24 / 0.26 ms, `r` = 2.99 / 3.46 µs, and `rows_f` = 70. It extrapolates below the fitted row range (730–2579).

## Exemption
The exemption is not lifted here. What the research found:
- `ScriptMutableDescriptorSlots` (`delta_drop_policy.rs`) marks every `is_animated` light and every `LightMembershipManifest` target as mutable. Mutable lights keep zero-payload entries and cube reach.
- Its stated rationale (`build_pipeline.md`, id 45): "later curve replacement can make an at-rest zero record nonzero". The cone-cull exemption followed from it for byte identity (`plans/done/lighting-scale--sh-delta-cone-reach-cull/research.md`).
- Payload is unit-radiance transport, and the only script surface is `setLightAnimation`. So a zero payload looks geometric and curve-invariant. That is an inference; no plan states it.
- Unchecked: whether an animated radius curve (`eval_animated_radius`) or entity follow (`cached_follow_positions`) can move a baked light's reach, and how `sh_runtime_envelope.rs` uses the mutable mask.
- Lifting the exemption would shrink entry rows further at the kinematic poses (780 entry rows). It needs its own grounding.
