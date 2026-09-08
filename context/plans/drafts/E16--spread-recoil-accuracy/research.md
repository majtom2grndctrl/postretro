# Research — Dynamic Weapon Accuracy

Investigation notes for `index.md`. Not the spec. Decisions live in the spec; this records why.

## What already exists (grounded this session)

**Static spread seam (shipped, `E16--shotgun-pellet-spread`).**
- `WeaponDescriptor` (`crates/foundation/src/data_descriptors/types/combat.rs`) carries `spread_degrees` (serde `spreadDegrees`, validated finite `0.0..=45.0`) and `pellet_count` (`1..=MAX_PELLET_COUNT`, `MAX_PELLET_COUNT = 32`). Container is `#[serde(rename_all = "camelCase")]`.
- `WeaponComponent` (`crates/entities/src/components/weapon.rs`, 1014 lines) mirrors the tuning and holds live state: `spread_degrees`, `cooldown_remaining_ms`, `shells_fired` (monotonic, wrapping, per-instance). `effective()` (the accessor every fire path reads) is a **straight field copy** today — no rolls, no modifiers, no fold. `refresh_from_descriptor` re-applies authored tuning on hot reload while preserving live state (`shells_fired`, cooldown, state timers).
- Cone sampler `crates/postretro/src/weapon/spread.rs`: `sample_cone_direction(axis, half_angle_rad, u1, u2)` — uniform solid-angle; at `half_angle_rad <= f32::EPSILON` with a finite non-zero axis it returns the axis **byte-for-byte**. `PelletRng` wraps `SplitMix64`; `pellet_rng_seed(shell_counter, salt_name, slot)` folds only the shell counter, a stable salt name, and the inventory slot — no velocity/dt/time/entity-id. `pellet_salt_name` chooses canonical descriptor name → `credit_source` → `UNKNOWN_WEAPON_CREDIT_SOURCE`, never allocation-ordered ids.

**Fire path (`crates/postretro/src/weapon/mod.rs`, 2726 lines).**
- Host: `tick_resolved_component` computes `spread_radians = stats.spread_degrees.to_radians()` (where `stats = weapon.effective()`), advances `shells_fired`, calls `fire_hitscan` (loops `pellet_count`, per pellet `sample_cone_direction(direction, spread_radians, rng.next_f32(), rng.next_f32())`). Receives **no dt**.
- Client: `resolve_client_fire` recomputes the angle **independently** (same `stats.spread_degrees.to_radians()`), advances its own `shells_fired`, calls `resolve_client_hitscan`. Gated by `advance_client_fire_state(weapon, button, frame_dt)`.
- dt reaches only the cooldown-decay gates: `authorize_fire` (host, `context.tick_dt`, `crates/postretro/src/sim/weapon_stage/fire.rs`) and `advance_client_fire_state` (client, `frame_dt`). Both already run every tick and decay `cooldown_remaining_ms`.
- Orchestration: `weapon_stage/machine.rs::tick_weapon_machine(..., tick_dt)` → `authorize_fire`; `weapon_stage/commands.rs::run_local_weapon_command_with_content` calls `tick_resolved_component`. The pawn `EntityId` + `&EntityRegistry` are in scope in both host and client orchestrators and already passed into the fire functions (`owner_pawn`, `registry`).

**Movement state IS reachable at fire time.** `weapon_stage/commands.rs` already reads `PlayerMovementComponent` (`crates/foundation/src/movement/player_movement.rs`) at fire time in `remote_projectile_aim`. Fields: `velocity: Vec3`, `is_grounded()`, `movement_state`, and `ground_params: GroundParams` whose `speed: SpeedParams { walk, run, crouch }` carries the authored run speed. `crates/foundation/src/movement/scope.rs` already derives horizontal `"speed" = |velocity.xz|`. Both the current horizontal speed and the pawn's authored `ground_params.speed.run` are on the same component the fire path already reads — no new plumbing for the movement term.

**Netcode.**
- Client-authoritative HIT: the client casts its own rays and declares; the host validates. `apply_valid_hit_record` (`crates/postretro/src/netcode/mod.rs`) checks only: target exists + has health; `collision::line_of_sight(eye, point)`; `distance <= range * HIT_RANGE_TOLERANCE`. It **never** compares the declared point's angle to the aim direction, and never re-samples the cone. So the host models no spread for a connected client's shot.
- The host "never casts a ray" for a connected client's FIRE — it authorizes (ammo/cooldown), mints the shot. So `tick_resolved_component` (which casts) runs for the host-local pawn, not for connected clients' pawns.
- Weapon tuning replicates via `WieldableTuningPayload` (`crates/postretro/src/netcode/tuning_payload.rs`) — **JSON**, snake_case keys, one row per slot. Already carries `spread_degrees`, `pellet_count`, `cooldown_ms`. Built in `tuning_payload_for_pawn` (`netcode/mod.rs`), installed on the client in `apply_net_wieldable_tuning` (`crates/postretro/src/scripting/builtins/net_descriptor.rs`, which clamps). Version gate is `TUNING_PAYLOAD_EPOCH` (currently 7), independent of transport `WIRE_VERSION`. A committed fixture `crates/postretro/src/netcode/tests/fixtures/tuning_payload.expected.json` is guarded by `payload_json_matches_committed_fixture` (bump epoch or re-bless).
- Engine-randomness carve-out (`networking.md`): "host-only, load-time, consequences-only … One carved exception: weapon pellet-spread sampling runs deterministic per-tick RNG on whichever machine casts the rays … Its seed is a pure function of replay-stable weapon state, so the determinism gate can replay it exactly."

