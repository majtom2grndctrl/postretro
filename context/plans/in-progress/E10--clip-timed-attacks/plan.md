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
- Switch counter equality → `MeshAnimation.switches` is a `SwitchCount` whose `PartialEq` is always true and is `serde(skip)`, so component equality, round-trips and the held-enemy check are unaffected; replication sends only the state name.

- Brute slam with one chance at 767 ms (was a retried fire at 250 ms) missed on facing while the commit kept chasing a slot → the slam's windup, commit and recover hold position (`motion: "hold"`); content judgment within the brief's timer latitude, flagged for the owner's playtest.
- Limitator burst: the authored 240 ms between three shots runs to 267 ms on whole ticks → `BURST_MS` 600 covers a full 16-tick `firing_rifle` pass plus the burst with four ticks spare.
- `ghost_link_coop_return_reunites_and_starts_the_final_wave` failed with the new limitator timing: a closet limitator's shot turns a brute on it, 3 units short of the closet line → the closet assertion exempts a brute whose target is a monster, extending the crossfire allowance the test's doc already states.
- Shipped-content tick tests ran the brute on a looping fixture mesh, so `animationDone` never turned true → they now load the descriptor's real mesh and clip library.

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

| AC | Proof | Status |
|---|---|---|
| Conv 1 `--event punch:hit=620` writes, other clip only | `tools/tests/test_mixamo_to_gltf.py` event write tests | achievable as stated |
| Conv 2 range trim carries/shifts; windup none; start-key rounding | `test_mixamo_to_gltf.py` trim-carry tests | achievable as stated |
| Conv 3 past-end, duplicate, `--clip-library` reject naming flag | `test_mixamo_to_gltf.py` reject tests | achievable as stated |
| Conv 4 suite passes unchanged across split | suite run on the split commit, before event code | achievable as stated |
| Load 1 `mixamo_brute` → `punch_strike` shifted `hit` | `crates/model` library binding test on the committed library | achievable as stated |
| Load 2 own same-named clip keeps own events or none | `crates/model` clip_library test | achievable as stated |
| Load 3 malformed events warn once, keys load | `crates/model` gltf_clips test with log capture | achievable as stated |
| Load 4 event with `end` loads name and time | `crates/model` gltf_clips test | achievable as stated |
| Fire 1 0.5 s → tick 30, 0.51 s → 31, no `at` → entry | `crates/ai` clip-timed tests | achievable as stated |
| Fire 2 event at 0 fires on entry | `crates/ai` clip-timed tests | achievable as stated |
| Fire 3 out of range whiff, fresh entry fires | `crates/ai` clip-timed tests | achievable as stated |
| Fire 4 no turn whiff; grant on moment tick fires | `crates/ai` clip-timed tests (arbitration) | achievable as stated |
| Fire 5 leaving before moment / exit on moment tick | `crates/ai` clip-timed tests | achievable as stated |
| Fire 6 last-tick event fires, `animationDone` next tick | `crates/ai` clip-timed tests | achievable as stated |
| Fire 7 shared non-looping clip past / reaching on entry | `crates/ai` clip-timed tests | achievable as stated |
| Fire 8 two activities on one state | `crates/ai` clip-timed tests | achievable as stated |
| Fire 9 looping past moment → next pass; variant restart at phase | `crates/ai` clip-timed tests | achievable as stated |
| Fire 10 impact switch cancels, off-and-back too | `crates/ai` clip-timed tests | achievable as stated |
| Fire 11 impact before AI on entry tick / after compute on moment tick | `crates/ai` clip-timed tests | achievable as stated |
| Fire 12 knockback launch cancels | `crates/ai` clip-timed tests | achievable as stated |
| Fire 13 travel variant switch restarts | `crates/ai` clip-timed tests | achievable as stated |
| Fire 14 rate 1.5 → 2/3 wall time, previous tick's rate | `crates/ai` clip-timed tests | achievable as stated |
| Fire 15 loop fires once per entry, phases differ | `crates/ai` clip-timed tests | achievable as stated |
| Fire 16 weapon attack starts at the moment | `crates/ai` clip-timed tests | achievable as stated |
| Fire 17 missing event warns once, fires on entry | `crates/ai` clip-timed tests with log capture | achievable as stated |
| Fire 18 implied mark holds turn from entry | `crates/ai` clip-timed tests (arbitration) | achievable as stated |
| Fire 19 mark agreement and load rejects | `crates/foundation` behavior descriptor tests | achievable as stated |
| Fire 20 same ticks at 1/2/4 ticks per frame, time scale 1/0.25 | `crates/sim` determinism test | achievable as stated |
| End 1 `animationDone` false through duration tick, true next; loop false; unresolved true | `crates/ai` clip-timed tests | achievable as stated |
| End 2 non-loop → non-loop entry reads false, moment fires | `crates/ai` clip-timed tests | achievable as stated |
| End 3 cancelled moment, clip end still turns true | `crates/ai` clip-timed tests | achievable as stated |
| End 4 parent and leaf guards read one value | `crates/ai` clip-timed tests | achievable as stated |
| Surf 1 example lowers identically TS/Luau, runs | `crates/scripting-core` surface example test + `crates/ai` fixture | achievable as stated |
| Surf 2 brute jab/slam at `hit`, limitator at `kick` | shipped-content test over `content/dev` scripts and libraries | achievable as stated |
| Surf 3 limitator whiff leaves and frees turn; fire leaves after third shot | shipped-content test | achievable as stated |
| Surf 4 grep gate: no wire/version/digest change | `git diff origin/main` over net/netcode/digest sources, empty | achievable as stated |
| Man 1 re-export reproduces libraries apart from events | owner, with source FBXs | manual |
| Man 2 single player timing | owner, in-engine | manual |
| Man 3 loopback co-op timing, no new warnings | owner, in-engine | manual |

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
