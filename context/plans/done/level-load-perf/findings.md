# Level Load Performance — Findings

> **Read this when:** choosing which level-load optimizations to build. Companion to `contract.md` (beside this file), whose decisions and invariants bind this file.
> **Key result:** on the large lit map, SH-streaming data structures cost about 1.2 s of main-thread freeze and 1.3 s of worker time per load. On the typical map, level-invariant GPU pipelines, the model sweep and texture copies dominate. Collision is negligible.
> **Status:** investigation record. Candidates 1, 2 and the single-parse half of 4 were built afterwards; `contract.md` beside this file records each outcome. The remaining candidates feed the pipeline spec (`context/plans/drafts/level-load-pipeline/`). Scratch instrumentation is described in §Method and was reverted.

---

## Method

| Item | Value |
|---|---|
| Profile | `--release` (opt-level 3, thin LTO). Symbols and line tables were kept (`CARGO_PROFILE_RELEASE_STRIP=false`, `CARGO_PROFILE_RELEASE_DEBUG=line-tables-only`) so perf could name functions. Debug info does not change codegen. |
| Machine | Linux container, 4 cores, 15 GB RAM. No GPU. |
| GPU stand-in | Mesa lavapipe (software Vulkan, llvmpipe 20.1) under Xvfb 640×480, `--windowed`. Every GPU-side number below is a lavapipe number. |
| Maps | `campaign-test` (typical: 3 094 vertices, 464 cells, 277 entities, 14 textures, 142 MiB PRL). `stress-warren-lit` (large lit: 15 052 vertices, 2 989 cells, 157 lights, 12 textures, 1.85 GiB PRL). Both baked warm with the default `prl-build` options. Model textures baked with `postretro-tool bake-model-textures` for every dev model. |
| Paths | **First load:** CLI map at boot. **Level change:** from Running back to the same map, which runs the full unload, then a fresh worker and install. |
| Runs | campaign-test: 5 boots, 10 level changes (warm page cache) and 3 cold-cache boots. stress-warren-lit: 4 boots, 8 level changes (warm) and 3 cold-cache boots. Cold means `drop_caches` before launch. |
| Stage source | Log line C (`level_timings`), plus temporary finer marks inside install, unload, texture load and geometry install. A temporary env hook triggered the level change and the exit. |
| Attribution | `perf record --call-graph dwarf` of one boot plus one level change per map (n = 1 each), demangled with `rustfilt`. Perf adds overhead. Perf figures give shares inside a stage, not absolute times. |
| Worker harness | A scratch example ran `load_prl` alone, 5–10 times per map, to confirm worker numbers without a renderer. |

Environment workarounds (none committed): lavapipe caps a storage-buffer binding at 128 MiB, so the renderer's 2 GiB device-limit check was clamped to the adapter limit for these runs. The first-launch accessibility hold was satisfied with `accessibility_panel_shown = true` in the player settings file. The release build requires `content/dev/start-script.js`, compiled once with `scripts-build`. There is no audio device, so level sounds were skipped (0 ms).

**Reading log line C.** `prl_parse` is measured on the worker and spliced in before `worker_delivered`. `worker_delivered` already contains it. Summing every entry double-counts the parse. Unload runs before `begin_level_load` resets the timings, so line C never shows it.

---

## Stage cost tables

Milliseconds, median (min–max). `W` = level worker, `M` = main thread (the loading screen is frozen), `U` = unload (main thread, before the worker spawns). Indented rows split the row above.

### campaign-test

Release, warm cache, 5 boots / 10 level changes.

