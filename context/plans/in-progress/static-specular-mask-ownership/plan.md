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
- **Review-panel owner calls (owner, 2026-10-10).**
  - *Mask padding:* owned channels pad with their owner's dilated edge values, the
    lightmap's rule, instead of zero. A zero pad halved every owned highlight at face
    edges. The Decision on owned channels is reworded to match.
  - *SDF directional specular:* stays positional, keyed on the light's origin like its
    SDF diffuse and shadow trace. The directional Decision and its Acceptance row now
    name owners. This reverses the build-time Correction that had SDF specular follow
    the aim.
  - *Estimate sample weight (P17):* accepted as recorded. One lit sample counts as
    `ESTIMATE_STRIDE²` texels, so a sub-floor promoted light on a lattice point can rank
    above the floor. It touches only cut faces, the drop report names any light it
    costs, and it is revisited only if a real map's drop report shows it.

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
      mismatch it logs a `[PRL]` error and leaves the owner stream empty, so the face
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

- **Residency dry run: the "omitted for width" state retires.** It inferred an omitted
  id 42 from a non-empty id 40, which ownership makes wrong. Under ownership, an absent id 42
  means no face has an owner. The state, its restored-mask columns, and their three tests are
  removed. The dry run reads id 42 with bounds taken from the section's own header
  (`ShadowmaskOwnerBounds::from_section_header`), since it holds neither id 17 nor the lights.
- **`cache_cross_bake_tests` density fixture now has no id 42.** It is one vertical triangle
  whose normal faces away from its lights, so no face admits an owner (AC 47). Its guard now
  asserts that the shadowmask memo was probed, rather than that a section exists.
- **Owner-fill design.**
  - The raw fill starts at 0 rather than 255, so padding and unwritten texels of an owned
    channel read occluded.
  - An evicted owner's channel is zeroed over the face's padded chart.
  - One bake layer's texel→face map (`u32`) is live at a time, and the peak model now charges
    it.
  - A face's sums are complete within one partition: an uncut face lies in one bake layer.
  - Ranking uses summed `(r + g + b) × quantized visibility`.
  - Drop warnings name the light by its parse-time source label (as the buried-light warning
    does), and come from the stage's caller thread.
- **Flake under CPU load.** `window_tests::lightmap_window_cache_io_holds_no_permit` hit its
  10 s timeout once, while the runtime worker's cold build loaded the machine. It passes alone
  and in two further full runs (about 6 s each).

- **Cut-face estimate as built.** `apply_face_cuts` now records a `CutParent` per cut face:
  the pre-cut index, the emitted sub-face range, and each sub-face's own segment of the parent
  grid. These reach `PreparedAtlas.cut_parents`.
  - The estimate samples the parent-grid lattice `g ≡ 2 (mod 4)` on both axes. An own segment
    that holds no lattice coordinate on an axis (a sub-face one to three texels across) is
    sampled at its centre on that axis instead, so a thin cut face still gets samples; the mode
    tag in the memo key names this placement. Each sample goes through the emitted sub-face
    whose own segment holds it, so samples never sit in padding or in a piece the polygon
    missed.
  - A light is traced only where its unshadowed term reaches the sample, after a parent-box
    reach cull.
  - Ranking reads `lit_samples × 16` and `contribution × 16`.
  - The walk counts each cut face's exact lit texels per light, each parent-grid texel once by
    its own segment. Those counts drive the post-walk clear (P15), the drop report, and the
    peak demand.
  - The memo key folds the stride, the mode tag and the cut map.
- **AC 9 (P17), clarification of the same meaning.** The Decision ranks a cut face on the
  estimate scaled to area, so one lit sample reads as 16 texels. A promoted light the lattice
  happens to sample therefore ranks in the upper tier however few texels it really lights. The
  row's "never" holds wherever the estimate is right, and no sparse estimate can promise more.
  - The test uses a promoted light that lights five texels between lattice points.
  - The row's clause "one sub-face alone would put it above the floor" cannot arise. A sub-face's
    texels are a subset of its parent's, so a sub-face count never exceeds the parent count.
  - Flagged for the owner at landing.
