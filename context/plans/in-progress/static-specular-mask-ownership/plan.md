# static-specular-mask-ownership — plan of record

mode: resumable
status: approved
read at: 75ac7e12f

Branch `static-specular-mask-ownership`, worktree `.claude/worktrees/static-specular-ownership`.
Claimed on `main` at 75ac7e12f. Since the brief's 4e127a7b8, only the buried-lights merge
touched source. It left the cut, atlas, walk, fill and shader code unchanged, and removed
buried lights from every light namespace. Every symbol that Decisions and Path cite was
re-read at 75ac7e12f in three read-only passes: compiler mask bake, face cuts with light
packing, and runtime.

## Owner amendments

- **Performance matrix before and after (owner, 2026-10-10).** Run this on several maps,
  because each stresses a different part of the renderer. The change must add little
  overhead. A gain is acceptable: recent wins came from cutting how far light travels, and
  this change cuts world specular from the chunk list (up to 17 lights) to at most four
  owners plus the SDF selection. Row M8 holds the protocol, and it subsumes M6.

## Corrections

- **Path: "a forward-only second vertex buffer, filled at load from per-face data through
  face index ranges; precedent `stamp_animated_block_ids`."**
  - **Current fact.** The runtime has no per-face index ranges.
    - `FaceIndexRange` is compile-time only (`level-compiler/src/geometry.rs:14-23`).
    - Id-17 `FaceMeta` holds only `leaf_index` and `texture_index`.
    - `stamp_animated_block_ids` runs in the compiler.
  - **What the runtime does have.** `collect_primitives` (`bvh_build.rs:78`) emits exactly
    one BVH primitive, and so one leaf, per face with at least one triangle, carrying that
    face's `index_offset`/`index_count`. Faces are written contiguously in face order.
  - **Approach.**
    - The loader rebuilds each id-17 face's index range from the BVH leaves sorted by
      `index_offset`, and stamps the face's owner entry onto every vertex its triangles
      reference.
    - The compiler guarantees every id-17 face has a triangle. `extract_geometry` can emit
      a face with fewer than three vertices today. Such a face fails a named check, or is
      dropped before id 17 if one is found in a real map.
    - The loader requires leaf count = face count, with contiguous ascending ranges. On a
      mismatch it logs a `[Loader]` error and leaves the owner stream empty, so the face
      gets no static non-SDF specular.
  - **What stays unchanged.** Vertex stride, id-17 version, the `SMB7` wire table and the
    binding inventory.
- **Path: id-42 presence is keyed on id 40 in three places.** Also:
  - the pack plan writes id 42 only inside `if has_usable_direct_sh_deltas` (needs id 41)
    and when `!section.channels.is_empty()` (`pack/section_plan.rs:100,193,212`);
  - the memo validator checks `channels.len() == selection.len()` (`memo.rs:176-188`);
  - `empty_section_for_selection` (`section.rs:29`);
  - the verbose skip line (`pipeline.rs:1869`).

  All are rekeyed to "any face admits an owner".
- **Refused, absent or placeholder mask pool binds a white placeholder.** Placeholder
  block 0 is flagged RESIDENT (`block_table.rs:54,128`), so owners would read fully lit.
  - A load-fixed "mask present" bit in the frame uniforms adds no binding. It gates all
    static non-SDF specular and the union subtraction.
  - The bit is cleared on refusal (`lighting/lightmap/plan.rs:110,179`), on placeholder,
    and when id 42 is missing. The refusal's `[Renderer]` error text changes from "falls
    back to fully lit" to "adds no static specular".
- **P13 detail.** `raw_visibility_is_covered(NaN)` is true, and `+Inf` quantizes to 255
  (`fill.rs:19,157`). Admission and the contribution sum both gate on `is_finite`.
  `nan_raw_visibility_is_retained_with_legacy_inclusion_semantics` flips.
