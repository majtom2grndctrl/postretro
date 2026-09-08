# Dynamic Weapon Accuracy (spread / recoil / bloom)

## Goal

Add the dynamic accuracy axis to weapons: a per-shot spread that grows under sustained fire and while moving, recovers when you stop, and (optionally) biases shots upward — composed into the existing hitscan cone sampler at the composition seam the shotgun pellet-spread spec reserved. Surface it as a neutral `player.spread` HUD value and a crosshair spread-ring that grows with lost accuracy, and prove it with a new full-auto assault-rifle reference weapon plus a gentler pistol signature. This is the Weapon Feel milestone's "distinct spread/recoil signatures" outcome — the tuning axis modders shape to build different-feeling retro FPS.

## Scope

### In scope
- Sustained-fire **bloom**: a live per-instance accumulator that grows per shot toward a cap and decays back after a delay.
- **Movement** inaccuracy: effective spread scales with the pawn's horizontal speed.
- Optional **directional bias**: the aim axis tilts upward by a fraction of the effective spread ("recoil climb" without camera motion).
- Composition of all three into the effective hitscan cone half-angle, computed identically on host and client fire paths, clamped to an engine ceiling.
- New descriptor tuning fields, validated, replicated in the weapon tuning payload, preserved across hot reload, and exposed through the regenerated SDK typedefs.
- `player.spread` HUD slot (effective half-angle in degrees, local-only) and a spread-ring in `hud.reticle`.
- A full-auto hitscan assault-rifle reference weapon (`reference_rifle`) with its own ammo and the unused rifle art; a gentler bloom tune on the reference pistol; the `combat-demo` walkthrough updated.

### Out of scope
- **Camera-moving / view-kick recoil** (authoritative view-angle mutation, recovery, manual-compensation). A separate spec on the movement view-angle seam; the "move the axis" mechanism here does not foreclose it.
- **Script-authored value curves for the ring** (map/scale/clamp on a bound value). The UI cannot compute per-frame today; that is the deferred UI computed-bindings layer. The reticle binds `player.spread` 1:1 with a tween.
- **Dynamic spread on projectile weapons.** Projectile launch converges on the crosshair and does not sample the cone; reference projectiles author no bloom and are unchanged.
- **The augment / `defineAugment` modifier layer.** A separate Weapon Systems item; this spec adds the dynamic-spread composition as dedicated `WeaponComponent` methods, not a general modifier fold on `effective()`.
- **ADS, scope rendering, aim assist** — sibling Weapon Feel bullets.
- **Per-weapon override of the movement reference speed** — the movement term normalizes against the pawn's authored `ground_params.speed.run`; a per-weapon reference-speed override is a possible future field, not built.
- **Modder-facing docs** in `docs/scripting-reference.md` — owned by `scripting-reference-weapon-docs`.

## Direction

**Problem.** The per-shot spread cone is static — `fire_hitscan` reads a constant `stats.spread_degrees.to_radians()` — so a weapon cannot lose accuracy from firing history or movement, and `effective()` is a straight copy with no composition point.

**Prior commitments.** The pellet-spread spec pinned the seam: dynamic accuracy composes by "moving the axis before sampling or scaling the effective spread; pellet sampling reads whatever axis and spread it is handed and needs no rework" (`context/plans/done/E16--shotgun-pellet-spread/`). Bloom and movement scale the effective spread; directional bias moves the axis. The engine-randomness carve-out (`networking.md`) covers deterministic per-tick weapon sampling "on whichever machine casts the rays … seed a pure function of replay-stable weapon state" — bloom is deterministic (not RNG) and leaves the seed unchanged, so it stays inside the carve-out. Client-authoritative HIT (`E16--client-authoritative-combat`) means the host validates declarations by LOS + range only (`apply_valid_hit_record`) and never re-derives the cone — so dynamic spread is per-casting-machine and only the tuning replicates. New descriptor fields update the SDK contract in the same pass ("primitive surface is a contract", `index.md`). Divergence from the roadmap "recoil" wording is deliberate and owner-decided: recoil here is accuracy loss (cone growth + optional axis tilt), not camera shake.