**Determinism gate.** `crates/postretro/src/sim/determinism_tests.rs`: `simulate_tick_determinism_harness_matches_run_to_run_and_spawn_order` and `simulate_tick_is_deterministic_for_random_command_stream` compare baseline / rerun / spawn-order-reversed runs; `events` carry `weapon_impact_points` (the pellet fans) and are asserted bit-equal. Fixture weapon `spawn_determinism_weapon` (`spread_degrees = 4.0`, `pellet_count = 8`). Any dynamic term must remain a function of replay-stable inputs.

**HUD.**
- `crates/postretro/src/scripting/systems/ui_proxy.rs`: `PlayerHudStatePublisher::write_hud_slot(name, SlotValue)`. `weapon_hud_values(registry)` already resolves the local pawn (`registry.local_player_movement_pawn()`) and the active weapon (`Inventory::active_wieldable` → `WeaponComponent`). Slot names are camelCase dotted strings (`"player.reloadProgress"`, `"player.ammoReserve"`).
- `player.*` SDK refs (`getGameState().player`) are generated into `sdk/types/postretro.d.ts` / `.d.luau` from `crates/entities/src/engine_state_catalog.rs` (`BUILTIN_ENGINE_STATE`, plus an ordered slot-name assertion list). `ReplicationScope` (`crates/entities/src/slot_table.rs`): `None` = local-only, not replicated, no `StateSlotId`, excluded from fingerprint; `SharedGlobal`; `OwnerPrivatePlayer`. `reloadProgress` uses `OwnerPrivatePlayer` (host-authoritative replicated); `player.weapon.current` is published locally every role.

**Ring.**
- Shipped `Ring` widget (`radial-ui-primitive`, done). `RingProps` (`sdk/lib/ui/widgets.ts`): `radius`/`thickness`/`startAngle`/`sweep` are each `number | RingBindProp` — a literal or a **1:1** slot/local bind with an optional `NumberTween` (`{ durationMs, easing }`). The UI cannot compute per-frame; there is no map/scale on a bound value (that is the deferred UI computed-bindings work). `content/dev/scripts/hud.ts` `hud.reticle` renders a Ring with all-literal geometry — static.

**Content.**
- Reference weapons are TS `defineEntity` scripts in `content/dev/scripts/`: `reference-pistol.ts` (hitscan/semi, `bullets.light`), `reference-shotgun.ts` (hitscan/semi, `pelletCount: 8`, `spreadDegrees: 5`, `shells.buck`), `reference-projectiles.ts` (plasma-bolt auto + rocket semi). Loadout in `player.ts`; registered in `start-script.ts` (`entities: [...]` + `uiTrees: [...]`).
- Ammo types are free-form ASCII identifier strings validated by `validate_ascii_identifier` — no registry. A rifle round is a new `type: "bullets.rifle"` string.
- Art `content/dev/models/cyberpunk_weapons/rifle/model.gltf` exists and is referenced by no weapon (the `PLASMA_RIFLE_MODEL` constant points at `rpg/`, `enemy-rifle.ts` has no model).
- `.d.ts`/`.d.luau` and `crates/postretro/src/scripting/typedef/tests/fixtures/expected.d.{ts,luau}` are generated by `gen-script-types`.

## Direction long-form

### Q1 — problem, cause not symptom
Weapons have only a **static** per-shot spread cone. It is identical shot-to-shot regardless of firing history or player movement (`fire_hitscan` reads `stats.spread_degrees.to_radians()`, a constant per weapon). Observation: every reference weapon except the shotgun fires with zero spread forever; you cannot build a weapon whose accuracy degrades under sustained fire or while moving. Cause: there is no live accuracy state and no composition point — `effective()` is a straight copy. The Weapon Feel milestone's "distinct spread/recoil signatures" outcome is unreachable without a dynamic axis.

