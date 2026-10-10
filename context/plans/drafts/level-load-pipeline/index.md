# level-load-pipeline

Brief · resumable · reads: `context/lib/boot_sequence.md` §1 (Loading screen), §2, §3, §4, §6 · `context/lib/rendering_pipeline.md` §12 · `context/lib/networking.md` §Admission and content parity · `context/lib/testing_guide.md` §Resource bounds · `context/plans/done/level-load-perf/` · read at da46511f7

**Sequence:** builds after `sh-streaming--reveal-gate-and-warm-horizon`, whose Settling state this brief consumes.

## Problem
Developer-raised, from measured cost (`research.md` §1–§2). A level load freezes the main thread for one redraw of ~0.7–1.0 s (lavapipe). That redraw does the whole install and also renders the first level frame. Meanwhile the loading bar can't move, the co-op transport poll stalls, and the parse worker sits idle. Cause: everything except the PRL parse runs serially on the main thread inside that frame, including CPU preparation, GPU resource creation, and rebuilding level-invariant pipelines and models, because `boot_sequence.md` §2 limits the worker to the parse. When done, the freeze shrinks to a short commit frame, and the time to a complete, revealed frame shrinks with it. The bar moves until reveal. Level-invariant work is not redone per load, so a level change pays nothing a first load doesn't.

## Decisions
- **Commit frame holds only main-pinned work.** The commit frame keeps resource hand-back and the script-VM and registry stages, in the `boot_sequence.md` §3 order. Sprite-collection registration stays there as a named, counted exception. Every other pipeline, texture, buffer or bind-group creation (fog targets included), `.prm` or glTF parse, sound decode or SH derivation happens there only as a counted fallback.
- **Metal is proof, not a gate.** Owner choice: build to the absolute-freeze target from the lavapipe floors (`research.md` §2.3), then report Metal numbers. Diverges from `plans/done/level-load-perf/contract.md`, which sent GPU-dependent candidates to Mac measurement first, and from `index.md` §1.1 "measure, don't assume".
- **Staged pipeline.** The parse emits ordered hand-outs as section groups decode. CPU prep and GPU upload consume each hand-out as soon as its inputs exist, and the main thread commits once the bundle is complete. This is research Floor B′. It diverges from `boot_sequence.md` §2 ("PRL parse only"), rewritten at promotion, and from `plans/done/loading-screen` "phases are sequential".
- **One bounded engine pool.** CPU prep runs on one size-capped, engine-owned pool with boot lifetime. There is no async runtime, and streaming keeps its own threads (`plans/done/sh-streaming--warm-set-and-io-contract.md`). This meets `plans/done/loading-screen` "no thread pool until measurement demands one": on campaign-test, prep alone (≈ 209 ms) exceeds the parse (≈ 127 ms) (`research.md` §2.2).
- **Renderer owns GPU** (`index.md` §2). The upload stage lives in the renderer crate and is the only off-main code that creates GPU objects. It touches only resources not yet published to the frame path, and its writes are submitted before the first level frame presents. Diverges from the `boot_sequence.md` §2 row that puts all GPU work on the main thread, and from streaming's budgeted main-thread install (`rendering_pipeline.md` §4), which stays as it is for in-play streaming.
- **Pipelines once per renderer.** The upload thread builds every install-time pipeline once, in the background, starting right after full init, and keeps them. SH compose reuses its full-init instance. Commit and unload create none, and a load that needs an unfinished pipeline waits for it off the main thread.
- **Model cache.** Parsed and uploaded models, with their hit-zone and clip source, survive a level change. The key covers file content and every install-time parameter that shapes the result, and a hit is indistinguishable from a fresh load. Each commit releases entries its level doesn't reference, and a failed or cancelled load leaves the cache as it found it. Steady state holds one level's set; the peak, between upload and commit, holds two. Diverges from the §4 clear-on-unload table, rewritten at promotion, and from `resource_management.md` "the level owns everything".
- **Predict models, then fall back exactly.** Prep predicts the model set. A miss takes today's synchronous path at commit and is counted, so correctness never rests on a prediction.
- **Spawn lightmap is a settle target.** The spawn view's lightmap blocks join Settling's settle check and are read without blocking a frame. No spawn-cell prediction and no synchronous spawn preload remain.
- **Scene extent.** A resize during Loading makes the commit rebuild the extent-bound bind groups once, counted like a prediction miss.
- **Settings are read once at request.** Player options never apply on a Loading frame, so the request's tiers are the commit's tiers. A later change follows its existing rule: Surface Depth live, shadow tier at the next level boundary.
- **Failures before commit.** A prep, upload or parse failure, including late parse validation after hand-outs went out, is one load failure raised before commit. Runtime loads go to Frontend; CLI boot loads exit non-zero. An upload failure means an error wgpu reports synchronously on the upload thread; device loss stays out of scope. The streamed-SH init `panic!` becomes an ordinary load failure.
- **Cancellation.** A superseding request, a queued unload, or suspend cancels a load that has not committed. A queued unload then enters Frontend; a backdrop arrives only through the load a return to frontend already queues. Generations never overlap, on the streaming retirement precedent (`boot_sequence.md` §4). A runtime request during a CLI boot load is still refused.
- **Loading screen.** It stays drawn through Settling, replacing reveal-gate's "splash stays painted". On a supersede it keeps its tree and UI clock, shows the new level's name, and restarts the bar at 0, which diverges from `boot_sequence.md` §1 "UI clock from zero each load".
- **Unload-first stays** (§4). One level's world data is resident at a time; the model cache is bounded by the same working set.
- **Progress covers the whole load.** `loading.progress` stays a monotone 0–1 slot, weighted across parse, prep, upload and settling, and reaches 1.0 at reveal. The 0.85 cap is removed. It stays "honest and coarse" (`plans/done/mod-loading-screen-contract.md`): every phase credits real units of its own work, and the counter stays in the level-loader or below.
- **Co-op unchanged in shape.** Parity, relevel id and join seed publish at commit (`networking.md` §Admission and content parity). The transport polls on every Loading frame.
- **Line C gains marks.** Each new stage, the progress-membership capture, the commit frame and the reveal get marks. Log-only, as in `plans/done/level-load-perf`.
- **Non-goal: boot pipeline.** Early CLI-map parse, a parallel `Session::build`, parallel full init or mod-init decodes, and a wgpu pipeline cache go to a separate brief. They cross the causal two-frame rule and the first-launch hold (`boot_sequence.md` §1), which this brief leaves alone.
- **Non-goals, load side.** Spawn-before-unload (§8 non-goal). Announcing a relevel at host request: a failed host parse would leave clients on a level the host never installs (`research.md` §6 Q9). A multi-frame install or moving parity publication. A cross-level world-texture cache: world textures are per-level content at ~80–96 MiB, so keeping them doubles resident texture memory across a change. Caching sounds or moving them to mod lifetime: each level reloads the whole `sounds/` directory, and the saving is unmeasured.