**Alternatives rejected.** True view-kick recoil (camera rotates, shots follow, recovery pulls back) — owner wants accuracy loss, not camera shake; heavier (authoritative view-angle mutation + the recovery double-count) and separable. Bloom as script/IR policy — it must feed the deterministic per-tick sampler and be replay-stable + client-predicted, which the per-tick evaluator (RNG-forbidden) is not. Publishing `player.spread` pre-mapped to pixels — bakes presentation into the engine; the project's identity is neutral engine data + script-shaped feel, so publish degrees and let the reticle bind it. Building an engine-side modifier fold now — the pellet-spread seam names two routes ("move the axis" and "scale the effective spread **via the modifier layer**"); this spec takes both mechanisms but composes them in a shared `effective_spread_degrees` method rather than a general modifier fold. The fold buys nothing yet: there are three engine-owned contributors and no augment authoring surface (that layer is a separate Weapon Systems item), and the real risk it would address — two-call-site drift between `tick_resolved_component` and `resolve_client_fire` — is already killed by the shared method. The fold is the natural addition when the augment layer actually lands, plugging into the same call sites.

## Acceptance criteria

- [ ] **AC1** A weapon with `bloomPerShotDegrees > 0` fires its first shot from rest at its base `spreadDegrees` cone; under sustained fire that outpaces decay (e.g. `bloomDecayDelayMs > fireRateMs`, so no decay lands between shots — Ordering pin 5) each successive shot samples a wider cone, up to `spreadDegrees + bloomMaxDegrees`.
- [ ] **AC2** After firing stops, effective spread holds for `bloomDecayDelayMs`, then shrinks at `bloomDecayDegreesPerSecond` until the accumulator reaches 0 (base spread).
- [ ] **AC3** With all new spread fields at their 0 defaults, `effective_spread_degrees` returns `spread_degrees` bit-for-bit and the axis tilt is the identity (bias 0 ⇒ axis unchanged), so `sample_cone_direction` receives the same half-angle and axis it does today and the cast directions are unchanged (base `spreadDegrees`; exact axis when base is 0).
- [ ] **AC4** Effective spread rises with the pawn's horizontal speed up to `movementSpreadDegrees` at the pawn's authored run speed (`ground_params.speed.run`) and is 0 at standstill; it adds to sustained-fire bloom.
- [ ] **AC5** With `spreadVerticalBias > 0`, the sampled cone axis tilts upward by `spreadVerticalBias × effectiveSpread`; at 0 the cone is symmetric about the aim axis.
- [ ] **AC6** Effective spread is clamped to the engine ceiling `MAX_EFFECTIVE_SPREAD_DEGREES` regardless of base + bloom + movement.
- [ ] **AC7** The bloom accumulator and its idle timer are live per-instance state preserved across `refresh_from_descriptor` (like `shells_fired`); authored tuning re-applies.
- [ ] **AC8** Each new field validates on author: `bloomPerShotDegrees`, `bloomMaxDegrees`, `movementSpreadDegrees` finite in `0.0..=45.0`; `bloomDecayDegreesPerSecond` finite `>= 0.0`; `bloomDecayDelayMs` finite `>= 0.0`; `spreadVerticalBias` finite in `0.0..=1.0`. Invalid values return a `components.weapon.<field>` shape error.
- [ ] **AC9** New tuning fields replicate to connected clients; a client's predicted weapon computes the same effective-spread growth from replicated tuning. `TUNING_PAYLOAD_EPOCH` is bumped and the committed JSON fixture updated.
- [ ] **AC10** The determinism gate passes with a bloom-authored weapon: a sustained burst's sampled fan is bit-identical run-to-run and across spawn-order reversal.
- [ ] **AC11** `player.spread` publishes the local active weapon's effective spread half-angle in degrees each tick, on every network role, as a readonly non-replicated (`ReplicationScope::None`) `player.*` slot reachable as `getGameState().player.spread`; it reads 0 with no active weapon.
- [ ] **AC12** `hud.reticle` shows a spread ring whose radius tracks `player.spread` with a tween: minimum at rest, visibly expanding during sustained fire and while moving, easing back as accuracy recovers. (Review/playtest-verified — a HUD visual, not an automated assertion.)
- [ ] **AC13** A full-auto hitscan `reference_rifle` exists using `content/dev/models/cyberpunk_weapons/rifle/model.gltf` and `bullets.rifle` ammo, is in the player loadout and registered, and demonstrates strong sustained-fire bloom, movement spread, and upward bias.
- [ ] **AC14** The reference pistol stays `fireMode: "semi"` and gains a gentler bloom signature — rapid trigger-pulling visibly widens its spread and the ring, recovering faster and to a smaller cap than the rifle. Shotgun and projectile reference weapons are unchanged.
- [ ] **AC15** Projectile weapons' launch direction is unaffected by dynamic spread.
- [ ] **AC16** The six new descriptor fields (`bloomPerShotDegrees`, `bloomMaxDegrees`, `bloomDecayDegreesPerSecond`, `bloomDecayDelayMs`, `movementSpreadDegrees`, `spreadVerticalBias`) appear in the regenerated SDK typedefs (`sdk/types/postretro.d.ts`, `.d.luau`) with the doc strings authored in the `register_type("WeaponDescriptor")` block (`crates/postretro/src/scripting/primitives/mod.rs`), and the committed typedef fixtures (`crates/postretro/src/scripting/typedef/tests/fixtures/expected.d.ts`, `expected.d.luau`) match.