- **The shadowmask fill never writes padding or unreached interior texels.** They stay
  at 255.
  - When a light becomes a face's owner, its channel is zeroed over the face's padded
    chart rect (`placement.x/y`, `chart.width_texels`/`height_texels`) before its values
    are written.
  - An evicting newcomer does the same. That covers P4, P16 and the
    owned-reads-occluded Decision.
- **Path: "Its SDF half iterates the selection directly."** This describes the target.
  Today, SDF specular rides the chunk-list loop through `sdf_visibility_for_light`, and
  an SDF light outside the selection gets visibility 1.0. The extracted function's SDF
  half loops over `sdf_sel` slots.
- **Owner-eligible set.** The walk's `layer_lights` (`lightmap_stage.rs:258-262`) still
  includes `bake_only` lights, which have no spec index. So the eligible set is
  `StaticBakedLights` ∩ AlphaLights ∩ `StaticLightMap`, mapped `source_index` →
  compact spec index. `compacted_static_lights` (`chunk_light_list_bake.rs:102`) is the
  compiler precedent.
- **Drop warnings come from the stage `finish()` on the caller thread.** The consume
  closure runs on a Rayon worker, and `LogCapture` cannot see it.
- **Cut-face parent.** `face_remap` and the `FaceCut` lines die in `prepare_atlas_stage`
  (`atlas_stage.rs:76-91`), so they are plumbed through `AtlasStageOutput` and
  `PreparedAtlas`.
  - Neighbouring sub-charts overlap by `CUT_OVERLAP_TEXELS = 2`. The parent's exact
    lit-texel count, and the estimate grid, count each parent-grid texel once using
    `[c_i, c_{i+1})` segments.
  - A window the polygon misses emits no sub-face, and gets no samples.
- **Pre-existing, out of scope.** For non-spot lights, `cone_attenuation_cos` gets a zero
  aim and the (1, −1) sentinels, so it returns 0.5. Point and directional world specular,
  and `shadowmask_direct`'s reconstruction, are therefore halved today
  (`forward.wgsl:441,1233,774`), contradicting the `spec_buffer.rs` comment.
  - Kept as is, so union subtraction still matches pre-change output (AC 18). Proposed
    as a follow-up.
  - The directional branch takes `L = −aim`. With the aim packed, `cone_attenuation_cos` reads
    `dot(−L, aim) = 1`, so a directional highlight gets a cone factor of 1.0. That is the correct
    value; the halved factor stays on points only, for the follow-up to fix. Directional world
    specular is new behaviour either way: before, it aimed at the entity origin.
  - Billboard loops read `cone_dir_and_type.xyz` (`billboard.wgsl:367,738`). Packing the
    sun's aim must leave billboard output unchanged.

- **Tasks 2 and 3 land as one commit, and the format change goes expand-then-contract.**
  The SMB7 codec takes the final names in `level-format::shadowmask_atlas`. The SMB6 codec
  moves unchanged to a temporary `shadowmask_atlas_smb6` module, and every consumer now
  imports it from there. Each consumer switches to SMB7 in its own task: the compiler in
  task 5, the loader and renderer in tasks 8–9. The legacy module is deleted with the last
  switch, so the branch never carries an id 42 that the runtime cannot read. The split and the
  import move touch the same files, so separate commits would leave a non-compiling state
  between them.
- **Brief "payload length that disagrees with the block arithmetic" (AC 44)** → SMB6 never
  rejected trailing bytes after the last group plane. SMB7 does: a section whose length
  differs from the end of its last blob is refused.

## Delegated answers

- **Lit-texel floor: 16 interior texels.** The census cleared every stress-map overflow
  at 16. The drop report per map lands in the results column of manual rows M5 and M7.
- **Cut-face estimate sampling: area samples.**
  - The estimate calls the walk's own `light_texel_contribution_and_visibility_with`, with
    the walk's seed (`chart_texel_seed`) and sample count. It runs at parent-grid texels
    on a 4 × 4 stride, so spacing follows the floor (√16).
  - Each sample equals the walk's value at that texel, so the estimate can never admit
    padding (P22) or disagree with the walk at a sampled texel.
  - Rays traced and stage time are reported with M7.