| Stage | First load | Level change |
|---|---|---|
| W prl_parse | 223 (207–249) | 218 (178–262) |
| W delivery wait (poll once per splash frame) | 5 (3–15) | 19 (3–31) |
| U unload total | — | 76 (68–106) |
| U  renderer release | — | 71 (60–93) |
| U   compute pipelines for an empty level | — | 58 (53–84) |
| M range check + static collision | 1 (1–1) | 1 (1–1) |
| M net digest, nav graph, material derive | 0 | 0 |
| M texture_upload (81.5 MiB of `.prm`) | 216 (175–219) | 103 (81–117) |
| M  `.prm` file read | 49 (44–50) | 30 (28–33) |
| M  `.prm` parse (copies each slot) | 52 (40–57) | 22 (21–28) |
| M  texture create + write | 104 (86–108) | 46 (32–57) |
| M uv_normalize | 0 | 0 |
| M geometry_upload | 384 (358–449) | 357 (331–433) |
| M  SH volume + SH streaming GPU init | 192 (178–251) | 190 (178–272) |
| M  SH / direct / scatter compose + animated lightmap | 182 (168–195) | 148 (138–165) |
| M  BVH + candidate cull | 10 (8–12) | 8 (7–10) |
| M  rest | 2 | 2 |
| M bridges, classname dispatch, data script, archetype sweep | 19 (18–25) | 14 (13–18) |
| M model sweep: renderer glTF parse + texture upload | 323 (290–384) | 228 (161–245) |
| M model sweep: hit-zone glTF re-parse | 70 (67–84) | 70 (65–78) |
| M clip resolve, `levelLoad`, host registration, camera, fog | 1 | 1 |
| M sprite collections | 5 (5–5) | 5 (5–8) |
| M level sounds (no audio device) | 0 | 0 |
| M streaming_preload | 142 (132–440) | 275 (117–320) |
| first_level_frame (lavapipe; not representative) | 2097 (1860–2166) | 763 (659–920) |
| **Σ worker (dispatch → delivered)** | **234 (211–252)** | **231 (182–278)** |
| **Σ main-thread install (frozen)** | **1229 (1072–1468)** | **1008 (819–1122)** |
| **Σ request → ready, excluding first frame** | **1463 (1306–1680)** | **1309 (1102–1469)**, unload included |

Cold cache, 3 boots: worker 306 (301–339), main-thread install 1317 (1293–1403), total 1656 (1599–1704). `.prm` read rises from 49 to 96 ms.

### stress-warren-lit

Release, warm cache, 4 boots / 8 level changes.

| Stage | First load | Level change |
|---|---|---|
| W prl_parse | 2207 (2169–2266) | 2189 (2125–2325) |
| W delivery wait | 6 (3–8) | 3 (1–10) |
| U unload total | — | 88 (82–108) |
| U  renderer release | — | 81 (76–94) |
| U   compute pipelines for an empty level | — | 59 (56–68) |
| M range check + static collision | 4 (3–5) | 3 (3–5) |
| M texture_upload (96 MiB) | 181 (166–216) | 104 (92–123) |
| M  `.prm` file read | 40 (36–45) | 32 (28–44) |
| M  `.prm` parse | 34 (34–39) | 29 (23–36) |
| M  texture create + write | 105 (97–132) | 42 (38–48) |
| M geometry_upload | 926 (871–1043) | 865 (783–908) |
| M  SH volume + SH streaming GPU init | 842 (798–956) | 791 (715–830) |
| M  compose family | 64 (59–73) | 59 (54–69) |
| M  BVH + candidate cull | 11 (9–14) | 9 (8–16) |
| M bridges, dispatch, data script, archetypes | 5 (4–6) | 1 (1–1) |
| M model sweep: renderer | 289 (259–300) | 232 (217–263) |
| M model sweep: hit-zone re-parse | 58 (56–59) | 60 (56–74) |
| M sprite collections | 4 (4–5) | 4 (4–6) |
| M streaming_preload | 612 (538–894) | 560 (511–791) |
| first_level_frame (lavapipe; not representative) | 2277 (2136–2310) | 990 (935–1100) |
| **Σ worker** | **2212 (2178–2270)** | **2196 (2129–2328)** |
| **Σ main-thread install (frozen)** | **2088 (1941–2478)** | **1854 (1702–2085)** |
| **Σ request → ready, excluding first frame** | **4322 (4167–4655)** | **4190 (3916–4350)**, unload included |

Cold cache, 3 boots: worker 2274 (2264–2393), main-thread install 2090 (2013–2707), total 4354 (4287–5100).

The map has one entity, yet the model sweep loads about ten models. Movement, weapon and projectile descriptors from the dev mod are preloaded on every map.

### Where the time goes inside the big stages

From perf (release, n = 1 run per map: one boot and one level change on campaign-test, one boot on stress-warren-lit). The main thread was on-CPU 97 % of the install window, so lavapipe threads did not starve it.