## Tasks

### Task 1: Dynamic-spread data shape and authoring surface
Add the six tuning fields to `WeaponDescriptor` (`crates/foundation/src/data_descriptors/types/combat.rs`), each `#[serde(default)]` so unauthored weapons default to 0: `bloom_per_shot_degrees`, `bloom_max_degrees`, `bloom_decay_degrees_per_second`, `bloom_decay_delay_ms` (`f32`), `movement_spread_degrees` (`f32`), `spread_vertical_bias` (`f32`). The container is `#[serde(rename_all = "camelCase")]`, so authoring keys are `bloomPerShotDegrees` etc. — no explicit renames needed. Extend `WeaponDescriptor::validate` mirroring the existing `spread_degrees` arm and its `DescriptorError::InvalidShape { reason: format!("`components.weapon.<field>` …") }` style, with the bounds in AC8; add cases to the `weapon_pellet_stats_validate_their_authored_bounds` sibling test covering one accepted and one rejected value per field (AC8). Mirror the six fields onto `WeaponComponent` (`crates/entities/src/components/weapon.rs`) as authored tuning, and add two live-state fields `bloom_accumulator_degrees: f32` and `bloom_idle_ms: f32` (both `#[serde(default)]`). Copy the tuning fields in `from_descriptor` / `from_descriptor_with_canonical`; in `refresh_from_descriptor` re-apply the tuning while preserving `bloom_accumulator_degrees` and `bloom_idle_ms` exactly as `shells_fired` is preserved (AC7). Do not add the six tuning fields to `EffectiveStats`: the Task 2 composition methods read the tuning and live state directly off `WeaponComponent`, the tuning payload sources from `WeaponComponent`, and the HUD calls `effective_spread_degrees` on the component — no fire path reads spread tuning through the borrowed `effective()` view. Regenerate the SDK typedefs (`sdk/types/postretro.d.ts`, `sdk/types/postretro.d.luau`) and the generated fixtures (`crates/postretro/src/scripting/typedef/tests/fixtures/expected.d.ts`, `expected.d.luau`) via the `gen-script-types` path; the `WeaponDescriptor` typedef surface and its per-field doc strings are hand-authored in the `register_type("WeaponDescriptor")` block in `crates/postretro/src/scripting/primitives/mod.rs` (not reflected from the Rust struct), so add six `.field("bloomPerShotDegrees?", "f32", "<doc>")` … entries there — all optional `?`, camelCase keys, doc strings authored inline, mirroring the existing `spreadDegrees?` / `pelletCount?` entries — before regenerating; the scalar fields need no per-field threading in the JS/Luau bridge (`crates/scripting-core/src/data_descriptors/{js,lua}/entity.rs`) — they flow through the serde deserialize + `validate()` like `spreadDegrees`. Grep `WeaponDescriptor {` across the workspace and add the new named fields to every literal construction (e.g. constructions in `crates/postretro/src/scripting/builtins/data_archetype.rs` and test fixtures). This task adds fields and validation only; composition and replication are Task 2. Delivers AC7, AC8, AC16.