## Acceptance
### Automated
**Equivalence**
- [ ] On every fixture, the staged parse's hand-outs together equal the monolithic parse's level data.
- [ ] On fixture levels that include at least one baked texture and one model, pipeline install and the main-thread oracle path produce the same renderer resource summary: texture count, sizes and formats; buffer sizes; SH residency state; lightmap pool plan.
- [ ] With a renderer present, both paths produce the same entity-id order and the same clip and hit-zone tables.
- [ ] Existing order guards stay green and run against the pipeline's commit path, not only the retained oracle: range rejection precedes all state mutation; light entity ids precede fog entity ids; pinned-seed trigger restart.

**Staging**
- [ ] On a streamed fixture with at least one baked texture, texture prep starts before the parse's last section read, and upload starts before the parse returns. The test holds the parse at each point until that stage has started, and a hold that times out fails the test.
- [ ] No stage reads the SH stream manifest's cluster topology before the parse installs it. A test holds a streamed fixture's parse just before that install and shows planner topology prep has not started; released, the load installs (pin P19).
- [ ] A map with geometry but no textures, and a map with no SH manifest, each reach bundle-ready and reveal. A stage whose kind gets no hand-out finishes when the parse returns.

**Commit frame**
- [ ] A level install creates no pipelines, parses no `.prm` or glTF, decodes no sound, and creates no textures or buffers on the main thread. The exceptions are sprite-collection registration and counted fallbacks (a model prediction miss, a scene-extent rebuild). Counts come from a device-wide source, either wgpu's internal counters or a chokepoint a grep gate proves every creation call uses, read across the commit call alone.
- [ ] The upload thread builds the install-time pipelines once, starting right after full init, off the main thread. The commit and unload create zero, and a second load reuses every one. A load dispatched before they finish waits off the main thread. A frontend-only boot builds them without blocking a frame.
- [ ] A level with a non-default fog pixel scale gets its fog target from the upload stage, not the commit.
- [ ] The commit frame never waits on a stage thread or on the GPU. It runs only once the bundle is ready. The commit path holds no blocking receive, thread join or device wait (grep gate). A Loading-step test shows that a bundle not yet ready paints instead of committing.
- [ ] The upload's last write is submitted before the first level frame presents, also when a frame's surface acquire is skipped (pin P13).
- [ ] A scene-extent change during Loading: the commit rebuilds the extent-bound bind groups once and counts it, and the first level frame binds the new extent's views. Two extent changes on different Loading frames still rebuild once. A resize recorded after the last Loading paint is applied by the first level frame's ordinary resize, and the commit counts no rebuild (pins P16, P23).
- [ ] Level sounds still reload from the mod's sounds directory on every load. A missing or bad sound file still warns and the level loads.
- [ ] Sounds register in sorted path order whatever order the pool decodes them. With two files sharing a key, the same file wins as today. A mod with no sounds directory, or an empty one, loads with no sounds.
- [ ] On a host level change, level parity, the relevel id and the join seed are unset on every Loading frame and set on the commit frame, after the range and collision checks (pin P14).
- [ ] Line C names each new stage, the progress-membership capture and the commit-frame mark. A stage timed on another thread prints inside the main-thread stage that contains it, so the line still adds up.