| Stage | Map | Breakdown (perf share of the stage) |
|---|---|---|
| Worker parse | stress-warren-lit | SH-streaming validation is about 95 %. Re-deriving the id-49 cluster coverage (`derive_ranges_with_counts`, including `locate_cell`) is about 1.2 s. `ValidationPlan` construction and chunk checks about 0.4 s. Probe-metadata and layout checks about 0.3 s. Positional reads are 67 ms. |
| Worker parse | campaign-test | Animated light weight-map decode about 65 ms (4 M records decoded one by one). Cluster-directory validation about 45 ms. Reads about 50 ms. |
| SH streaming GPU init | stress-warren-lit | BTreeMap-keyed dense-node layout (`derive_dense_node_layout` plus lookups and inserts) about 500 ms. `ShResidencyState::from_manifest` about 100 ms. Sparse compose capacity 60 ms. Compute pipeline creation 140 ms. |
| SH streaming GPU init | campaign-test | Compute pipeline creation 142 of about 190 ms. Dense-node layout and `from_manifest` about 20 ms. |
| Compose family | campaign-test | Compute pipeline creation 73 ms. Buffer creation 45 ms. Weight-map re-pack to bytes 38 ms. Second weight-map consistency check 10 ms. |
| streaming_preload | stress-warren-lit | Planner topology (`PlannerTopology::from_manifest`, BTreeMap of dense nodes) about 510 ms. Lightmap block reads 47 ms. Staging map and texture writes about 105 ms. |
| streaming_preload | campaign-test | Lightmap block reads, staging map and writes about 120 ms. Topology about 25 ms. |
| Model sweep, renderer | campaign-test | Model `.prm` read 76, `.prm` parse copy 78, texture upload 83 ms: about 250 ms of model textures (three 21 MiB diffuse bundles). glTF JSON parse 27 ms, mesh and clips 16 ms, blake3 of source PNGs 7 ms. |
| Model sweep, hit-zone | both | Same `gltf_loader::load_model` call per model again: JSON parse, buffer read, PNG hashing. |
| Texture install | both | `write_texture` staging copy is about 45 %, file read 25 %, slot `to_vec` copy 25 %. |

---

## Candidates, ranked by expected savings

The contract says a main-thread stage that costs the same as a worker stage ranks higher. Savings are per load unless marked.