### Task 2: Compose dynamic spread into the fire paths, replicate, and lock determinism
Compose the effective cone half-angle from the fields Task 1 added, identically on both fire paths, and replicate the tuning. Add `WeaponComponent` methods (home them as a `WeaponComponent` impl block in `crates/entities/src/components/weapon.rs`, alongside `effective()`, so `weapon/mod.rs` gains only call-site edits. The arithmetic cannot live in `crates/postretro/src/weapon/spread.rs`: `postretro-entities` does not depend on `postretro`, so a `WeaponComponent` method cannot reach a `postretro` helper or constant. `crates/postretro/src/weapon/spread.rs` keeps the RNG sampler and the postretro-side axis tilt): a per-tick `tick_bloom(dt_ms)` that increments `bloom_idle_ms` by `dt_ms` and, once `bloom_idle_ms >= bloom_decay_delay_ms`, decays `bloom_accumulator_degrees` by `bloom_decay_degrees_per_second * (dt_ms / 1000.0)` (the rate is per-second, the argument is milliseconds) clamped `>= 0`; an `effective_spread_degrees(horizontal_speed, run_speed)` that returns `clamp(spread_degrees + bloom_accumulator_degrees + movement_term, 0.0, MAX_EFFECTIVE_SPREAD_DEGREES)` where `movement_term = movement_spread_degrees * clamp(horizontal_speed / run_speed, 0.0, 1.0)` and `run_speed <= 0.0` yields a 0 movement term (define `MAX_EFFECTIVE_SPREAD_DEGREES` — e.g. 45.0, matching the static `spread_degrees` validation cap — in `crates/entities/src/components/weapon.rs` (with `WeaponComponent`), so both this compose method and the engine-state catalog in `crates/entities` (Task 3) can name it — not in `postretro`, which the catalog cannot reach); and an `apply_bloom_shot()` that sets `bloom_accumulator_degrees = min(bloom_max_degrees, bloom_accumulator_degrees + bloom_per_shot_degrees)` and resets `bloom_idle_ms = 0`. Call `tick_bloom` where cooldown already decays with dt in hand: `authorize_fire` (`crates/postretro/src/sim/weapon_stage/fire.rs`, `context.tick_dt`) on the host and `advance_client_fire_state` (`crates/postretro/src/weapon/mod.rs`, `frame_dt`) on the client. In `tick_resolved_component` (host) and `resolve_client_fire` (client), replace `stats.spread_degrees.to_radians()` with the composed angle: read the pawn's horizontal speed (`|velocity.xz|`, as `crates/foundation/src/movement/scope.rs` derives) and its authored run speed (`ground_params.speed.run`) from `PlayerMovementComponent` via `owner_pawn` + `registry` (both 0.0 when absent), call `effective_spread_degrees`, then tilt the aim axis upward before sampling (the "move the axis" mechanism): rotate the aim direction in world space about the camera-right axis `right = normalize(aim_dir × Vec3::Y)` by `(spread_vertical_bias * effective_degrees).to_radians()`, leaving the axis unchanged when `right` is degenerate (aim near ±Y) or `spread_vertical_bias` is 0; pass `effective_degrees.to_radians()` and the tilted axis to the unchanged `sample_cone_direction`, then call `apply_bloom_shot()` after `shells_fired` advances (read-before-grow, per the Ordering pins). Do **not** route the composed angle into the projectile launch path — only `fire_hitscan` / `resolve_client_hitscan` consume it, so projectiles stay unchanged (AC15). Add the six fields to `WieldableTuningPayload` (`crates/postretro/src/netcode/tuning_payload.rs`) and grep `WieldableTuningPayload {` across the workspace to populate them in every struct literal (test fixtures set representative values); source them in `tuning_payload_for_pawn` (`crates/postretro/src/netcode/mod.rs`) from the `WeaponComponent`, and copy+clamp them in `apply_net_wieldable_tuning` (`crates/postretro/src/scripting/builtins/net_descriptor.rs`), clamping each field to its own AC8 bound (`bloom_per_shot_degrees`, `bloom_max_degrees`, `movement_spread_degrees` to `0.0..=45.0`; `bloom_decay_degrees_per_second` and `bloom_decay_delay_ms` to `>= 0.0` with no upper clamp; `spread_vertical_bias` to `0.0..=1.0`) exactly as the `spread_degrees` clamp enforces its own bound (AC9); bump `TUNING_PAYLOAD_EPOCH` and re-bless / update `crates/postretro/src/netcode/tests/fixtures/tuning_payload.expected.json`. Extend the determinism coverage in `crates/postretro/src/sim/determinism_tests.rs` so a bloom-authored fixture weapon's sustained-burst fan is asserted bit-identical run-to-run and across spawn-order reversal (AC10). The existing `spawn_determinism_weapon` fixture is `FireMode::Semi` and `fixed_command_stream` fires isolated shots too far apart for bloom to accumulate — author bloom tuning on the fixture and drive a genuine sustained burst (`FireMode::Auto` or a trigger held across consecutive ticks) so the accumulator actually grows across the fan being compared. Delivers AC1–AC6, AC9, AC10, AC15. Consumes Task 1's fields; the integration point that falsifies the composition, determinism, and netcode assumptions.