**Failure**
- [ ] An injected prep failure and an injected upload failure each end a runtime load in Frontend and a CLI boot load in a non-zero exit, with no registry mutation and no parity publish.
- [ ] A streamed-SH init error is a load failure, not a panic. A fault-free load still installs.
- [ ] A fixture whose late parse validation rejects the map after earlier hand-outs went out: the load fails once, every stage joins, nothing commits, and no cache entry from it remains (pin P18).
- [ ] A fixture whose late validation fails because handed-out data is out of range, such as a cluster directory naming a cell past the cell table, fails as one load failure. No stage thread panics on the unvalidated hand-out.
- [ ] A panic injected into the parse, into a prep task and into the upload stage each ends the load as one failure, as a parse panic does today: Frontend at runtime, a non-zero exit at boot. The process does not abort.
- [ ] With a renderer present, a real wgpu validation error raised on the upload thread, such as a write past a buffer's end, is caught by that thread's error scope and ends the load as one failure, with no panic.
- [ ] When the parse fails while prep runs, or two stages fail together, the load reports exactly one failure, every stage joins, and nothing commits (pin P10).

**Cancellation**
- [ ] Suspend during prep, and suspend during upload: every stage thread joins via retirement, also on frames before the resumed renderer exists. No join runs on the event-loop thread. Nothing commits. The first load after resume starts its stages only after they join (pin P6).
- [ ] A cancelled load reports nothing: its stages' errors, the cancel itself included, never reach the failure path. After suspend during a CLI boot load, the resumed boot load installs and the process does not exit (pin P20).
- [ ] Suspend while the pipelines build: none of them reaches the resumed renderer, and the resumed renderer builds them once (pin P22).
- [ ] A superseding request mid-pipeline: the old load never commits and the new one installs. A request after commit unloads normally.
- [ ] Two superseding requests on one frame: only the last installs.
- [ ] Three loads requested on successive frames, each before the previous cancelled load has joined: only the last installs, the middle one runs no stage, and no two loads' stages run at the same time (pin P4).
- [ ] A request already queued when its frame finds the bundle ready cancels the load, which never commits (pin P1). A request the same frame's transport poll receives is handled on the next frame, after commit, through the ordinary unload (pin P2).
- [ ] An unload queued behind an in-flight load cancels it. The load never commits, and the game enters Frontend. Without a queued load, no backdrop loads (pin P3).
- [ ] A load and a return to frontend on one tick with a backdrop declared: the load never starts, and the backdrop installs (pin P24).
- [ ] A runtime request during a CLI boot load is still refused with a warning and cancels nothing, whether it arrives during the parse, prep or upload. A runtime request during a runtime load no longer waits for it to install (pin P8).
- [ ] Cancelling a load while the pipelines build neither cancels nor restarts the build, and the next load creates zero pipelines (pin P5).
- [ ] No stage work starts while a request only waits in the queue: none during the first-launch hold or the OS-preference wait, and on a level change none before the old level's unload (pin P12).
- [ ] A superseded load keeps its loading tree and UI clock, shows the new level's name, and restarts the bar at 0. The bar never decreases within one load.
- [ ] After a supersede, the old load's stages credit nothing to the new load's bar.

