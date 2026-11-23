# glossy-world-reflections — plan of record

mode: resumable
status: approved
read at: 8dda8a34a

Source unchanged since the brief's `a7bf2a19`; every cited symbol re-read at `8dda8a34a` (compiler, runtime, shaders).

## Block (2026-11-22, resolved): the 256 cap does not hold on production-scale content

**Resolution (owner, 2026-11-22).** Build the brief as decided, behind a compile-time gate: glossy placement runs only under `prl-build --glossy-reflections`, used for the glossy test map, until large-map probe support (`reflection-probe-streaming` and its prerequisites) lands. The brief gains Decision "Gated", three Gate rows (G1–G3) and a deferred docs row. The world-face probe class (option 1 below) is noted for the streaming brief, not built here.

**Premise.** Decisions "Probe cap stays at 256" and "Generalized probes" (one probe per coplanar glossy face group) together assume glossy content fits the cap, with drops at the margin. The first slice existed to test that on real content (Path, First slice).

**Evidence (Task 1 census).** Groups are coplanar (1°, 1 mm), edge-connected glossy faces, the probe estimate; water is id 54's existing count. The owner's private maps are labelled A–L; the label-to-name key is held outside this repo. Their committed PRLs predate current loader versions (AlphaLights v4, ClusterShPayloads v2), so geometry is a past compile's: right for a census, not for anything else.

| Map | Glossy faces | Planes | Groups (up/down/other) | Groups < 1 m² | Water | Estimate |
|---|---|---|---|---|---|---|
| A | 7148 | 472 | 1708 (273/229/1206) | 1330 | 37 | 1745 |
| B | 5893 | 770 | 2203 (400/146/1657) | 1411 | 26 | 2229 |
| C | 1562 | 159 | 355 | 243 | 0 | 355 |
| D | 1210 | 188 | 336 | 144 | 28 | 364 |
| E | 1099 | 207 | 293 | 109 | 0 | 293 |
| G | 665 | 205 | 320 | 201 | 1 | 321 |
| F, H, I, J | 747, 468, 199, 182 | — | 225, 219, 94, 98 | — | 0 | under cap |
| `specular-texel-sharpness` (dev) | 56 | 14 | 15 | 6 | 0 | 15 |

Six of twelve maps pass 256; A and B by 7–9×. One probe per plane does not fit A or B (472, 770 planes), and groups of 1 m² or more alone are 378 and 792. Under the Decisions as written, smallest-first drops would leave most glossy faces on those maps unreflected, so the Problem's "every static world face whose material paints sharpness above a floor reflects its room" fails on the content the owner calls closest to production. No `content/dev` map but the sharpness dev map has a glossy material (`campaign-test` has none).

