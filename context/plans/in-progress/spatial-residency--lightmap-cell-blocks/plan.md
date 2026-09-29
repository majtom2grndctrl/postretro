# spatial-residency--lightmap-cell-blocks — plan of record

mode: resumable
status: approved
read at: 6526dc627

Owner approved 2026-09-28: AC 1 and AC 7 restated as proposed below (applied to `index.md`), the id-22 header confirmed (mode in header, Rg8 direction fixed, densities dropped; applied to `index.md` Wire format), and the plan approved.

## Corrections

Path and research claims checked against source at 6526dc627. No Decision premise was found false.

- **Vertex attribute location.** Research §3 says location 4 carries the static-layer field. → Location 4 is only the UV. The static layer is location 5, `lightmap_layer_block`, a `Uint16x2` at offset 32: `.x` is the layer and `.y` the animated block id (`forward.wgsl:365-367`, `renderer_init_pipelines.rs:96-107`). Planning around it: block id + 1 replaces `.x`. That caps a level at 65,534 blocks, the overflow limit AC 1 tests.
- **Fragment sampled budget.** "Forward FRAGMENT sits at 15/16 sampled." → It is 16/16 with cube-array support and 15/16 without (`pipeline_budget_tests.rs:153-159`). The Decision holds more strongly: no fragment binding is free.
- **Varyings.** `VertexOutput` has 8 user locations, and wgpu 29 allows 16 (15 on downlevel). The engine requests the default limit. Planning: add one flat `vec4<u32>` (pool offset xy, block extent xy) beside the existing flat layer and animated id, for 9 locations. The block extent feeds the shadowmask clamp.
- **Shadowmask clamp.** `sample_shadowmask_atlas` clamps only U, per mask-group half. It never clamps V (`forward.wgsl:744-745`). Planning: clamp U and V to the block's half-texel rect in the pool layer.
- **`validate_cross_section`** lives in `render-cpu/src/animated_lightmap.rs:74`, not in renderer `render/animated_lightmap.rs`, which only calls it.
- **Block alignment.** The dry run aligns blocks to `lcm(4, irr_w/dir_w, irr_h/dir_h)` = 4 (`lightmap_residency_dry_run.rs:177-188`, `DIRECTION_TEXEL_SCALE = 2`). The research §1 overhead (1.234×) and layer counts were measured at 4. Direction is uncompressed Rg8 and needs only integral scale. See AC 1 restatement.
- **The dry run is test-only.** `lightmap_residency_dry_run` is `#[cfg(test)]` in the `prl-build` bin (`main.rs:40`). `postretro-level-loader` and `postretro-visibility` are dev-dependencies of level-compiler. Planning:
  - Promote both to normal dependencies. That is a same-layer edge, and `layering_invariants_hold` guards it.
  - Move `pvs_sampling`, `brief_set` (lead map, dilation, `portal_neighbours`) and `pack_cell_block` into non-test compiler stage modules.
  - Point the dry run at the moved code.
- **The dilation lives in `brief_set.rs`, not `visible_set.rs`.** `visible_set.rs` is the older cluster-inclusive, undilated set. The brief-set code (`visible_sources`, `portal_neighbours`, `build_lead_map`) is what moves.
- **The dry run walks its own portal world.** It keeps every portal. The loader's `convert_usable_portals` (private) drops the whole set when any portal is bad. Planning:
  - The bake builds its visibility world through the loader's own conversion, making it `pub(crate)`-reachable via a small public constructor.
  - A level whose portals the loader would reject bakes no residency set, and runs all-resident per the Decision.
- **The dry-run reader parses only old-format PRLs.** `inputs.rs::read_dry_run_input` reconstructs charts from static-atlas UVs and the v2 id-22 header. Planning:
  - The set part (cells, portals, locator, id 49) is format-independent.
  - The byte part reads the new block records.
  - `inputs.rs` is updated in the same task that changes the format, and the ignored yardstick keeps running on new PRLs.
- **The id-49 partition runs late.** `canonical_cell_partition` runs only in the ClusterDirectory stage, after the lightmap stages (`pipeline.rs:2084`). Cluster-major block order is needed when id 22 is assembled. Planning:
  - Compute the canonical partition once, before AtlasPreparation. Its inputs (cells, portals, BVH, seam ids from `resolve_streaming_hints`) all exist after Visibility and BvhBuild.
  - ClusterDirectory consumes that result instead of recomputing it.
