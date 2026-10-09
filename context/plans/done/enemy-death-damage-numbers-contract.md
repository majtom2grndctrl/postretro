# Enemy death and damage-number baseline — contract

## Goal

In the dev mod, every damageable enemy shows a floating damage number on each hit and dies at zero HP: it plays its `death` clip, lingers, and despawns. Today both behaviors come from the dev mod's `enemy-death` impact policy, which matches only entities tagged `enemy`. Many placements and every spawner omit that tag, so those enemies freeze mid-pose at zero HP and show no numbers. Separately, each shotgun pellet spawns its own number at the target's origin, and the dev template's sub-pixel rise and scatter values stack all eight numbers into one. Numbers should appear where each pellet hit.

## Decisions

- **Reach is content tagging, not a new filter.** Every damageable enemy placement in `content/dev/maps/` carries `enemy` in `_tags`, and every `entity_spawner` that releases enemies carries `enemy` in `spawned_tags`. The stress-map generator emits both. Consequence: the `enemy-death` policy and its filter stay unchanged, and the SDK and manifest wire do not change. Any future map or spawner that omits the tag regresses, and the guard test below catches that for `content/dev/maps/`.
- **The combat-demo zombie override stays.** `combatZombieLifecycle` (tags `enemy combat-zombie`) keeps its resurrection and `-3` overkill gib. The dummies' `dummy`-tag lifecycle and `ammo-on-kill` are untouched.
- **`present()` anchors at the hit point by default.** When an impact carries a contact point, the presentation spawn's `world_anchor` is that point. Otherwise it falls back to the target's `Transform.position`, which is today's behavior. No new template field. Consequence: any mod's `present()` numbers move to the contact point, and the change is host-side only, because the co-op wire already carries the anchor.
- **Template motion values are device pixels.** The engine already reads `motion.rise` and `spawnScatter.radius` as device pixels. The dev template's `0.45` and `0.12` were authored as if they were metres. The dev template moves to `rise: 40` and `spawnScatter.radius: 8`, and the SDK documents the unit.
- **campaign-test's six characters stay props.** They are mesh-only descriptors with no health, so they cannot be damaged. They are out of scope.

## Invariants

- **Field names and type.** `DamageContext.point: Option<glam::Vec3>` and `ImpactDispatch.point: Option<glam::Vec3>`. The point is in world space, in engine coordinates (metres, Y-up), at the exact ray or contact point with no offset applied. It is data for the evaluator only. It never becomes an IR input or a command-target token, and `ImpactDispatch::ir_values` stays four entries.
- **`DamageContext::new` sets `point: None`.** Producers that set `Some`:
  - **Player hitscan:** `WeaponImpact.point`, once per pellet.
  - **Projectile direct contact:** the contact point.

  These stay `None`: splash (each receiver is not at the blast centre), script and reaction damage, mover crush, and AI contact or melee. AI hitscan may set it when the point is already in hand. That is optional.
- **Anchor rule.** Lives in `ImpactPolicyRuntime::apply_presentation_spawn`. It uses `dispatch.point` when the point is `Some` and finite, and otherwise the live target's `Transform.position`. A target with no Transform and no point still skips with today's warning. A target with no Transform but a finite point spawns at the point.
- **The wire and the mod digest do not change.** Nothing under `crates/net`, `crates/netcode` or `mod_digest.rs` changes.
- **Tag spelling.** The tag is exactly `enemy`, lowercase. In `.map` KVPs, multiple tags are space-separated, and existing tags are kept: `"_tags" "enemy closet"`, `"spawned_tags" "closet enemy"`.
- **Damageable enemy classnames** (descriptors with both `health` and `behavior`): `reference_enemy`, `limitator`, `crossfire_raider`, `crossfire_sentinel`, `faction_sentiment_cabal`, `faction_sentiment_resistance`, `positional_sound_grunt`, `pose_fixture_enemy`.

## Tracks and file ownership

**Engine track, `sonnet`.** Owns `crates/entities`, `crates/sim`, and `crates/ai` only if AI hitscan is plumbed. Also owns the `gen-script-types` source and its generated `sdk/types/postretro.d.ts` and `.d.luau`, for the unit doc only. Compile-forced spillover elsewhere, such as test struct literals, is allowed and must be reported.

**Content track, orchestrator.** Owns:
- `content/dev/maps/*.map` and `content/dev/scripts/combat-presentation.ts`
- `content/dev/maps/*.README.md`
- `tools/gen_stress_map.py` and `tools/tests/`
- `sdk/lib/ui/presentation.ts` and its `.luau` twin, if one exists (comments only)
- `context/lib/`

## Acceptance

Engine:
- `cargo test -p postretro-sim --lib impact_policy` passes, with a nonzero count. It includes new tests for three cases:
  - A dispatch with a point anchors the spawn at that point.
  - A dispatch without a point anchors at the target position.
  - A non-finite point falls back to the target position.
- `cargo test -p postretro-sim --lib weapon_stage` passes, with a nonzero count. It includes a test showing a hitscan pellet's `ImpactDispatch.point` equals its `WeaponImpact.point`.
- `cargo test -p postretro-entities --lib health` passes.
- `git diff --stat main -- crates/net crates/netcode crates/postretro/src/mod_digest.rs` is empty.

Content:
- `python3 -m unittest discover -s tools/tests -p 'test_*.py'` passes. It includes:
  - A generator test: every emitted `reference_enemy` carries `enemy`, and every emitted spawner's `spawned_tags` contains `enemy`.
  - A dev-map guard: every placement of a listed classname, and every `entity_spawner`, in `content/dev/maps/*.map` carries the tag.
- `git diff main -- content/dev/maps/*.map` touches only `_tags` and `spawned_tags` lines.

## Open

- **Not tested:** whether eight numbers at a one-metre point-blank range read cleanly. The pellets land about 7 cm apart. The question is whether natural spread plus 8 px of scatter separates them. This needs a manual check in play.

## Results

- Every acceptance command passed:
  - Python tool tests: 22 OK.
  - `impact_policy`: 55. `weapon_stage`: 109. `projectile_stage`: 24.
  - Entities `health`: 27. `typedef`: 39.
  - The frozen-path diff is empty.
  - Map diffs touch only tag lines.
- One review-and-fix round found zero blockers.
- In the full workspace preflight, format, clippy, `cargo check --release` and `crate-graph --check` came back clean. `cargo test --no-fail-fast` had one failure, `ui_slot_snapshot_clones_present_values_and_skips_valueless_slots`, which counts 60 slots against an expected 58. That failure predates this work: no change here touches UI slots.
- Still needs a manual check in play: whether eight point-blank pellet numbers read cleanly.
- Known gap: a connected client's hitscan or projectile claim supplies a point the host checks for range, finiteness and, for hitscan, line of sight, but never against the target's volume. A dishonest client can therefore misplace only its own damage numbers. The fix belongs in `crates/netcode`.
