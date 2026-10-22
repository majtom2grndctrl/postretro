# ghost-entities — plan of record

mode: resumable
status: approved
read at: 260f10029

Brief grounded at d6f3471; 76 commits since. Every Decision and Path symbol was re-read at 86a5bd62c (four read-only sweeps: PRL/compiler/map identity, netcode/replication, rendering/limiter, audio/scripting). No Decision premise is false. Path drift is below.

Merged `main` at 08f8813f9 (reflection-probe-capture-exclusion, compile-time only). It touches `parse.rs`, `map_data.rs` and the FGD, but none of the runtime renderer, capture, limiter or netcode files this plan builds on, and not the pre-BSP peel path `ghost_link` shares with teleporters. No Correction changes.

## Corrections
- `WIRE_VERSION` 32 → 33 (Wire format) → it is already 33 (`crates/net/src/handshake.rs:43`, PRLC Input-lane bump ae87ca368). This brief bumps **33 → 34**; `SNAPSHOT_VERSION` 21 → 22 stands (`crates/net/src/wire.rs:357`). AC "handshake across the old and new wire versions is refused" keeps its meaning.
- "The seamless landmark → target transform becomes one shared pure function" → today it is inline in `physics::movement::teleport::resolve` (`crates/physics/src/movement/teleport.rs:62-73`, velocity rotation :86-104), yaw-only about world up. Extracting it is Task 1: a pure fn in `foundation::movement::teleport` beside `wrap_teleport_yaw`, called by `resolve` and by ghost mapping. Aim pitch passes through unchanged because the transform is yaw-only.
- A stale old-level link record must never bind, the host restarting the same map included (P37), but teleporter binding matches only index + source entity, and those repeat on a restart → snapshot frames carry the participation epoch and the client transport drops frames from a retired epoch (`crates/net/src/transport.rs:1394`, `accept_snapshot_epoch` :1451). A host level change demotes every slot. The Decision premise holds; link records ride the same frames.
- Teleporter demotion leaves `armed` where replication put it (`client.rs` `despawn_all_mapped` / `reset_for_demotion`) → P38 needs new code: on demotion and unload the client drops received link state and re-seeds local spawn state, presenting through the membership ramp.
- `filter_out_client_host_replicated_placements` → defined in `crates/sim/src/scripting/builtins/host_replicated.rs:61`. The tuning payload moved to `crates/combat-model/src/carried_loadout.rs` (`WieldableTuningPayload.projectile` :90, strip at ~:208, `same_pawn_tuning` :241). The shot-id normalizer is `observer_shot_id` in `crates/netcode/src/weapon_cues.rs:15`. The section registry is `crates/level-format/src/section_registry.rs`, and the level content digest is `level_content_digest` in `crates/postretro/src/runtime_movers.rs:86`.
- Remote `entity_class` is now prefix-tagged `descriptor:` / `pawn:` / `projectile:` (`crates/netcode/src/descriptor_class.rs:8-37`) → descriptor `ghost` resolution decodes the prefix first.
- `ProjectileComponent` has no weapon class (`crates/entities/src/components/projectile.rs`). Enemy projectiles carry provenance in `owner_weapon`; the class exists only on the presentation-mirror spawn (`EnemyProjectilePresentationSpawn.descriptor_class`, `local_projectile_presentation_descriptor_class`) → add the firing weapon's descriptor class to the gameplay projectile at spawn, as the Path anticipated.
- The limiter runs inside the resolve, after every scene pass, and only on the windowed path (`screen_effects.rs:257-260`, `limiter_frame.take()` at `renderer_render_frame.rs:983`) → per-ghost clamping runs CPU-side when ghost instances pack, with its own `history_live`. It is fed by the same `pack_limiter_frame` time (presented-frame dt, hitch ceiling) and the same fail-safe enable flag. `EffectChannel` (`flash_clamp.rs`) is already value-agnostic, so it becomes the extracted per-value clamp, and `ChannelClamp` keeps using it. Capture bypasses it, as for screen channels.
- Mover beauty upload keys each entry to the first instance per `mover_id` (`kinematic_brush.rs:728-757`, `.or_insert`) → mover ghosts ride their own instance list with no mover-id dedupe (P27 needs two transforms for one mover).
- Mesh shadow `AllRetained` draws the whole `plans.world`, so `dynamic_shadow_visible = false` would not keep ghosts out of shadows → ghosts get their own `MeshFramePlans.ghost` plan, as the viewmodel did, and never enter `plans.world`.
- The skinned `Instance.base_and_pad.zw` is free (`skinned_mesh.wgsl:189`), so packed opacity and look ride it with no new binding. The mover instance entry is a bare 64-byte mat4, so the mover ghost pipeline gets its own 80-byte instance layout. Ghost pipelines are unlit and drop the light/shadow and SH groups, so no binding wall applies (the 8/8 fragment-storage note covers forward, billboard and fluid, not these passes).
- Sprite billboards have no tint channel ("Tint has no GPU channel", `particle_render.rs`) and blend additively → a projectile billboard ghost needs a ghost billboard variant that carries tint, saturation and opacity. A model-bodied projectile goes through the skinned ghost path.
- Third-person weapon: a separate rigid attachment instance emitted after its holder by `attachments::emit_for_holder`, sampling the holder's modifier-applied pose (`aimSpine` pitch included) → a pawn ghost re-emits holder plus attachments as one group under the mapped transform, all with one look.
- Audio voice release hardcodes `BusId::Sfx` in `stop`, `fade_out_positional`, `update` reclaim and failed starts (`audio_core.rs:341,355,402,413`), and `admit_positional` hardcodes SFX → positional voices carry their bus and release on it. The Ghost track takes one SFX child-track slot, so SFX `sub_track_capacity` grows by one to keep two full SFX caps of headroom (an AC). `MAX_TOTAL_VOICES` 64 holds the new 60-voice total.
- Projectile-ghost parsing: `components.weapon.projectile` is shape-checked in `js/weapon.rs:56` / `lua/weapon.rs:78`, then deserialized by serde into `ProjectileDescriptor` (`foundation/.../types/combat.rs:40`) → `ghost` goes on the struct with a serde default, plus a boolean shape check in both weapon validators, so both runtimes reject alike.
- Map-entity companions: `lifecycle_world_cpu.rs:180` excludes the `trigger_teleport` companion from descriptor and classname dispatch → the `ghost_link` companion needs the same exclusion. `TeleporterParser.destinations` is private and fills during `extract` → ghost-link destination resolution shares that map (the parser gains link records, or exposes destinations to a sibling `parse/ghost_links.rs` resolved at `finish`).
- Build-time `getMapEntities` has its own member tables (`level-format/src/light_membership.rs` `MapMemberKind`, `level-compiler/src/script_light_membership.rs`, `script-compiler/src/light_membership.rs` wrapper list and `WORLD_QUERY_COMPONENTS`) → `ghostLink` registers there too, or build-time queries stub it.
- The `postretro/ui` drift test lives in `crates/script-compiler/src/light_membership.rs:2818` (`luau_build_prelude_exports_match_runtime_prelude`), not in sim → `postretro/audio` extends that test.
- AC "A host reload flipping only a weapon's `projectile.ghost` changes ghosting on a client without rebuilding its pawn" → under the Decisions each peer resolves `projectile.ghost` from its own descriptors, so the client's ghosting changes through its own committed reload of the shared content. The host's change never reaches the client: the field is stripped from tuning and kept out of the mod digest, so no pawn rebuild and no demotion follow. Proof runs the reload on both peers of a loopback session and asserts all three. Same meaning; recorded as a clarification.