### Task 3: Publish the `player.spread` HUD slot
Add a `player.spread` entry to `BUILTIN_ENGINE_STATE` in `crates/entities/src/engine_state_catalog.rs` mirroring the `player.reloadProgress` bounded-number precedent but with `network: ReplicationScope::None` (local-only, each machine publishes its own predicted spread; not replicated, no `StateSlotId`): `wire_name: "player.spread"`, `sdk_path: &["player", "spread"]`, `EngineStateValueType::Number`, default `0.0`, `range` `0.0..=MAX_EFFECTIVE_SPREAD_DEGREES`, `persist: false`, `EngineStateCapability::Readonly`. Add `"player.spread"` to the ordered slot-name assertion list in the same file (it sorts between `player.reloadProgress` and `player.weapon.current`); `player.*` slots are catalog-driven, so adding the `BUILTIN_ENGINE_STATE` entry is the registration and `crates/entities/src/slot_table.rs` needs no manual per-slot change. Publish it every tick on **every** network role from `crates/postretro/src/scripting/systems/ui_proxy.rs`, outside the host-authoritative suppression. `tick_for_role_and_report_sampled_weapon` returns early for a connected client after `publish_local_weapon_state`, and the ammo/reload writes in `tick_and_report_sampled_weapon` run on the host path only — so the `player.spread` write must **not** go there, or it stays `0.0` on co-op guests. Have `weapon_hud_values` (already resolves the local pawn via `registry.local_player_movement_pawn()` and the active `WeaponComponent` via `Inventory::active_wieldable`) also return the active weapon's effective spread — reading the pawn's horizontal speed (`|velocity.xz|`) and authored run speed (`ground_params.speed.run`) from `PlayerMovementComponent` and calling the same `effective_spread_degrees` method Task 2 added, `0.0` when there is no active weapon — and write it with `write_hud_slot("player.spread", SlotValue::Number(effective_degrees))` on the every-role path, beside `publish_local_weapon_state`, which runs in both the connected-client early-return branch and the host branch. Regenerate the affected typedef fixtures/`.d.ts`/`.d.luau` so `getGameState().player.spread` appears as a `ComputedRef<number>` (AC11). Delivers AC11. Consumes Task 2's `effective_spread_degrees`.