## AC-to-proof

Harness rows use the forward-function GPU harness pattern (`shadowmask_sample_test.rs`)
and run with `POSTRETRO_REQUIRE_GPU=1`; without a BC adapter they prove nothing. Compiler
rows are `prl-build` bin tests (`--bin prl-build`).

| AC | Proof | Status |
|---|---|---|
| 1 walled-off light adds zero specular to receiver; reaching light keeps highlight | compiler fixture (wall between light and receiver face: no channel) + harness (non-owner adds 0, owner adds lit) | achievable as stated |
| 2 owns R not S; both fragments at shared edge | harness: two faces sharing an edge with distinct owner streams, sampled each side of the edge | achievable as stated |
| 3 P1 cut sub-faces identical owner sets across layers, incl. one sub-face alone would rank another fourth; own baked values per sub-face | compiler fixture: oversize face cut across two bake layers | achievable as stated |
| 4 cut face: walled-off light no channel; partial light gets one | compiler fixture | achievable as stated |
| 5 P16 owner lighting one sub-face reads zero across the others incl. padding | compiler raw-fill inspection | achievable as stated |
| 6 P22 light behind wall reaching only cut face edge takes no channel | compiler fixture | achievable as stated |
| 7 P14 above-floor light missed by every sample: no channel, warned | compiler fixture with a sample spacing that misses a lit patch + log capture | achievable as stated |
| 8 P15 estimated owner baking no lit texel cleared from owner table | compiler fixture | achievable as stated |
| 9 P17 below-floor promoted light on cut face never beats above-floor specular-only | compiler fixture | achievable as stated |
| 10 within owned face specular follows mask: 0 occluded, full lit | harness | achievable as stated |
| 11 SDF light outside selection 0; inside at SDF visibility | harness | achievable as stated |
| 12 directional highlight along baked direction; moving origin changes nothing | `spec_buffer` pack test + harness | achievable as stated |
| 13 P9 missing data (block miss, missing id 42, placeholder, refused pool all-resident and streamed): no static non-SDF specular, SDF unchanged; refusal logs `[Renderer]` error, no panic | harness (block miss, mask-present bit off) + renderer plan tests for refusal both modes with log capture + loader test for missing id 42 | achievable as stated |
| 14 P4 owner occluded at face edge reads zero incl. padding taps | compiler raw-fill inspection | achievable as stated |
| 15 each owner position reads its own channel in either group; swap invariance | harness over all four positions + `capture_shadowmask_groups` rewritten to swap two owners with their planes | achievable as stated |
| 16 P12 streamed = all-resident at existing streaming poses with owner specular on | `capture_lightmap_streaming` `#[ignore]` captures, `--features capture`, run on the 1660 | achievable as stated |
| 17 static light outside id 40 owns channels and gets shadowed specular | compiler fixture + harness | achievable as stated |
| 18 union subtraction matches pre-change when every promoted light owns its face | harness: new union vs a test-held copy of the pre-change function | achievable as stated |
| 19 promoted light not owning a face subtracts nothing | harness | achievable as stated |
| 20 five lights → four owners by contribution + warning naming fifth; four → no warning | compiler test + log capture | achievable as stated |
| 21 warning names face and light; cut face once as parent with sub-face range; `--verbose` peak | compiler test + log capture | achievable as stated |
| 22 P24 ascending owners whatever arrival order; masks follow | compiler test | achievable as stated |
| 23 P2 warm id-42 memo hit repeats drop warnings and peak | compiler cache test + log capture | achievable as stated |
| 24 P4 eviction leaves none of the evicted values; unreached reads 0 | compiler raw-fill test | achievable as stated |
| 25 promoted smallest keeps channel over brighter specular-only; none promoted → smallest drops | compiler test | achievable as stated |
| 26 P5 below-floor promoted drops silently, never takes from above-floor | compiler test + log capture | achievable as stated |
| 27 P19 below-floor promoted incumbent evicted by four above-floor arrivals | compiler test | achievable as stated |
| 28 3 above + 2 below: last channel to brighter below-floor even when dimmer is promoted | compiler test | achievable as stated |
| 29 directional takes channel on lit face, none under a roof | compiler fixture | achievable as stated |
| 30 fifth below floor: no warning; above floor: warned | compiler test + log capture | achievable as stated |
| 31 P7 tie → lower index whatever arrives first; identical across worker counts | compiler test | achievable as stated |
| 32 P6 id 42 byte-identical under reversed partition order and across window sizes | compiler test (template `reversed_partition_fill_keeps_compact_selection_bytes`) | achievable as stated |
| 33 P18 cut face across two layers: id 42 and report identical across workers, windows, cold/warm/reuse; estimate tie → lower index | compiler test | achievable as stated |
| 34 P8 dynamic/SDF/bake-only before a static light: entries name the filling light; bake-only takes none | compiler test | achievable as stated |
| 35 P13 non-finite visibility neither admits nor contributes | compiler test | achievable as stated |
| 36 light lit only in padding takes no channel | compiler test | achievable as stated |
| 37 occluded on every texel: none; one lit texel: one | compiler test | achievable as stated |
| 38 id 42 byte-identical across workers, cold, warm, and lightmap-reuse rebuild | extend `fused_cold_warm_and_selection_only_paths_match_reference_bytes` | achievable as stated |
| 39 P3 editing a non-id-40 light or only id-40 membership rebuilds id 42 = cold, incl. cached lightmap | same test; flip its unselected-edit memo-hit assertion | achievable as stated |
| 40 reuse rebuild reads each eligible partition ≤ once per layer, one raw fill, no trace beyond estimate | `cache.test_access("lightmap_layer")` counts + raw-fill allocation counter + trace counter | achievable as stated |
| 41 P20 warm id-42 memo hit traces nothing, estimate included | estimate/walk trace counter test | achievable as stated |
| 42 no cut face → no estimate sample; estimate traces only lights whose unshadowed term reaches | trace counter test | achievable as stated |
| 43 P23 estimate density or mode change rebuilds id 42 on warm build | memo key test | achievable as stated |
| 44 load rejects owner ≥ static spec count, duplicate, out of order / after empty, face count ≠ id 17, payload length; `SMB6` asks for re-bake | `level-format` codec tests + loader test | achievable as stated |
| 45 no eligible light in id 40 still emits id 42 and shadows specular | compiler pipeline test | achievable as stated |
| 46 P10 id 40 cleared at load keeps id 42; former promoted owners keep shadowed specular, subtract nothing | loader test (id 41 missing) + harness | achievable as stated |
| 47 eligible lights light no face interior → no id 42 | compiler pipeline test | achievable as stated |
| 48 P11/P21 over the owner-index cap → named error before any lightmap work incl. estimate | compiler test with 65 536 static lights and trace/bake counters | achievable as stated |
| 49 forward binding inventory test passes, snapshot unedited | `forward_bindings_add_only_the_vertex_block_table` | achievable as stated |
| 50 vertex stride 36, id-17 version unchanged | existing `level-format` stride test + version constant untouched | achievable as stated |
| M1 hallway pose (−17, 21, 86): no sheen; 0x40 toggle shows no patch | executor capture at pose, owner visual read | manual |
| M2 campaign-test leak poses p1–p4: no leak specular; lit-surface highlight remains | executor captures (three masks per pose) before/after | manual |
| M3 animated light's world highlight gone, nothing else changes, diffuse animation unchanged | executor captures + owner read | manual |
| M4 campaign-test highlights continuous across 8 m chunk planes | owner, in-engine | manual |
| M5 kinematic-platform floor and movement-feel: drop report read, highlight loss assessed | executor reports drop report; owner assesses visuals | manual |
| M6 1660 dense pose forward GPU ms before/after (`capture,dev-tools`) | subsumed by M8 (hallway arena and sheen poses) | manual |
| M7 hallway cold `--release` before/after: shadowmask + lightmap stage time incl. estimate, peak RSS | owner launches in terminal if over ~2 h; executor otherwise | manual |
| M8 (owner amendment) runtime perf matrix before/after on five maps: no forward or frame-GPU regression past run-to-run spread at any pose; gains reported | executor on the 1660; protocol below | manual |