### Q2 — right level (placement)
Placement axes here: engine-floor Rust vs mod-authored script; within the engine, which crate; presentation vs authority; content (TS) vs engine.
- **Spread math + live state → engine floor** (`crates/entities` component + `crates/postretro/src/weapon`), exactly where static `spread_degrees` + `shells_fired` already live. It feeds the deterministic per-tick cone sampler and must be replay-stable and client-predicted; script/IR is not on that path (the per-tick evaluator forbids RNG, `scripting.md` §12). This is the strongest evidence of correct placement: the static half of the same axis already lives there.
- **Tuning (the numbers) → content/script** (descriptor authored in TS), a primitive contract. Matches the project's identity: engine is the neutral mechanism, designers shape feel.
- **`player.spread` value → engine** (published from `ui_proxy` in a neutral unit, degrees), **local-only** (`ReplicationScope::None`) because each machine computes its own predicted spread. **Ring visual + mapping → content** (`hud.ts`). The rich script-authored curve is the deferred UI computed-bindings layer — a UI-layer feature, deliberately not pulled into a weapon-feel spec.

### Q3 — foreclosures / one-way doors
- New `WieldableTuningPayload` fields: a JSON payload change, reversible with a `TUNING_PAYLOAD_EPOCH` bump. Nothing material foreclosed.
- The **descriptor field vocabulary** is a published authoring contract the moment modders use it; renaming later is a content-breaking change. This is the one measure-twice item. Undoing = a deprecation cycle. Named deliberately (see field-vocabulary below), reviewable before promotion.
- Modeling recoil as symmetric cone-growth + an optional axis tilt (no camera motion) does **not** foreclose a future true view-kick recoil: `networking.md` reserves authoritative view-angle mutation for that, and the pellet-spread pin already guarantees the "move the axis" mechanism composes without rework. A later spec can add camera-moving recoil on the movement view-angle seam.

### Q4 — prior commitments touched
- **Pellet-spread composition pin** (`E16--shotgun-pellet-spread`, Direction → Placement): dynamic accuracy composes by moving the axis before sampling or scaling the effective spread; "pellet sampling reads whatever axis and spread it is handed and needs no rework." This spec plugs in exactly there — bloom + movement scale the effective spread; directional bias moves the axis.
- **Engine-randomness carve-out** (`networking.md`): the bloom accumulator is deterministic (not RNG); the RNG seed is unchanged (still `pellet_rng_seed(shells_fired, salt, slot)`) — only the angle handed to the sampler changes. Stays inside the carve-out because bloom is a function of replay-stable state.
- **Client-authoritative HIT** (`E16--client-authoritative-combat`): host validates LOS + range only. Work-eliminating claim, warranted: `apply_valid_hit_record` never re-derives the cone, so the host needs no bloom/movement/bias model — dynamic spread is per-casting-machine, and only the **tuning** must replicate.
- **Primitive surface is a contract** (`index.md`): new descriptor fields require SDK types (regenerated) + validation in the same pass.
- **Live state preserved across hot reload**: bloom state joins `shells_fired` in `refresh_from_descriptor`'s preserved set.

### Q5 — reversibility
Field-vocabulary is the only sticky decision (deprecation cost). The wire/tuning change is epoch-reversible. Nothing else material.

### Q6 — strongest alternative
- **True view-kick recoil** (camera rotates, shots follow, recovery pulls back). Rejected per owner intent (accuracy loss, not camera shake); heavier (authoritative view-angle mutation + the recovery/manual-compensation double-count problem); separable — addable later on the movement view-angle seam without redoing this.
- **Bloom as script/IR policy** rather than engine state. Rejected: it must feed the deterministic per-tick cone sampler and be replay-stable + client-predicted; the per-tick evaluator forbids RNG and is not where cone sampling lives.
- **Publish `player.spread` pre-mapped to pixels** (engine owns ring size). Rejected on layering: bakes presentation into the engine; the project's identity is engine-neutral data + script-shaped feel. Publish degrees; the reticle binds it as a relative indicator; rich curves await computed-bindings.

## Field vocabulary (the one-way-door decision)

