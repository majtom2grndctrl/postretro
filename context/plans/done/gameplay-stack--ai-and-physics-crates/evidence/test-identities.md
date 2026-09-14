# Test identity audit

One-time extraction audit. Generated test lists and Cargo output are not
retained. This summary preserves the revisions, method, normalization, hashes,
and differences needed to evaluate the result.

## Revisions and method

- Baseline: `ffff98fead791b2ee7dc502157c3b31dbdf7ea7d`
- Candidate: `7c18d11a060884c65887ae06659a08574150a7a6`
- Both source worktrees were clean and used separate external Cargo targets.
- Complete and ignored workspace test lists were captured from combined Cargo
  stdout/stderr so each libtest identity could be qualified by its runner.
- Runner artifact hashes were stripped. Normal and doctest targets remained
  distinct.
- Capture rejected missing runner context, duplicate target-qualified
  identities, dirty revisions, ignored identities outside the complete set,
  normalization collisions, and baseline-to-itself comparison.
- Sorted target-qualified baseline identities were normalized only by the
  reviewed extraction mappings below, then compared with the literal candidate
  identities. Missing identities or ignored-status changes failed the audit.

## Reviewed normalization

| Baseline identity prefix | Candidate identity prefix |
|---|---|
| `target::postretro_sim::scripting_systems::ai::` | `target::postretro_ai::` |
| `target::postretro_sim::kinematic_mover::commands::` | `target::postretro_sim::mover_commands::` |
| `target::postretro_sim::collision::` | `target::postretro_physics::collision::` |
| `target::postretro_sim::movement::` | `target::postretro_physics::movement::` |
| `target::postretro_sim::kinematic_mover::` | `target::postretro_physics::kinematic_mover::` |
| `target::postretro_sim::agent::tests::agent_capsule_half_height_builds_centered_parry_capsule` | `target::postretro_sim::agent::tests::agent_capsule_builds_engine_owned_collision_shape` |

The command relocation was applied before the broader kinematic-mover rule.
Normalization was baseline-only. No doctest mapping was allowed.

## Result

| Measure | Baseline | Candidate |
|---|---:|---:|
| Unqualified unique names | 7,246 | 7,254 |
| Target-qualified occurrences | 7,407 | 7,415 |
| Ignored target-qualified occurrences | 18 | 18 |

- Status: pass
- Missing identities: 0
- Added identities: 8
- Ignored-status changes: 0
- Baseline normalization collisions: 0
- Baseline ignored normalization collisions: 0

## Checksums

| Artifact | SHA-256 |
|---|---|
| Baseline snapshot manifest | `4c156db942be8490b5312db7b203348af4de4e639835326c6372c65bd122805a` |
| Candidate snapshot manifest | `22b7eddf6563b7afd6abc63004af03507cba58f776c5dc6bda877a58f7743684` |
| Baseline unqualified unique identities | `5558dfc222033782254fb915d70ccd6f26234d0c78c216c510689ad63ee4210a` |
| Candidate unqualified unique identities | `7eeb734d2397038eec80616ad7ee84ece0252e3b9966b5096b376a098a6508a5` |
| Baseline target-qualified identities | `afc17a07fe5641f1050f7b8081082ddd3ba9ca9a15a1a0bd6517e8694d82f616` |
| Candidate target-qualified identities | `d08cb525504e6173d671e3cc863d7e84439124ec956d9b9758d34098294202b3` |
| Normalized baseline target-qualified identities | `d5038802430106847fdbfd814eace2266581cbf9ef197462a2b2070c490e80c0` |
| Baseline and candidate ignored identities | `bc13418eaf608bf0a0bb906a9069aff94d78a9e6006d0949ae7ee76218fb12a5` |
| Added identities | `4b8fa6a371e781cd9edb17d09f96b1ac3e37bbe628b4547c2fb9c48c8697e9f3` |
| Empty missing/ignored-change/collision sets | `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855` |

## Added identities

- `target::postretro_ai::ai_tests::registry_exhaustion_rejects_projectile_attack_without_fire_side_effects`
- `target::postretro_ai::ai_tests::sim_tick_decays_sentiment_before_same_tick_ai_target_selection`
- `target::postretro_physics::collision::moving::tests::combined_ray_glam_adapter_preserves_hit_data`
- `target::postretro_physics::collision::tests::collision_world_is_thread_shareable`
- `target::postretro_physics::collision::tests::glam_shape_adapters_preserve_toi_and_normal`
- `target::postretro_sim::nav::tests::nav_graph_is_thread_shareable`
- `target::postretro_sim::sim::determinism_tests::no_locomotion_graph_restores_authored_playback_rate_through_sim_tick`
- `target::postretro_sim::sim::determinism_tests::test_tick_runner_nav_bridge_preserves_section_and_path_queries`

The full generated lists and raw Cargo streams were intentionally omitted from
Git history. The summary records their checksums but does not claim to replace
the deleted byte-for-byte artifacts.
