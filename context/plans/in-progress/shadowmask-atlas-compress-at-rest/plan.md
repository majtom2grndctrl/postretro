# shadowmask-atlas-compress-at-rest — plan of record

mode: resumable
status: approved
read at: 804351717

Brief read at 8e4753ce1; 92 commits since. Every symbol cited by Decisions and Path was
re-read at 804351717. All Decision premises hold: `encode_bc5_rg` is serial, min/max
endpointed, and reproduces 0 and 255 exactly; the analytic graph walk already computes a
per-texel covered-light list (peak overlap is a max over it); the loader's malformed path
logs `ShadowmaskAtlas malformed; ignoring section: {err}`; `TEXTURE_COMPRESSION_BC` is
required at device acquisition; `MAX_ATLAS_DIMENSION` and `REQUIRED_MAX_TEXTURE_DIMENSION_2D`
are both 8192; atlas dims are powers of two ≥ `MIN_ATLAS_DIMENSION` (64).

## Corrections

- Path: "`LevelGeometry { lightmap: None, .. }` builders in `mesh_render.rs`,
  `particle_render.rs` and `startup/lifecycle.rs`" → those are `LevelWorld` literals. The
  `LevelGeometry` literals are `renderer_resources.rs` (empty geometry), the
  `render/shadowmask.rs` test and `capture/setup.rs`. All of them follow the type change.
- Path: "`level_world_to_geometry` and `LevelGeometry` carry [the payloads] to
  `LightmapResources::new`" → `install_level_geometry` takes `&LevelGeometry`, so a
  payload can't be moved through it. Planning around it: the payloads go to
  `install_level_geometry` by value, beside the borrowed geometry. `LightmapResources::new` also has a
  second caller, `build_full_renderer`, whose geometry is always `None`. It gets no
  payload.
- Acceptance *Lifecycle* row 1, "has no place to hold their payloads", clarified. The
  no-upload-install row requires a renderer-less install to keep the payloads, so the
  world must be able to hold them before install. Planned meaning: the world's retained
  id-42 and id-22 types are header types with no payload field. The payloads live in one
  separate movable value, which install takes and leaves `None`. A header whose payload
  was taken is unrepresentable, as the Decision requires. Owner confirmed with plan approval.

- Wire: `from_bytes` also rejects zero width, height or layer count. Without it, a
  pre-tag payload with zero dims and the tag's value in its width word parses as a
  valid tagged section, which the tag-collision pin forbids. The compiler never emits
  zero dims, and the renderer already sent them to the placeholder. Same outcome (fully
  lit), now named at load.
- Test surface: fill-semantics tests read the raw fill (`finish_raw`, plus a
  test-only record of the last raw fill for the public bake paths). BC4 reproduces
  only block endpoints exactly. Fixtures that pass through the encoder moved to
  4-aligned dims; historical goldens keep their slot tables and raw values.
- Observed, not this brief's: capture of any SH-streaming PRL needs
  `POSTRETRO_SH_STREAMING=sync-proof`, so the existing
  `specular_shadowmask_capture_scene_compiles_loads_and_writes_png` fails when run
  without it. The new capture test sets it on its child process.

- Wide-layer warning names the level by its input path: `prepare_fused_shadowmask`
  now takes a level label and returns `Result`, so the misaligned-atlas error fails the
  build through `bake_fused_prepared`'s `?`.

## Owner decisions during build

- **M1 and M4, union path (2026-09-24).** The capture harness never promotes static
  lights, so its promoted-union term is zero and does not read the mask. Restated with
  the owner:
  - M1: "Four overlapping selected lights, spread across both groups, each resolve to
    their own mask — no drop, no cross-talk between groups. World specular is proven by
    offscreen capture on an adapter. The promoted-union path is proven on an adapter by
    a GPU readback of `forward.wgsl`'s sampling helper and that path's channel select.
    A skipped run does not count."
  - M4: "Moving a light from the first group to the second leaves world-specular output
    unchanged for surfaces covered only by first-group lights (offscreen capture on an
    adapter). Static→static world shadowing stays exactly zero: the GPU readback shows
    the union attenuation is zero at entity visibility 1 for every slot in both groups.
    A skipped run does not count."

## Delegated answers

- Tag position and width: a `u32` format tag is the first header word. Its value is
  ASCII `SMB5`, so no compiler-emitted pre-change width can equal it. If the tagged parse
  fails and the bytes parse under the pre-change raw layout, `from_bytes` reports a
  *format mismatch* rather than the first structural error. That covers the
  tag-collision pin deterministically.
- Overlap-count carrier: a `u32` prefix inside the same memo entry, ahead of the section
  bytes. With no sibling entry, nothing evicts independently, and an entry too short for
  the prefix is a miss. The shipped section never carries it.
- Header/payload split: header types in `level-format` beside the section types
  (`LightmapSection` and `ShadowmaskAtlasSection` split into header and payload, and
  rejoin by move for upload). No copy is made.