- **AC 7 (P14) proof is split.** A real light that lights 16+ texels while missing every
  lattice point needs slit geometry aligned to the lattice. Instead, two parts:
  - the lattice itself (`the_estimate_traces_only_cut_faces_and_lights_that_reach_them`: 576
    samples on the 24 m floor);
  - a fill test whose fixed owners leave the missed light out, which then takes no channel and
    is warned once, by its parent, with its de-duplicated count.

- **Runtime as built (worker, integrated by the executor).**
  - `build_selection_spec_light_indices` stays: the union needs the metadata's compact spec
    index to find a promoted light among the owners. Only the channel table and the metadata
    channel field retire; `meta1.z` is a reserved −1.0, and the record stride is unchanged.
  - The frame uniforms have no padding (all 128 bytes are used). The trailing
    `spec_shadowmask_force_one` word became a flags word: bit 0 force-one, bit 1 mask present.
    Its WGSL name is `spec_shadowmask_flags`.
  - An SDF directional light (`light_sun` with `_shadow_type sdf`) is valid authoring.
    - Its SDF diffuse keeps today's cone factor through `positional_cone_aim`, so its diffuse
      output is byte-identical.
    - Its SDF specular stays positional as well: keyed on its origin, with the zero-aim cone,
      exactly as before the change. Its visibility slice is traced toward that origin, and the
      directional Decision follows a bake an SDF light does not have. Only owner specular
      follows a directional light's aim (review fix;
      `sdf_directional_specular_stays_keyed_on_its_origin`).
  - World specular skips the two mask samples on a face with no owner, or when owners are
    unreadable. Owners are unreadable at a block miss and at the no-lightmap entry, whose
    all-visible read is no owner's mask (review fix). The union keeps reading the mask there;
    no valid compile gives a no-lightmap face an owner. `sample_shadowmask_atlas` uses
    `textureSampleLevel`, so the branch is uniformity-safe.
  - `pool_sample_test.rs` called the retired select. It now tests the owners-readable gate
    against the real pool.
  - Two `capture_shadowmask_groups.rs` tests cannot be expressed in SMB7, because owners must
    ascend. They were removed for task 10 to rewrite. The no-shadowmask capture now expects
    no static specular.
  - Two P10 tests flipped: id 42 is kept when ids 40/41 clear.
  - The compiler now refuses to write id 42 when a Geometry face has no triangle, with a named
    error (`pack_refuses_shadowmask_owners_when_a_face_has_no_triangle`). That closes the
    loader's one-leaf-per-face seam.
  - The legacy `shadowmask_atlas_smb6` module is deleted.
  - `cargo clippy -p postretro --features capture,dev-tools` has 4 errors, in capture files this
    branch does not touch (`prepared.rs`, `report.rs`, `scene.rs`). They are not introduced
    here.
- **Occluded-edge padding after dilation (clarifies the occluded-edge Acceptance row; same
  meaning).** Padding repeats the owner's edge by the lightmap's 8-neighbour rule (owner,
  2026-10-10). Beside a run of occluded edge texels the padding is zero, so a tap past an
  occluded edge reads zero. Where that run ends next to a lit texel, the padding blends
  the two, which is the same texel-scale softening bilinear sampling already gives the
  shadow boundary inside the face. No padding texel ever holds another light's value.
  Test: `an_owner_occluded_along_a_face_edge_pads_that_edge_with_zero`, with a full-grid
  check against the reference rule.

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

Results: the full gate on 2026-10-10 (fmt, clippy `-D warnings`, `cargo test` at 10 134
passed, `cargo check --release`, crate graph), with the GPU harnesses and captures on the
1660 under `POSTRETRO_REQUIRE_GPU=1`.

