# E24--water-and-environment-volumes — plan of record

mode: resumable
status: blocked
read at: 08822b8a4 (main 784af6536 plus the claim commit; the brief's cfd2d861d is an ancestor)

## Block (Task 4, push rule 3)
**False premise:** "Push adds to velocity after the state intent and is never clipped by the intent's speed caps." Adding after the intent protects the push only on the tick it is added. Velocity carries into the next tick. There, with movement input held and `air.bunny_hop` off, the airborne cap in `intents/normal.rs` and `intents/crouching.rs` clamps total horizontal speed to the walk/run/crouch tier (`movement.md` §6: airborne Normal re-clamps carried momentum). With input held, airborne wind stalls at that cap plus `push·dt`; with no input it grows until the terminal cap. M15's airborne half ("not clipped by the air speed cap") therefore fails under the Decision's mechanism. Evidence: `crates/physics/src/movement/push_tests.rs` `airborne_push_survives_the_air_cap_on_the_tick_it_is_added` pins only the per-tick half. Options are in the owner question. The rest of Task 4 (lift-off, terminal speed, ground drift, slide, agents, upward level gravity, harness) is built and passing, and is unaffected by options (a)–(c).

## Corrections
Source re-read by three read-only agents at 08822b8a4. No Decision premise is false. Path and detail drift:

- **Stress fixture writes vector gravity.** `content/dev/maps/stress-env-volumes.map` has 426 `gravity_volume`s with `gravity "0 0 -14"`-style 3-vectors, generated before `gravity` became a scalar. Under the brief, a vector `gravity` fails the build. → Regenerate it with `tools/gen_stress_map.py` / `tools/stress_environment.py` emitting the scalar (the Quake-Z component), keeping counts, zones and start poses. The baseline map is unchanged. The fixture still matches the manual stress row: 1,000 fluid and gravity volumes.
- **FogVolumes has no version field** (`level-format/src/fog_volumes.rs`: `u32 pixel_scale, f32 initial_gravity, u32 count…`), so "bumps its version" has nothing to bump. → Dropping `initial_gravity` adds a leading `u16` version. Old-PRL detection does not rely on it: the loader checks that EnvironmentVolumes (always present) exists *before* decoding FogVolumes, and its absence is a `PrlLoadError` carrying the recompile message, replacing today's `NoWorldspawnGravity` role.
- **Next free section id is 52** (`section_registry.rs`: 45–51 taken). This matches "next free after `CellResidencySet` (51)".
- **`build_brush_volumes_with_ids` is private to `parse.rs` and returns `BrushSide { texture: String, tex_projection }`**, not a texture index plus UVs. `resolve_brush_region_bounds` is `pub(super)` with private `vertices`. → The peel lands in a new `parse/environment_volumes.rs`, a sibling of `authoring_regions.rs`, so `pub(super)` reaches it. `build_brush_volumes_with_ids` is widened to `pub(super)`. Surface UVs come from `tex_projection` and the texture's dimensions, the same derivation static faces use. The texture name maps to the level texture-table index at pack time.
- **Quake classname dispatch lives inline in `parse.rs`, not `format/`.** `trigger_volume` and `fog_volume` are dispatched at `parse.rs:1181`/`:1342`. → The canonical `MapEnvironmentVolume` record goes in `map_data.rs`, beside `MapTriggerVolume`. The three FGD classnames translate in `parse/environment_volumes.rs` and nowhere else, so no downstream stage sees the authoring split. `pack/section_plan.rs` does exist (the first agent missed it); the section is planned there after TriggerVolumes and passed as a required `&EnvironmentVolumesSection`, like FogVolumes, because it is always written.
- **`convex_overlaps` / `convex_subtract` are private** in `partition/face_extract.rs`. → Widen to `pub(crate)` as the Path says.
- **`tuning_payload_for_pawn` has two production call sites** (`main.rs:4162` pawn install, `main.rs:4214` per-poll resend) plus one conditioned test, not three. `TuningPayload` lives in `combat-model/src/carried_loadout.rs`, derives `PartialEq`, and the host rebuilds and compares it every poll (`host_send_tuning_if_changed`). → Level gravity and the fluid table are added as `TuningPayload` fields. Dedupe resends on change with no new logic, as the brief assumed.
- **The knockback launch passes `events.jumped || knockback_launch`** into the substrate's `jumped` parameter; there is no separate flag. → Push lift-off ORs in a third term at the same site.
- **No per-voice occlusion exists** in `postretro-audio` (`grep -i occlu` is empty; `audio.md` §5 lists it as planned). → The muffle is an effect on the Sfx (world) sub-track, following the `MonoFoldBuilder` pattern, which sits on the main track today. Any later per-voice occlusion sits on each voice's spatial child track, so the two compose by construction. The Presentation Decision keeps its meaning; "composes with per-voice occlusion" is satisfied structurally until occlusion exists.
- **The audio crate's public API already names kira**: `pub struct Audio<B: kira::backend::Backend = DefaultBackend>` (`audio/src/lib.rs:48`). Only in-crate tests use another backend (`CapturingBackend`, `MockBackend`). → To make AC P4's grep gate hold as written, seal it: a public concrete `Audio` over `DefaultBackend` wraps a `pub(crate)` generic core that the tests drive. This is a mechanical change inside the audio crate, with no behavior change.
- **Pass order has a "Kinematic Brush Pass"** between Textured and Skinned Mesh (`renderer_render_frame.rs:577`). → The fluid pass goes after Skinned Mesh and before Billboard Sprite, as the brief says, with its own gate. Smoke's `has_any_sheet() && !particle_collections.is_empty()` gate is not inherited. Smoke binds SH at group 3; kinematic and skinned bind it at group 4. The fluid pass uses smoke's camera(0)/lighting(2)/SH(3) layouts. Its material group takes free slot 1 or 4 and carries no storage buffers: 8/8 fragment storage is pinned by `billboard_pipeline_fragment_storage_request_matches_bgl_definitions`.
- **The tint channel changes `EffectUniform`.** `ChannelClamp::apply` (`render-cpu/src/flash_clamp.rs`) hard-codes flash and vignette, and `EffectUniform` is a 64-byte struct pinned by offset asserts and mirrored in `screen_effects.wgsl`. → The tint is a third `EffectChannel` with its own uniform field, fed by an engine-owned non-slot input to the pack step. `screen.*` slots are never written. `rendering_pipeline.md` §7.8 already names the channel.
- **`player_slot_value_with` returns early for Health/MaxHealth and otherwise requires an active weapon.** → `Swimming`/`Immersion` resolve in the same pre-weapon branch, so an unarmed pawn still reads them. `PlayerSlot` goes from 12 to 14 variants. The 16-cap const assert is in `ir/scopes.rs:462`, not `player_slots.rs`.
- **Movement sounds have only `land`/`jump`** (`MovementSounds`), resolved by address in `sound_events/descriptors.rs::movement_sound`. → `swim_started`/`swim_ended` join `state_entry_address`/`state_exit_address`. The emission carries the fluid index, and the app drain resolves `enterSound`/`exitSound` from the local fluid presentation table. Replay already discards events by construction (`prediction.rs` `replay`), so "never from replay" holds structurally.
- **Fluid registry policy has no shipped precedent.** `FactionRegistry::from_descriptors` hard-rejects the whole manifest on a duplicate; entity types are last-write-wins. → Fluids implement the Decision's own policy, warn and drop with the first declaration standing, in new drain code. They are wired through both JS/Lua drainers, both mod-init and staged paths, `StagedManifest`, `ModManifestResult` and `commit_staged_manifest_result`. `mod_digest.rs` gets an explicit skip.
- **Movement descriptor validation returns `DescriptorError`** from the JS/Lua parsers (`validate_in_range_finite`…). → A bad swim block (`exitDepth ≥ enterDepth`) is rejected through that same parser error, naming `movement.swim.exitDepth`, and surfaces through the existing descriptor-rejection warning.
- **content/dev has a TypeScript manifest and player descriptor only** (`start-script.ts`, `scripts/player.ts`; mod id `postretro.dev`). → TypeScript lands in those files. The Luau half lands as a twin fixture pair run as manifests, the established route (`player-events.ts`/`.luau` via `player_events_surface_tests.rs`). The mod id is unchanged. This clarifies AC P9/P11 without changing their meaning; flagged for the owner's skim.
- **`particle_sim` is already a CPU stage** (`ParticleStage::Sim`, binary crate). → The particle environment resolve nests inside it.
- **The particle pool is not built** (`drafts/perf-particle-sim-cost` is still a draft; `presentation_pool.rs` is unrelated). → Per the Path's landing order, the particle rows (M19, M21, M22) and the stress run's particle metric wait at a checkpoint.

- **A brushless volume entity fails the build** ("entity N 'push_volume' has no brushes"), as `switch` and the streaming hints do, rather than falling through to an inert `MapEntityRecord` as a brushless `trigger_volume` does. A degenerate brush (no vertices, zero volume, fewer than 4 planes) fails naming entity and brush. Diagnostics name volumes `entity {index} '{classname}'`, with worldspawn = 0.
- **Push axis conversion is the swizzle only.** `quake_to_engine` is `(-y, z, -x)`; push is m/s², so the map-unit scale is not applied.
- **Open for Task 9 (C3 vs wire format).** Fluid surface faces carry a `texture_index` into the level texture table (§Wire format). A fluid texture used nowhere else in the world would add a TextureNames/TextureCacheKeys entry, so those sections would stop being byte-identical to the map without the volume, against C3 as written. Raise with the owner at Task 9: either restate C3 to exempt the texture tables' appended fluid entries, or carry fluid texture names inside the EnvironmentVolumes section.
- **Mapper docs.** `docs/level_design.md` documents brush entities and has no section for the three volume classnames; it gains one in Task 13.

## Delegated answers
- Hysteresis margin and tint ramp — provisional: eye-depth band ±0.05 m (on at 0.05 m inside, off at 0.05 m outside), linear ramp 0.2 s in and out. Final values are recorded here once AC P7 passes with them.

## AC-to-proof
Row ids follow the brief's Acceptance order.

| AC | Row (short) | Proof | Status |
|---|---|---|---|
| Q1 | point inside/on-face/outside; contents mask | `collision` unit test | achievable as stated |
| Q2 | pool + room fields; equal-priority smaller; identical → earlier; none → defaults | `collision` resolve test | achievable as stated |
| Q3 | large high-priority beats small low-priority | `collision` resolve test | achievable as stated |
| Q4 | two-brush summed volume loses to single brush | `collision` resolve test | achievable as stated |
| Q5 | zero volumes → empty BVH, defaults | `collision` test | achievable as stated |
| Q6 | BVH ≡ brute force, seeded, faces and AABB boundaries | `collision` proptest | achievable as stated |
| Q7 | immersion 0/1/proportional; floating block; wading; two fluids | `collision` resolve test | achievable as stated |
| Q8 | side overlap only → dry | `collision` resolve test | achievable as stated |
| Q9 | volumes built before digest and first tick; second level clears first (P8) | `postretro` install test | achievable as stated |
| C1 | gravity/push shapes compile or fail naming entity | `prl-build` bin test `environment_volume*` | proven (3a) |
| C2 | volume leaves cells/portals/collision/navmesh identical | `level-compiler --test environment_volumes_compile` | proven (3a) |
| C3 | other PRL sections byte-identical; section carries record | `level-compiler --test environment_volumes_compile` | proven (3a); recheck at Task 9 (texture table) |
| C4 | buried and sibling-shared faces emit no surface | `prl-build` test | achievable as stated |
| C5 | partly buried face emits exposed part | `prl-build` test | achievable as stated |
| C6 | blank `fluid`/`gravity` fails naming entity | `prl-build` bin test `environment_volume*` | proven (3a) |
| M1 | enter at/above enterDepth, leave below exitDepth, no toggling between | `physics` swim test | achievable as stated |
| M2 | exactly exitDepth stays in swim | `physics` swim test | achievable as stated |
| M3 | viscosity drag only while swimming | `physics` swim test | achievable as stated |
| M4 | fluid jump eye-out; none submerged | `physics` swim test | achievable as stated |
| M5 | re-entry latch (P10) | `physics` swim test | achievable as stated |
| M6 | exit + jump same tick → jump | `physics` swim test | achievable as stated |
| M7 | sink scales with gravity; current drift | `physics` swim test | achievable as stated |
| M8 | crouch/dash/slide enter swim, standing capsule | `physics` swim test | achievable as stated |
| M9 | no swim block walks pool floor; warns once | `physics` test + `postretro` log-capture test | achievable as stated |
| M10 | jump uses tick eye, not camera eye (P5) | `physics` swim test | achievable as stated |
| M11 | swim out of floating block bottom → level gravity | `physics` swim test | achievable as stated |
| M12 | jump into floating block bottom → swim | `physics` swim test | achievable as stated |
| M13 | low/zero gravity volume; leaving → level gravity; agent too | `physics` `player_falls_at_volume_gravity…`, `player_keeps_vertical_velocity…`; `sim` `agent_falls_at_volume_gravity…` | proven (T1) |
| M14 | push lift-off, no ground-stick snap, terminal cap; weak push stays grounded | `physics` push test | achievable as stated |
| M15 | sideways push drift settles; not clipped by air cap | `physics` push test | achievable as stated |
| M16 | slide gets no push slope boost | `physics` push test | achievable as stated |
| M17 | agent ignores push; low gravity; upward-gravity agent grounded/holds vy | `sim` agent test | achievable as stated |
| M18 | upward `initialGravity` unchanged behavior | `physics` test | achievable as stated |
| M19 | particle push regardless of buoyancy | particle-pool step test | **waits on particle pool** |
| M20 | crossing tick integrates volume gravity, next tick level (P1); agent too | `physics` `player_crossing_out_of_gravity_volume…`; `sim` `agent_crossing_out_of_gravity_volume…` | proven (T1) |
| M21 | particle low gravity × buoyancy; drift out; zero buoyancy floats | particle-pool step test | **waits on particle pool** |
| M22 | particle crossing step uses post-step gravity (P2) | particle-pool step test | **waits on particle pool** |
| M23 | setGravity outside volumes only; initialGravity-only map; old PRL recompile message | `postretro`/`level-loader` test | achievable as stated |
| M24 | two setGravity one tick → later wins; volume holds | `sim`/`postretro` test | achievable as stated |
| N1 | harness: fluid + low-gravity crossings zero corrections, incl. replay spanning both | `netcode` `low_gravity_volume_crossings_reconcile_without_correction_under_light_link` (gravity half) | gravity half proven (T1); fluid half at T7 |
| N2 | harness: Swim-acked replay exits; Normal-acked replay enters (P3, P4) | `netcode` harness | achievable as stated |
| N3 | digest changes on shape/priority/gravity/fluid name; not on default gravity/presentation | `postretro` digest test | achievable as stated |
| N4 | map-order swap changes digest; surface texture/UV not; skips named | `postretro` digest test | achievable as stated |
| N5 | join with differing level gravity predicts host value, zero corrections | `netcode` harness | achievable as stated |
| N6 | mid-level setGravity reaches client, ≤ 1 correction | `netcode` harness | achievable as stated |
| N7 | harness seeds different gravity and fluids; installs via tuning path | `netcode` harness (structure of N5/N6/N10) | achievable as stated |
| N8 | client tint differs → host viscosity, own tint; no `water` → no tint, still swims | `netcode` harness + presentation-resolve test | achievable as stated |
| N9 | host manifest reload adds fluid → no longer dry, host and client | `postretro` reload test | achievable as stated |
| N10 | client numbers/order/absence differ → host values, zero corrections | `netcode` harness | achievable as stated |
| N11 | host lacks fluid → dry on client even if client declares it | `netcode` test | achievable as stated |
| N12 | undeclared fluid loads dry, warns with entity index and name | `postretro` log-capture test | achievable as stated |
| N13 | fluid declaration changes leave mod digest unchanged | `postretro` `mod_digest` test | achievable as stated |
| P1 | splash counts: air→fluid, out, floating block, blocked, none | `sim` weapon test | achievable as stated |
| P2 | pool floor one splash (P6); two-brush shared face none (P7); fact fields | `sim` weapon test | achievable as stated |
| P3 | origin on face no splash; adjacent entities two | `sim` weapon test | achievable as stated |
| P4 | muffle on world bus; UI/music unfiltered; no per-voice filter; reversal smooth; no kira in public API | `audio` tests + grep gate (after sealing `Audio<B>`) | achievable as stated |
| P5 | entry/exit sounds once per transition; never bobbing; never replay | `postretro`/`netcode` test | achievable as stated |
| P6 | fluid tint composes over mod tint; `screen.*` untouched | `render-cpu` pack/clamp test | achievable as stated |
| P7 | bob toggles ≤ 1; 5 Hz → ≤ 3 flashes/s on presented time; limiter off still ≤ 1 and ramps | `render-cpu` trigger + clamp test | achievable as stated |
| P8 | grep gates: projectile flight no resolve; FGD keys name no swim field | xtask surface gate or test | achievable as stated |
| P9 | Scripting surface fixture TS + Luau; negative viscosity / bad depths rejected naming field | `postretro` fixture test (twin TS/Luau) | achievable as stated (see Corrections: Luau twin) |
| P10 | out-of-range fluid / duplicate rejected with warning; first stands | `scripting-core` test | achievable as stated |
| P11 | fluids + swim land in content/dev manifest and descriptor, TS + Luau; mod id kept | `postretro` fixture test | achievable as stated (see Corrections: Luau twin) |
| S1 | slots read dry/swimming values; two players own HUD; host `byPlayer` | `netcode` state-slot test | achievable as stated |
| S2 | `players().on(becomes/ceases)` per player; first sight fires once | `postretro` player-events test | achievable as stated |
| S3 | local `onStateCrossing` fires only for own player | `postretro` test | achievable as stated |
| S4 | immersion component never read by intents/replay/merge | `netcode` harness | achievable as stated |
| S5 | E16 catalog-derived per-player tests cover both slots | `player_slot_lookup_test.rs` | achievable as stated |
| X1 | fluid faces look and composite right | owner, in-engine | manual |
| X2 | tint/muffle on/off, no flicker or pumping | owner, in-engine | manual |
| X3 | swim and fluid-jump playtest | owner, in-engine | manual |
| X4 | low-gravity room playtest with pool and floating block | owner, in-engine | manual |
| X5 | co-op client swims and crosses gravity, no visible corrections | owner, in-engine | manual |
| X6 | stress run: bodies < 0.25 ms/tick, particles < 0.5 ms/frame, BVH < 2 ms | owner's Mac, release + dev-tools | manual; particle metric **waits on particle pool** |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | **Parity slice (riskiest premise).** Create the `postretro-collision` crate (workspace member, crate-graph regen) with convex leaves, contents mask and a flat-scan point query as the reference path. Add the pure `resolve_environment` for gravity and push over plain input types, plus an `EnvironmentField` (volumes + level gravity). Widen `movement::tick`/`dispatch_state_intent`/intents from `gravity: f32` to the resolved environment, resolved once at tick-start position inside `movement::tick`, so host, prediction and replay share one call. Agents take `min(gravity, 0)`. Thread the field from host tick, client prediction, reconcile and the headless driver in place of `script_ctx.gravity`, which now seeds the field's level gravity. Volumes are injected in-memory for tests (no PRL yet). Prove M13 (player), M20, and N1's gravity half in the harness. | integrating executor | — | done: `postretro-collision` (leaf, exact polytope volume, flat-scan `EnvironmentVolumes`, `outranks`, `resolve_environment`); `movement::tick` takes `EnvironmentView` and resolves once at tick start; dispatch takes `&ResolvedEnvironment` and hands intents `gravity` (swim will take the struct); agents resolve at own position, `min(gravity, 0)`; `App.environment_volumes` empty, cleared in `unload_level`; headless driver holds its own empty set (Task 3 fills it). Proof: `collision` 12, `physics --lib environment` 4, `sim --lib environment_tests` 4 (incl. no-allocation), `netcode --lib predict_reconcile` 21 incl. `low_gravity_volume_crossings_reconcile_without_correction_under_light_link` (mutation: client without volumes → 0.11 m correction, fails). |
| 2 | **Collision substrate.** Load-time BVH generic over leaf payload. Point, capsule-segment immersion and ray-crossing queries (entity union; start-on-face rule) with contents masks. Overlap winner: priority, then summed brush volume, then map order. Fluid reach by segment. Seeded brute-force property test; empty BVH. Swap task 1's flat scan behind the same API. Q1–Q8. | worker A | 1 | |
| 3 | **PRL, compiler, loader, install.** EnvironmentVolumes section 52 codec. FogVolumes drops gravity and gains a `u16` version. FGD `fluid_volume`/`gravity_volume`/`push_volume` with help text (one-entity-pool note). Peel and validate in `parse/environment_volumes.rs` (shape, blank and axis conversion; not in BSP, collision, lightmap, SDF or navmesh). Loader decodes into `LevelWorld`; a missing section gives the recompile error. Build volumes and BVH in `install_level_payload` before the digest, mirror the renderer-less return, clear in `unload_level`. Volume records join `level_content_digest` with `FINGERPRINT_EPOCH` 2→3. Seed the field's level gravity from the header. Regenerate the stress fixtures with scalar gravity. C1, C2, C3, C6, Q9, M18, M23, M24, N3, N4 (record half). | worker B (compiler/format) + integrating executor (loader/install/digest) | 1 | 3a done: codec, peel, validation, FGD, stress regen (C1 C2 C3 C6 pass: `level-format --lib environment_volumes` 13, `prl-build environment_volume` 10, `--test environment_volumes_compile` 1; stress map 1,000 volumes, other 35 sections byte-identical to baseline). 3b pending: FogVolumes gravity removal, loader, install, digest. |
| 4 | **Push.** Lift-off ORed into the substrate's `jumped`. Terminal speed along the push. Push added after the intent, exempt from caps. Slide stays gravity-only. Agents ignore push. M14–M17, M13 (agent). | integrating executor | 1 | |
| 5 | **Level gravity on the wire.** `TuningPayload.level_gravity`, `TUNING_PAYLOAD_EPOCH` 10→11. The client installs the host value into its field and never reads its own. Resend on `setGravity` via whole-struct dedupe. N5, N6, N7 (gravity half). | integrating executor | 1, 3 | |
| 6 | **Fluid registry and swim descriptor.** `defineFluid` (TS/Luau SDK, both drainers, staged manifest, commit, reload) with warn-and-drop validation and a mod-digest skip. `movement.swim` block on `PlayerMovementDescriptor` with parser validation. Fluid table (name, viscosity) on `TuningPayload`. Host resolves names at install and manifest reload (undeclared → dry + warn); client resolves only against the host table; presentation fields resolve locally. P10, P9 (validation half), N8–N13. | worker C + integrating executor (tuning seam) | 3, 5 | |
| 7 | **Swim state.** `MovementState::Swim`/`MovementStateKind::Swim`, `intents/swim.rs`: thresholds, standing capsule, viscosity drag, gravity-scaled sink, push current, tick-eye fluid jump, re-entry latch, wade-only without a block plus warn-once. `WireMovementState::Swim`, `WIRE_VERSION` 28→29, `SNAPSHOT_VERSION` 18→19. Harness P3/P4. M1–M12, N1 (fluid half), N2. | integrating executor | 2, 6 | |
| 8 | **Swim slots.** Output-only per-pawn immersion component (`foundation`/`entities`), written after each authoritative tick, never read by intents, replay or merge. `PlayerSlot::{Swimming, Immersion}` in the pre-weapon branch; catalog entries (owner-private-player); two-pawn fixtures; SDK regen (`gen-script-types`); player-events fixture extension. S1–S5. | worker D | 7 | |
| 9 | **Fluid surfaces.** Compile-time surface faces: drop faces buried in solid or shared with a sibling brush; clip partly buried faces (`convex_overlaps`/`convex_subtract` widened); texture name → index; UVs from projection. Written into the section; digest skip named. C4, C5, N4 (surface half). | worker B | 3 | |
| 10 | **Fluid render pass.** Renderer-owned translucent pass after Skinned Mesh, before Billboard, with its own gate. Alpha blend at surface opacity, depth test on, depth write off, two-sided, turbulent UV warp. Per-fragment SH indirect, direct SH and dynamic lights on smoke's layouts, plus a texture/uniform-only material group. AABB-vs-visible-cell culling collector. X1. | worker E (renderer) | 6, 9 | |
| 11 | **Eye presentation.** Camera-eye signed-depth hysteresis and ramp; tint as a third `EffectChannel` plus an `EffectUniform` field (WGSL mirror, asserts); never touches `screen.*`. Sfx-bus low-pass effect on the MonoFold pattern; seal `Audio<B>`. Swim enter/exit sounds from forward prediction via the fluid's keys. P4–P7, X2. | worker F (render-cpu + audio) + integrating executor (app wiring) | 6, 7 | |
| 12 | **Hitscan splash.** Fluid ray crossings before the blocking `toi` in `fire_hitscan` (host and client-predicted paths) emit `SplashFact { point, normal, fluid }` per E16's fact layer. Grep gates. P1–P3, P8. | worker G | 2, 3 | |
| 13 | **content/dev fixtures.** `start-script.ts` fluids (`water`, `sludge`), `player.ts` swim block, Luau twin fixture, a test map with a pool, floating block, low-gravity room, updraft and current, and the fixture test. P9, P11; enables X3–X5. | integrating executor | 6, 7, 8 | |
| 14 | **Particles** — waits on `perf-particle-sim-cost`. Fill the pool's per-particle gravity and push from the same resolve, at the post-step position. M19, M21, M22. Build checkpoints here if the pool has not landed. | integrating executor | pool, 2 | blocked on particle pool |
| 15 | **Stress instrumentation and run.** `SimStage::Environment` (player + agent call sites), nested particle stage, hitscan crossing timer, BVH build load record. Owner runs X6 on the pinned fixture. | integrating executor + owner | 3, 4, 7, 12 | |

Hot paths and constraints, carried into every delegated task: `resolve_environment` runs per body per tick and per particle per frame, with no allocation (BVH traversal on a fixed stack) and a deterministic fixed-order float path. The BVH is built once at install. The fluid pass draws only culled visible surfaces, with no per-frame buffer growth. Tint and muffle are change-driven.