### Task 4: Assault-rifle reference weapon and pistol bloom tune
Author a new full-auto hitscan assault-rifle reference weapon and give the pistol a gentler bloom signature; leave the shotgun and projectile weapons untouched. Create `content/dev/scripts/reference-rifle.ts` following the `reference-shotgun.ts` / `reference-pistol.ts` pattern: a `defineEntity` named `reference_rifle`, `resolution: "hitscan"`, `fireMode: "auto"`, moderate damage and a fast `fireRateMs`, `range` ~80, `thirdPersonModel`/`viewmodel`/`mesh` = `models/cyberpunk_weapons/rifle/model.gltf`, an authored `muzzleOffset`, `touchable`, and a `resource` of `kind: "ammo"`, `type: "bullets.rifle"` (a new free-form ammo string — no registry to touch), a ~30 magazine, ~120 reserve, magazine reload. Author strong dynamic-spread tuning that visibly reads: sizable `bloomPerShotDegrees`, a high `bloomMaxDegrees`, a moderate `bloomDecayDegreesPerSecond`, a short `bloomDecayDelayMs`, a meaningful `movementSpreadDegrees`, and a positive `spreadVerticalBias` for upward climb (starting values in the Rough sketch and the Script syntax example — adjust for feel). Export `referenceRifleEntity`, add it to the `loadout` array in `content/dev/scripts/player.ts` and to the `entities` array in `content/dev/start-script.ts`. Retune `content/dev/scripts/reference-pistol.ts`: keep `fireMode: "semi"`, add a gentler bloom (smaller `bloomMaxDegrees` and faster `bloomDecayDegreesPerSecond` than the rifle, small `movementSpreadDegrees`, `spreadVerticalBias` 0) so rapid trigger-pulling widens its spread but recovers quickly. Update `content/dev/maps/combat-demo.README.md` to describe trying sustained fire on the rifle and rapid-firing the pistol to watch the spread ring open. This is content only; it depends on the field surface from Task 1 and the behavior from Task 2 to be exercised. Delivers AC13, AC14; with Task 2, completes AC3's "existing weapons unchanged" for the shotgun and projectiles.

### Task 5: Spread ring in the reticle
Make `hud.reticle` (`content/dev/scripts/hud.ts`) show a spread ring driven by `player.spread`. Keep a fixed inner aim mark, and add a second `Ring` whose `radius` binds the new slot: `radius: bindState(player.spread, { tween: { durationMs: <short>, easing: "easeOut" } })`, where `player` comes from `getGameState()` and `player.spread` is the `ComputedRef<number>` Task 3 exposes. Size the ring's fixed `diameter` (start ~72) large enough that the effective-spread range maps to a visible radius (the bind is 1:1 in pixels, runtime-clamped to `diameter/2`; the ring is a relative indicator, not a pixel-precise cone); at rest `player.spread` is 0, so confirm a bound radius of 0 renders cleanly (or floor the ring at a small minimum). Choose `fill`/`thickness` consistent with the existing reticle. The result: at rest the spread ring sits at its minimum, expands during sustained fire and while moving, and eases back as accuracy recovers (AC12). Delivers AC12. Consumes Task 3's `player.spread` slot.

## Sequencing

**Phase 1 (sequential):** Task 1 — data shape + authoring surface; blocks everything.
**Phase 2 (sequential):** Task 2 — composition + replication + determinism; the thin slice that falsifies the composition/determinism/netcode assumptions; consumes Task 1's fields and shares its files.
**Phase 3 (concurrent):** Task 3 (HUD slot — engine/SDK files), Task 4 (reference weapons — content files); both consume Task 2, disjoint files.
**Phase 4 (sequential):** Task 5 — ring binding; consumes Task 3's `player.spread` slot.

## Rough sketch

- Constant `MAX_EFFECTIVE_SPREAD_DEGREES` in `crates/entities` with `WeaponComponent` (engine ceiling, e.g. 45.0 — matches the static validation cap; the layering that forces this home is in Task 2). The movement reference speed is not an engine constant — it is the pawn's own authored `ground_params.speed.run`, so a designer who retunes movement automatically rescales weapon movement-spread and `movementSpreadDegrees` reads as "spread at full run."
- One implementation of decay/growth/compose as `WeaponComponent` methods (`tick_bloom`, `effective_spread_degrees`, `apply_bloom_shot`), called from both host and client paths — avoids the two-call-site drift between `tick_resolved_component` and `resolve_client_fire`.
- Axis tilt: the aim direction rotates upward about the world-space camera-right axis before `sample_cone_direction`, unchanged at bias 0 (full mechanics, including the near-vertical degenerate guard, in Task 2).
- Starting tune (content, adjust for feel): **rifle** `damage` ~9, `fireRateMs` ~110, `bloomPerShotDegrees` ~1.3, `bloomMaxDegrees` ~8, `bloomDecayDegreesPerSecond` ~14, `bloomDecayDelayMs` ~120, `movementSpreadDegrees` ~3, `spreadVerticalBias` ~0.3. **pistol** `bloomPerShotDegrees` ~1.6, `bloomMaxDegrees` ~4.5, `bloomDecayDegreesPerSecond` ~20, `bloomDecayDelayMs` ~90, `movementSpreadDegrees` ~1.5, `spreadVerticalBias` 0.
- Ring: a second `Ring` in `hud.reticle`; `diameter` ~72 so ~30° effective spread maps to a visibly large radius under the 1:1 px clamp.