| AC | Proof | Status | Result |
|---|---|---|---|
| 1 walled-off light adds zero specular to receiver; reaching light keeps highlight | compiler fixture (wall between light and receiver face: no channel) + harness (non-owner adds 0, owner adds lit) | achievable as stated | pass |
| 2 owns R not S; both fragments at shared edge | harness: two faces sharing an edge with distinct owner streams, sampled each side of the edge | achievable as stated | pass |
| 3 P1 cut sub-faces identical owner sets across layers, incl. one sub-face alone would rank another fourth; own baked values per sub-face | compiler fixture: oversize face cut across two bake layers | achievable as stated | pass |
| 4 cut face: walled-off light no channel; partial light gets one | compiler fixture | achievable as stated | pass |
| 5 P16 owner lighting one sub-face reads zero across the others incl. padding | compiler raw-fill inspection | achievable as stated | pass |
| 6 P22 light behind wall reaching only cut face edge takes no channel | compiler fixture | achievable as stated | pass |
| 7 P14 above-floor light missed by every sample: no channel, warned | compiler fixture with a sample spacing that misses a lit patch + log capture | achievable as stated | pass |
| 8 P15 estimated owner baking no lit texel cleared from owner table | compiler fixture | achievable as stated | pass |
| 9 P17 below-floor promoted light on cut face never beats above-floor specular-only | compiler fixture | achievable as stated | pass; the owner accepted the estimate's 16-texel sample weight as a recorded limit (owner amendments) |
| 10 within owned face specular follows mask: 0 occluded, full lit | harness | achievable as stated | pass |
| 11 SDF light outside selection 0; inside at SDF visibility | harness | achievable as stated | pass |
| 12 directional highlight along baked direction; moving origin changes nothing | `spec_buffer` pack test + harness | achievable as stated | pass for owners; SDF directional lights stay keyed on their origin (owner amendment, `sdf_directional_specular_stays_keyed_on_its_origin`) |
| 13 P9 missing data (block miss, missing id 42, placeholder, refused pool all-resident and streamed): no static non-SDF specular, SDF unchanged; refusal logs `[Renderer]` error, no panic | harness (block miss, mask-present bit off) + renderer plan tests for refusal both modes with log capture + loader test for missing id 42 | achievable as stated | pass |
| 14 P4 owner occluded at face edge reads zero incl. padding taps | compiler raw-fill inspection | achievable as stated | pass, as clarified under Corrections (occluded-edge padding after dilation) |
| 15 each owner position reads its own channel in either group; swap invariance | harness over all four positions + `capture_shadowmask_groups` rewritten to swap two owners with their planes | achievable as stated | pass |
| 16 P12 streamed = all-resident at existing streaming poses with owner specular on | `capture_lightmap_streaming` `#[ignore]` captures, `--features capture`, run on the 1660 | achievable as stated | pass, 3/3 on the 1660, re-run after the review fixes |
| 17 static light outside id 40 owns channels and gets shadowed specular | compiler fixture + harness | achievable as stated | pass |
| 18 union subtraction matches pre-change when every promoted light owns its face | harness: new union vs a test-held copy of the pre-change function | achievable as stated | pass |
| 19 promoted light not owning a face subtracts nothing | harness | achievable as stated | pass |
| 20 five lights → four owners by contribution + warning naming fifth; four → no warning | compiler test + log capture | achievable as stated | pass |
| 21 warning names face and light; cut face once as parent with sub-face range; `--verbose` peak | compiler test + log capture | achievable as stated | pass |
| 22 P24 ascending owners whatever arrival order; masks follow | compiler test | achievable as stated | pass |
| 23 P2 warm id-42 memo hit repeats drop warnings and peak | compiler cache test + log capture | achievable as stated | pass |
| 24 P4 eviction leaves none of the evicted values; unreached reads 0 | compiler raw-fill test | achievable as stated | pass |
| 25 promoted smallest keeps channel over brighter specular-only; none promoted → smallest drops | compiler test | achievable as stated | pass |
| 26 P5 below-floor promoted drops silently, never takes from above-floor | compiler test + log capture | achievable as stated | pass |
| 27 P19 below-floor promoted incumbent evicted by four above-floor arrivals | compiler test | achievable as stated | pass |
| 28 3 above + 2 below: last channel to brighter below-floor even when dimmer is promoted | compiler test | achievable as stated | pass |
| 29 directional takes channel on lit face, none under a roof | compiler fixture | achievable as stated | pass |
| 30 fifth below floor: no warning; above floor: warned | compiler test + log capture | achievable as stated | pass |
| 31 P7 tie → lower index whatever arrives first; identical across worker counts | compiler test | achievable as stated | pass |
| 32 P6 id 42 byte-identical under reversed partition order and across window sizes | compiler test (template `reversed_partition_fill_keeps_compact_selection_bytes`) | achievable as stated | pass |
| 33 P18 cut face across two layers: id 42 and report identical across workers, windows, cold/warm/reuse; estimate tie → lower index | compiler test | achievable as stated | pass |
| 34 P8 dynamic/SDF/bake-only before a static light: entries name the filling light; bake-only takes none | compiler test | achievable as stated | pass |
| 35 P13 non-finite visibility neither admits nor contributes | compiler test | achievable as stated | pass |
| 36 light lit only in padding takes no channel | compiler test | achievable as stated | pass |
| 37 occluded on every texel: none; one lit texel: one | compiler test | achievable as stated | pass |
| 38 id 42 byte-identical across workers, cold, warm, and lightmap-reuse rebuild | extend `fused_cold_warm_and_selection_only_paths_match_reference_bytes` | achievable as stated | pass |
| 39 P3 editing a non-id-40 light or only id-40 membership rebuilds id 42 = cold, incl. cached lightmap | same test; flip its unselected-edit memo-hit assertion | achievable as stated | pass |
| 40 reuse rebuild reads each eligible partition ≤ once per layer, one raw fill, no trace beyond estimate | `cache.test_access("lightmap_layer")` counts + raw-fill allocation counter + trace counter | achievable as stated | pass |
| 41 P20 warm id-42 memo hit traces nothing, estimate included | estimate/walk trace counter test | achievable as stated | pass |
| 42 no cut face → no estimate sample; estimate traces only lights whose unshadowed term reaches | trace counter test | achievable as stated | pass |
| 43 P23 estimate density or mode change rebuilds id 42 on warm build | memo key test | achievable as stated | pass |
| 44 load rejects owner ≥ static spec count, duplicate, out of order / after empty, face count ≠ id 17, payload length; `SMB6` asks for re-bake | `level-format` codec tests + loader test | achievable as stated | pass |
| 45 no eligible light in id 40 still emits id 42 and shadows specular | compiler pipeline test | achievable as stated | pass |
| 46 P10 id 40 cleared at load keeps id 42; former promoted owners keep shadowed specular, subtract nothing | loader test (id 41 missing) + harness | achievable as stated | pass |
| 47 eligible lights light no face interior → no id 42 | compiler pipeline test | achievable as stated | pass |
| 48 P11/P21 over the owner-index cap → named error before any lightmap work incl. estimate | compiler test with 65 536 static lights and trace/bake counters | achievable as stated | pass at the stage; the pipeline-level early check has no test of its own (needs a 65 536-light map) |
| 49 forward binding inventory test passes, snapshot unedited | `forward_bindings_add_only_the_vertex_block_table` | achievable as stated | pass |
| 50 vertex stride 36, id-17 version unchanged | existing `level-format` stride test + version constant untouched | achievable as stated | pass |
| M1 hallway pose (−17, 21, 86): no sheen; 0x40 toggle shows no patch | executor capture at pose, owner visual read | manual | outstanding: no sheen in either arm at a four-yaw sweep; the owner confirms the view or records it as gone before this change |
| M2 campaign-test leak poses p1–p4: no leak specular; lit-surface highlight remains | executor captures (three masks per pose) before/after | manual | pass (owner read, 2026-10-10) |
| M3 animated light's world highlight gone, nothing else changes, diffuse animation unchanged | executor captures + owner read | manual | pass (owner read, 2026-10-10) |
| M4 campaign-test highlights continuous across 8 m chunk planes | owner, in-engine | manual | pass (owner read in-engine, 2026-10-10) |
| M5 kinematic-platform floor and movement-feel: drop report read, highlight loss assessed | executor reports drop report; owner assesses visuals | manual | pass (owner read in-engine, 2026-10-10) |
| M6 1660 dense pose forward GPU ms before/after (`capture,dev-tools`) | subsumed by M8 (hallway arena and sheen poses) | manual | pass, within M8 |
| M7 hallway cold `--release` before/after: shadowmask + lightmap stage time incl. estimate, peak RSS | owner launches in terminal if over ~2 h; executor otherwise | manual | pass: lightmap + shadowmask 393.3 → 393.1 s, peak 4.31 → 4.32 GiB |
| M8 (owner amendment) runtime perf matrix before/after on five maps: no forward or frame-GPU regression past run-to-run spread at any pose; gains reported | executor on the 1660; protocol below | manual | pass at every pose; `forward` up to −23%; see §Task 11 results |

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
| 4 | Compiler eligibility and presence: eligible set and `source_index` → spec index map. Owner-index cap error before any lightmap work (AC 48). Decouple id-42 presence from ids 40 and 41 in prepare, pack plan, memo validator and residency dry run (AC 45, 47) | integrating executor | 3 | done with 5: `shadowmask_bake/eligibility.rs`. `check_owner_index_capacity` runs first in `bake_fused_windowed`. Id 42 is decoupled from ids 40/41 in prepare, the pack plan, the memo and the dry run. Tests: `eligibility_skips_dynamic_sdf_bake_only_and_animated_lights`, `the_owner_index_cap_is_the_u16_range_less_the_empty_sentinel`, `too_many_static_spec_lights_fail_before_any_lightmap_work`, `fused_stage_progress_is_exact_and_a_cleared_selection_keeps_the_shadowmask`, `pack_write_emits_shadowmask_whatever_the_entity_shadow_sections_hold`, `an_occluded_light_takes_no_channel_and_one_lit_texel_takes_one` |
| 5 | In-walk ownership for uncut faces. Per-layer texel → face map. Per-face shadowed sums with an `is_finite` gate. Floor tier, then id-40 tier, then contribution, then index. Eviction zeros the padded rect; an owner's first write zeros unwritten texels. Before encode, sort owners ascending and permute the channel planes. Drop report and `--verbose` peak from `finish()`. Retire the graph and assignment (AC 20–32, 34–37, 14, 24) | integrating executor | 4 | done for uncut faces. `ownership.rs` (`Candidate::rank_cmp`, `FaceChannels`, `OwnerReport`) and `fill.rs` (`OwnershipFill`). The analytic graph, assignment, standalone reference and section modules are deleted, and the compiler no longer imports SMB6. 22 unit tests in `shadowmask_bake::tests` plus stage tests `occluded_lights_and_suns_own_only_the_faces_they_reach`, `the_drop_report_names_light_and_face_and_repeats_on_a_warm_memo_hit` and `fused_cold_warm_and_lightmap_reuse_paths_match_cold_bytes`. Compiler: lib 186, bin 1441, integration 8 pass. Cut faces still rank per sub-face until task 6 |
| 6 | Cut-face estimate. Plumb `FaceCut` and `face_remap` to the walk. Sample emitted sub-chart interiors on the floor stride with deduplication. Trace only lights whose analytic term reaches. Rank and fix owners before the walk; clear zero-bake owners after it. Exact parent lit-texel count. Report a cut face once, by parent (AC 3–9, 33, 42) | integrating executor | 5 | done: `shadowmask_bake/estimate.rs`, plus cut support in `fill.rs`. Unit tests: `a_light_the_estimate_missed_takes_no_channel_and_is_reported_once_by_its_parent`, `an_estimated_owner_that_bakes_nothing_is_cleared_after_the_walk`, `a_cut_owner_lighting_one_sub_face_reads_zero_across_the_other`, `estimate_parameters_carry_stride_mode_and_cut_map`. Stage tests, on a 24 m floor cut 2×2 across four bake layers: `cut_sub_faces_share_one_owner_set_ranked_over_the_whole_face`, `cut_face_owners_are_identical_across_workers_windows_and_cache_paths`, `cut_face_admission_needs_a_lit_interior_texel`, `a_small_promoted_light_on_a_cut_face_never_displaces_an_above_floor_light`, `the_estimate_traces_only_cut_faces_and_lights_that_reach_them`. Compiler: bin 1450, lib 186, integration 8 |
| 7 | Cache. The id-42 memo key covers every eligible light's layer hashes, compact spec indices, id-40 membership, floor, estimate density and mode, and the cut map; bump the stage version. The memo stores the drop report and peak demand. The reuse path reads every eligible partition. Trace and fill counters (AC 23, 38–41, 43) | integrating executor | 6 | partly done with 5: the key covers eligible spec indices, id-40 membership, layer hashes, face count and floor (stage version 7). The entry carries the drop report, and the reuse path reads every eligible partition (AC 23, 38, 39, 40 proven). The estimate's parameters and cut map joined the key in task 6. AC 41: a memo hit logs no estimate. AC 42: no cut face means no estimate; only reaching lights are traced. AC 43: params carry stride, mode and cut map, and the key changes with them. Done |
| 8 | Runtime load and delivery. Parse and validate id 42 against the static spec count and the id-17 face count. Retire the id-40 reconcile. Rebuild face ranges from BVH leaves and build the owner stream into `LevelGeometry`, with the mask-present bit (AC 13, 44, 46) | worker in own worktree, after 3 lands; integrated by executor | 1, 3 | done: `level-loader/src/prl_spec_owners.rs` with its tests. Bounds-checked SMB7 read in both load modes; the reconcile is retired. Tests: `load_prl_rejects_owners_the_level_contradicts_and_the_retired_slot_layout`, `a_bogus_face_count_is_rejected_without_reading_the_owner_table`, `load_prl_keeps_shadowmask_owners_when_missing_direct_deltas_clear_the_selection`, `load_prl_without_shadowmask_gives_every_vertex_no_owners`, `load_prl_stamps_each_faces_owners_onto_its_vertices` |
| 9 | Renderer and shader. Second vertex buffer at both build sites (dummy when absent) and `@location(9)` flat owners. The owner function replaces the chunk-list specular; union subtraction by ownership. Retire `cone_cos.z`, the channel tables and the metadata channel field. Directional `L = −aim`. Mask-present gate (AC 1, 2, 10–19, 49) | same worker as 8; integrated by executor | 1, 8 | done. GPU harnesses run with `POSTRETRO_REQUIRE_GPU=1` on the 1660: `world_specular_test` (7), `shadowmask_union_test` (pre-change union copy; non-owner subtracts nothing; no mask), `shadowmask_sample_test`, `pool_sample_test`. Plan tests cover refusal in both modes. The binding-inventory snapshot is unedited. Suites: renderer 878, loader 288, lighting 65, render-cpu 320, level-format 592, postretro 1569; workspace check, `capture,dev-tools` check and workspace clippy are clean |
| 10 | Engine-level proofs: rewrite `capture_shadowmask_groups` for owner swap, the streaming capture identity (AC 15, 16), and end-to-end compile-and-render fixtures for AC 1 and 17 | integrating executor | 7, 9 | done except AC 16. All on the 1660 with `--features capture`:
- `capture_shadowmask_groups`, 4/4: `owners_in_either_mask_group_reach_the_frame_through_their_own_channels` (AC 15 engine half); `static_specular_comes_only_from_face_owners_through_their_baked_masks` (AC 1 and 10 end to end: unowned and closed masks render a black specular term, and the baked mask shadows part of the open highlight); the no-shadowmask and animated captures.
- `capture_frame`: specular-shadowmask, hdr and spawner pass. The sampled-row test needs a pre-baked `stress-warren-mini.prl`, which this worktree lacks; that is environmental.
- `capture_lightmap_streaming::forced_missing_lightmap_block_drops_exactly_its_static_direct_and_specular` passes (AC 13 engine half).
- An engine-level id-40 strip cannot load: the cluster directory requires id 40 beside id 41. P10 stays proven by the loader test.
- AC 16, run in task 11 on the new compiler's campaign-test and hallway PRLs: `capture_lightmap_streaming -- --ignored` 3/3 pass (`streamed_lightmap_capture_matches_all_resident_on_campaign_test`, `..._on_hallway_inspection`, `forced_missing_lightmap_block_drops_exactly_its_static_direct_and_specular`) |
| 11 | Manual captures and measurements M1–M8 before and after. Build the pre-change binary from 75ac7e12f in a throwaway target and compile both PRL sets. M8 runs interleaved in one session | integrating executor + owner | 10 | done except M1. The sheen view is unconfirmed: the owner picks a view or gives the original angles. Results: §Task 11 results |
| 12 | `/preflight`, then `/review-panel` → `/fix-review-findings` loop, then the full gate | integrating executor | 11 | done. Preflight is clean. Three review rounds:
- **Round 1:** 10 reviewers across three slices plus a seam pass. No 🔴. Fixed (`f3253deb9`): nine 🟡, the nits, and about 25 comment-drift items. The 🟡 items:
  - SDF-tagged suns had their highlight follow their aim while their shadow came from their origin.
  - The cut-face drop warning gave the wrong cause.
  - Thin cut faces got no estimate samples.
  - The owner-index cap check ran late in the pipeline.
  - Pack did not check the owner table against id 17.
  - The memo-hit estimate test could not fail.
  - `cut_parents` was set after construction.
  - An owner-stream length mismatch was silent.
  - GPU test helpers were duplicated.