## Delegated answers
- Floor style and floor bus values — provisional: floor style tint `[1, 1, 1]`, opacity 0.5, saturation 0, `audio` false, `impactAudio` false. Floor bus: volume 0.6, high-pass 200 Hz, low-pass 4000 Hz, drive 0. The opacity value is tuned at the legibility check (manual row 46) and recorded here with the final numbers.
- Alpha blend vs screen-door dither — **owner amendment (2026-10-21): both**, chosen mod-wide by `ModManifest.render.ghosts: "blend" | "dither"`, default blend, following `render.reflections` validation and reload rules. The brief's Decisions, Scripting surface, Boundary inventory and Acceptance, `rendering_pipeline.md` §7.9 and `scripting.md` §Mod render profile are amended to match. Ghost passes run after the skinned mesh pass and before fluid. Blend uses fluid's blend/depth state as its template. Dither uses blend off plus a fragment `discard` against a screen-space Bayer pattern, still with no depth write. Each ghost pass builds both pipelines and batches ghosts by mode. Projectile billboard ghosts blend in both modes. Task 4 checks that unlit ghost colors stay under the bloom threshold, so bloom does not smear the dither.
- Docs placement — a new `docs/ghost-links.md`, with a short pointer added to `docs/teleporters.md` "A closet loop and a time shift". Ghost links stand apart from teleporters (a link can look into a copy nobody teleports to), and the page carries styles, the bus and the photosensitivity notes.
- Level-scoped ghost-audio record identity — a snapshot-envelope side record (precedent: `faction_sentiment_record`, `host.rs` ~L176-195), level-scoped, with no entity binding. Participation-epoch frame gating rejects stale copies. Level unload and demotion drop it.

