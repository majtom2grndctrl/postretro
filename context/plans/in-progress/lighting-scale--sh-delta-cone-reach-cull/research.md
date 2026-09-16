# lighting-scale--sh-delta-cone-reach-cull — research

Ephemeral. Grounding and pinned orderings behind `index.md`. Symbols and counts as
of e36e86b; they drift — the brief states only what survives.

## Illustrative scale — stress-warren-hallway-inspection

- Pre-regeneration baseline (e36e86b): 338 promotable spots (`_bake_only 0`, `_shadow_type
  static_light_map`, 48° outer cone, `_falloff_range 1024` ≈ 26 m); 0 animated lights → the
  OOM was purely id-41.
- Pre-cull dense (id-41 alone): 659,904 `(affinity-cell, light)` CSR entries × 18,432 B/entry
  ≈ 12 GB (matches the `lighting-scale--compile-peak-ram` decomposition).
- The cone-vs-cube solid-angle estimate of ≈10× fewer id-41 entries did not hold. The
  production planner measured a 29% id-41 dense-byte reduction at the practical 10 m
  probe spacing and a post-cull 5.97 GB projected peak at the default 1 m spacing.
- The fixture was regenerated with a nonzero animated-light share (owner decision), which
  activates id-45 (animated-direct, cone-cullable) and id-27 (indirect, **cube reach, not
  cone-cullable** — bounce leaves the cone). The first-slice measurement is over all three
  active sections, not id-41 alone; an id-27-dominated shortfall is Phase 2's bound.

## Implementation measurements