- **Round 1 owner calls:** mask padding, SDF directional specular and P17 went to the owner (owner amendments).
- **Round 2:** 3 reviewers on the fix commit. No 🔴, only nits, fixed with the owner-approved padding dilation (`a8cb20c5a`).
- **Round 3:** 1 adversarial reviewer on the dilation. No code defect. It added an occluded-edge clarification (Corrections) and a layer-offset test.
- **Padding check:** capture A/B of campaign-test rebaked with dilation against zero padding, same engine. Changed pixels lie only on face-boundary lines and all brighten: at most 360 specular pixels per 1080p frame, at most 12/255, under 0.3% of specular energy.
- **Full gate:** clean, 10 134 tests |

## Task 11 results

- **Arms:** before = 75ac7e12f binaries (`capture,dev-tools`) in the `ssmo-before` worktree; after = branch `--release --features capture,dev-tools`. Both compilers baked sdf-shadow-test, campaign-test, kinematic-platform, movement-feel and hallway from the same `.map`s against one seeded cache.
- **Bakes.** Branch hallway, warm: 2888 s total, lightmap 1464 s, ShadowmaskAtlas 9.1 s, peak per-face demand 3, 0 drops. The pre-change hallway dropped 121 selected masks to fit four global channels.
- **M5 drop reports** (branch compiler): campaign-test 1 drop (entity 32, face 577, 4288 lit texels); kinematic-platform 20 drops, peak demand 15, one cut face (estimate: 6,677 traces over 35,108 samples, 16.8 ms); movement-feel 248 drops, peak demand 22; hallway 0. Logs print each warning twice: once inline, once in the end-of-bake summary.
- **M2/M3 captures** at 1920×1080 with `sync-proof`, three masks per pose (all; 0x13F; 0x40). Specular-only lit pixels before → after, and the specular-off image diff:
  - p1 136 642 → 11 739; p2 47 522 → 2 267; p3 555 236 → 74 837; p4 64 282 → 41 098. Specular-off images are bit-identical at all four poses. At p3 the wall keeps its highlight above the shadow line and loses it below; at p4 the south-floor patch is gone and the lit-wall highlights remain. Owner read 2026-10-10: passes.
  - M3, style-2 room (`[-53, 3.5, 40]`, yaw −139.8 and 40, pitch −20): the yellow style-2 highlight is gone (animB 274 699 → 0 lit px); the white static spot's highlight remains (animC); specular-off images are bit-identical. Owner read 2026-10-10: passes.