Angle fields carry the `Degrees` suffix (mirrors `spreadDegrees`); time fields carry `Ms` (mirrors `fireRateMs`, `reloadMs`). All new fields `#[serde(default)]` → **default 0**, so an unauthored weapon composes to exactly `spread_degrees` (existing behavior preserved byte-for-byte via the sampler's zero-angle exact-axis return).

| Field (serde/TS/Luau) | Meaning | Contributor |
|---|---|---|
| `bloomPerShotDegrees` | degrees added to the bloom accumulator per shot | sustained fire |
| `bloomMaxDegrees` | cap on the accumulator | sustained fire |
| `bloomDecayDegreesPerSecond` | recovery rate while idle | sustained fire |
| `bloomDecayDelayMs` | grace after last shot before decay begins (default 0) | sustained fire |
| `movementSpreadDegrees` | additional degrees at/above the reference move speed, scaled by current horizontal speed fraction | movement |
| `spreadVerticalBias` | 0..1; fraction of the effective spread by which the aim axis tilts upward (0 = symmetric) | directional bias |

Rejected naming alternatives: `bloomRecoveryMs` (time-to-recover) — a rate composes independently of `bloomMaxDegrees`, a time does not. A multiplicative bloom (`spreadMultiplier`) — **cannot** bloom a zero-base weapon like the pistol (`0 × k = 0`); additive degrees is required, which is why bloom is authored in degrees, not as a scale.

Movement reference speed: the pawn's own authored `ground_params.speed.run` (on `PlayerMovementComponent`, reachable at fire time; `SpeedParams.run` is "the omnidirectional horizontal speed target"), **not** a hidden engine constant and **not** a per-weapon field. This is the engine-neutral / script-shaped-feel identity applied: a designer who retunes movement speed automatically rescales weapon movement-spread, and `movementSpreadDegrees` reads as "spread at full run." A per-weapon reference-speed override is a possible future field, out of scope. (Rejected: a `MOVEMENT_SPREAD_REFERENCE_SPEED` engine constant — an arbitrary, invisible feel decision baked into the floor.)

## Lifecycle — per-tick bloom advance

```mermaid
sequenceDiagram
    participant Gate as authorize_fire (host) / advance_client_fire_state (client)
    participant Resolve as tick_resolved_component (host) / resolve_client_fire (client)
    Note over Gate: every tick, dt available
    Gate->>Gate: cooldown_remaining_ms -= dt
    Gate->>Gate: bloom_idle_ms += dt
    Gate->>Gate: if bloom_idle_ms >= bloomDecayDelayMs:<br/>bloom_accumulator -= bloomDecayDegreesPerSecond*dt (clamp >=0)
    alt fire authorized this tick
        Resolve->>Resolve: read pawn horizontal speed (PlayerMovementComponent)
        Resolve->>Resolve: eff = clamp(spread_degrees + bloom_accumulator + movement_term, 0, MAX_EFFECTIVE_SPREAD_DEGREES)
        Resolve->>Resolve: axis' = tilt(aim, spreadVerticalBias * eff)
        Resolve->>Resolve: sample_cone_direction(axis', eff.to_radians(), rng) per pellet
        Resolve->>Resolve: shells_fired += 1
        Resolve->>Resolve: bloom_accumulator = min(bloomMaxDegrees, bloom_accumulator + bloomPerShotDegrees); bloom_idle_ms = 0
    end
```

Read-before-grow gives "first shot of a burst is base-accurate." Decay in the gate runs even on a firing tick, before the resolve reads — so a shot sees the decayed-but-not-yet-grown accumulator. `bloomDecayDelayMs > fireRateMs` ⇒ no decay between sustained shots (pure growth to cap); `< fireRateMs` ⇒ partial recovery between shots.

To avoid the two-call-site drift risk (host and client compute the angle independently, `fire_hitscan`-caller vs `resolve_client_fire`), the decay/growth/compose logic should be `WeaponComponent` methods (e.g. `tick_bloom(dt)`, `apply_bloom_shot()`, `effective_spread_degrees(horizontal_speed)`), called from both paths — one implementation, two call sites.

## Observers (vantage × lifecycle)

| Vantage | Computes bloom? | Notes |
|---|---|---|
| Host-local pawn (SP / listen-host) | Yes — host path | Determinism gate replays it |
| Connected client, own pawn | Yes — client prediction path | Casts + declares; host does not re-derive |
| Host validating a client's declaration | **No** | `apply_valid_hit_record` = LOS + range only |
| Host, connected client's FIRE | No cone (authorizes only) | Host never casts for client pawns |
| HUD `player.spread` | Each machine, own local pawn | `ReplicationScope::None`; remote pawns' spread not shown |
| Replay / determinism gate | Yes | Requires replay-stable inputs |

The one work-eliminating claim — "the host needs no dynamic-spread model" — is warranted by `apply_valid_hit_record`'s LOS+range-only validation, not asserted.

## Projectiles are unaffected
Dynamic spread feeds the **hitscan** cone sampling only (`fire_hitscan` / `resolve_client_hitscan`). The projectile launch path converges the direction on the crosshair target (`networking.md` "Fire origin composes on placement") and does not sample the cone; reference projectiles author no bloom (fields default 0). So plasma-bolt and rocket are unchanged. Matches the owner's intent (non-ballistic weapons have no recoil).

## Oversized-file note
`weapon/mod.rs` is 2726 lines. New logic homes in `weapon/spread.rs` (composition helpers) and a `WeaponComponent` impl block in `weapon.rs`; `mod.rs` gains only call-site edits (read the composed angle, call the shot-growth method). A preemptive split of `mod.rs` is out of proportion to that footprint — not scheduled.