## Boundary inventory

Weapon spread fields are descriptor-owned; they are **not** on the FGD/map KVP surface. Descriptor authoring is camelCase (JS/TS/Luau); the tuning payload is JSON keyed by Rust identifiers (snake_case).

| Rust field (`WeaponDescriptor` / `WeaponComponent`) | Descriptor authoring (TS/Luau) | Tuning payload JSON | FGD KVP |
|---|---|---|---|
| `bloom_per_shot_degrees` | `bloomPerShotDegrees` | `bloom_per_shot_degrees` | n/a |
| `bloom_max_degrees` | `bloomMaxDegrees` | `bloom_max_degrees` | n/a |
| `bloom_decay_degrees_per_second` | `bloomDecayDegreesPerSecond` | `bloom_decay_degrees_per_second` | n/a |
| `bloom_decay_delay_ms` | `bloomDecayDelayMs` | `bloom_decay_delay_ms` | n/a |
| `movement_spread_degrees` | `movementSpreadDegrees` | `movement_spread_degrees` | n/a |
| `spread_vertical_bias` | `spreadVerticalBias` | `spread_vertical_bias` | n/a |
| HUD slot | `getGameState().player.spread` | `"player.spread"` (`sdk_path` `["player","spread"]`) | n/a |

## Ordering pins

| # | Scenario | Ordering | Expected |
|---|---|---|---|
| 1 | First shot of a burst from rest | read accumulator (0) → sample → grow | Samples base `spreadDegrees`; exact axis if base 0 (AC1, AC3) |
| 2 | Sustained fire | per shot: grow after sampling, cap at `bloomMaxDegrees` | Each shot grows the accumulator toward the cap; while growth outpaces decay the cone widens shot-to-shot, up to `spreadDegrees + bloomMaxDegrees` (AC1; pin 5 is the pure-growth condition) |
| 3 | Fire, then idle | each tick: `bloom_idle_ms += dt`; decay only once `>= bloomDecayDelayMs` | Holds for the delay, then shrinks at the rate to 0 (AC2) |
| 4 | Fire + decay on one tick | gate decays (`tick_bloom`) → resolve reads → resolve grows | Shot sees decayed-but-not-yet-grown accumulator (AC1, AC2) |
| 5 | `bloomDecayDelayMs > fireRateMs` | no idle tick reaches the delay between shots | Pure growth to cap during sustained fire (AC1) |
| 6 | All new fields 0 (default) | compose returns exactly `spread_degrees` | Byte-for-byte parity with today; exact axis at base 0 (AC3) |
| 7 | Standstill vs moving | movement term = `movementSpreadDegrees × clamp(speed / pawn run speed, 0, 1)`; run speed `<= 0` or no movement component ⇒ 0 | 0 at rest; full at/above the pawn's run speed; adds to bloom (AC4) |
| 8 | Weapon switch mid-bloom | per-instance state persists; `tick_bloom` runs only while active | Stowed weapon's bloom freezes; resumes decay when re-drawn |
| 9 | Reload | `authorize_fire` still runs, so `tick_bloom` decays during reload | Accuracy recovers while reloading (AC2) |
| 10 | Sum exceeds ceiling | clamp to `MAX_EFFECTIVE_SPREAD_DEGREES` | Effective spread never exceeds the ceiling (AC6) |
| 11 | HUD sample on a firing tick | game logic: `tick_bloom` decays → cast reads accumulator → `apply_bloom_shot` grows; `ui_proxy` publishes after game logic | `player.spread` samples the **post-grow** accumulator — one `bloomPerShotDegrees` above the cone this tick's shot cast; the ring leads the cast by one shot by design (AC11, AC12) |
| 12 | Connected-client publish site | `tick_for_role_and_report_sampled_weapon` early-returns for a connected client, before the ammo/reload write block | `player.spread` is written on the every-role path (beside `publish_local_weapon_state`), never only in `tick_and_report_sampled_weapon`, so a co-op guest's ring tracks its own spread rather than holding 0 (AC11) |
| 13 | `bloomDecayDelayMs = 0` (default) | last shot resets `bloom_idle_ms = 0`; next idle tick `bloom_idle_ms += dt` satisfies `>= bloom_decay_delay_ms` | Decay begins the first idle tick after firing stops — no grace period at the default (AC2) |
| 14 | `bloomDecayDegreesPerSecond = 0` | accumulator grows to `bloomMaxDegrees`; `tick_bloom` decays by `0`; no reload/switch/`refresh_from_descriptor` zeroes it | Accumulator holds at the cap permanently — a valid never-recovers tune (AC8 permits the 0 rate) |

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Dynamic spread is a pure function of replay-stable weapon state (no wall-clock, frame-counter, or allocation-ordered id feeds the composed angle or the sampler seed) | Task 2 | `tick_bloom` must use the sim tick dt; movement reads the pawn's own deterministic velocity; seed stays `pellet_rng_seed(shells_fired, salt, slot)` | AC10 |
| Host and client compute the same effective-spread growth for a given weapon | Task 1 (mirrored tuning + preserved state), Task 2 (single shared methods, replicated tuning) | Two independent fire paths (`tick_resolved_component`, `resolve_client_fire`) must call the same `WeaponComponent` methods; tuning must replicate | AC9, AC10 |
| The host never re-derives a connected client's cone | (existing) `apply_valid_hit_record` LOS+range validation | No task adds angle/spread re-derivation to hit validation; dynamic-spread state never crosses the wire (only tuning does) | AC9 |
| Zero-default fields preserve exact current behavior | Task 1 (defaults 0), Task 2 (compose returns base at 0) | Compose/clamp must not perturb the axis when all dynamic terms are 0 | AC3 |
| `player.spread` reflects each machine's own predicted spread | Task 3 (`ReplicationScope::None`, published every role) | Must not be given a replicated scope, and must be published outside the host-authoritative early return | AC11 |

