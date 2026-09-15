# lighting-scale--sh-delta-cone-reach-cull — research

Ephemeral. Grounding and pinned orderings behind `index.md`. Symbols and counts as
of e36e86b; they drift — the brief states only what survives.

## Illustrative scale — stress-warren-hallway-inspection (estimated, not measured this session)

- As committed: 338 promotable spots (`_bake_only 0`, `_shadow_type static_light_map`, 48°
  outer cone, `_falloff_range 1024` ≈ 26 m); 0 animated lights → the OOM is purely id-41.
- Pre-cull dense (id-41 alone): 659,904 `(affinity-cell, light)` CSR entries × 18,432 B/entry
  ≈ 12 GB (matches the `lighting-scale--compile-peak-ram` decomposition).
- Cone-vs-cube solid-angle estimate ≈ 10× fewer id-41 entries — **unmeasured**. The first
  slice measures the post-cull projection with `--sh-delta-working-set-max-size 0`.
- The fixture is being regenerated with a nonzero animated-light share (owner decision), which
  activates id-45 (animated-direct, cone-cullable) and id-27 (indirect, **cube reach, not
  cone-cullable** — bounce leaves the cone). The first-slice measurement is over all three
  active sections, not id-41 alone; an id-27-dominated shortfall is Phase 2's bound.

## Pinned orderings

| id | scenario | ordering pinned | expected outcome |
|----|----------|-----------------|------------------|
| R1 | Spot whose outer-cone surface passes exactly through a candidate probe (`cos θ == cos_outer`). | Boundary evaluation of `spot_cone_attenuation` at decompose vs bake. | `smoothstep(cos_outer, cos_inner, cos_outer) == 0` exactly; probe bakes exact zero. A cell all of whose in-AABB probes sit at/past `cos_outer` may be culled; emitted `.prl` byte-identical. |
| R2 | Spot cone-frustum grazes / clips a corner of a cell AABB, but no id-34-valid probe of that cell is strictly in-cone. | Conservative test resolves a tangent overlap before any tile is baked. | Cell MAY be kept (superset is legal); `drop_direct_zero_entries` then removes it. Never emitted with nonzero bytes; never excluded if any valid probe is in-cone. |
| R3 | One cell straddles the cone: ≥1 id-34-valid probe strictly in-cone, ≥1 out-of-cone. | Conservative-keep vs exact-cull at a cell mixing zero and nonzero probes. | Cell retained in the direct CSR; emitted payload byte-identical (in-cone probes bake nonzero, out-of-cone bake exact zero, handled by valid-probe compaction). |
| R4 | An id-41-selected static-direct light every one of whose affinity cells is cone-culled. | Canonical-entry retention must occur at decomposition (reach predicate / CSR build), not solely in `drop_direct_zero_entries`. | Exactly one canonical entry for that selection index survives in emitted `affinity_lights`; id-40/id-41 all-or-nothing load contract holds. (Mechanism is an open owner decision — see index.md Open questions.) |
| R5 | One animated spot light contributing to BOTH id-27 (indirect) and id-45 (animated direct). | Transport is per-decompose-call: id-27 (`decompose_affinity`) stays cube; id-45's `decompose_affinity_for_lights` becomes cone. | For that light, id-27 CSR byte-identical (cube reach) while id-45 CSR cone-clamped. id-45 is in Phase 1 scope (owner decision); the executor reverses the `animated_direct_sh_bake.rs` intentional-no-clip comment after confirming the frozen rest-direction cone matches the bake. |
| R6 | A `light_sun` (`LightType::Directional`, `cone_angle_*` = None). | Directional lights bypass the cone clamp entirely; reach stays whole-world. | Every affinity cell overlapping the directional world-AABB retained; id-35/id-41 sun bytes byte-identical. |
| R7 | Warm-cache build after the cull, and a cull-affecting edit (e.g. widening a spot cone). | Stage-version bump vs per-`(cell,light)` delta cache key vs base-direct cache key. | Two warm builds under the new predicate byte-identical; a cone-widening edit re-keys affected `(cell,light)` sub-blocks (miss) and reuses the rest (hit); a warm cache written by the pre-change binary is invalidated via the stage-version bump. |
| R8 | Two independent runs of the BC6H encode over identical input tiles for the at-rest sections entering the byte-identity comparison. | Run-to-run BC6H determinism is an assumption under the byte-identity ACs; pin it beside the SHA-256. | id-35 base direct (`encode_direct_section_bc6h`) and id-34 base BC6H are bitwise stable run-to-run for identical input. id-22 lightmap is BC6H but exempt from byte-identity, so the `.prl` hash must exclude/tolerate it. NB: id-41/id-45 delta payloads are f16 (`delta_subblocks`), not BC6H — the BC6H at-rest section in scope is id-35. |

## SHA-256 (byte-identity fixtures)

Recorded at implementation: `<id-41 fixture .prl>` and `<id-45 fixture .prl>`, cold and warm,
before and after the change. Populated by the executor.
