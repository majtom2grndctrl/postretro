# E24--water-and-environment-volumes — research

Read at 2d3fca5f2. Findings that inform the brief but do not decide it.

## Substrate direction

This came out of the owner conversation that preceded the brief.
- **Static collision today.** A parry `TriMesh` built from render triangles (`CollisionWorld::from_level`). The skin-cast dispatcher keeps parry's BVH only for choosing candidate triangles and answers capsule–triangle pairs itself.
- **The known defects come from the representation, not the BVH:**
  - internal-edge tilt at coplanar seams, worsened because collision reads the sub-faces from the oversize-face lightmap cut;
  - no solidity, so a start-inside has no defined way out;
  - parry's leaf cast (dimforge/parry#452).
- **Modern engines (general knowledge, not re-verified):**
  - the static world collides against triangle meshes with active-edge or welding handling (Jolt, PhysX, Chaos);
  - dynamic and kinematic objects use convex shapes or primitives;
  - per-asset collision is authored with the art (Unreal `UCX_`, Godot `-col`);
  - level-placed blocking volumes supplement this sparingly;
  - queries filter by channel or layer;
  - water and gravity live on physics volumes (Unreal `PhysicsVolume`: gravity, fluid friction, water flag, priority).
- **Planned E24 order, all in `postretro-collision`:**
  1. Contents masks, convex volumes and overlap queries, with water first (this brief). A load-time BVH generic over leaf payload.
  2. Convex mover collision: exact sweep and push-out. Moves mover collision out of `physics::collision`.
  3. Static trimesh cleanup: faces from before the lightmap cut, welded, with active-edge flags. Triangle leaves reuse the generic BVH.
  4. The gravity-frame spec.
  5. Clip volumes.
- **Format neutrality.** Brushes are the Quake front end's source of convex solids. A mesh front end supplies authored hulls. The section stores "convex volumes", never brushes.

## Commitments touched

- `entity_model.md` §4, `build_pipeline.md` §Load-time gameplay declarations and `movement.md` §7 each say a map may seed only level-wide gravity at load. Per-archetype tuning is descriptor-owned and never map-overridable. Done plans repeat "never map-overridable" (`movement--slide`, `movement--crouch`, `movement--view-feel`, `movement--state-transition-feel`, `E17--doors-blocking-movers`). None of them addresses volumes.
- `plans/done/M7--gravity-primitives` settled gravity as a scalar in `ScriptCtx::gravity`, seeded per level, with `prl-build` erroring when the KVP is absent.
- `plans/done/E15--session-lifecycle`: "`worldSetGravity` mutates gravity mid-level… needs a continuous replication lane this spec does not open. Deferred on mechanism."
- `movement.md` §2 has a closed predicate set, and new predicates are added deliberately. §4 says state intents never query collision, because contact flows forward from the substrate's prior-tick result.
- `networking.md` §What gates says to hash only what cannot be replicated, and that the client predicts with host tuning with no local fallback.
- `level_content_digest` (`crates/postretro/src/runtime_movers.rs`) hashes mover data and static collision positions and indices. It does not hash gravity.

## Gravity in co-op today

- Client prediction and reconcile replay both read the client's own `script_ctx.gravity`. Host pawn simulation reads the host's.
- Gravity is on neither the wire nor the digest.
- A host-only `worldSetGravity` makes clients mispredict, and reconcile replays with the wrong value, so corrections should recur while the values differ. This was inferred from code, not run.
- A differing `initialGravity` between peers is not detected.

## Up-axis survey

- **Pure integration**, mechanical to widen: `velocity.y += gravity * dt` in the normal, crouching, slide and dash intents; the agent's vertical integration; the particle sim. Slide already projects `Vec3::new(0, gravity, 0)` onto the floor.
- **Frame assumptions**, about 60–80 sites in physics plus about 10 in the agent:
  - `classify_contact` and `COS_WALKABLE` key on `normal.y`;
  - `Capsule::new_y` throughout;
  - the substrate's step-up, ground-stick and snap casts run along `NEG_Y`;
  - horizontal motion is in the XZ plane;
  - jump sets `velocity.y`;
  - crouch resizes along Y.
- **Camera:** `look_direction` and `look_at_mat4(…, Vec3::Y)`.
- **Navmesh:** a Y-up 2.5D span rasterizer (`navmesh_bake.rs`, `sim/src/nav`), which cannot represent walls or ceilings as walkable.
- **What breaks without re-keying "ground" to gravity:** flipping gravity to +Y makes the player land on a ceiling-classified contact and never ground. Sideways gravity leaves the player permanently airborne, pinned against a wall. Only downward gravity behaves, hence the down-only validator.