| # | Candidate | Expected savings | Thread | Crosses a contract? |
|---|---|---|---|---|
| 1 | SH-streaming dense-node maps: dense arrays, built once | lit ≈ 0.9–1.1 s freeze; campaign ≈ 45 ms | M | No for the data-structure change. Building it on the worker crosses the thread split. |
| 2 | SH-streaming load validation: drop the quadratic coverage re-derivation | lit ≈ 1.3 s (prototype); campaign ≈ 22 ms | W | No, if accept/reject stays identical. |
| 3 | Level-invariant compute pipelines: create once per device | ≈ 140–215 ms per install, plus ≈ 58 ms per unload (lavapipe) | M | No |
| 4 | Model sweep: parse glTF once; keep models across level changes | 60–70 ms every load; ≈ 290 ms more on a level change | M | Cache: yes, the clear-on-unload table |
| 5 | Texture install: zero-copy slots; read `.prm` on the worker; keep textures across a change | 25–50 ms (zero-copy); 30–96 ms (read); ≈ 100 ms on a change (cache) | M | Read: thread split. Cache: clear-on-unload. |
| 6 | Animated light weight maps: upload section bytes directly | campaign ≈ 40 ms M + ≈ 50 ms W | M+W | No |
| 7 | Overlap level-invariant install work with the worker parse | up to min(worker, invariant work): ≈ 0.2–0.5 s wall | M | Yes: install order and thread split |
| 8 | Spawn the worker before unload on a level change | ≈ 76–88 ms on a change (≈ 20 ms after #3) | M | Yes: runtime load/unload order |
| 9 | Spawn-cell lightmap block reads on the worker | ≈ 50–65 ms (campaign) | M | Yes: thread split |

### 1. SH-streaming dense-node maps (main thread)

- **Evidence (measured, perf share).** stress-warren-lit, release. The renderer's stored-node ownership layout keys a `BTreeMap` per valid probe, about 0.6 s of the 0.84 s "SH volume + streaming GPU init". The streaming planner builds a second `BTreeMap<DenseNode, u32>` from the same manifest in `PlannerTopology::from_manifest`, about 0.51 s of the 0.61 s preload. Both are derived from immutable manifest data, and both rebuild on every load.
- **Change.** Keys are bounded grid coordinates, so a `Vec` indexed by affinity-node index replaces each map. Code reading suggests the two derivations overlap and could share one product. Both are plain data from an `Arc` manifest, so they could also be built on the worker.
- **Crates.** `postretro-renderer` (SH streaming ownership and residency), `postretro` (SH streaming topology). The worker variant also touches `postretro-level-loader`.
- **Risk.** Moderate. Residency and eviction correctness rest on these maps. The existing streaming tests and capture goldens cover the behavior.
- **Contract.** The data-structure swap crosses none. Moving the build to the worker crosses the thread split (`boot_sequence.md` §2: "PRL parse only"). That is a decision for the owner. The payload stays plain `Send` data.
- **Proof.** Keep the split geometry_upload marks (§Instrumentation). On stress-warren-lit, `geometry_upload` and `streaming_preload` should fall by at least 0.8 s combined, release, n ≥ 4 boots and 8 changes. Residency logs and SH capture output must not change.

### 2. SH-streaming load validation (worker)

- **Evidence (prototype measurement).** The loader re-derives the cluster directory's range table from cells and probes, then compares it with the baked table. The derivation tests every active brick against every member cell of every cluster: O(bricks × cells). With the comparison skipped (throwaway env switch, reverted), the release harness, warm cache, n = 5 each, measured:
  - stress-warren-lit: 2317 → 1014 ms.
  - campaign-test: 193 → 171 ms.
  
  The remaining 1.0 s on stress-warren-lit is mostly other id-49/50 semantic validation (`ValidationPlan`, probe layout). That is the next target.
- **Change.** Keep the check exact but make it sub-quadratic: bin cell bounds into the affinity grid, or test each brick only against cells whose bounds overlap it. Skipping the check would weaken the malformed-file contract. If that is ever wanted, it is an owner decision.
- **Crates.** `postretro-level-format` (cluster directory validation).
- **Risk.** Low if accept and reject stay identical. Prove it with the existing malformed-section tests, plus a randomized equivalence test against the current derivation.
- **Contract.** None. The PRL format and cache keys stay as they are.
- **Proof.** The `load_prl` harness or `prl_parse` in line C, release, on stress-warren-lit, both paths. Loader rejection tests stay green.
- **Why it ranks second.** It is worker time, so it delays the first frame without freezing the loading screen. It still has the largest single measured saving.

### 3. Level-invariant compute pipelines (main thread)

- **Evidence (measured, perf share; lavapipe-specific magnitude).** Each geometry install creates up to ten compute pipelines from static WGSL, depending on which lighting sections the level has: BVH cull, candidate cull, SH compose, direct and animated-direct compose, billboard scatter compose, animated lightmap (two), and the SH-streaming compose passes. SDF shadow and fog pipelines are already created once at full init. On campaign-test this is ≈ 215 ms per install. Naga is about 10 ms of that; the rest is the driver compile. Unload releases level resources by installing an empty level, which rebuilds the compose pipelines (58 ms median, n = 10) and is then discarded.
- **Change.** Create these pipelines and their layouts once at renderer full init. Rebuild only buffers and bind groups per level. Make unload drop level buffers instead of installing an empty level.
- **Crates.** `postretro-renderer`.
- **Risk.** Low to moderate. Code reading shows static shader sources. Bind group layouts sized per level (storage-buffer counts, streaming pass variants) must stay fixed or be keyed by variant.
- **Contract.** None. Renderer-internal; install order unchanged.
- **Proof.** Pipeline-creation count per install drops to zero (wgpu trace, or a counter). `geometry_upload` and unload release fall on the owner's Mac. Metal compile cost differs from lavapipe's LLVM compile, and the system shader cache affects warm runs, so the Mac number decides the rank.

### 4. Model sweep (main thread)

- **Evidence (measured).** The hit-zone pass re-parses every glTF the renderer just parsed: 70 ms (campaign-test) and 58–60 ms (stress-warren-lit) per load, n = 5–10. Unload drops the model cache, so a level change re-uploads the same models: 228–232 ms renderer plus 60–70 ms hit-zone, release, n = 8–10. On the lit map, models come from mod-wide descriptor preloads, not from the map. Model textures are about 250 of the 323 ms renderer cost (perf). Each material's source PNG is also blake3-hashed at runtime, twice per load (code reading plus perf: 7 ms per pass).
- **Change.**
  - Parse each glTF once and hand the CPU model to both the renderer upload and the hit-zone store.
  - Keep uploaded models across a level change, keyed by handle and content, and evict models the next level does not use.
  - Optionally, read and parse model `.prm` files off the main thread.
- **Crates.** `postretro`, `postretro-sim` (hit zones), `postretro-renderer`, `postretro-model`.
- **Risk.** Low for the single parse. Moderate for the cache: clip tables and hit zones must stay coherent, and memory rises between levels.
- **Contract.** The single parse crosses none. The cache crosses the clear-on-unload table in `boot_sequence.md` §4, where per-level GPU resources include models. That is an owner decision.
- **Proof.** Keep the model-sweep split marks. On a level change, the hit-zone mark goes to ≈ 0, and the renderer mark goes to ≈ 0 when the model set is unchanged.

### 5. World texture install (main thread)

- **Evidence (measured).** Each `.prm` is copied three times on the main thread: file read, then `to_vec` of every slot payload, then wgpu's staging copy. Per load, with the release build:

  | Map | Size | Read (warm) | Read (cold, n = 3) | Parse copy | Create + write |
  |---|---|---|---|---|---|
  | campaign-test | 81.5 MiB | 30–49 ms | 96 ms | 22–52 ms | 46–104 ms |
  | stress-warren-lit | 96 MiB | 32–40 ms | 96 ms | 29–34 ms | 42–105 ms |

  On a same-map change every texture is re-read and re-uploaded.
- **Change.**
  - Parse slots as borrowed slices of the read buffer (no format change).
  - The worker already knows the cache keys and the `.prm` root, so it can read the bytes in parallel with the parse and ship them in the payload.
  - Keep textures whose key persists across a change.
- **Crates.** `postretro-level-format` (`.prm` reader), `postretro-renderer`, `postretro` (worker).
- **Risk.** Low for zero-copy. Payload memory peaks earlier for the worker read.
- **Contract.** Zero-copy crosses none. The worker read crosses the thread split, which today keeps `.prm` work on the main thread. The cache crosses clear-on-unload.
- **Proof.** Keep the texture read/parse/upload split marks, cold and warm. GPU-side upload cost must be checked on Metal; see §Could not measure.

### 6. Animated light weight maps (both threads)

- **Evidence (measured, perf share, campaign-test).** The worker decodes about 4 M records element by element (about 65 ms). The main thread validates consistency again and re-packs the records to bytes that equal the little-endian wire layout (38 ms), then uploads them.
- **Change.** Keep the section's raw record bytes and upload them directly. Validate once, on the worker.
- **Crates.** `postretro-level-format`, `postretro-level-loader`, `postretro-renderer`, `postretro-render-cpu`.
- **Risk.** Low. Every target is little-endian. Alignment needs a check.
- **Contract.** None.
- **Proof.** `prl_parse` and the compose-family mark on campaign-test. Animated-lightmap capture output must not change.

### 7. Overlap level-invariant work with the worker parse

- **Evidence (code reading plus measured sizes).** The main thread paints the splash for the whole parse (0.23 s campaign-test, 2.2 s stress-warren-lit). Several pieces of install work do not depend on the PRL: descriptor-driven model preloads (≈ 230–290 ms), pipeline creation (if #3 does not move it to init), and the impact sprite collection.
- **Change.** Start those during Loading, before delivery.
- **Crates.** `postretro`, `postretro-renderer`.
- **Risk.** Moderate. The preload set partly depends on map placements, so it must be split into an invariant part and a level part.
- **Contract.** Crosses install order (`boot_sequence.md` §3, where the model sweep is stage 12 after the archetype sweep). Work done during Loading also blurs the §2 split.
- **Proof.** The wall from request to ready falls while the per-stage marks stay similar.

### 8. Unload before worker spawn (level change)

- **Evidence (measured).** Unload takes 76 ms (campaign-test, n = 10) and 88 ms (stress-warren-lit, n = 8). It runs synchronously before the worker spawns, and log line C does not show it.
- **Change.** Spawn the worker first, then unload while it parses. The worker touches no engine state.
- **Crates.** `postretro`.
- **Risk.** Low. Peak memory briefly holds two levels' CPU data.
- **Contract.** Crosses `boot_sequence.md` §4 ("Unload current level first, then spawn worker").
- **Proof.** The level-change wall falls by about the unload time, using a permanent unload mark.

### 9. Spawn-cell lightmap preload reads

- **Evidence (measured, perf share).** campaign-test preload is about 120 ms of block reads, staging map and texture writes on the main thread. The spawn eye comes from map entities, which the worker already parses.
- **Change.** Read the spawn cell's mandatory blocks on the worker.
- **Crates.** `postretro-level-loader`, `postretro`.
- **Risk.** Moderate. The spawn pose can change at install (start pose, frontend backdrop).
- **Contract.** Crosses the thread split.
- **Proof.** The `streaming_preload` mark on both maps.

### Checked and not worth building now

| Item | Measured cost | Note |
|---|---|---|
| Static collision trimesh | 1–4 ms | Too small to justify moving it to the worker. |
| Net parity digest | 0 ms | Single-player has no endpoint. With one, code reading shows a blake3 `update` per float; batching the bytes would keep the digest identical. |
| Data script (QuickJS) | 9–13 ms | |
| Classname dispatch, bridges, archetypes | 1–19 ms | |
| Sprite collections | 4–5 ms | Code reading: decodes every PNG frame only to count frames. |
| Worker delivery latency | 3–31 ms | The channel is polled once per vsynced splash frame. A wake-up would save under one frame. |
| Dropping the old `LevelWorld` | 0.2–5 ms | |
| Wireframe index buffer, per-element vertex/index byte copies | < 1 ms at these sizes | |

---

## Instrumentation worth keeping

Recommend these as permanent marks: always compiled, cheap. None is committed here.

| Mark | Why |
|---|---|
| Unload duration, in line C on a level change or its own line | Unload is invisible today. |
| Split of `geometry_upload`: SH streaming init, compose and pipeline creation, cull | One mark hides three unrelated costs. |
| Split of `model_load`: renderer upload vs hit-zone parse | The duplicate parse is otherwise unseen. |
| Own mark for sprite collections, fog masks and host registration | Today they land in `audio_load`. |
| `texture_upload` split into read, parse and create+write | Separates I/O from upload, and shows cold cache. |

Line C should also stop counting the parse twice: `prl_parse` sits inside `worker_delivered`. The scratch patch used for this investigation is outside the repository, at the scratchpad path given in the hand-off report.

---

## Could not measure

| Gap | What the owner should run on the Mac |
|---|---|
| GPU-side texture and geometry upload, and the first-frame present | Release `cargo run -p xtask -- run --release -- content/dev/maps/<map>.prl` with `POSTRETRO_GPU_TIMING=1`. Read log line C and a Metal System Trace (`rendering_pipeline.md` §12) across the install frame and first frame. lavapipe numbers here are CPU rasterization and are not representative. |
| Compute pipeline compile cost on Metal | Same run, with and without a warm system shader cache (first launch after a rebuild vs second launch). This sizes candidate #3. |
| Level sounds | No audio device here, so `audio_load` was 0 ms. Measure on the Mac with real sounds. |
| Gameplay-heavy large maps (`stress-warren-showcase`, `stress-warren-hallway-inspection`) | Not baked. stress-warren-lit took 32.6 min and pushed the stage cache to 13 GB. These maps would size install segment B (dispatch, archetypes, triggers) at scale. |
| Connected-client and listen-host installs | Not exercised. The parity digest and host registration ran in single-player only. |
| A different map on level change | Both change paths reloaded the same map. A change to a different map would show the cross-level cache candidates (#4, #5) with partial overlap. |

## Open question carried from the contract

The owner chooses the target: faster time to first frame, or a loading screen that never freezes. Candidates 1, 3, 4, 5 and 6 serve both. Candidate 2 serves only time to first frame. Candidates 7–9 serve the freeze mainly by moving work off the main thread, and each crosses a contract.
