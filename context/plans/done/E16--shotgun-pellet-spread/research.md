# Shotgun Pellet Spread — Research Notes

Source inventory behind the spec. All symbols verified against source this session
(post-merge of `E16--wieldable-switching-inventory` / `E16--wieldable-pickup-drop`).

## Owner decisions (session, 2026-08)

- Uniform cone sampling only; no authored per-pellet trajectory patterns.
- Cone spread is a weapon descriptor stat behind `effective()` — mutable via future
  modifiers, not runtime-writable state.
- `damage` is per pellet; shell total = damage × pelletCount.
- `pelletCount` hard-capped at 32 by descriptor validation — one bound for gameplay,
  wire, and presentation.
- One trigger pull fires one shell; do not foreclose multi-shell activations
  (double-barrel).
- Hit model per pellet: per-pellet impact fires, per-pellet zone multipliers.
- PvE focus — bias toward smooth client experience over anti-cheat strictness.

## Fire-path inventory

Two production fire paths, one aim source, no spread anywhere today:

- **Aim source:** `Camera::aim_ray()` (`crates/postretro/src/camera.rs:144`) — exact
  screen center from yaw/pitch, normalized. Consumers: `build_post_movement_command`
  (`main.rs:1215`, host/SP) and the client post-loop fire site (`main.rs:5778`).