**Prediction**
- [ ] On every dev-mod map, baked first and run on demand, the model prediction misses zero times.
- [ ] On a connected-client fixture with a host-replicated placement, the model prediction includes that placement's model and misses zero times.
- [ ] A load requested after a hot reload that changed a mesh block predicts the new model and misses zero times. The old level kept the old model until its unload (pin P9).
- [ ] A forced model miss (a model only the registry adds) loads correctly and logs one miss.
- [ ] The spawn view's lightmap blocks are a settle target: reveal waits for them, no frame blocks on their reads, and a level with no streamed lightmap settles without them.
- [ ] A `--start-pose` elsewhere in the map settles the blocks around that view, not around the first player spawn.

**Caches**
- [ ] A same-content level change hits for every shared model, with no re-parse and no re-upload.
- [ ] Changed file content misses.
- [ ] A surface-depth tier change between two loads still hits for every model whose materials the tier does not shape.
- [ ] A hot-reloaded mesh descriptor takes effect at the next level boundary and not before.
- [ ] An entry the next level doesn't reference is released at its commit. A referenced entry is kept.
- [ ] Suspend clears GPU entries. A bundle ready but not committed at suspend never reaches the resumed renderer. The first load after resume re-uploads every model it uses and is correct (pin P7).
- [ ] A failed or cancelled load leaves the model cache as it found it. The next commit still releases every entry its level doesn't reference (pin P11).

**Progress, pool, settings**
- [ ] Progress never decreases. On a map with a streamed SH manifest it moves before the parse's first section read. It is exactly 1.0 at reveal, including a reveal released by the settle timeout. A near-empty level also reaches 1.0.
- [ ] The loading tree stays drawn through Settling. Progress stays below 1.0 until reveal, and rises during prep, upload and settling, not only during the parse. A test samples it at each stage boundary of a fixture load (pin P15).
- [ ] A load with an empty prep set completes.
- [ ] Load stages start no threads beyond the engine pool, the renderer's upload thread and the existing streaming threads (grep gate).
- [ ] Tiers are read when the request dispatches, not when it is queued. A tier changed on the frame a request is queued reaches the level that request installs (pin P21).
- [ ] A tier change on the commit frame applies after commit through the live path, and the shadow tier waits for the next level, as today (pin P17).

**Gate**
- [ ] Every row above that needs a renderer runs on an adapter-present test run, and the gate confirms those tests ran rather than skipped.