## Script syntax examples

```ts
// content/dev/scripts/reference-rifle.ts — full-auto hitscan AR
export const referenceRifleEntity = defineEntity({
  canonicalName: "reference_rifle",
  components: {
    weapon: {
      damage: 9.0,
      range: 80.0,
      fireRateMs: 110.0,
      fireMode: "auto",
      resolution: "hitscan",
      // dynamic accuracy
      bloomPerShotDegrees: 1.3,
      bloomMaxDegrees: 8.0,
      bloomDecayDegreesPerSecond: 14.0,
      bloomDecayDelayMs: 120,
      movementSpreadDegrees: 3.0,
      spreadVerticalBias: 0.3,
      thirdPersonModel: "models/cyberpunk_weapons/rifle/model.gltf",
      viewmodel: "models/cyberpunk_weapons/rifle/model.gltf",
      muzzleOffset: [0.0, 0.3, -0.9],
      resource: { kind: "ammo", type: "bullets.rifle", magazine: 30, reserve: 120, reloadMs: 1500, reloadStyle: "magazine" },
    },
    // mesh / touchable as in reference-shotgun.ts
  },
});
```

```ts
// content/dev/scripts/hud.ts — spread ring bound to player.spread
const { player } = getGameState();
export const reticle = defineUiTree({
  name: "hud.reticle",
  alwaysOn: true,
  tree: Tree(
    { anchor: "center", offset: [0.0, 0.0] },
    Ring({ diameter: 8.0, radius: 2.0, thickness: 2.0, fill: color.hud.text }), // fixed aim mark
    Ring({
      diameter: 72.0,
      radius: bindState(player.spread, { tween: { durationMs: 90, easing: "easeOut" } }),
      thickness: 2.0,
      fill: color.hud.text,
    }),
  ),
});
```

## Open questions

None outstanding. Two earlier items resolved against PostRetro's engine-neutral / script-shaped-feel identity: the movement term normalizes against the pawn's authored `ground_params.speed.run` (no hidden engine constant, no per-weapon field — see Direction and Task 2), and the reticle's degrees→pixel bind is a deliberate relative indicator whose richer script-authored mapping is the deferred UI computed-bindings layer (see Out of scope), not an engine-side pixel map. Content tuning of the rifle/pistol feel and the ring `diameter` is Task 4/5 work, not an unresolved decision.