**Options for the owner** (executor recommends 1):
1. **A world-face probe class at the minimum-blur resolution.** World faces never read sharper than m_min = 2, the 32² tile, so their probes capture at 32² with a chain to 4², packed many per array layer, under their own cap (4096 ≈ 13.6 MiB: colour ≈ 1.3 KiB + distance 2 KiB per probe, about today's 256-probe water cost). Water's probes and their 128² arrays stay exactly as built, so water parity is by construction; a probe binding water and a glossy floor stays a water-class probe. Changes Decisions Blur, Probe cap and the Wire format (a second tile size and per-class layout); the world share table and sums move to storage buffers in the overlay's own layout. Coarser distance tiles widen the capture-shadow fade on world faces. Capture rays fall ~16× per world probe.
2. **Merge small groups.** Groups under an area floor bind to the nearest same-facing probe whose capture point lies in front of them in the same room, else draw no reflection. Keeps format and cap; changes Generalized probes. Not enough alone for A and B.
3. **Raise the cap at 128²** (the roadmap's `reflection-probe-streaming` territory): ≈ 115 MiB for B, layers past 256 need packing anyway.
4. **Keep the Decisions**: accept that most glossy faces on large maps drop, and ask authors to cut probe counts with `reflection_volume` boxes (one probe per facing per box).

## Corrections
- **Self-transparency is per face, the exclusion mask per brush.** `hit.rs` `ProbeExclusion` / `closest_hit_masked` skip whole brushes (`face_brushes`); a glossy face's own brush usually has other faces the probe must see. → Extend the primary-ray mask with a per-probe face set (bound reflector faces), beside the brush set, keyed into `stage_key.rs` `cache_key` with `exclusions`.
- **More Y-only sites than Path lists.** Also `capture_point_over` and `over_water` (top-down XZ outline), `PointClassifier::fault`'s ±X/±Z crowding, `fit_box`'s column logic, `authored_capture_point`'s lowest plane, `_capture_offset` (XZ only), the "more than one height" warning, `exclusion.rs` `classify`'s one global `PlaneBasis::new(WATER_REFLECTOR_NORMAL)`, and `exclusion_underside.rs` `bound_water` (id 52 only; its `filter_map` would silently skip id-17 faces). → Each takes the probe's plane (normal, offset) and an in-plane basis; `UpFacingFace` generalizes to a reflector face carrying its source (fluid `(volume, face)` or world `face`) and its outline in plane coordinates.
- **Placement reads material bytes from disk.** `MaterialStageResult` holds no slot texels; the probe stage reaches them only through `texture_keys` → `.prm` under `prm_root` (`PrmFile::from_bytes_partial`, as capture already loads albedo, `reflection_probes/mod.rs:431`). → A new loader reads mip 0 of `PrmSlot::Specular` per used material once per build; the opt-in verdict joins the cache key. The stage's skip gate (`mod.rs:291`, "no reflection volumes and no up-facing fluid faces") gains a glossy-face term.
- **No runtime glossy flag exists.** Buckets are material indices; nothing marks a material glossy. → The opt-in predicate is one pure function over a specular slot's mip-0 bytes, in level-format beside `reflection_probe_lobe_weights`. The compiler applies it to place probes; the loader applies it to the same bytes to mark glossy buckets. Same bytes, same verdict, no format field.
- **Forward's specular is already split.** `forward.wgsl:1680` returns `base·total_light + emissive + tinted_specular`; untinted specular rides `total_light` via `specular_sum` (1477); dynamic lights add no specular. D_static = `base·(ambient + indirect + static_direct)` at 1408–1410, after the shadowmask subtraction (1404). The debug early returns (1688–1734) must write every declared output.
- **Mover pixel path folds highlight per light.** `kinematic_brush.wgsl` `accumulate_pixel_direct` adds untinted specular into `total` per light (667); `tinted_total` is separate. Writing highlights apart there needs the second accumulator the file's own note (674–677) says crosses the register cliff. Smooth already keeps one specular sum. → Task 2 screens it before any floor-layer work builds on it.
- **Forward and mover pick specular style differently.** Forward reads a uniform bit (`SPEC_SHADOWMASK_FLAG_STYLE_SMOOTH`); the mover builds one beauty pipeline per style. Forward has no Surface Depth or style variants; Surface Depth reaches forward through each material's own uniform (group 1), so an overlay binding group 1 follows a live toggle for free (P11).
- **`@invariant` is duplicated, not shared.** Copies in `depth_prepass.wgsl:36`, `forward.wgsl:458`, `kinematic_brush.wgsl:165/201`. → The overlay's vertex stage repeats forward's exact expression (`uniforms.view_proj * vec4(position, 1)`) over the same uniform, and a source test pins it to forward's.
- **The planar mirror shares forward's pipeline** (`planar_reflection/record.rs:295`, one attachment at :279). → Forward keeps a one-output form; the mirror binds it (Delegated answers: mirror records no overlay).
- **Skinned meshes write depth** (`mesh_pass/pipeline.rs`, Less + write), so an `Equal` overlay after them already fails where a character covers the world; only coplanar movers need the guard (P15).
- **Split list grows.** Beyond `compute_cull.rs` (1400), `renderer_render_frame.rs` (1374) and level-compiler `pipeline.rs` (2357), this work extends `reflection_probes/mod.rs` (630), `box_fit.rs` (686) and `capture_exclusion.rs` (741). Each splits behaviour-preserving in its own commit before it grows. New recording lands in new files (`renderer_glossy_frame.rs` beside `renderer_fluid_frame.rs`).
- **Ownership scans pin today's owner.** `fluid_surface_tests.rs` :5776 and :6077 require `FluidProbeResources::new(` only in `install.rs` `build_surfaces`; :5608 greps `has_probe_term_draws`' body. → Rewritten with the move (Task 4), keeping every fence they assert.
- **Water's lookup can sit a hair below the horizon.** Its reflected ray is floored above the horizon, but the probe lookup runs from the capture point 2 mm above the water, so a box hit just under that lift has y slightly below 0. Five lobes spread y²/4 to the sides there; six put y² on −Y, whose water share is 0. The ratio moves by about y² × share, far under 1/255; row 2's A/B is the proof.
- **`context/lib` still describes version 2** (`build_pipeline.md` §PRL section IDs, `resource_management.md` §5.3 sums 20 480 B). Updated at landing with the rest.
- **A usable id 54 binding no face now allocates nothing** (before, a fluid level uploaded such a section's arrays). The renderer does not read world bindings until Task 5; the loader validates them.
- **More `context/lib` text describes fluid ownership**: `rendering_pipeline.md` §7.3a Dispatch, Storage, Install and §12 (`fluid_reflection_sums`), `resource_management.md` §5.3. Updated at landing.
- **Pre-existing on `main`: `postretro` clippy fails** on `clippy::chunks_exact_to_as_chunks` in `scripting/frame_systems/light_bridge.rs` (toolchain lint, file untouched here). Fixed at preflight in its own commit.
- **Glossy floors group like water, not like the census.** Placement groups up-facing glossy faces by plane height, then by fitted box (`box_match_components`), as water does; the census's edge-connected groups were an estimate only. A matte strip or an inlay's surround never splits a floor's probe, so probe boundaries stay out of open floor.
- **Self-transparency covers the whole reflector plane.** A glossy probe's primary rays skip every id-17 face in its reflector plane facing the same way, bound or not (`self_transparency.rs`, `ProbeExclusion::with_transparent_faces`): that is what clears a coplanar matte strip (row 11). Shadow rays and the box fit still see them. Task 9: once glossy probes exclude brushes, suppress fills in the transparent plane, or a fill rebuilds the floor under an excluded monument.
- **Derived world probes place after water's derived probes** and take only the slots water leaves; their drops warn separately. Water's placement and cap are byte-identical (A/B, below).
- **Probes binding world faces classify no exclusion yet** (`exclusion::classifies`): the width and underside rules read bound water only. Until Task 9 the test map warns "sees 45% of the glossy floor" behind its monument, as research §Monument predicts.
- **D_static format.** `Rg11b10Ufloat` where renderable (else `Rgba16Float`). Its ~3% rounding means forward − W·D_stored can dip just below 0 where W ≈ 1, R ≈ 0 and nothing but static light shades the pixel, against row 30 (no channel below 0). Task 6 decides: `Rgba16Float` (66 MB at 4K vs 33 MB) or a composite that cannot undershoot.
- **The overlay applies under SDF debug view modes** too; forward's debug early returns write D_static, so the term is consistent but not meaningful there. Accepted for dev views.
- **Forward's always-run entry changed shape**: `fs_main` now returns `forward_shade().colour`, with `fs_main_static_diffuse` beside it. Its AMD Metal cost is measured at checkpoint 2 (M6).
- **GPU-create chokepoint.** Every new buffer, texture, bind group and pipeline goes through `GpuCreate` (`gpu_create_gate`). New timing pairs: overlay, scene-colour chain, floor layer (`TIMING_PAIR_COUNT` 23 → 26).

## Delegated answers
- **Sharpness floor, fade band, minimum blur.** Floor n = 8; the weight fades in linearly in log₂ n from 0 at n = 8 to full at n = 16, so it is continuous across the contour. Exponent → mip: m(n) = log₂(θ(n) / α₀), θ(n) = 2·acos(0.5^(1/n)) the reflected-direction half-width at half maximum, α₀ = 180° / 64 the mip-0 interior texel angle; clamped to [m_min, last], m_min = 2. So n ≥ ~90 reads mip 2 (32² tile, ≈ 11° texels), n = 64 reads ≈ 2.6, n = 6 the last mip. Engine constants in level-format; tuned by the manual rows, not by tests. Reason: research §Blur puts n = 256 at ≈ 1.6 mips; m_min = 2 keeps every face visibly soft.
- **One facing for an authored box.** Normals within 1° (`UP_FACING_MIN_COS`'s tolerance) are one facing. The build warns above 6 probes per box, naming the box and its count. Reason: a box room's floor, ceiling and four walls is 6.
- **`_capture_offset` per facing.** The stored horizontal offset (x, 0, z) is projected into each per-facing probe's plane and applied after that probe's capture point is chosen: unchanged on floors, along the wall on walls. The existing rules hold per probe: it must stay inside the box and on the front side of every face the probe binds, else the build error names the box and facing. Reason: one key, one meaning per plane, no new key.
- **Planar mirror and the overlay.** The mirror records no overlay: its glossy faces draw forward only, matching the build before (row 59's first branch). Reason: zero mirror cost and no second D_static view; the mirror shows a small reflected area.
- **Cost ceiling and floor-layer tier values.** Set at two owner stops inside the build, not at this one: the overlay's ceiling after Task 5 measures the probe term on this Mac (5300M), the floor layer's per-tier values and ceiling after Task 11 measures its prototype. Each ceiling is the measured cost + 25% (`testing_guide.md` §Resource bounds). Starting floor presets, to measure: water's steps and divisors, target fraction halved (medium ¼, high ½, very high ½, ultra ½), so the compiler's floor stride equals water's until the owner picks otherwise.
- **Floor-blur tolerance (row 36).** The floor layer's spread, as reflected-angle half-width at half maximum, is within ±30% of the probe term's for the same exponent at the reference hit distance. Reason: the scene chain's levels are powers of two under trilinear filtering.

## Owner checkpoints
1. **Plan stop:** approved 2026-11-22. Fixtures (owner): the census covers `content/dev` water and glossy maps plus every map of the owner's adjacent private game project (sibling checkout; its name stays out of this repo), which is closer to production detail than the dev fixtures. Its committed PRLs and baked material root are read directly, no rebake. M8's stress fixture is that project's largest map by source size, compiled cold.
2. **After Task 5:** overlay cost on the glossy fixture (1440p, 4K, Auto) and the proposed ceiling. Measured 2026-11-22 (`measurements/fluid-base-pass-cost/results-glossy-m5m6.md`, 28/28 runs valid, High, mask 0x17F). Proposed overlay ceiling, measured + 25%, per pose on the glossy fixture: across: Auto 0.76 ms, 1440p 1.83 ms, 4K 4.07 ms; down (whole screen floor): Auto 1.43 ms, 1440p 3.16 ms, 4K 7.05 ms. Forward's added cost on a glossy level (second output, two sweeps): ceiling 0.41 ms at 4K across (measured 0.33). Probe sums: ceiling 0.025 ms. Awaiting owner.
3. **After Task 2, only if the mover screen crosses the cliff:** options for the mover's highlight separation.
4. **After Task 11:** floor-layer per-tier values and ceiling.

## AC-to-proof

Rows numbered in brief order. GPU proofs run with `POSTRETRO_REQUIRE_GPU=1` on Metal. "A/B" means against binaries or bakes from `8dda8a34a`, baked fresh (memory: A/B baselines rebaked).

| AC | Row (short) | Proof | Status |
|---|---|---|---|
| G1 | Without the flag: byte-identical id 54, no world binding, no overlay, no world-face probe resource | compiler A/B over `content/dev` + GPU frame | achievable as stated |
| G2 | Flag in the probe stage's cache key | compiler cache-key test | achievable as stated |
| G3 | With the flag the rows below hold | fixtures build with the flag | achievable as stated |
| 1 | Water placement, tiles, exclusion byte-equal; shares on 5 lobes, −Y zero | ignored compiler test over every water map in `content/dev`, comparing against A/B bakes, probes matched by bound faces | achievable as stated |
| 2 | Water renders within 1/255 at parity views, every tier, both styles; water reads mip 0 | ignored capture A/B at the reflection-probes parity views; source scan pins water's `textureSampleLevel(…, 0.0)` | achievable as stated |
| 3 | Non-opt-in material: no probe, binding, overlay draw; byte-identical render | compiler unit (each of no `_p`, no `_s`, no qualifying texel) + GPU frame A/B | achievable as stated |
| 4 | Floor/band fade, grazing continuity, `_s` 0 and halving | render-cpu reference unit tests (fade, contour difference bound) + GPU readback | achievable as stated |
| 5 | Derived probes on floor, wall, ceiling, 45° slope: front side, lifted; coplanar sharing | compiler unit | achievable as stated |
| 6 | `reflection_volume` claims glossy faces; empty box drops, warning | compiler unit + log assertion | achievable as stated |
| 7 | Floor↔wall mutual reflection; pool↔wall, wall shows pool bed (P3) | compiler capture test reading tile texels toward each face | achievable as stated |
| 8 | Two floor heights + two walls → three probes, authored box each | compiler unit | achievable as stated |
| 9 | Multi-plane box: capture point front of every bound face, or face binds nothing + warning (P4) | compiler unit + log assertion | achievable as stated |
| 10 | Unobstructed floor and wall: no self capture shadow | compiler capture + render-cpu search reference over bound fragments | achievable as stated |
| 11 | Flush inlay and coplanar matte strip cast no capture shadow (P2) | compiler capture + reference | achievable as stated |
| 12 | Behind pillar / kept brush: no probe term; no fill; SSR same at every s | GPU frame + source gate (water gate extended) | achievable as stated |
| 13 | Up-facing-only probes exclude small brushes, keep undersides; others capture all; `always`/`never` | compiler unit | achievable as stated |
| 14 | Water + floor probe keeps brushes under the coarser stride (P24), test stride both ways | compiler unit with test-only stride | achievable as stated |
| 15 | Floor probe thin rule keyed to floor stride; water-only unchanged; re-key leaves water captures byte-identical | compiler unit + cache-key test | achievable as stated |
| 16 | Compiler floor stride pinned to floor-layer tiers | unit, as `floor_stride_divisor_matches_the_coarsest_screen_space_tier` | achievable as stated |
| 17 | Monument: no withheld floor; reflects at floor tiers incl. first frame after low → medium; absent at Low (P16) | compiler unit + GPU frames | achievable as stated |
| 18 | Six lobe weights sum to 1; y ≥ 0 equals today's five | level-format proptest | achievable as stated |
| 19 | Ceiling over floor `light_area` takes −Y share; pulse moves both | compiler unit + GPU frame | achievable as stated |
| 20 | Floor under skylight reflects glass at glow, blurred, no highlight | compiler capture + GPU frame | achievable as stated |
| 21 | Script off scales glass and room together, same frame, every tier | GPU frames | achievable as stated |
| 22 | Glossy `light_area` glass keeps glow at W near 1 | GPU readback | achievable as stated |
| 23 | Exponent → mip: one pinned function, CPU reference, monotone, clamps | level-format unit + source scan pinning WGSL to it | achievable as stated |
| 24 | Prefiltered one-texel source spreads to the mapped half-width per mip | compiler unit | achievable as stated |
| 25 | Per-mip borders copy wrapped interior | compiler unit | achievable as stated |
| 26 | Per-mip direction reads its texel down to 4², both mips of a selection (P6) | render-cpu reference + GPU readback | achievable as stated |
| 27 | Reflection colour follows highlight tint, both layers | reference + GPU readback | achievable as stated |
| 28 | Composite = forward + W·(R − D_static); W = 0 byte-identical; R := D_static parity; perturbed fails; tint-1 keeps highlight | GPU readback with test-only variants | achievable as stated |
| 29 | Forward's D_static carries each static term, no other | GPU fixtures, one per term | achievable as stated |
| 30 | Character shadow fades by (1 − W); no channel below 0 (P13) | GPU readback | achievable as stated |
| 31 | All-terms mask bit off → no overlay term, toggle frame included (P14) | GPU frames | achievable as stated |
| 32 | Dynamic light, muzzle flash, emissive deltas unchanged by overlay; layout refuses lighting bindings | GPU readback + source gate | achievable as stated |
| 33 | Floor, wall, ceiling live lighting match reference within 1/255 through sums | GPU vs render-cpu reference | achievable as stated |
| 34 | Strength 0.04 untinted / 1 tint-1 / raw albedo without `_t`; grazing → 1; s scales share | reference + GPU readback | achievable as stated |
| 35 | Pixel style constant per texel, smooth varies; first frame after style switch (P9) | GPU readback | achievable as stated |
| 36 | Floor blur grows as exponent falls, matches probe blur within pinned tolerance; grows with distance; cap = min-blur | GPU readback; tolerance ±30% (Delegated answers) | achievable as stated |
| 37 | Character and door reflect at floor tiers on the frame they move; not at Low; no ghost/fluid/billboard/fog | GPU frames | achievable as stated |
| 38 | Water reflects composited glossy floor; water's SSR term s-invariant | GPU frames | achievable as stated |
| 39 | Floor layer chain and target at current extent on boundary frames; released at low/off/unload (P21) | GPU frames + `GpuCreationCounts` | achievable as stated |
| 40 | Floor march is water's, unmodified; binds no probe array; blur reads exponent, min blur, distance only; F·c·X s-invariant | source gate + GPU | achievable as stated |
| 41 | Floor reflects walls and mover without highlights; own outputs byte-identical (P26) | GPU readback + A/B | achievable as stated |
| 42 | Character/mover keep their colour; next frame no trace (P27) | GPU frames | achievable as stated |
| 43 | Highlight separation follows the floor layer's lifecycle; ledger; no pipeline built on tier change (P28) | GPU frames + `GpuCreationCounts` | achievable as stated |
| 44 | B2–B4 source gate over pass, downsample shader and recording | source scan | achievable as stated |
| 45 | No floor layer off up-facing faces; target only with an up-facing glossy face at a floor tier | GPU + counts | achievable as stated |
| 46 | Ghost, fluid, billboard, fog draw over the overlay, undimmed (P12) | GPU frame + recording-order test | achievable as stated |
| 47 | Off: no overlay pass, frame as before; Low: probe term alone; no planar on world faces | GPU frames + A/B | achievable as stated |
| 48 | Live off → low → off; built-at-off level switched to low (P7) | GPU frames + counts | achievable as stated |
| 49 | Coplanar mover takes no term; movers/characters in front none; next frame term returns (P15, P23) | GPU frames | achievable as stated |
| 50 | Overlay and sums gates (P17, P20) | GPU recording tests | achievable as stated |
| 51 | Forward layout and budget pins unchanged; colour byte-identical; no second output without glossy faces or at off | existing pins + A/B capture | achievable as stated (risk: Metal fast-math reorder after the entry split) |
| 52 | Exactly one overlay fragment per still-showing glossy pixel, dropped probes included; production byte-identical there | GPU test-only constant variant | achievable as stated |
| 53 | No-history gate covers overlay and floor pass | source scan | achievable as stated |
| 54 | Probe-layer-from-binding scan covers overlay | source scan | achievable as stated |
| 55 | Sums gates cover moved dispatch; overlay reads sums, evaluates no stored light | source scans + GPU | achievable as stated |
| 56 | Every colour mip from id 54 at install; no render-attachment/storage usage on probe arrays | source gate + GPU | achievable as stated |
| 57 | Stored light forced off changes floor, wall, ceiling reflection on that frame (P5) | GPU frames | achievable as stated |
| 58 | Very high: plane selection unchanged by a glossy floor | unit over the candidate builder + GPU path | achievable as stated |
| 59 | Mirror view: no overlay, matching before (P10) | GPU frame | achievable as stated |
| 60 | Mirror records without validation error, world colour as before; first low frame after off, after resize (P22) | GPU frames under a validation scope | achievable as stated |
| 61 | Id 54 v3 round-trip; v2 refused; bad world bindings refused, named | level-format unit | achievable as stated |
| 62 | One past cap drops the smallest world-face-only probe, warning names faces; water kept (P18) | compiler unit + log | achievable as stated |
| 63 | Water-only past cap drops as before; warning names faces (P25) | compiler unit + log | achievable as stated |
| 64 | Ledger rows; level change; unload zero; second level's faces only (P8) | GPU | achievable as stated |
| 65 | Glossy level without fluid uploads probes and sums; neither allocates nothing (P1) | GPU | achievable as stated |
| 66 | Authored water probes past cap refuse; world-face-only drop first (P19) | compiler unit + error text | achievable as stated |
| 67 | Surface Depth on: per-face marched reflection; off: no march; toggle same frame (P11) | GPU readback | achievable as stated |
| 68 | Box over warning threshold builds, warns | compiler unit + log | achievable as stated |
| 69 | `docs/level_design.md` and FGD help text | deferred until the gate lifts (owner); `context/` documents the flag and test map, review-panel docs lens | deferred (owner) |
| M1 | Dev map: glossy floor and wall look | owner, in-engine | manual |
| M2 | Dev map: monument and character | owner, in-engine | manual |
| M3 | Pixel style blocks; no octahedral-fold seam at coarse mips | owner, in-engine | manual |
| M4 | e24 and probes map water unchanged | owner, in-engine | manual |
| M5 | 5300M overlay and floor-layer cost per tier vs ceiling | executor on this Mac (A/B kit), owner reviews | manual |
| M6 | 5300M forward cost unchanged without glossy faces and at off; added cost reported | executor A/B | manual |
| M7 | Mover pass with separation clear of the cliff, both styles; cost reported | register screen + trace | manual |
| M8 | Probe-stage compile time, cold, stated workers, vs before | executor, on the fixture the owner picks | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | **Census (risk: cap lock on real content).** Opt-in predicate in level-format; slot loader from `.prm`; reflector-face census over id-17 faces grouped by plane; a `prl-build` report line (glossy faces, groups, probes vs cap). Run on the owner's census fixture. Rows 3 (compiler half) | integrating executor | — | done: `level-format` `specular_smoothness` (the one decode; render-cpu re-exports it) and `glossy_opt_in`; compiler `glossy_materials`, `world_reflectors`, `glossy_census` (one info line per build), `plane_basis` (moved out of `exclusion_underside.rs`). Placement, capture, cache key and output bytes unchanged. Harness: `POSTRETRO_GLOSSY_CENSUS_PRLS=<absolute paths, :-separated> POSTRETRO_GLOSSY_CENSUS_PRM_ROOT=<baked/materials> cargo test -p postretro-level-compiler --bin prl-build -- glossy_probe_census --ignored --nocapture`. Result: Block, above |
| 2 | **Mover register screen (risk: cliff).** Spike the pixel and smooth mover entries writing highlights to a second output; screen with `tools/metal_register_screen/`. Land nothing if it crosses; go to checkpoint 3 | integrating executor | — | done: clear. Exactness-preserving shape (a third, untinted-specular sum beside `total` / `tinted_total`; second `@location(1)` `Rgba16Float` output; colour expressions untouched): `fs_pixel` 127 → 127 VGPRs, `fs_smooth` 119 → 119 (cliff at 128), SGPRs 104 both. Checkpoint 3 not needed. Task 11 must also teach the screen tool a second target: `screen.swift` declares `colorAttachments[1]`, and `regs.py` accepts the AMD slice's `air.new.packedFragmentFunction` tag (same offsets) a two-output entry carries. Spare shape if headroom is ever needed: one specular sum, added to `total` only when untinted, `tinted_total` = `select(0, sum, tinted)` |
| 3 | **Format v3.** Six lobes (−Y last), colour mip count (1 until Task 7), world binding list in id-17 numbering; loader cross-checks; v2 refused. Rows 18, 61 | worker (level-format, loader) | — | done. `level-format` `reflection_probes.rs` split into `wire.rs`, `error.rs`; named `ReflectionProbesError` variants; renames `ReflectionProbeBinding` → `ReflectionProbeFluidBinding`, `bindings` → `fluid_bindings`; loader checks world faces against id 17; runtime probe array takes the section's mip count (chain uploads layer-major); WGSL five-lobe literals → six, sums 1536 vec4s, `FLUID_PROBE_SUMS_BYTES` 24 576, table unchanged at 61 456; probe `STAGE_VERSION` 15 → 16. Row 18: proptest (4096 cases) bit-equal to the old five lobes for y ≥ 0. Water A/B (fresh `--no-cache` bakes from `8dda8a34a`, three water maps, 17 probes): records, five-lobe shares, fluid bindings, mip-0 and distance tiles byte-equal, −Y 0, every other section identical. Row 61: level-format and loader unit tests. Tests: level-format 862, level-loader 433, render-cpu 204 (filtered), prl-build 294 (filtered) + 10 integration, renderer 179 GPU (filtered) |
| 4 | **Probe resources leave fluid ownership.** Level-owned probe records (arrays, table, sums), ledger row, sums dispatch ahead of the first probe-term draw, gated on fluid or overlay draws; ownership scans rewritten keeping their fences. Rows 55, 57, 64, 65 | integrating executor | 3 | done. `ReflectionProbeResources` (`reflection_probe_resources.rs`) on `LevelResourceSet`, built off-frame in `build_geometry_set` when id 54 is installable and binds any face, swapped with the set; fluid group 4 borrows it (`fluid_probe_bind_group_entries`), dummies only for a fluid level without probes. `ReflectionProbeSumsPass` (`reflection_probe_sums.rs`) records from `renderer_probe_sums_frame.rs` after depth/SDF and before forward, gated on `frame_dispatches_probe_sums(fluid has_probe_term_draws, glossy_overlay_probe_term_draws())`, the overlay input false until Task 5; pair 22 kept. Ledger: `ReflectionProbeResidencyReport` (every mip, distance, table, sums), install log line and capture report field `renderer_accounted_reflection_probes`. Rows 64, 65 (resource halves) by GPU tests; row 55 scans rewritten. Whole renderer lib on GPU: 1468 passed, 23 ignored |
| 5 | **First slice: one glossy floor (risks: self-occlusion, `Equal` landing).** The `--glossy-reflections` gate (G1–G3); author `content/dev/maps/glossy-reflections-test.map` (glossy floor and wall from the sharpness dev materials, a pillar, a freestanding monument; grows with later tasks). Splits first (`renderer_render_frame.rs`, `compute_cull.rs`, `reflection_probes/mod.rs`, `pipeline.rs`). Derived up-facing floor probe with per-face self-transparency; world probe-index side stream; glossy bucket plan; overlay pipeline on its own layout; forward's D_static entry and pipeline forms; additive composite at m_min; timing pair. Measure on the 5300M → checkpoint 2. Rows 10, 28, 29, 51, 52 | integrating executor | 1, 3, 4 | done. Compiler (5a): `--glossy-reflections` → `ReflectionProbeInputs.glossy_reflections` → stage key (gate byte, per-probe world and transparent faces); `ReflectorFace { source: Fluid \| World }`; `glossy_placement.rs`, `self_transparency.rs`; test map `content/dev/maps/glossy-reflections-test.map` + README (7 floor faces, 2 probes, 7 world bindings). Water A/B vs `9fe77f87e`: e24, reflection-lake, reflection-probes, vault, campaign-test byte-identical in every section, gate on and off (G1). G2 cache test. Rows 10, 11 (floor halves) via the render-cpu search on the stage's own distance tile, with a control that withholds over half the floor. prl-build 2793 passed. Runtime (5b): `LoadedTexture.reflects` from `specular_slot_reflects`; world probe stream (`prl_world_probe_stream.rs`); forward `forward_shade` + `fs_main` / `fs_main_static_diffuse`, three forms built at init, two sweeps on a glossy level; overlay (`glossy_overlay.rs`, `renderer_glossy_frame.rs`, `glossy_overlay.wgsl`) on its own layout, Equal, One+One, W = Fresnel(0.04) × s; sums gate live; timing pair `glossy_overlay` (count 24). Rows 28, 51 (same-build forms), 52 (bound and unbound; skinned-in-front rests on Equal), 29 (static terms; exclusions by source order), P1. Whole renderer lib on GPU 1485 passed |. Checkpoint 2 measurement (5300M, before `8dda8a34a` vs after `7ea466c0f`, 2 interleaved rounds, 300+300 frames): overlay across 0.61 / 1.46 / 3.26 ms and down 1.14 / 2.52 / 5.64 ms at Auto / 1440p / 4K; sums 0.015 ms; forward on e24 near 31.45 → 31.47 ms at 4K (unchanged), on the glossy map at High +0.33 ms at 4K, at Off 25.08 → 25.05; overlap ~0.04 ms both builds, so overlay time adds to the frame one for one. Auto runs are clock-unstable (busy medians 741–980 MHz); 1440p and 4K are the reliable figures. 4K captures before-High and after-Off byte-identical (row 51 at Off). Kit: `run_ablation.py` glossy configs, `--report glossy`
| 6 | **Weights and colour.** Sharpness fade, `_s` scale, Fresnel on the 0.04 base with tint, highlight-tint colour, capture shadow, mask gate. Rows 4, 27, 30, 31, 32, 34 | integrating executor | 5 | |
| 7 | **Colour mips.** Shared exponent → mip function; compile-time prefilter per mip with borders, BC6H to 4²; runtime `mip_level_count`; per-mip UV. Rows 2 (mip-0 scan), 23–26, 36 (probe half), 56 | worker (compiler prefilter) + integrating executor (runtime) | 3, 5 | |
| 8 | **All orientations and authored facings.** Split `box_fit.rs`, `capture_exclusion.rs`; plane-generic fit rays, settle, capture point, shares (six lobes, face hemisphere); per-facing authored probes, `_capture_offset`, warnings. Rows 5–11, 19, 68 | worker (compiler) | 3, 5 | |
| 9 | **Exclusion and cap.** Per-probe plane basis; underside rule over bound faces; thin rule keyed to the floor stride; cap drop order and per-face warnings; authored refusal. Rows 12–17 (compiler halves), 62, 63, 66 | worker (compiler) | 8 | |
| 10 | **Overlay completeness.** Pixel/smooth style, Surface Depth march, coplanar-mover guard (mask target), live lighting per orientation, area lights, lifecycle gates, mirror. Rows 19–22, 33, 35, 46–50, 54, 58–60, 67 | integrating executor | 6, 7, 8 | |
| 11 | **Floor screen-space layer.** Highlight separation (forward, mover per Task 2, skinned clear); scene-colour mip chain; floor march pass and target lifecycle; layered composite. Measure → checkpoint 4. Rows 12, 17, 36–45, 53 | integrating executor | 2, 10 | |
| 12 | **Parity and A/B.** Water placement and render parity; forward byte-identity; manual measurements M5–M8. Rows 1, 2, 47, 51 | integrating executor | 11 | |
| 13 | **Docs and context.** `docs/level_design.md`, FGD help, roughness-conversion note; `context/lib` marks turned into built rules at landing. Row 69 | integrating executor | 11 | |
