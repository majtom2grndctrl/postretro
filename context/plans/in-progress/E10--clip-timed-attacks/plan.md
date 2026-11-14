# E10--clip-timed-attacks — plan of record

mode: compact
status: active
read at: eb444995a

## Corrections
- Path is silent on how the AI tick reaches clips → model clips live only in sim's `HitZoneStore`, which `AiTickInputs` does not carry; planning around it by adding the store to `AiTickInputs` and reading a state's clip (duration, events) through `ModelHitZones::state_clip`, the lookup the playback-rate and jump one-shot paths already share.
- `ActionVerb::Attack(String)` cannot carry a sibling `at` key in its externally tagged serde shape → `ActionVerb::Attack` becomes a struct variant `{ name, at }` with a hand-written `{ attack, at? }` (de)serializer; every `Attack(name)` match updates in the same pass (pre-release API policy).
- "A switch of the mesh off the path's animation … cancels the pending moment", including a switch off and back between AI ticks (P8), is not observable from `current_state` alone → `MeshAnimation` gains a runtime-only switch counter bumped by `switch_animation_state` on `Switched`; the brain records the count its own apply produced, and compute cancels the open moment when the mesh count moved.

- `context/lib/scripting.md` §11 has `animationDone` appending after `activityRoll` and `damageInActivity`, neither built → it appends after `hasAttackTurn` (slot 21); the stances brief's facts append after it. Fix the line at landing.
- Decision "Load rejects `at` … on an activity whose path selects no animation" → unreachable: only leaves author `action`, every leaf requires `animation`, and an action-bearing leaf is never a locomotion leaf, so its own animation is selected. Proven by `action_at_on_a_leaf_without_an_animation_rejects` (the existing leaf rule rejects); no dead check added.

- Design note "reuse the tracked name's `String`" → the brain clones twice per enemy per tick, so a `String` would allocate per tick; the tracked state is an `Arc<str>`, allocated only on restart. A fresh clock binds to the drawn state on its first tick without advancing, so `animationDone` does not read true on a fresh brain.
- Switch counter equality → `MeshAnimation.switches` is a `SwitchCount` whose `PartialEq` is always true, so component equality and the held-enemy check are unaffected. It and the brain's recorded count serialize (`serde(default)`) so a restore behaves as an unsaved run; replication sends only the state name (`WireMeshAnimationState`, bitcode, hand-built), and no digest reads it.
- Pickup/drop replaces an enemy's mesh → the drop marks it (`note_replaced_mesh`: the brain records the fresh count and requests a redraw); apply redraws the drawn state and clip time restarts from the redraw. P12's "fires on clip time" becomes clip time from the redraw at release.

- Brute slam with one chance at 767 ms (was a retried fire at 250 ms) missed on facing while the commit kept chasing a slot → the slam's windup, commit and recover hold position (`motion: "hold"`); content judgment within the brief's timer latitude, flagged for the owner's playtest.
- Limitator burst: the authored 240 ms between three shots runs to 267 ms on whole ticks → `BURST_MS` 600 covers a full 16-tick `firing_rifle` pass plus the burst with four ticks spare.
- `ghost_link_coop_return_reunites_and_starts_the_final_wave` failed with the new limitator timing: a closet limitator's shot turns a brute on it, 3 units short of the closet line → the closet assertion exempts a brute whose target is a monster, extending the crossfire allowance the test's doc already states.
- Shipped-content tick tests ran the brute on a looping fixture mesh, so `animationDone` never turned true → they now load the descriptor's real mesh and clip library.

- Review: clip time restarts when the brain's own switch redraws its tracked state after a foreign switch moved the mesh off it. This applies the Decision's "time since the animation its path selects began playing"; P11's keep-running holds while the mesh stays off.
- Review: a re-entered attack's moment could meet its cooldown and whiff while holding a turn → brute jab `cooldownMs` 900 (below `JAB_REST_MS` + `hit` = 917 ms); limitator `WAIT_MS` = rifle recovery 1000 + three-shot span 267 = 1267 ms (a started burst finishes its shots after `burst` leaves, each restarting the recovery). Pacing change for the owner's playtest: first limitator burst ~1.3 s after reaching range, steady cycle longer.
- Review: a swing entered while a launch is still under way is cancelled: knockback velocity remains (a slide), or the body is in the airborne stretch a launch began after drag spent it (P15) — kept, flagged for the owner.

## Delegated answers
- Event name rules beyond non-empty and unique — a name matches `[A-Za-z_][A-Za-z0-9_]*`, case-sensitive; the converter rejects others, the loader treats them as malformed (warn, no events), and descriptor load rejects an `at` outside the rule. It keeps names valid as script string literals and unambiguous in log lines, and an `at` that cannot match any event is a typo worth rejecting at load.