### Manual
- [ ] Metal release on this Mac, stress-warren-lit and campaign-test, the pool at its shipped size, first load (n ≥ 5, model cache cold) and change (n ≥ 8, model cache warm), medians against the named baseline commit: line C before and after, reporting time to reveal, the commit-frame mark, unload, and the `renderer_full_init_complete` delta. First load and level change sit side by side per map, and any work a level change pays that a first load doesn't is named. Follows `research.md` §5 and the Mac measurement memory (caffeinate, screen saver check).
- [ ] Peak RSS for boot plus one change, before and after, same build, pool size and maps, naming the caches' steady-state cost (§Resource bounds).
- [ ] Record, for the boot brief, whether Metal compiles pipeline state at creation or at first use (`research.md` §5 #1, #5).
- [ ] The loading bar animates without a stall through a stress-warren-lit load on Metal. Any Loading-frame stall from large upload writes is reported with its write size.
- [ ] A co-op client stays connected through a host relevel, and its transport does not stall while the host loads.
- [ ] `boot_sequence.md` §7 lifecycle checklist, including `Alt+Shift+L`. No wgpu validation errors across load, unload and load-different. No stale models after a change.

## Path
- Shape: a staged pipeline that ends in a short commit (Floor B′). Rivals:
  - Floor B, where prep starts at parse delivery. It is simpler, but on stress-warren-lit it serializes ≈ 300 ms of prep and upload behind an ≈ 800 ms parse (`research.md` §2.3). A whole-blob loader is also the wrong shape for later large-map streaming (`context/plans/large-map-spatial-residency.md`).
  - Floor A (prep off-main, GPU creation still in the install frame) leaves a 150–350 ms freeze.
  - Measurement-gated order reaches the same end state. Owner chose to build first.
  - Budgeted main-thread upload: uploads spread across Loading frames under a per-frame budget, as streaming installs, then one commit. Fewer new mechanisms and no cross-thread GPU seam, but on short-parse maps such as campaign-test (≈ 330 ms of R behind a ≈ 127 ms parse) it adds roughly R's budgeted wall time to time to reveal (`research.md` §2.2).
  - Time-slicing the install across frames needs no thread, but vsync roughly doubles R's wall time. It also breaks the one-frame install, so parity and `levelLoad` timing would need re-deciding (`research.md` §6 Q3).
- First slice: split pipelines out of each compose constructor, build them once on a background thread after full init (SH compose reuses its full-init instance), and remove the empty install from `release_level_resources`. Next, split `load_textures` into a CPU prep pass and a GPU pass, and run the GPU pass on a cloned queue off-main. That falsifies the riskiest assumption: no validation errors, and the writes land before first use.
- Staged parse: `load_prl_from_container` already decodes in a fixed order (`research.md` §8). The cluster-directory and cross-section validations at its end can reject after hand-outs, which the failure path cancels.
- Pool: rayon is already a workspace dependency. Prediction inputs: descriptors, map placements, `prop_mesh` model keys and client-suppressed placements for models. Cancellation: a cooperative flag checked between stages, never inside a positional read.
- CPU-only renderer entries (no wgpu types) for SH residency derivation and texture prep, or move the derivation to a lower crate. Catch upload errors with an error scope on the upload thread. Texture install adds samplers to a shared map: build them privately and merge at commit. Fog's extent-sized target moves into the upload stage. The `per-frame-upload-batching` drift guard needs a class for off-main writes. The renderer already depends on the level-loader, so it can credit progress.
- Seams: `Renderer::install_level_geometry`, `install_textures`, `load_textures`, `register_smoke_collection`, `ShResidencyState::from_manifest` and `initialize_gpu`, every `sparse_compose_capacity` call site, `UploadQueue::installation`, `install_level_payload`, `finish_level_payload`, `install_spawn_streaming`, `set_fog_pixel_scale`, `load_level_sounds`, `distinct_mesh_models`, `spawn_level_worker`, `drain_level_requests`, `begin_loading_screen`. Keep-across-levels precedent: `spot_shadow_pool_needs_rebuild`. Retirement precedent: the SH async worker pool.
- Prepared products must be `Send`. Assert it at compile time, like `LevelPayload`.
- Equivalence oracle: keep the main-thread path and the monolithic parse callable from tests until landing (`plans/done/level-load-perf` Invariants).
- Settling comes from `sh-streaming--reveal-gate-and-warm-horizon`: its one settle chokepoint takes the spawn lightmap target, and its splash paint becomes the loading tree. Today `present_world_less_frame` clears level streaming on every Loading paint, so nothing streamed may be set up before commit.
- Session-verified facts, pipeline creation sites and ordering pins: `research.md` §8–§9.

## Open questions
- Pool size cap and whether it scales with core count — **delegated**: the executor decides and reports it in the plan of record.
- Cache key: whether to hash file content or reuse the existing content keys, per asset kind — **delegated**.
- Phase weights for progress — **delegated**.
- Hand-out granularity of the staged parse — **delegated**.
- Whether to chunk large upload writes to bound Loading-frame stalls — **delegated**.