- GPU mask proofs: offscreen captures through the capture harness on a dev fixture map.
  The group-move and seam cases rewrite id 42 inside a compiled PRL, which controls group
  placement without depending on channel coloring. The `u` = 0 and `u` = 1 edge rows use
  a GPU readback of the shader helper against a hand-built BC5 texture, the
  `sdf_light_select_test.rs` pattern.

## AC-to-proof

Test names are working names. Rows marked *adapter* must actually run on an adapter. A
self-skip is not a pass.

| AC | Proof | Status |
|---|---|---|
| W1 round-trip: empty, single, full four-slot | `level-format` `shadowmask_atlas::tests` round-trip cases | achievable as stated |
| W2 `from_bytes` rejects length, misalignment, unknown tag, bad slot | `level-format` reject cases | achievable as stated |
| W3 pre-change payload: named warning, fully lit, no panic, tag collision | `level-format` legacy and collision cases, plus a `level-loader` test capturing the `[PRL]` warning with a `None` section | achievable as stated |
| W4 stale memo never served; rebuilt equals uncached | compiler test seeding a raw pre-change entry under the old and new keys | achievable as stated |
| W5 second build logs a memo hit | compiler test with log capture | achievable as stated |
| W6 all-sentinel: half raw bytes, loads, fully lit | compiler test for bytes, plus a loader test for the load; the sentinel read is D1 | achievable as stated |
| D1 sentinel fully lit; second group vs 2×1 placeholder in range | renderer GPU helper readback (*adapter*) | achievable as stated |
| D2 `2W` = pinned dimension kept; one block wider → placeholder + `[Renderer]` error | `filter_usable_shadowmask_section` unit test with log capture | achievable as stated |
| D3 `W` = 8192 → placeholder; 4096 kept | same filter test | achievable as stated |
| D4 8192-wide bake warns, no id 42; 4096 emits | stage-seam test with a synthetic wide `SharedAtlas` | achievable as stated |
| D5 warm rebuild of 8192-wide warns again, no id 42, no memo entry | same test, run twice against one cache | achievable as stated |
| D6 hand-built misaligned section → placeholder + `[Renderer]` error | filter unit test | achievable as stated |
| D7 misaligned bake fails naming dims, release as debug | compiler test; run the focused test once under `--release` too | achievable as stated |
| M1 four lights across both groups, both decode paths | world specular: capture on a four-colored-light fixture with per-slot id-42 rewrites (*adapter*); union path: GPU readback of the helper plus channel select (*adapter*) | restated (owner) |
| M2 seam-bleed | capture on a fixture whose groups differ at the seam (*adapter*) | achievable as stated |
| M3 `u` = 0 / `u` = 1 per group, groups differing at the seam | renderer GPU helper readback against a hand-built BC5 texture (*adapter*) | achievable as stated |
| M4 group-0→1 move leaves first-group specular unchanged; static→static stays zero | capture A/B with id 42 rewritten in a compiled PRL (*adapter*); union attenuation zero via GPU readback (*adapter*) | restated (owner) |
| M5 all-255 atlas decodes fully lit | compiler encode→CPU-decode unit test | achievable as stated |
| M6 grep gate over `forward.wgsl` | rewritten `forward_shader_shadowmask_fallback_clamps_multilayer_indices` | achievable as stated |
| M7 no new texture or sampler | `forward_pipeline_sampled_texture_request_matches_bgl_definitions`, untouched and green | achievable as stated |
| B1 payload exactly half raw in the footprint report | compiler test reading `PrlFootprint` for a populated fixture | achievable as stated |
| B2 texture description: BC5, `2W × H × L`, half raw bytes; the upload uses it | a pure `shadowmask_texture_descriptor` fn, unit-tested and called by the upload | achievable as stated |
| B3 miss holds one raw fill + one output + ≤ 3 raw layers of scratch; raw gone before cache/return | test-only byte-residency tracker in the encode module | achievable as stated |
| B4 byte-identical across worker counts; warm = uncached | adapt `shadowmask_fixture_is_deterministic_across_rebuilds_and_workers` and the cached/uncached golden tests | achievable as stated |
| B5 max and mean per-channel encode error, measured | measurement test printing both on a fixture; yardstick numbers in the landing note | manual (measured, not gated) |
| L1 world keeps headers; payloads have no place after install | type-level split plus a loader/seam test (see *Corrections* clarification) | achievable as clarified |
| L2 released only after upload; renderer-less install keeps them | unit test on the install take seam | achievable as stated |
| L3 capture takes the payloads the same way | capture setup test: world holds headers only after install | achievable as stated |
| L4 animated atlas dims equal static after take; animated lights render | renderer test: `usable_atlas_dimensions` reads the header after the take; animated capture fixture (*adapter*) | achievable as stated |
| L5 lightmap-without-shadowmask and neither install without panic; first releases | take-seam unit test plus a capture of a no-shadowmask fixture (*adapter*) | achievable as stated |
| O1 `--verbose` overlap line on cold and warm; none when not verbose | compiler test with log capture across miss, hit and non-verbose | achievable as stated |
| MN1 id 42/22 bytes, layers, dims, peak RSS pre/post on the yardstick | owner or attended run, landing note | manual |
| MN2 process memory after install pre/post, net of texture bytes, metric named | attended run | manual |
| MN3 specular highlight capture A/B, both images inspected | attended capture | manual |
| MN4 capture-harness CPU completion median and p95 | attended run per `testing_guide.md` §Resource bounds | manual |
| MN5 peak overlap on the yardstick | attended `--verbose` bake | manual |
| MN6 second-group shadows after reload and a dev level cycle | owner, in-engine | manual |
| MN7 level cycle between differing lightmap widths and back | owner, in-engine | manual |