## Design notes (for resumption)
- **Clip clock on `BrainComponent`** (serialized with the activity clocks, P13): tracked drawn state (path-selected state plus presented travel variant), unwrapped clip time and the previous tick's time (f64), and runtime-only cached clip facts (duration, looping, resolved) rebound lazily after a restore.
- **Advance** at the top of `compute::evaluate`, beside `tick_activity_timers`: `prev = time; time += dt × rate`, rate = the mesh's `rate` when the mesh plays the tracked state, else 1 (P2). `brain.animationDone` = non-looping and `prev ≥ duration` (P4), or clip unresolved; false on a loop.
- **Rebind** after heading resolve, before the Attack block: when the path-selected state or the presentable travel variant differs from the tracked one, restart (time = 0, or `instance_phase(id, duration)` on a loop; prev just below start so a moment at the start counts as reached).
- **Moment reached** = crossing `prev < event ≤ time` (with whole-tick epsilon; loops test every pass `event + k·duration`). Checked only while the active action carries `at` and the per-entry moment latch is open; reaching it consumes the latch whatever the gates say (whiff). Latch opens on entry of the activity owning the action.
- **Cancel** (closes the latch): mesh switch counter moved since the brain's own last switch (checked before transitions, so an entry this tick is untouched — P6), or a knockback launch (`launch.impact_yaw` set after heading resolve — P15).
- **Implied mark**: descriptor normalization sets `attack_turn` from an activity's own `action.at` before `validate_attack_turn_marks`, so mark agreement is the existing rule.
- **Missing event**: warn once per (entity class, state) when `BrainPrograms::sync` first binds an entity; the runtime then fires as without `at`.

## AC-to-proof