### M8 protocol

- **Arms.**
  - *Before:* the 75ac7e12f engine, built `--release --features capture,dev-tools` in a
    throwaway target. Copy the binary out and delete the target at once.
  - *After:* the branch head, same profile and features.
  - Each arm loads PRLs compiled by its own compiler from the same `.map` sources (`SMB6`
    for before, `SMB7` for after). Record each source map's SHA-256 prefix.
- **Maps and poses.** Each map stresses something different:

  | Map | Pose | Stresses |
  |---|---|---|
  | `stress-warren-hallway-inspection` | arena `21.13,2.44,30.48,0,0`; sheen `-17,21,86` | dense static lights, chunk lists up to 17: the specular loop this replaces |
  | `campaign-test` | spawn; leak pose p1 | representative content, promoted lights, union subtraction, streaming |
  | `kinematic-platform` | station `-6.5,1.22,-27.94,0,0` | movers (separate pipeline, a control) and a large overflow floor |
  | `movement-feel` | spawn | heaviest owner overflow (17 candidates per face), open sightlines |
  | `sdf-shadow-test` | spawn | SDF selection, whose specular half now loops `sdf_sel` |

- **Metrics per pose.**
  - `forward` and total frame GPU ms (`POSTRETRO_GPU_TIMING=1`).
  - Work CPU and `render_record` (`POSTRETRO_CPU_TIMING=1`).
  - All from complete post-warmup 120-sample capture windows.
  - Per map, also the level-load time (startup log line C, which now includes building the
    owner stream) and the owner buffer's bytes.