- **M1:** a four-yaw sweep at (−17, 21, 86) m, pitch −20, is nearly identical before and after (at most 438 lit px differ). The owner picks the view or supplies the original angles.
- **M4, M5 owner reads (2026-10-10, in-engine):** pass. The owner reports campaign-test, kinematic-platform and movement-feel all read more correct and closer to Quake. Highlights stay continuous across chunk planes, and the drop-report highlight loss is acceptable. The deliberate ceiling shimmer in combat-test stays clearly legible.
- **M7**, run by the owner with `measure-hallway.ps1 -Cold -SkipBuild`, one run per arm (`--release`, no cache, i5-9400F, 5 jobs). Results, before → after:
  - Wall: 1248.7 → 1254.3 s. CPU: 4821 → 4719 s.
  - Peak working set: 4.31 → 4.32 GiB.
  - Lightmap Bake: 359.4 → 385.5 s. ShadowmaskAtlas: 34.0 → 7.7 s. The two together: 393.3 → 393.1 s.
  - Ownership fill now runs inside the lightmap walk, so time moves from the shadowmask stage to the lightmap stage. The hallway has no cut face, so it ran no estimate and logged no drops.
  - Unchanged stages varied by about 2% between runs (SH Bake 621.5 → 608.5 s), with n = 1.
  - The PRL grew by 67,170 bytes, matching the 8437-face owner table (8 B per face).