- **Id 42 has no section version.** It carries the format tag `SMB5` (`shadowmask_atlas.rs:12`). `SHADOWMASK_ATLAS_STAGE_VERSION` is only a compiler cache key. Planning: the "version bump" is a new tag, `SMB6`. The old tag rejects, which satisfies AC 17's older-version row.
- **Container versions are unchecked.** The loader checks entry versions only for id 49 (`prl_loader.rs:1537`). Bumping `section_plan.rs` versions for ids 17, 22 and 25 rejects nothing by itself. Planning: add loader checks for the bumped container versions of ids 17, 22, 25 and 42. The id-22 and id-25 payload versions also bump (`LIGHTMAP_SECTION_VERSION` 2→3, `ANIMATED_LIGHT_WEIGHT_MAPS_VERSION` 4→5).
- **Id 42 mismatches are warn-and-ignore today** (`prl_loader.rs:1915-1939`). AC 17 requires rejection. Planning: a block-count mismatch or a malformed v-new id 42 becomes a hard load error.
- **Id-22 header fields the brief drops. Owner confirmed 2026-09-28.** The brief's header drops `dir_format`, both texel densities, and the optional LMOD `LightmapMode` trailer. Runtime reads `lightmap_mode` (`renderer_full_init.rs:251`, `renderer_diagnostics.rs:269`) and `direction_format` (`lighting/lightmap.rs:737`). Planning:
  - Direction is fixed Rg8; legacy Rgba8 is rejected by the version bump.
  - The texel densities are informational and are dropped.
  - `mode u32` joins the header after `irradiance_format`, so the index read carries it. It replaces the trailer.
- **Loader reads the whole file.** Without id 50, or in SH `off` mode, the loader reads the whole file image (`prl_streaming.rs:127`). AC 12 forbids holding either payload. Planning:
  - Lightmap streaming mode forces the positional container, whatever SH's mode.
  - The loader retains a shared `Arc<File>`: `ShStreamManifest`'s handle when present, otherwise its own.