## AC-to-proof
Row numbers follow the brief's Acceptance order; 9a is the owner-amended `render.ghosts` row, numbered to keep later rows stable. "harness" = netcode loopback predict/reconcile or relay harness, run on host and on a connected client where the row says both.

| AC | Proof | Status |
|---|---|---|
| 1 PRL round-trip incl. default categories | `level-format` ghost_links round-trip tests + `prl-build` bin parse test | achievable as stated |
| 2 compile errors | `prl-build` bin parse tests, one per error | achievable as stated |
| 3 unmatched `ghost_tags` warns once on host and client | `postretro` install test with `test-log-capture`, host role and client role | achievable as stated |
| 4 decode rejects (PRL, link and audio records) | `level-format` decode tests + `net` wire decode tests | achievable as stated |
| 5 digest unchanged by a last-entity link; teleporter changes it | `runtime_movers` digest test | achievable as stated |
| 6 named / no / undeclared style resolution and warnings | `postretro` install test with log capture | achievable as stated |
| 7 malformed style / duplicate; both runtimes | `scripting-core` ghost-style drain tests, JS and Luau | achievable as stated |
| 8 staged reload re-resolves styles; script fields keep (P16) | `postretro` staged-reload commit test | achievable as stated |
| 9 floor style no sound; floor bus no distortion | unit tests on the floor constants and the built bus | achievable as stated |
| 9a `render.ghosts` blend / dither / malformed; both runtimes; billboards blend (amendment) | `scripting-core` render-profile drain tests, JS and Luau + ghost batch-by-mode unit test in render-cpu | achievable as stated |
| 10 differing styles admitted; opacity-only filter (P15) | harness test, two manifests | achievable as stated |
| 11 ghost pose = seamless arrival, rotated/unrotated, vertical offset | `foundation` proptest on the shared mapping fn + `physics` teleport parity test | achievable as stated |
| 12 membership edges (ten cases), host and client | membership unit tests (`postretro` ghost module) + harness tests for client-side cases | achievable as stated |
| 13 arm/disarm ramps, P2/P3/P35/P4 | membership ramp unit tests + harness test for P35 | achievable as stated |
| 14 mapped-cell culling | ghost collector test against `VisibleCells` | achievable as stated |
| 15 teleport swap (P1) | harness test arming both seamless teleporters | achievable as stated |
| 16 P18 host seat and client; P33 | harness tests (relay harness for host-seat delay) | achievable as stated |
| 17 P19, P20, P22 | membership unit tests | achievable as stated |
| 18 category off fades; P43 | membership unit tests, each category | achievable as stated |
| 19 `ghost_members` default; P44, P45 | membership unit tests + `prl-build` default test | achievable as stated |
| 20 reload flips `ghost` (P46); `projectile.ghost` without pawn rebuild | `postretro` reload test (P46) + harness test reloading both peers, asserting no tuning resend, no demotion, client ghosting changed (see Corrections) | achievable as clarified |
| 21 non-boolean `ghost` / `projectile.ghost` rejects init, same error both runtimes | `scripting-core` descriptor tests, JS and Luau | achievable as stated |
| 22 ghost never a hit, overlap, trigger or AI target | `postretro` test with a presenting ghost: hitscan, splash, trigger and AI acquisition through the mapped pose | achievable as stated |
| 23 four 10 Hz strobes ≤ 3 flashes, rest at pre-flash level | ghost clamp tests driving membership and look through the per-ghost channel | achievable as stated |
| 24 red tint strobe desaturates; single change keeps hue | ghost clamp test | achievable as stated |
| 25 arm ≤ 250 ms; 1500 ms fade unchanged; P21, P31 | ghost clamp + ramp tests | achievable as stated |
| 26 limiter off strobe; hitch ceiling (P30); pause (P42) | ramp tests with injected frame time | achievable as stated |
| 27 per-field `setColorFilter`, P5, P7, P41 | look-resolution unit tests + sim primitive validation tests | achievable as stated |
| 28 malformed `audio.ghost` → whole floor, warns | `scripting-core` audio-profile tests, JS and Luau | achievable as stated |
| 29 `periodMs` ramps; P29, P36; non-finite args | look/bus ramp unit tests + primitive validation tests with log capture | achievable as stated |
| 30 fire-sound admission, P24, mover open sound, P40, P34 | ghost-voice admission tests in `postretro` sound path + harness test for P34 | achievable as stated |
| 31 own fire no ghost voice (P25) | ghost-voice admission test | achievable as stated |
| 32 impact ghosting rules | ghost-voice admission tests with normalized shot ids | achievable as stated |
| 33 voice plays out (P23); follows SFX volume and muffle | `audio` mock-backend tests | achievable as stated |
| 34 seventeenth ghost voice refused; SFX headroom | `audio` capacity tests | achievable as stated |
| 35 mock tone filtered by bus, changes after `setGhostAudio`; slot release | `audio` mock-backend tests (pattern: `tests/muffle.rs`) | achievable as stated |
| 36 P13, P17 | `postretro` unload/demotion and staged-reload tests, host and client | achievable as stated |
| 37 host commands reach client; ramp end within one snapshot interval; P8 | harness tests | achievable as stated |
| 38 late joiner, P9–P12, P14, P37, P38 | harness tests | achievable as stated |
| 39 P6 limiter on host, off on client | harness test + clamp | achievable as stated |
| 40 same steps on a client change nothing | sim primitive test in client role | achievable as stated |
| 41 demotion and unload drop received state | `netcode` client tests | achievable as stated |
| 42 old/new wire handshake refused | `net` handshake test | achievable as stated |
| 43 dev manifest + TS/Luau scripts load with zero warnings; `postretro/audio`; `ghostLink` query; set-pieces drive changes (P28) | `postretro` surface test loading the dev map in both runtimes, mirroring `startup/teleporters_surface_tests.rs`; typedef committed-file test | achievable as stated |
| 44 headless capture of the armed barrier: tinted, blended, no shadow; identical under two dynamic-light setups; occluded part not drawn; dither coverage (amendment) | `crates/postretro/tests/capture_frame.rs` test, `#[ignore = "requires a GPU adapter"]`, run on this Mac with `--features capture -- --ignored` | achievable as stated (GPU-gated) |
| 45 two-peer loopback co-op set-pieces, listen host and client, harness lag | owner, in-engine | manual |
| 46 floor style legibility incl. high-contrast; record values | owner, in-engine | manual |
| 47 dev style punchy and muffled; swell audible | owner, in-engine | manual |