| AC | Proof | Status | Result |
|---|---|---|---|
| Conv 1 `--event punch:hit=620` writes, other clip only | `tools/tests/test_mixamo_to_gltf.py` event write tests | achievable as stated | pass — `ClipEventTests` (converter 99/99) |
| Conv 2 range trim carries/shifts; windup none; start-key rounding | `test_mixamo_to_gltf.py` trim-carry tests | achievable as stated | pass — `ClipEventTests` trim-carry tests |
| Conv 3 past-end, duplicate, `--clip-library` reject naming flag | `test_mixamo_to_gltf.py` reject tests | achievable as stated | pass — `ClipEventTests` / `ConverterArgumentTests` rejects |
| Conv 4 suite passes unchanged across split | suite run on the split commit, before event code | achievable as stated | pass — 83/83 on d6ca8b3ea before event code |
| Load 1 `mixamo_brute` → `punch_strike` shifted `hit` | `crates/model` library binding test on the committed library | achievable as stated | pass — `reference_enemy_model_binds_its_clip_library` |
| Load 2 own same-named clip keeps own events or none | `crates/model` clip_library test | achievable as stated | pass — `embedded_override_keeps_its_own_events_or_none` |
| Load 3 malformed events warn once, keys load | `crates/model` gltf_clips test with log capture | achievable as stated | pass — `malformed_clip_events_warn_once_and_keep_the_keys` |
| Load 4 event with `end` loads name and time | `crates/model` gltf_clips test | achievable as stated | pass — `read_clip_events_loads_name_and_time_and_ignores_span_end` |
| Fire 1 0.5 s → tick 30, 0.51 s → 31, no `at` → entry | `crates/ai` clip-timed tests | achievable as stated | pass — `ai_tests::clip_timed::fire1_*` |
| Fire 2 event at 0 fires on entry | `crates/ai` clip-timed tests | achievable as stated | pass — `ai_tests::clip_timed::fire2_*` |
| Fire 3 out of range whiff, fresh entry fires | `crates/ai` clip-timed tests | achievable as stated | pass — `ai_tests::clip_timed::fire3_*` |
| Fire 4 no turn whiff; grant on moment tick fires | `crates/ai` clip-timed tests (arbitration) | achievable as stated | pass — `ai_tests::clip_timed::fire4_*` |
| Fire 5 leaving before moment / exit on moment tick | `crates/ai` clip-timed tests | achievable as stated | pass — `ai_tests::clip_timed::fire5_*` |
| Fire 6 last-tick event fires, `animationDone` next tick | `crates/ai` clip-timed tests | achievable as stated | pass — `ai_tests::clip_timed::fire6_*` |
| Fire 7 shared non-looping clip past / reaching on entry | `crates/ai` clip-timed tests | achievable as stated | pass — `ai_tests::clip_timed::fire7_*` |
| Fire 8 two activities on one state | `crates/ai` clip-timed tests | achievable as stated | pass — `ai_tests::clip_timed::fire8_*` |
| Fire 9 looping past moment → next pass; variant restart at phase | `crates/ai` clip-timed tests | achievable as stated | pass — `ai_tests::clip_timed::fire9_*` |
| Fire 10 impact switch cancels, off-and-back too | `crates/ai` clip-timed tests | achievable as stated | pass — `ai_tests::clip_timed::fire10_*` |
| Fire 11 impact before AI on entry tick / after compute on moment tick | `crates/ai` clip-timed tests | achievable as stated | pass — `ai_tests::clip_timed::fire11_*` |
| Fire 12 knockback launch cancels | `crates/ai` clip-timed tests | achievable as stated | pass — `ai_tests::clip_timed::fire12_*` |
| Fire 13 travel variant switch restarts | `crates/ai` clip-timed tests | achievable as stated | pass — `ai_tests::clip_timed::fire13_*` |
| Fire 14 rate 1.5 → 2/3 wall time, previous tick's rate | `crates/ai` clip-timed tests | achievable as stated | pass — `ai_tests::clip_timed::fire14_*` |
| Fire 15 loop fires once per entry, phases differ | `crates/ai` clip-timed tests | achievable as stated | pass — `ai_tests::clip_timed::fire15_*` |
| Fire 16 weapon attack starts at the moment | `crates/ai` clip-timed tests | achievable as stated | pass — `ai_tests::clip_timed::fire16_*` |
| Fire 17 missing event warns once, fires on entry | `crates/ai` clip-timed tests with log capture | achievable as stated | pass — `ai_tests::clip_timed::fire17_*` |
| Fire 18 implied mark holds turn from entry | `crates/ai` clip-timed tests (arbitration) | achievable as stated | pass — `ai_tests::clip_timed::fire18_*` |
| Fire 19 mark agreement and load rejects | `crates/foundation` behavior descriptor tests | achievable as stated | pass — foundation `clip_timed_actions` tests |
| Fire 20 same ticks at 1/2/4 ticks per frame, time scale 1/0.25 | `crates/sim` determinism test | achievable as stated | pass — sim `clip_timed_attacks_fire_on_the_same_ticks_at_any_frame_split_and_animation_time_scale` |
| End 1 `animationDone` false through duration tick, true next; loop false; unresolved true | `crates/ai` clip-timed tests | achievable as stated | pass — `ai_tests::clip_timed::end1_*` |
| End 2 non-loop → non-loop entry reads false, moment fires | `crates/ai` clip-timed tests | achievable as stated | pass — `ai_tests::clip_timed::end2_*` |
| End 3 cancelled moment, clip end still turns true | `crates/ai` clip-timed tests | achievable as stated | pass — `ai_tests::clip_timed::end3_*` |
| End 4 parent and leaf guards read one value | `crates/ai` clip-timed tests | achievable as stated | pass — `ai_tests::clip_timed::end4_*` |
| Surf 1 example lowers identically TS/Luau, runs | `crates/scripting-core` surface example test + `crates/ai` fixture | achievable as stated | pass — `clip_timed_surface_example_lowers_identically_in_both_runtimes` + `surf1_the_surface_example_jab_fires_at_hit_and_leaves_on_animation_done` |
| Surf 2 brute jab/slam at `hit`, limitator at `kick` | shipped-content test over `content/dev` scripts and libraries | achievable as stated | pass — `shipped_brute_jab_and_slam_fire_at_the_punch_hit`, `shipped_limitator_first_burst_shot_fires_at_the_rifle_kick` |
| Surf 3 limitator whiff leaves and frees turn; fire leaves after third shot | shipped-content test | achievable as stated | pass — `shipped_limitator_whiffing_its_kick_leaves_burst_and_frees_its_turn`, `shipped_limitator_leaves_burst_after_its_third_shot_at_every_phase` |
| Surf 4 grep gate: no wire/version/digest change | `git diff origin/main` over net/netcode/digest sources, empty | achievable as stated | pass — `crates/net` untouched; `netcode` diff is a test literal and the `ActionVerb` shape in a harness test; no digest change |
| Man 1 re-export reproduces libraries apart from events | owner, with source FBXs | manual | outstanding — owner, with source FBXs (events were stamped via `stamp_events_in_library`; a reviewer confirmed in memory that stamping reproduces the committed events from the `license.txt` flags) |
| Man 2 single player timing | owner, in-engine | manual | outstanding — owner, in-engine (note: limitator first burst ~1.3 s after reaching range; brute slam holds position) |
| Man 3 loopback co-op timing, no new warnings | owner, in-engine | manual | outstanding — owner, loopback co-op |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Converter: split post-processing into a sibling module, behavior-preserving, own commit | worker (Python) | — | done: d6ca8b3ea, 83/83 before and after |
| 2 | Converter: `--event`, trim carry, rejects, tests, `docs/character-animation.md` | worker (Python) | 1 | done: `tools/mixamo_clip_events.py`, 97/97 |
| 3 | Loader: `AnimationClip` events, `parse_clip` extras, library carry and override, malformed warn | integrating executor | — | done: `crates/model/src/clip_events.rs`; model lib 220 pass. Libraries stamped via `stamp_events_in_library` (content part of task 7) |
| 4 | Thin slice: clip clock, `at` on contact attacks, `AiTickInputs` clip access, jab fixture on `punch` | worker (Rust) | 3 | done with 6 |
| 5 | Descriptor and surface: `ActionVerb` `at`, implied mark, load rejects, `animationDone` fact, TS/Luau types and prelude, typedef | worker (Rust) | — | done: `clip_timed_actions.rs`; foundation 197, scripting-core 1050, ai 367 pass |
| 6 | Runtime completion: cancellation, loops and phase, travel variants, weapon `at`, missing-event warn, rate | worker (Rust) | 4, 5 | done: `ai/src/clip_clock.rs`, `entities/.../brain/clip_clock.rs`; 28 clip-timed tests (`ai_tests::clip_timed`), ai 395, entities 315, sim lib 1980, netcode lib 1023 pass |
| 7 | Content: events in `mixamo_brute`/`mixamo_rifle`, brute and limitator scripts, `license.txt`, script comment | worker (content) | 2, 6 | done: jab exit and slam recover on `animationDone`; slam windup 467 ms, commit 400 ms; `BURST_MS` 600 |
| 8 | Remaining AC tests (determinism, shipped content, surface example), docs | worker (content) | 7 | done: `clip_timed_shipped_tests.rs` (4), sim determinism Fire 20; ai 399, sim lib 1981, postretro bin ghost_link 143 |