## Precedents

- **`trigger_volume`:**
  - peeled in the `parse.rs` entity sweep;
  - AABB only (`MapTriggerVolume`), section id 44, versioned;
  - runtime `capsule_overlaps_aabb` in `TriggerSystem`;
  - host-only, so it cannot carry predicted movement state.
- **`fog_volume`:** `resolve_brush_region_bounds` (`parse/authoring_regions.rs`) yields engine-space planes, AABB and vertices. `MAX_FOG_VOLUMES` and `MAX_PLANES_PER_VOLUME` are GPU-buffer caps; a CPU volume does not inherit them.
- **No non-solid visible brush role exists.** `kinematic_mover` is the precedent for faces drawn outside the static BSP, by its own pass with AABB-vs-visible-cell culling.
- **No translucent world pass exists.** Smoke is depth-test on, depth-write off, additive, and runs after opaque forward. The fog composite follows it. Water needs alpha blend instead of additive.
- **Audio:** no reverb or low-pass exists. `MonoFoldBuilder` on the main track is the effect-on-track pattern. Reverb zones (E12 step 3, `audio.md` §6) are unbuilt and would later share the listener's volume or cell lookup.
- **Manifest registries:** `factions` (`defineFaction`) is the named-registry precedent for `liquids`.
- **Movement descriptor:** state blocks such as `dash`, `crouch` and `slide` under `movement` are the precedent for `swim`. `player-descriptor-composition` (draft) makes states the primary axis, and `swim` fits as a state block.

## Direction review (validate-plan, 2026-10-05)

- **Verdict: Reshape, narrow.** The first draft forwarded immersion on the movement component as prior-tick substrate output. Reconcile clones the latest-predicted component and merges only `WirePlayerMovementState`, so replay began from the client's newest immersion, not the acked tick's. It also threaded one gravity value through every replayed tick. The fix, which the owner accepted: one pure resolve, called per tick at the body's position.
- **Owner rulings on the review notes:**
  - name the real rival to the BVH (a flat list). The owner then kept the BVH: "we don't need it yet" is what caused a week of perf work, and authors are imaginative, so structures are sized for ambitious content and paired with a stress budget;
  - reverb zones fold into the environment overlap rule;
  - accept the physics recompile trade, with the substrate in a new crate;
  - surface texture from brush faces, opacity on the fluid;
  - the ownership amendment is a closed field set;
  - cite M7's `area.setGravity`;
  - water draws before smoke.
- **Later owner rulings:**
  - "fluid" means swimmable substances only; gas stays with fog;
  - any convex brush can be a fluid, floating included;
  - the surface jump needs no ledge.

## Settled open questions (2026-10-05)

Grounded by read-only research at c495e49ea, then decided by the owner.
- **Surface lighting.**
  - The smoke pipeline layout (`SmokePass::new`) is: camera_bgl, a sheet BGL, lighting_bgl, sh_bgl, and an instance BGL. That totals 8 FRAGMENT storage buffers, pinned by `billboard_pipeline_fragment_storage_request_matches_bgl_definitions`.
  - A separate pipeline is charged only for the BGLs its own layout contains. It does not inherit the forward pass's 16/16 sampled-texture total (`FORWARD_SAMPLED_TEXTURE_BUDGET`).
  - Smoke lights each sprite in `vs_main` through `sample_sh_indirect`, `sample_sh_direct` and the `billboard_direct_light` loop. The water pass moves that same evaluation into the fragment shader with the same bindings.
  - Rejected: unlit (looks flat against lit geometry); full forward lighting (takes the full 16 sampled textures).
- **Fluid-name lookup.**
  - prl-build never reads the mod manifest. Its only script contact is the worldspawn `data_script`, evaluated for light membership (`script_light_membership.rs`).
  - Map-to-registry references are checked by the engine with a warning, and PRL stores names, not indices. Precedents: `warn_unknown_sound_keys` (sound keys stored as strings in `kinematic_geometry.rs`) and the classname routing against `ModManifest.entities`.
- **Crate layering.**
  - `crate-graph.md` layers: `level-format` L0; `level-loader` L1; `physics` and `visibility` L2.
  - `visibility` and `CollisionWorld::from_level` consume `LevelWorld` directly. The collision core departs from that precedent: it takes plain input types, so the seeded brute-force property test can build volumes without a level.
  - `level-loader` is already in physics' dependency closure, so the adapter costs nothing extra.

## Stress pre-flight (2026-10-05)