## Tasks
Each task ends with focused tests, a `plan.md` row update and one commit. Workers never edit this file or commit.

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Shared seamless mapping: extract the pure landmark → target fn (position, yaw delta, rotation) into `foundation`; `teleport::resolve` calls it; proptest + teleport parity (AC 11) | integrating executor | — | |
| 2 | PRL id 56: `level-format/ghost_links.rs` + registry + loader + `LevelWorld` field; compiler `parse/ghost_links.rs` sharing teleport destinations, id-29 companion, pack/section plan; FGD `ghost_link`; `GhostLink` component kind + `runtime_ghost_links` install + companion dispatch exclusion; digest test (AC 1, 2, 4-PRL, 5) | integrating executor | — | |
| 3 | Membership core (thin slice, game logic): per-frame membership from presented state, floor style only, players category, membership ramp on presented-frame time, fade at last in-volume mapped pose, generation-keyed ghost state, link-state history read at each member's presentation clock; `ghost` descriptor flag stubbed to the player descriptor in tests | integrating executor | 1, 2 | |
| 4 | Thin-slice draw: split `mesh_pass.rs` (ghost pipeline in a sibling module), `MeshFramePlans.ghost`, unlit skinned ghost shader (`base_and_pad.zw` look) with blend and dither pipelines batched by mode, ghost collector re-emitting holder + attachments at the mapped transform, culled at the mapped cell; capture collector mirror; blend-vs-dither confirmed in-engine | integrating executor | 3 | |
| 5 | Thin-slice replication: link record (armed, categories) as `ComponentPayload::GhostLinkState` in a new `net/src/wire/ghost.rs`, host registration at install, client bind by index + source entity, joiner local spawn state, demotion/unload re-seed; `WIRE_VERSION` 34 / `SNAPSHOT_VERSION` 22; loopback swap + P18 harness tests. **Slice checkpoint: report to owner.** | integrating executor | 3, 4 | |
| 6 | Descriptor opt-ins: top-level `ghost` (JS + Luau, `EntityTypeDescriptor`), `projectile.ghost` (serde + both shape checks), stripped from tuning and out of the mod digest; gameplay projectiles record the firing weapon class (players and monsters); client resolution by prefixed `entity_class`; the no-opt-in install warning; entities + projectiles categories; reload re-resolution (AC 12 parts, 17, 20, 21) | delegated worker (scripting-core + sim + combat-model) | 3 | |
| 7 | Movers and projectile bodies: `ghost_tags` resolution + unmatched warning, mover ghost instance list (no mover-id dedupe), unlit kinematic-brush ghost pipelines, blend and dither (80-byte instance); ghost billboard variant with tint, saturation and opacity; movers category (AC 3, 12 parts, 14) | delegated worker (renderer + runtime_movers) | 4 | |
| 8 | Ghost styles: `defineGhostStyle`, `ModManifest.ghostStyles` drains (JS + Luau, warn-and-drop by field), out of the mod digest, install and staged-reload resolution by name, undeclared warning; `render.ghosts` render-profile field (JS + Luau drains, renderer profile hand-off) (AC 6–9, 9a) | delegated worker (scripting-core + startup) | 3 | |
| 9 | Per-value clamp: extract `EffectChannel` from `flash_clamp.rs` as the reusable clamp (screen channels keep behavior), a keyed per-ghost clamp container fed by the presented-frame time and enable flag, fresh start on enable, ghost opacity and tint through it (AC 23–26) | delegated worker (render-cpu) | 3 | |
| 10 | Script surface: `ghostLink` map kind (runtime + build-time member tables), `arm`/`disarm`/`setMembers`/`setColorFilter` host-only sequenced primitives with validation, look resolution floor → style → filter with per-field script-time ramps; TS + Luau SDK, typedef templates, regenerated `sdk/types` and fixtures (AC 13 parts, 18, 19, 27, 29 parts, 40) | integrating executor | 5, 8 | |
| 11 | Full link replication: categories and look ramps in the link record, host-time rebase, deferral before the clock anchor, P8 no re-rebase, late join mid-ramp, the client's own limiter (AC 10, 16, 37–39, 41) | integrating executor | 10 | |
| 12 | Audio bus + pool: Ghost track under SFX with kira high-pass, low-pass, distortion and volume; bus-aware positional voices; ghost cap 16; SFX child capacity +1; `audio.ghost` resolver + drains; floor profile (AC 9 part, 28, 33–35) | delegated worker (audio + scripting-core) | — | |
| 13 | Ghost voices + `setGhostAudio`: admission beside admitted real voices at the fire-time mapped point, impact attribution through normalized shot ids, own-pawn exclusion; `postretro/audio` virtual module (runtime, compiler allowlist, build-side require, drift test, typedefs, tsconfig); level-scoped audio record replication; unload and demotion restore (AC 29 parts, 30–32, 36) | integrating executor | 11, 12 | |
| 14 | Dev content: dev map (two room copies, two-way link pair, two-way seamless pair, barrier puzzle with tagged mover, battle swap), dev manifest style + bus, TS + Luau scripts, `docs/ghost-links.md`; surface fixture test; GPU capture test (AC 43, 44) | integrating executor | 6, 7, 9, 13 | |
| 15 | Preflight, review panel, fix loop; full gate incl. `--features postretro/dev-tools` and `capture`; AC results | integrating executor | 14 | |

## Hot paths
- Per-frame membership: bounded by armed links × eligible candidates. Point-in-convex tests run against the link's leaves, reusing frame-owned buffers, with no per-frame allocation. Ghost state is keyed by generation-checked id in a reused map.
- Ghost instances share the 4096 skinned palette/instance budget and drop through its existing overflow policy. Mover ghosts are a separate small list. Neither ghost pipeline binds lights, shadows or SH, so ghosts add no lighting cost.
- Per-ghost clamp: CPU-only at pack time, O(ghosts), no GPU work.
- Link replication: one small record per link per snapshot (always on, the teleporter precedent); look ramps ride only after a script set them.
- Ghost voices: capped at 16, admitted only beside an admitted real voice.