- **Byte recording is test-only.** `record_positional_read` is `#[cfg(test)]`. AC 12 wants counts in the game and in capture. Planning: add an always-on per-section byte counter at the positional reader, under both the loader and the issuer.
- **No install-time preload in game.** SH's controller is created lazily in the first frame (`sh_residency.rs:490-504`). "Doors never wait" is SH's in-play miss policy, not an install preload. The only precedent for ready-before-frame residency is `capture/prepared.rs::preload_visible_sh`. Planning: the lightmap install path reads the spawn cell's mandatory set synchronously through the positional reader and installs it before the first frame. That is new code, modelled on capture's preload.
- **Settle chokepoint.** `drafts/sh-streaming--reveal-gate-and-warm-horizon` adds a Settling boot state with one per-resource "settled?" check. Planning: expose `lightmap residency settled` as one query (the camera cell's mandatory set sampleable). Install preload makes it true at the first frame. Leave the chokepoint's owner to that draft, and note the seam in the findings.
- **Growth copy and retirement.**
  - The copy is `render/sh_streaming/gpu.rs::copy_texture`, not `growth.rs`.
  - One-retiring is per family (four families).
  - Release is an `on_submitted_work_done` flag that `release_completed_retirement` polls at drain start.
  - Planning: the lightmap pool is one family (irradiance, direction and shadowmask retire together), under the same polling pattern.
  - Pool textures need `COPY_SRC` for growth and repack. Today's lightmap textures are `TEXTURE_BINDING | COPY_DST`.
- **`StagedUploads` visibility.** It is `pub(in crate::render::sh_streaming)`. Planning: widen it to `pub(crate)` in the render module for block region uploads.
- **Fault injection.** SH's `RecordingSource` holds only the first read. P2 needs a chosen read held (only the shadowmask half). Planning: a new per-range hold source over the shared issuer.
- **No read-order or budget trace.** The existing trace test (`no_hint_controller_trace_...`, `controller_tests.rs:191`) records neither the issuer's read order nor the per-drain budget. AC 18 needs a trace from the pre-extraction controller. Planning: Task 4 records it and commits it as a fixture before Task 5 touches SH code.
- **Hint decode.** It lives inline in `PlannerTopology::from_manifest_view` and is built only from `ShStreamManifest`, so it needs id 50. Id 49 is reachable without id 50 via `prl_lighting.rs:214 cluster_directory()`. Planning: the shared layer decodes pins and priorities from id 49 alone, and SH's topology consumes that result.
- **No streaming sliders exist.** The Streaming tab has one checkbox and gauges. Planning: pool-cap and lead sliders are new dev-tools widgets in that tab.
- **SH mode is env-only.** `ShStreamingMode`, via `POSTRETRO_SH_STREAMING`. Streamed-SH capture requires `sync-proof`, and `capture_command` already sets it. Planning: lightmap mode mirrors this pattern. `POSTRETRO_LIGHTMAP_STREAMING` = `all-resident` | `stream`, default `stream` when a valid residency section is present.
- **Split-first list.** The listed files are all past 800 lines. There is no renderer `pipeline.rs`; the brief means level-compiler `pipeline.rs` (3048). Also past 800 and touched here:
  - `shadowmask_bake.rs` (5519)
  - `animated_light_weight_maps.rs` (2164)
  - `animated_light_chunks.rs` (1030)
  - `session/sh_residency.rs` (834)

  Each is split along the touched seam, in its own commit, right before the task that extends it.

## Delegated answers

- **Allocator** — the shelf allocator (`BlockPool`), moved from the dry run to `render-cpu` so the renderer and the dry run share one implementation. Repack counts are reported in the findings. It was measured; guillotine was not.
- **Baked maximum lead** — 32 m (`BRIEF_MAX_LEAD_METERS`). It is the measured basis. Every row fits Low at 170.0 MiB worst.
- **Default L** — 16 m, the lean. Worst mandatory set is 136.5 MiB and 12 shelf layers.
- **Default pool cap** — 15 layers (210 MiB). At L = 16 m it never grows, and band retain repacks on 2.54% of tour steps (research §1).
- **Budget unit** — one per-drain byte budget, shared: SH decoded bytes plus block upload bytes against today's `MAX_INSTALL_DECODED_BYTES_PER_DRAIN` (8 MiB). The first request is always admitted, and a lightmap/shadowmask pair is admitted or deferred whole (P12). Keeping SH's constant leaves SH-only behaviour unchanged (AC 18).
- **Interleave** — the tiers are mandatory (SH Visible/Pinned, block mandatory/visible/pinned) then optional (SH SeamWarm/Prefetch/Hysteresis, block band), across both resources. The drain admits in that tier order. Within the mandatory tier, SH keeps its class order ahead of blocks at equal class, and blocks order by lead then block id. The issuer reads each tier in ascending file offset.
- **Miss-shader flag** — the vertex table entry's residency bit travels in the flat `vec4` (extent 0 means non-resident). The fragment stage skips the lightmap and shadowmask taps, and so drops the block-dependent terms (see AC 7).

## AC-to-proof

Numbered in brief order.

| AC | Proof | Status |
|---|---|---|
| 1 Charts inside block, no overlap, BC edges, ≤ pool layer, id overflow rejects / one-below builds | `cell_block_pack_*` compiler tests; `block_count_limit_*` over the validation seam | restated (owner, 2026-09-28) |
| 2 Baked set = dry-run mandatory set, dilation included, all cells × leads | `residency_set_bake_matches_direct_evaluation` (synthetic fixture); ignored yardstick on both maps | achievable as stated |
| 3 Pinned cluster mandatory everywhere; unflagged only via lead/vis; priority reorders band prefetch | bake test + `lightmap_controller_priority_*` | achievable as stated |
| 4 Pool holding every block renders pixel-identical to all-resident, both maps | ignored GPU capture test, byte-compare PNGs, run on this Mac | achievable as stated |
| 5 Vertex block id + block-local UV address the same chart texel as the static-atlas UV | compiler test comparing pre-rebase atlas texel to block-frame texel for every lightmapped vertex | achievable as stated |
| 6 Animated keys address the same texels; load rejects animated block outside cell block | compiler rebase test + loader reject test | achievable as stated |
| 7 Forced-missing block: static direct + specular absent, SH present; resident renders lit; matches masked capture | ignored GPU capture test; capture gains a light-term mask field; SDF-free fixture | restated (owner, 2026-09-28) |
| 8 Held shadowmask read: neither half sampleable; release → both in one drain | controller + per-range hold issuer test (P2) | achievable as stated |
| 9 Cap below mandatory grows; cap above refuses band beyond cap; out-of-band freed next drain | controller + allocator tests | achievable as stated |
| 10 Repack allocates no second pool; each block samples own texels | pool-model test (texture-creation counter) + renderer GPU readback test | achievable as stated |
| 11 Spawn mandatory set resident before first frame | install-path test over a synthetic PRL | achievable as stated |
| 12 Bytes read = block ranges + index; no whole payload; counted in game and capture | positional byte-counter tests (loader + issuer) + capture JSON assertion | achievable as stated |
| 13 Stale completions discarded after reload; unload releases pool, handle, workers, retiring | controller/session tests (P3, P9) | achievable as stated |
| 14 Non-portal paths request only the camera cell's set; solid/exterior nothing new; empty world no lookup; no portals → all-resident | controller tests per `VisibilityPath` variant + loader test | achievable as stated |
| 15 Steady frames: no table writes, no allocation; cell change writes only changed entries | controller test: buffer capacities + block-table write counter | achievable as stated |
| 16 No-block level boots; one-block level streams | loader + boot tests on tiny fixtures | achievable as stated |
| 17 Load rejects the eight listed cases | one loader test per case | achievable as stated (id 42 "version" = format tag, per Corrections) |
| 18 SH request order + install budget unchanged with lightmap idle; SH tests pass unchanged | trace fixture recorded in Task 4, replayed after extraction | achievable as stated |
| 19 One issuer thread performs every read, mandatory before optional across both, ascending offset | shared-issuer ordering test | achievable as stated |
| 20 P1–P12 outcomes | one test per pin row over the controller + fault-injected issuer | achievable as stated |
| 21 BGL: FRAGMENT unchanged; only VERTEX addition is the block table | new `forward_pipeline_bindings_*` test in `pipeline_budget_tests.rs` | achievable as stated |
| 22 Walk metrics logged and in capture JSON (both maps) | measurement run; recorded in findings | reported |
| 23 Lightmap residency CPU time in `[CpuTiming]` | new `RenderStage`/`FrameStage` entry; measurement run | reported |
| 24 PRL size delta and time-to-first-frame delta vs whole-resident | measurement run on both maps | reported |
| 25 Dry-run dilation cost (worst/p95 with and without) | ignored yardstick rerun on new PRLs | reported |
| 26 Owner walk, both maps, sliders live | owner, in-engine | manual |
| 27 Windows `POSTRETRO_GPU_TIMING=1` forward delta | Windows handoff (no timestamp queries on this Mac) | manual |
| 28 Findings note | `findings.md` beside this plan, owner reads | manual |

### AC 1 — restatement (approved)

Current: "Block edges are multiples of 4 × the direction texel scale."

Proposed: "Block edges are multiples of both 4 (the BC block) and the direction texel scale."

The reason: with scale 2, the current text demands 8-alignment, but the measured basis (research §1, 1.234× overhead, the layer counts, the default pool cap) was taken at 4. Direction is uncompressed Rg8 and needs only an integral scale. The other clauses of AC 1 stand.

### AC 7 — restatement (approved)

Current: "The missing-block pixels match the same capture with the static-direct and static-specular light terms masked off."

`BAKED_DIRECT_STATIC` (bit 3) and `SPECULAR` (bit 6) also gate SDF-shadowed static lights, which don't depend on a block (`forward.wgsl:1025-1027, 1161-1202`). On a map with SDF static lights, either the miss drops light that isn't missing, or the pixels can't match.

Proposed: "A missed block drops exactly the terms that read its block: lightmap irradiance, shadowmask-gated static specular, and the shadowmask union subtraction. On a fixture without SDF-shadowed static lights, the missing-block pixels match the same capture with the static-direct and static-specular light terms masked off."

Recommended over the alternative: drop SDF-light direct and specular on a miss as well. That keeps the proof map-independent but blacks out light that is present.

## Tasks

Split-first commits precede the task that extends each file. Every task ends with focused tests (checking the counts matched) and a disk check. Below 15 GB free, `cargo clean -p` the crates edited so far.

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | **Compiler block format (riskiest slice, part 1).** Split `lightmap_bake.rs`, `lightmap_layer.rs`, `shadowmask_bake.rs`, `animated_light_weight_maps.rs`, `animated_light_chunks.rs` and `pipeline.rs` along the touched seams. Move the canonical partition before AtlasPreparation. Pack per-cell blocks by moving `pack_cell_block`, ordered cluster-major. Emit id 22 v3, id 42 `SMB6`, id 17 block id + 1 and block-local UV, and id 25 v5 block-local keys. Bump the versions and cache keys. Update the dry-run `inputs.rs` to the new format. Proof: AC 1, 5, 6 (compiler half). | integrating executor; delegable: animated rebase | — | |
| 2 | **Loader block format.** Parse ids 22, 42 and 25 into a CPU block index plus blobs (all-resident path). Add container-version checks and the AC 17 reject list (minus the residency-section rows), the animated outside-block reject, and the zero- and one-block fixtures. Split `prl_loader.rs` first. Proof: AC 6 (loader half), 16, 17 (partial). | delegable | 1 | |
| 3 | **Renderer all-resident pool (riskiest slice, part 2).** Move `BlockPool` to `render-cpu`. Pool textures get `COPY_SRC`. Place every block at install. Add the group-6 VERTEX-only block table and the `forward.wgsl` vertex decode, fragment offset and shadowmask clamp. Rebase `animated_block_uv` and update the spliced shader-harness tests. Update the byte meter and `usable_static_layers` consumers. Add the BGL test. Split `forward.wgsl` and `lighting/lightmap.rs` first. Proof: AC 21, plus a visual smoke on both maps. Checkpoint: report the slice to the owner before streaming starts. | integrating executor | 2 | |
| 4 | **Record the SH baseline trace.** Build a synthetic-schedule harness that records request order, per-drain install budget, and issuer read order from the current controller and issuer. Commit it as a fixture. No SH code changes. Proof: AC 18 baseline. | delegable | — | |
| 5 | **Residency-set bake (id 51).** Promote the visibility and loader crates to dependencies. Move `pvs_sampling` and `brief_set` into a compiler stage that builds its world through the loader's conversion. Write the CSR section, plus the loader decode and the residency-section reject rows (cell past count, decreasing CSR). Proof: AC 2, 3 (bake half), 17 (rest), 25. | delegable | 1 | |
| 6 | **Shared streaming layer.** Extract a resource-neutral issuer (one thread, tiered, ascending offset, coalescing), a shared drain budget, and the id-49 hint decode. SH keeps its decode, owner closure and target classes. Split `sh_residency.rs` first. Proof: AC 18 replay, 19; the existing SH tests pass unchanged. | integrating executor | 4 | |
| 7 | **Lightmap residency controller (CPU).** Demand from the baked set and L, recomputed only on a cell or L change. Also: visible-cell demand, band prefetch with retain, pins and priority, the miss policy, and the non-portal paths. The pair request goes through the shared issuer, with a per-range hold test source. Proof: AC 3, 8 (CPU side), 9 (CPU side), 13, 14, 15; P1, P4, P5, P8, P10, P11, P12. | integrating executor | 5, 6 | |
| 8 | **Renderer streaming.** Pair install in one drain, generation-matched, with table writes batched with the texel writes (P6). Growth with one retiring generation, and in-place repack through the spare layer. Install journal and undo (P7), and release on submitted-work-done (P9). Proof: AC 8, 9, 10, 13; P2, P3, P6, P7, P9. | integrating executor | 3, 7 | |
| 9 | **Loader streaming I/O and install preload.** Positional container in streaming mode, a retained handle, the always-on per-section byte counter, the `POSTRETRO_LIGHTMAP_STREAMING` mode, the synchronous spawn-set preload before the first frame, and the settled query. Proof: AC 11, 12, 14 (no-portals half), 16 (streaming half). | delegable | 2, 7, 8 | |
| 10 | **Capture and parity.** Capture preloads the view's mandatory and visible blocks synchronously. The capture scene gains a light-term mask field. GPU byte-compare tests. Proof: AC 4, 7. | integrating executor | 8, 9 | |
| 11 | **Diagnostics and levers.** Counters (resident and mandatory bytes, pool layers, repacks, growth transient peak, install time, bytes read, both miss kinds). The throttled `[Lightmap streaming]` log, the capture JSON, the `[CpuTiming]` stage, and the pool-cap and lead sliders in the Streaming tab. Proof: AC 22 and 23 surfaces exist. | delegable | 8, 9 | |
| 12 | **Measurements and findings.** Hallway and campaign walks, PRL size and time-to-first-frame deltas, the dilation-cost rerun, and `findings.md` with a recommendation. Proof: AC 22–25, 28. Then hand AC 26 and 27 to the owner. | integrating executor | 10, 11 | |

## Hot paths (development_guide §1.4)

- Forward vertex: one block-table fetch per vertex. Fragment: one offset per lightmap lookup and no new binding. Bounded GPU work; measured by the Windows handoff (AC 27).
- Demand from the baked set is recomputed only on a camera-cell or L change. Visible-block demand reuses per-frame buffers. No steady-state allocation (AC 15).
- Block-table writes happen only on install, eviction or repack, never per frame (AC 15).
- Uploads stay within the shared drain budget (Delegated answers).