- **Method.**
  - GTX 1660 Super on the High performance power plan. Balanced downclocks under light load
    (`balanced-plan-vsync-downclock-hitch`).
  - A capture resolution that saturates the GPU (4K).
  - Before and after runs interleaved in one session, at least three per arm per pose.
  - Report each arm's median and spread.
  - Close the engine after each run.
- **Budget.**
  - At every pose, after's `forward` and frame-GPU medians may not exceed before's by more
    than the larger arm's spread.
  - Load time may not grow past the spread of log line C.
  - A gain passes.
  - A regression past budget goes to the owner before landing, with the
    `light_term_mask` 0x40 delta isolating the specular block's share.
- **Not covered.** The owner's 5300M Mac is CPU-bound at the hallway, and this change adds
  no per-frame CPU work, so a Mac run is an optional owner cross-check.

## Tasks

Each task commits with its plan update.

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | **First slice: owner delivery.** Extract the world specular loop from `fs_main` into a callable function. Add the owner-based static half (4 owners, `0xFFFF` stop, channel = position) and the `sdf_sel` SDF half. Build a GPU-required harness that renders owner-only specular from a hand-built per-vertex owner stream through a flat varying. Also check face-range reconstruction from BVH leaves on shipped PRLs (campaign-test, hallway, kinematic-platform, movement-feel): leaf count = face count, contiguous ranges. Riskiest premises: the flat-varying owner path, and face recovery without changing id 17 | integrating executor | — | done: `world_static_specular` and `world_static_light_specular` in `forward.wgsl`; the old chunk loop calls the per-light helper until task 9. `world_specular_test` runs 6 GPU tests with `POSTRETRO_REQUIRE_GPU=1` on the 1660 (43 passed with the shader and mask-sampling suites). Owners ride a second `Uint16x4` vertex buffer as a flat varying. Face recovery (scratch reader over ids 17/19): campaign-test 774 faces = 774 leaves; kinematic-platform 87; movement-feel 279; sdf-shadow-test 24; hallway 8437; stress-warren-mini 1271; combat-demo 91. All contiguous, covering every index, with disjoint per-face vertex ranges |
| 2 | Split `pipeline/lightmap_stage.rs` and `shadowmask_bake.rs` along their responsibilities, with no behavior change. Move the analytic graph and standalone reference paths out of the root | integrating executor | — | done (with 3): `lightmap_stage.rs` 1671 → 439 lines plus `peak.rs`, `partitions.rs`, `tests.rs`; `shadowmask_bake.rs` 1003 → 437 plus `standalone.rs` and `analytic_graph.rs` (both retire in task 5). No behavior change: 101 tests in those modules pass unedited apart from imports |
| 3 | `level-format` `SMB7`: `SHADOWMASK_OWNER_EMPTY = 0xFFFF`, a face-count header, the owner table, and load rejects (AC 44). Rework the prefix helpers for streaming. `SMB6` becomes a named re-bake. Readers compile against it | integrating executor | 2 | done: SMB7 codec with `ShadowmaskOwnerBounds` and `validate_face_owners`; 18 codec tests cover every AC 44 reject and the SMB5/SMB6 re-bake messages. Consumers moved to `shadowmask_atlas_smb6`. Workspace and `capture,dev-tools` targets check clean; level-format 604, compiler shadowmask and lightmap 110, loader 51, renderer lightmap and shadowmask 82, lighting 65 tests pass |
| 4 | Compiler eligibility and presence: eligible set and `source_index` → spec index map. Owner-index cap error before any lightmap work (AC 48). Decouple id-42 presence from ids 40 and 41 in prepare, pack plan, memo validator and residency dry run (AC 45, 47) | integrating executor | 3 | |
| 5 | In-walk ownership for uncut faces. Per-layer texel → face map. Per-face shadowed sums with an `is_finite` gate. Floor tier, then id-40 tier, then contribution, then index. Eviction zeros the padded rect; an owner's first write zeros unwritten texels. Before encode, sort owners ascending and permute the channel planes. Drop report and `--verbose` peak from `finish()`. Retire the graph and assignment (AC 20–32, 34–37, 14, 24) | integrating executor | 4 | |
| 6 | Cut-face estimate. Plumb `FaceCut` and `face_remap` to the walk. Sample emitted sub-chart interiors on the floor stride with deduplication. Trace only lights whose analytic term reaches. Rank and fix owners before the walk; clear zero-bake owners after it. Exact parent lit-texel count. Report a cut face once, by parent (AC 3–9, 33, 42) | integrating executor | 5 | |
| 7 | Cache. The id-42 memo key covers every eligible light's layer hashes, compact spec indices, id-40 membership, floor, estimate density and mode, and the cut map; bump the stage version. The memo stores the drop report and peak demand. The reuse path reads every eligible partition. Trace and fill counters (AC 23, 38–41, 43) | integrating executor | 6 | |
| 8 | Runtime load and delivery. Parse and validate id 42 against the static spec count and the id-17 face count. Retire the id-40 reconcile. Rebuild face ranges from BVH leaves and build the owner stream into `LevelGeometry`, with the mask-present bit (AC 13, 44, 46) | worker in own worktree, after 3 lands; integrated by executor | 1, 3 | |
| 9 | Renderer and shader. Second vertex buffer at both build sites (dummy when absent) and `@location(9)` flat owners. The owner function replaces the chunk-list specular; union subtraction by ownership. Retire `cone_cos.z`, the channel tables and the metadata channel field. Directional `L = −aim`. Mask-present gate (AC 1, 2, 10–19, 49) | same worker as 8; integrated by executor | 1, 8 | |
| 10 | Engine-level proofs: rewrite `capture_shadowmask_groups` for owner swap, the streaming capture identity (AC 15, 16), and end-to-end compile-and-render fixtures for AC 1 and 17 | integrating executor | 7, 9 | |
| 11 | Manual captures and measurements M1–M8 before and after. Build the pre-change binary from 75ac7e12f in a throwaway target and compile both PRL sets. M8 runs interleaved in one session | integrating executor + owner | 10 | |
| 12 | `/preflight`, then `/review-panel` → `/fix-review-findings` loop, then the full gate | integrating executor | 11 | |