## Review loop
- Round 1 (8 reviewers: 3 tracers, 2 contract verifiers, 1 adversarial, 2 hygiene): no 🔴; 8 🟡 fixed in 157e15068 — own switch masking a same-batch flinch, redraw vs clip time, held pickup/drop phantom cancel, restore losing a flinch, latch tied to the per-tick action, spent moment firing through an untimed variant, `animationDone` lag on entry, content cooldowns meeting the one moment. Nits and comment drift fixed alongside; `context/lib` drift left for landing.
- Round 2 (tracer + adversarial on the fixes): 1 🟡 (save around a drop) and nits, fixed in e05ae5cbe — switch counts serialize, the drop marks the replaced mesh and requests a redraw.
- Round 3 (tracer on the drop path): 3 🟡 fixed in 8c0ff8e92 — a flinch drops a pending redraw, the clock holds until the redraw lands, drop tests use production index timing.
- Stopped after round 3: its findings were confined to the drop path and are pinned by tests that fail with the fixes reverted.
- Final gate: `cargo test --workspace --features postretro/dev-tools` — 91 targets, 15,936 passed, 0 failed. `/preflight` is owner-invoked and has not run.

## Owner calls and follow-ups
- Owner call: a swing entered while a launch is still under way (a knockback slide, or the airborne stretch after drag spent it) is cancelled. Kept.
- Owner call: a brute flinched mid-slam redraws `attack_slam` on `recover` and replays the punch, holding the slam turn ~1 s longer (redraw restarts clip time). Latent: shipped content never flinches the brute. Decide before the stances brief builds on the shared-clip slam.
- Owner call: an entry whose first drawn clip is an untimed travel variant fires as without `at`, even after a flinch or launch (consistent with "a missing event fires as without `at`").
- Follow-up: `crates/ai/src/compute.rs` is past the 600-line guideline; split the Attack block out.
- Follow-up: a slam started after a target switch could in theory meet its 1,800 ms cooldown at the commit moment.
- Follow-up: script-reaction animation switches drain once per frame, so their cancel lands on the next frame's first tick; frame splits can change whether such a cancel catches a moment (reaction cadence, not covered by Fire 20).
- Known limit: `MeshAnimation.rate` is not serialized, so the first tick after a restore advances at rate 1. No save-game system exists yet; restore paths are exercised by tests only.