- **Local path (single-player + listen-host's own pawn):** `simulate_tick` →
  `weapon_stage::run_local_weapon_command` (`sim/weapon_stage/commands.rs:139`) →
  `weapon::tick_resolved_component` (`weapon/mod.rs:316`) → `fire_hitscan`
  (`weapon/mod.rs:350`). Produces `WeaponFireEvents { activate, impact:
  Option<WeaponImpact>, dry_fire }` — **at most one impact per fire**; this path is
  NOT pellet-general. `commands.rs:260-265` then runs, per fire:
  `spawn_impact_effect_at` → `apply_weapon_impact_damage` → `on_impact`.
  `WeaponImpact` (`weapon/mod.rs:199`): `point`, `normal`, `target: Option<EntityId>`
  (None = world hit), `zone: Option<String>`, `outcome`. `event_names()`
  (`weapon/mod.rs:225`) pushes `"dry_fire"` / `"activate"` / `"impact"` at most once
  each per fire.
- **Client path (connected client):** post-loop, once per frame —
  `resolve_client_fire` (`weapon/mod.rs:396`) → `resolve_client_hitscan`
  (`weapon/mod.rs:456`) → 0..1 `LocalHitRecord { target, point, zone }`
  (`weapon/mod.rs:57`); only an entity hit produces a record. Already returns `Vec`,
  wire already carries 0..N records — pellet-general in shape. Multi-tick frames:
  only the first client tick casts (`main.rs:5840-5849`); every additional tick
  sends an **empty** declaration to retire its shot_id. Load-bearing invariant.
- **Shared cast:** `resolve_nearest_hit` (`weapon/mod.rs:509`) — world
  (`collision::cast_ray`) vs entity (`nearest_entity_hit`,
  `scripting/systems/hit_zones.rs:654`, interpolated pose via `anim_time`), nearest
  wins, tie to wall. `nearest_entity_hit` `debug_assert`s a **unit-length**
  direction (`hit_zones.rs:667`) — sampled cone directions must be normalized.
- **Host view of a remote pawn's fire:** `run_remote_weapon_commands`
  (`sim/weapon_stage/commands.rs:100-129`) mints `AuthorizedShot` from
  `effective()` (`damage`, `range`, `credit_source`) with `pellet_count: 1`
  hardcoded at `commands.rs:128`. The host casts no ray for remote fire.

## Netcode inventory

- `AuthorizedShot` (`netcode/mod.rs:453`) already has `pellet_count: usize`.
  Construction sites: `sim/weapon_stage/commands.rs:128` (production),
  `netcode/mod.rs:2954`, `netcode/lifecycle.rs:800`, `:1074` (tests).
- Ingest: `ingest_hit_declaration` (`netcode/mod.rs:1762`) — shot-binding +
  ownership first, unconditional retire, `pellet_count == 0` → hit rejected,
  `records.iter().take(pellet_count)` (`:1793`), `apply_valid_hit_record` per
  record with `on_impact` fired **per accepted pellet** (`:1805-1811`).
  Per-record checks (`:1817`): NetworkId→entity resolve, finite point, live
  crouch-aware `attacker_eye`, `has_static_world_los` (one world cast per record),
  range × `HIT_RANGE_TOLERANCE = 1.25`. Damage applied per record —
  **`AuthorizedShot.damage` is already per-record damage**, so "damage is per
  pellet" is the no-change reading of shipped ingest.
- Regression tests already pellet-driving:
  `hit_declaration_runs_impact_consumer_after_each_accepted_pellet`
  (`netcode/mod.rs:3364`, asserts policy sees 90 then 80 — effects settle between
  pellets), `hit_declaration_clamps_accepted_records_to_default_pellet_count`
  (`:3505`), `hit_declaration_pellet_clamp_counts_declared_invalid_records`
  (`:3549` — invalid records consume clamp budget).
- Wire: `HitRecord` / `HitDeclaration` (`crates/net/src/wire.rs:964,974`) —
  0..N records, no count field, no change needed. `ShotVerdict.hit_accepted` is one
  bool per declaration — no per-pellet verdict channel (accepted; PvE).
  `local_hits_to_wire_records` (`netcode/mod.rs:1237`) silently drops records whose
  target has no NetworkId — under N pellets that just declares fewer records.
- Tuning payload: `WieldableTuningPayload` (`netcode/tuning_payload.rs:21`)
  replicates `canonical_name`, `range`, `cooldown_ms`, `fire_mode`, `resolution`,
  `lower_ms`, `raise_ms`. `damage` deliberately not replicated (host-owned).
  `TUNING_PAYLOAD_EPOCH: u32 = 3` (`:13`); committed fixture
  `netcode/tests/fixtures/tuning_payload.expected.json` (snake_case keys); bless
  env `POSTRETRO_BLESS_COMPATIBILITY_FIXTURES=1`. Built by `tuning_payload_for_pawn`
  (`netcode/mod.rs:1316`); consumed by `apply_net_wieldable_tuning` /
  `materialize_net_local_wieldable_at_slot`
  (`scripting/builtins/net_descriptor.rs:351,309`) and `remote_materialize.rs`.
  The client's predicted fire reads its **local** `WeaponComponent`, seeded only by
  this payload — a client-cast stat must ride it.

## Descriptor / SDK inventory

- `WeaponDescriptor` (`crates/foundation/src/data_descriptors/types/combat.rs:70`):
  `damage`, `range`, `cooldown_ms` (wire `fireRateMs`), `fire_mode`, `resolution`
  (only `Hitscan`), `credit_source`, `third_person_model`, `viewmodel`,
  `resource` (`WeaponResource::Ammo` only), `lower_ms`, `raise_ms`,
  `block_during_reload`. No accuracy/spread/pellet field. `validate()` at `:100`,
  errors `DescriptorError::InvalidShape`.
- `WeaponComponent::effective()` (`crates/entities/src/components/weapon.rs:331`) →
  `EffectiveStats` (`:29`) — pure passthrough, no modifier layer.
  `refresh_from_descriptor` (`:352`) reseeds authored tuning, preserves live state.
- Parsers: `scripting-core/src/data_descriptors/js/entity.rs:80-93` and
  `lua/entity.rs:108-121` — serde-driven; a new optional numeric field needs no
  manual reader edit. Shared parse tests: `data_descriptors/tests/entity.rs`.
- Typedef source of truth: `crates/postretro/src/scripting/primitives/mod.rs:321`
  (`register_type("WeaponDescriptor")` + per-field `.field(...)`); regen
  `cargo run -p postretro --bin gen-script-types`; committed
  `sdk/types/postretro.d.ts` (+ `.d.luau`) asserted by
  `scripting/typedef/tests/committed.rs` and fixture copies under
  `scripting/typedef/tests/fixtures/`.
- Hot reload: `refresh_plan.rs` `plan_weapon_replace` (`:448`) →
  `refresh_from_descriptor`. New authored stats belong in the reseeded set.
- Zone multipliers live on the **target**: `HealthDescriptor.zone_multipliers`
  (`combat.rs:222`), applied in `apply_weapon_impact_damage_with_source`
  (`sim/weapon_stage/impact.rs:59-96`) per impact — per-pellet zone scaling is
  free on both paths.

## RNG inventory

- No `rand` crate. Two in-tree precedents:
  - `emitter_bridge.rs`: LCG + `sample_cone_direction(axis, spread, state)`
    (`:389`) — solid-angle-uniform inverse CDF (`cos θ = 1 − u·(1 − cos α)`,
    uniform azimuth, hand-built orthonormal basis, collapses to `axis` when
    `spread <= f32::EPSILON`). Private, seeded **non-deterministically by design**
    (position bits ⊕ frame step) — the seeding must not be copied; the math can.
    Distribution test precedent: `spawn_directions_distribute_within_cone` (`:651`).
  - `trigger_pools.rs`: `SplitMix64` (`:51`) — the deterministic precedent.
- Determinism gate: `sim/determinism_tests.rs` records `AuthorizedShot` fields
  including `pellet_count` (`:197,:616`) and compares `TickEvents.weapon` names +
  deaths across two identical runs. Any RNG reaching `simulate_tick` must be
  seeded from sim state only.

## Presentation inventory

- No tracer, no muzzle-flash renderer. `"activate"` / `"impact"` weapon script
  events are the presentation hooks; `WeaponActivation` is `#[allow(dead_code)]`.
- Impact FX: `spawn_impact_effect_at` (`weapon/impact.rs:37`), 9 particles ×
  0.18 s, called only from the local path (`commands.rs:262`). The authorized-hit
  ingest sets `normal: Vec3::ZERO` and spawns no FX.
- No fire-FX replication: `ServerMessage` = `TimeSync` | `ShotVerdicts`
  (owner-private). Other clients see a shot only via replicated consequences.
  A spread pattern is visible to the shooter alone today.

## Vantage × behavior map

| Vantage | Today | Under this spec |
|---|---|---|
| Single-player pawn | 1 ray in `simulate_tick`, inline damage + FX + `on_impact` | N sampled rays, per-pellet impact/damage/FX/policy, deterministic directions |
| Listen-host's own pawn | same local path | same as single-player (same code path — `run_local_weapon_command`) |
| Host ingesting a remote pawn's fire | mints `AuthorizedShot { pellet_count: 1 }`, casts nothing | mints with effective pellet count; ingest clamp/validation unchanged in shape |
| Firing connected client | predicts, casts 1 ray at rendered pose, declares 0..1 records | casts N rays (first tick of frame only), declares 0..N |
| Other clients | see nothing of the fire | unchanged — no fire-FX replication exists |

## Derivations that shaped decisions

- **Per-pellet damage is the no-change host semantics.** Ingest already applies
  `AuthorizedShot.damage` once per accepted record; the local path already builds
  `DamagePayload { amount: damage }` per impact. Declaring `damage` per pellet
  changes zero application code — only authored numbers.
- **The real structural work is the local path.** `WeaponFireEvents.impact:
  Option<WeaponImpact>` must become a collection; `fire_hitscan` must sample and
  cast N; `commands.rs` must loop its apply sequence. The client/declaration side
  is already N-shaped.
- **Sampler extraction, not reuse.** The emitter's cone math is correct but
  private and welded to `EmitterBridgeState` + a deliberately non-deterministic
  seed. Extract the pure math; keep seeding policy per consumer.
- **Spread stat name.** `spreadDegrees` (half-angle, degrees) over radians:
  weapon descriptor fields are author-facing (`fireRateMs` precedent —
  author-unit-friendly names); the emitter's radians `spread` is an FGD KVP,
  a different authoring surface.
- **Seed inputs.** A per-weapon monotonic shell counter (`WeaponComponent` live
  state) mixed with a spawn-order-stable instance salt: canonical-name hash
  (from `DescriptorProvenance`; `credit_source` fallback) + inventory slot. No
  owner id — none is reachable from either fire path, and each machine samples
  only its own pawn's fire, so owners never share a sampler. Two constraints
  force this shape: no
  tick reaches the local weapon stage (`run_local_weapon_command`,
  `simulate_tick`, and `SimCommand` carry none — threading one crosses ~46 call
  sites), and `EntityId`/`NetworkId` bits are allocation-ordered, which the
  shipped spawn-order-reversal determinism assertion
  (`simulate_tick_determinism_harness_matches_run_to_run_and_spawn_order`)
  would catch as divergence. Host and client never need matching directions
  (host casts no rays).