- Regenerated fixture (`tools/gen_stress_map.py --preset warren`) has 763 static
  baked lights: 678 spots, 85 points, 344 runtime-present/promotable lights, and
  exactly 6 animated spotlights (the generator's bounded animation budget).
- Pre-cull control at the fixture's practical documented settings
  (`--sh-probe-spacing 10.0 --lightmap-density 0.25`, cold, zero working-set
  budget) projected 50,208,768 cumulative dense bytes and 150,626,304 bytes at
  the normal 3× copy-chain factor: id 27 = 1,990,656; id 41 = 46,227,456;
  id 45 = 1,990,656. This coarse-spacing control was already below 16 GiB and
  is not the brief's default-1m risk measurement.
- Post-cull control with the same 10 m / 0.25 settings projected 35,260,416
  cumulative dense bytes and 105,781,248 bytes at 3×: id 27 = 1,990,656
  (unchanged), id 41 = 32,864,256 (29% below the 46,227,456-byte control),
  and id 45 = 405,504 (80% below the 1,990,656-byte control).
- Post-cull at the default 1 m SH probe spacing, the production CSR planner
  projected 1,989,642,240 cumulative dense bytes and a 5,968,926,720-byte
  peak at the normal 3× factor: id 27 = 363,184,128; id 41 = 1,616,283,648;
  id 45 = 10,174,464. This is 11,210,942,464 bytes below the unchanged
  16 GiB default gate. The measurement used a temporary, uncommitted bypass
  of the base-SH ray bake so the real post-selection CSR planner and gate could
  run without materializing unrelated base irradiance; the bypass was removed
  immediately after capture and is absent from the implementation.
- The admitted end-to-end warren build completed under the unchanged default
  16 GiB gate with `--sh-probe-spacing 10.0 --lightmap-density 0.25 --no-cache`.
  It finished in 297.96 s and emitted a 66 MiB PRL with SHA-256
  `1c470f92d151bf0e65e1b4f9aecc9bc35edd019f2f0267eae45fc987456beab7`.
- Pre-change `campaign-test.map` whole-PRL SHA-256 baselines (current compiler,
  before cone reach): cold `--no-cache` =
  `56aaa1a77eeac66f57bf34902e3d7097c2f0aa68a5e639736ed0b38904224671`;
  cache-enabled empty-cache build =
  `84d57c5f11836241712eaea6b6327fb7f2250475e1c0033ebf4fc792dc77ce06`.
  The hashes differ because warm base indirect SH is intentionally approximate;
  each mode is compared only with the same mode after the reach change.
- The first post-change cold comparison exposed an existing exception to id-45
  exact-zero dropping: script-mutable animated descriptor slots deliberately
  retain cube-reach zero records for future curve replacement. Cone-clipping
  those slots changed ids 45 and 48. The final policy therefore leaves only
  script-mutable id-45 slots unclipped; ordinary animated spots (including all
  six warren lights) remain cone-clipped. A focused policy regression pins this.

## Pinned orderings

| id | scenario | ordering pinned | expected outcome |
|----|----------|-----------------|------------------|
| R1 | Spot whose outer-cone surface passes exactly through a candidate probe (`cos θ == cos_outer`). | Boundary evaluation of `spot_cone_attenuation` at decompose vs bake. | `smoothstep(cos_outer, cos_inner, cos_outer) == 0` exactly; probe bakes exact zero. A cell all of whose in-AABB probes sit at/past `cos_outer` may be culled; emitted `.prl` byte-identical. |
| R2 | Spot cone-frustum grazes / clips a corner of a cell AABB, but no id-34-valid probe of that cell is strictly in-cone. | Conservative test resolves a tangent overlap before any tile is baked. | Cell MAY be kept (superset is legal); `drop_direct_zero_entries` then removes it. Never emitted with nonzero bytes; never excluded if any valid probe is in-cone. |
| R3 | One cell straddles the cone: ≥1 id-34-valid probe strictly in-cone, ≥1 out-of-cone. | Conservative-keep vs exact-cull at a cell mixing zero and nonzero probes. | Cell retained in the direct CSR; emitted payload byte-identical (in-cone probes bake nonzero, out-of-cone bake exact zero, handled by valid-probe compaction). |
| R4 | An id-41-selected static-direct light every one of whose affinity cells is cone-culled. | Canonical-entry retention occurs at the cull (reach predicate / CSR build), not solely in `drop_direct_zero_entries`; the retained entry is the SAME canonical cell `drop_direct_zero_entries` keeps today. | Exactly one canonical entry for that selection index survives in emitted `affinity_lights`, byte-identical to today's canonical pick; id-40/id-41 all-or-nothing contract holds. A promoted light's baked far-LOD may legitimately be zero while its runtime near-tier term is not, so the entry is semantically required — the light stays promoted. |
| R5 | One animated spot light contributing to BOTH id-27 (indirect) and id-45 (animated direct). | Transport is per-decompose-call: id-27 (`decompose_affinity`) stays cube; id-45's `decompose_affinity_for_lights` becomes cone. | For that light, id-27 CSR byte-identical (cube reach) while id-45 CSR cone-clamped. id-45 is in Phase 1 scope (owner decision); the executor reverses the `animated_direct_sh_bake.rs` intentional-no-clip comment after confirming the frozen rest-direction cone matches the bake. |
| R6 | A `light_sun` (`LightType::Directional`, `cone_angle_*` = None). | Directional lights bypass the cone clamp entirely; reach stays whole-world. | Every affinity cell overlapping the directional world-AABB retained; id-35/id-41 sun bytes byte-identical. |
| R7 | Warm-cache build after the cull, and a cull-affecting edit (e.g. widening a spot cone). | Stage-version bump vs per-`(cell,light)` delta cache key vs base-direct cache key. | Two warm builds under the new predicate byte-identical; a cone-widening edit re-keys affected `(cell,light)` sub-blocks (miss) and reuses the rest (hit); a warm cache written by the pre-change binary is invalidated via the stage-version bump. |
| R8 | Two independent runs of the BC6H encode over identical input tiles for the at-rest sections entering the byte-identity comparison. | Run-to-run BC6H determinism is an assumption under the byte-identity ACs; pin it beside the SHA-256. | id-35 base direct (`encode_direct_section_bc6h`) and id-34 base BC6H are bitwise stable run-to-run for identical input. id-22 lightmap is BC6H but exempt from byte-identity, so the `.prl` hash must exclude/tolerate it. NB: id-41/id-45 delta payloads are f16 (`delta_subblocks`), not BC6H — the BC6H at-rest section in scope is id-35. |

## SHA-256 (byte-identity fixtures)

`campaign-test.map` exercises ids 27, 35, 41, 45, and 48. Whole-file hashes are
identical before/after within each supported bake mode:

- exact cold (`--no-cache`), pre and post:
  `56aaa1a77eeac66f57bf34902e3d7097c2f0aa68a5e639736ed0b38904224671`
- approximate warm, pre, first post run against the old cache, and second post
  run against the updated cache:
  `84d57c5f11836241712eaea6b6327fb7f2250475e1c0033ebf4fc792dc77ce06`

Relevant section hashes are also identical before/after and stable across the
two post-change warm runs:

| section | cold SHA-256 | warm SHA-256 |
|---|---|---|
| id 35 | `17391eb88ec87dc6e761831b61f1b7e7eb927b475c7cf76b5f747fae4ae77ad5` | `f36809994d3f04afd36962cd9b96ef2aef8e26006dda46b4c6ba26f41f0ffeff` |
| id 41 | `21c159e9ca7a7f08b391d707333868aa5288b7c37a4fc255897c54937890da11` | same as cold |
| id 45 | `5cb1ea6c70329a877e02da1dd9c866cd33e4c4cc34044ce2146bd62eee261257` | same as cold |
| id 48 | `185423fbb38e52f2f7a844d91dde692ab12bed46851455b664cf6deceab91607` | same as cold |

The expected cold/warm whole-file difference is isolated to approximate base
SH content (including id 35); direct-delta ids 41/45 remain identical across
both modes.