- **Fixture authorable now:** `parse_map_file` drops unrecognised brush-entity classnames without adding them to `world_brush_ids`. A fixture carrying `fluid_volume` and `gravity_volume` entities therefore compiles as inert today and gains behaviour when E24 lands.
- **Upstream limits found by research:**
  - No in-process multi-pawn route exists (`LoopbackHarness` is test-only and single-client). The owner dropped the fixture to 1 player pawn plus 64 agents; per-body cost is linear and agents dominate.
  - Particle sim had no CPU stage.
  - `particle_sim::tick` clones every `ParticleState` and does a per-particle `HashMap` lookup. At 10,000 particles that is a perf risk independent of E24.
  - Batch headless runs reject CPU timing. Measurement is a windowed `POSTRETRO_CPU_TIMING` run at a fixed `--start-pose`.
- **Pre-flight results (branch `e24/stress-preflight`):**
  - **Fixture.** `--preset env-volumes`:
    - an 8×6×3 warren grid of 138 rooms plus an arena;
    - 1,000 volumes, 64 enemies, and 40 emitters (each 50/s × 5 s, about 10,000 particles);
    - zones a–h plus a mixed zone, with start poses in `content/dev/maps/stress-env-volumes.README.md`.
  - **Feature coverage.** The fixture includes:
    - lifts carrying lights;
    - animated lights;
    - arenas, enemies and pickups;
    - emitters;
    - fog;
    - doors, triggers and switches;
    - baked and runtime spots with crates;
    - monster closets and spawners.
  - **Inert today.** The volume and baseline maps compile to byte-identical `.prl` files: 9,205 cells, 9,362 portals, 27,770 triangles. That is the pre-E24 form of the "volumes never change the BSP" row.
  - **Bake.** 60–75 s on a quiet machine, about 155 s under load. Lightmap takes about 35% and the cell residency set about 22–26%, followed by navmesh and SH. No hard limit was hit: about 4k leaves against the 131,072 cap.
  - **Runtime.** Release with dev-tools, mixed-zone pose.
    - Frame time is about 88 ms at about 5.3 ticks per frame.
    - `sim_ai` is about 7.4 ms per tick and grows as agents converge on the player.
    - `particle_sim` is about 4.5 ms per frame (max 7–10 ms); `particle_emit` is about 0.3 ms.
    - `sim_movement` is about 0.03 ms.
    - The water-only zone reads the same, because AI and particles run map-wide.
  - **Premise correction.** The particle sim steps per rendered frame on `frame_dt`, not per fixed tick, so the brief's particle rows now say "per step" and "per frame".
  - **Pre-existing issues seen.** 710 "light inside a solid leaf" warnings from the warren corridor spotlights. `MAX_FOG_VOLUMES` = 16 (the fixture uses 8).

## Bake versus load-time build

The trade-off for the volume BVH is between building it at level load and baking it into PRL.

- **Baking pays when:**
  - the build is expensive: large leaf counts, or high-quality SAH or spatial splits worth computing offline;
  - load time is tight;
  - the tree should stream with spatial regions (see `large-map-spatial-residency.md`).
  The render `Bvh` (id 19) is baked for these reasons. Static triangle collision (E24 step 3) is the likely case where it pays again.
- **Baking costs:**
  - the node layout becomes a PRL contract, so any retune (wide nodes, quantized bounds, a SoA layout for SIMD) is a format bump plus a recompile of every map;
  - the loader must validate untrusted node data (index ranges, no cycles, tree consistent with leaves);
  - the tree is derived data stored beside its source, so the two can disagree;
  - the compiler takes a dependency on the collision crate's builder.
- **Load-time build pays when:**
  - leaves are few enough to build in negligible time. A binned-SAH build over thousands of leaves is sub-millisecond (estimate, to be confirmed by the stress run's build-time metric);
  - the layout is still being tuned;
  - a runtime builder will be needed anyway for future dynamic volumes (script-toggled or mover-attached, both non-goals here).
- **Determinism:** query results never depend on tree shape (per-field resolution is order-independent), so baking buys no correctness. It would only make performance identical across peers.
- **Decision:** build at load now. The builder is a pure function of the leaves, so a later bake can serialize its exact output with no behavior change.

## Adjacent drafts

- `E22--kinematic-assemblies` touches compiler brush ingest, which is a merge-order risk only.
- `bvh-leaf-clustering` changes the render BVH. The collision volume BVH is separate, so don't share names or leaf semantics.
- `gameplay-stack-decomposition` is moving sim, AI and netcode code. Land new sim code in its target layout.
- `E17--mover-relative-rider-presentation`: volumes on movers are excluded here.