## Measurements

- B5 encode error (ignored `shadowmask_bc5_encode_error_on_fixture_bakes`, default
  density, soft-shadow samples 32). Max and mean absolute error per used slot, over
  every texel:
  - `shadowmask-groups-capture`: 1024²×2, max 0/255, mean 0.
  - `soft_shadow_test`: 1024²×1, max 17/255, mean 0.0031/255.
  - `gate-heavily-lit`: 1024²×2, max 17/255, mean < 0.0001/255.
  Hard shadows are exact because their blocks hold endpoints only. Error sits in
  penumbra blocks. The yardstick number is part of MN1.

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | **First slice.** Tagged BC5 wire format and `from_bytes`. Encode submodule under `shadowmask_bake/` used by `ShadowmaskFill::finish` and `empty_section_for_dimensions`. One header writer shared by `to_bytes` and the streamed cache writer. Bump `SHADOWMASK_ATLAS_STAGE_VERSION`. Renderer `2W` filter, alignment guard, BC5 upload through the descriptor fn, 2×1 placeholder. Two-sample shader helper with per-group clamp; rewrite the shader guard. Move the 5×5 fixtures to 4-aligned dims; raw-byte tests decode the payload. Four-light fixture map with a mask edge at the seam, checked by capture on an adapter (M1 first pass). | integrating executor | — | done — `capture_shadowmask_groups` passes on this Mac's adapter and fails when the shader's group order is swapped; compiler, renderer, level-format and loader suites green |
| 2 | **Bake contract.** Wide-layer omit before the memo probe and fill (D4, D5). Misaligned-bake error in every profile (D7). Stale memo, logged hit and all-sentinel tests (W4–W6). Scratch-bound tracker (B3). Determinism and warm=cold (B4). All-255 decode (M5). Encode-error measurement (B5). Footprint half-raw (B1). | integrating executor | 1 | done — D4/D5 `eight_k_wide_layers_omit_the_shadowmask_on_every_build_and_four_k_emits`; D7 `fused_prepare_rejects_a_misaligned_atlas_naming_its_dimensions` (debug and `--release`); W4 `pre_bc5_memo_entries_are_never_served_and_the_rebuild_matches_uncached`; W5/B4 `second_fused_build_hits_the_memo_and_warm_equals_uncached_on_real_masks`; W6 bytes in `all_filtered_selection_keeps_empty_bytes_and_indeterminate_progress`; B1 `shadowmask_footprint_payload_is_half_the_raw_rgba_arithmetic`; B3 `cache_miss_holds_one_raw_fill_one_output_and_bounded_encode_scratch`; B4 workers `shadowmask_fixture_is_deterministic_across_rebuilds_and_workers`; M5 `all_visible_payload_equals_encoding_an_all_visible_fill_and_decodes_fully_lit`; B5 measured by the ignored `shadowmask_bc5_encode_error_on_fixture_bakes` |
| 3 | **Overlap report.** Peak per-texel count from `build_analytic_overlap_graph_in_order`, carried as the memo-entry prefix. `--verbose` line on miss and hit; an omission line for wide layers (O1). | integrating executor | 2 | |
| 4 | **Wire and renderer fan-out.** W1–W3 including the loader warning, D2, D3, D6, B2. GPU helper readback for D1 and M3. | integrating executor | 1 | |
| 5 | **Payload ownership.** Header/payload split for ids 22 and 42. `LevelWorld` keeps headers plus one movable payload value. `install_level_payload` and the capture install take it; `install_level_geometry` receives it by value; `LightmapResources::new` consumes and drops it. `LevelWorld` and `LevelGeometry` literals follow. Lifecycle tests L1–L5. | integrating executor (may delegate: loader, startup and capture files; no overlap with 2–4 once task 1 lands) | 1 | |
| 6 | **GPU mask proofs.** Capture tests for M1 (final), M2, M4 (id-42 rewrite A/B), L4 and L5 captures. All run on an adapter. | integrating executor | 4, 5 | |
| 7 | **Land.** `/preflight`, then a `/review-panel` → `/fix-review-findings` loop. Update `build_pipeline.md` (id-42 row, §PRL ShadowmaskAtlas, §Build Cache) and `rendering_pipeline.md` §4 from decided to built. Result column. Hand manual rows MN1–MN7 to the owner with the exact commands. | integrating executor | 1–6 | |