- **M8**, 2026-10-10, GTX 1660 Super, Vulkan, 3840×2160, `sync-proof`. Three interleaved runs per arm per pose, alternating which arm goes first; each run is 120 warmup frames plus 360 sampled (three 120-sample windows).
  - **Power.** The plan stayed Balanced with the Windows 11 *Best performance* power-mode overlay. Under load the GPU held 1935–1950 MHz (nvidia-smi probe), so it did not downclock.
  - **Sources.** The source `.map` SHA-256 prefixes match across arms: hallway `5f70a4233f53`, campaign-test `7fa6fae30d3b`, kinematic-platform `fc2efc5df054`, movement-feel `9e6f66efe600`, sdf-shadow-test `1ce84d64a530`.
  - **Sheen pose.** The sheen row uses yaw 90 / pitch −20 as a stand-in, because the original angles are unrecorded (M1).
  - **GPU,** median [min..max] ms:

    | Pose | `forward` before | `forward` after | GPU sum before | GPU sum after |
    |---|---|---|---|---|
    | hallway arena | 13.860 [13.822..13.898] | 13.235 [13.177..13.306] | 15.138 [15.097..15.181] | 14.509 [14.458..14.589] |
    | hallway sheen y90 | 11.702 [11.696..11.705] | 8.972 [8.960..8.973] | 12.815 [12.807..12.819] | 10.088 [10.078..10.090] |
    | campaign spawn | 5.854 [5.847..5.857] | 5.848 [5.847..5.854] | 6.833 [6.829..6.838] | 6.831 [6.829..6.836] |
    | campaign p1 | 6.385 [6.384..6.388] | 6.100 [6.097..6.104] | 7.441 [7.439..7.441] | 7.157 [7.153..7.159] |
    | kinematic station | 9.121 [9.112..9.129] | 8.513 [8.507..8.518] | 11.733 [11.731..11.749] | 11.211 [11.168..11.267] |
    | movement spawn | 10.258 [10.244..10.268] | 9.486 [9.480..9.524] | 11.227 [11.212..11.233] | 10.457 [10.449..10.497] |
    | sdf spawn | 8.697 [8.644..8.709] | 8.499 [8.479..8.506] | 10.876 [10.802..11.148] | 10.958 [10.653..10.986] |

    - `forward` gains at six poses and ties at campaign spawn. The largest gain is the sheen pose, at −2.73 ms (−23%).
    - The only GPU-sum rise is sdf spawn, at +0.082 ms. That is inside both arms' spread (before 0.346, after 0.333), so it passes.
  - **CPU `render_record`** median ms, before → after:
    - hallway arena 0.861 → 0.847; sheen 0.393 → 0.378; campaign spawn 0.174 → 0.173; p1 0.196 → 0.195; movement 0.130 → 0.128; sdf 0.076 → 0.074.
    - kinematic station 0.818 → 0.828: +0.010 ms, inside the 0.012 spread.
    - `render_sh_drain` is flat everywhere.
  - **Load,** log line C sum, median [min..max] ms. One discarded warmup launch per arm per map, then three interleaved launches, all `--windowed`:

    | Map | Before | After |
    |---|---|---|
    | hallway | 2317.5 [2306.3..2416.1] | 2280.3 [2270.1..2384.5] |
    | campaign-test | 578.9 [572.7..596.8] | 596.0 [591.6..597.7] |
    | kinematic-platform | 666.4 [659.6..685.3] | 682.2 [677.5..687.4] |
    | movement-feel | 438.3 [428.9..438.3] | 444.1 [436.8..446.1] |
    | sdf-shadow-test | 188.4 [187.8..191.4] | 192.1 [190.7..195.4] |

    - Every rise sits inside the larger arm's spread, so load passes.
    - The rises fall in stages this change does not touch: `settle_hold` (+3 to +18.5 ms, largest on kinematic-platform) and `audio_load` (+2 to +3 ms on every map). The `settle_hold` rise is plausibly the different id-42 block bytes held at settle.
    - `worker_delivered` falls by 45.8 ms on the hallway and 13.1 ms on kinematic-platform, mostly in `prl_parse`. `geometry_upload` moves by 1.5 ms or less.
    - Measuring needed the pre-change worktree's gitignored `content/dev` script build outputs, copied from the branch. `content/` and `sdk/` show no diff between 75ac7e12f and the branch.
  - **Owner stream bytes,** 8 B per vertex: hallway 269,952 (33,744 vertices); campaign-test 24,752; movement-feel 8,704; kinematic-platform 1,968; sdf-shadow-test 768. sdf-shadow-test has no id 42, because none of its lights are eligible (AC 47), so it binds a placeholder.
  - **Budget:** passes at every pose. There is no regression for the owner to rule on
