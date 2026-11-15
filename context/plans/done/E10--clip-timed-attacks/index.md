# E10--clip-timed-attacks

Brief · compact · Epic 10 · reads: `context/lib/entity_model.md` §7c, `context/lib/scripting.md` §2 §11, `context/lib/resource_management.md` §7, `context/lib/networking.md` §Current contract · read at 919d34a05

## Problem
Owner request (roadmap, Epic 10): an enemy attack lands on a moment named in its clip. Today an attack fires on the first tick after its activity's entry on which its gates pass, so the hit lands when the activity starts, not when the clip strikes. The reference brute's jab deals damage on entry while `punch` still has its fist drawn back at 467 ms, and its slam waits on a hand-tuned 250 ms timer over the same clip; scripts time attacks with `timeInActivityMs`, so clip and hit drift apart whenever either changes. When done, a clip carries named moments shared by every character on its library, an attack fires at one, an enemy that is out of reach at that moment misses, and a graph can leave a swing when its clip has played through. The brute's jab and slam land as the fist connects. The stances brief (`ready/E10--enemy-stances-and-telegraphs`) builds its releases on this.

## Decisions
- **Events live in the clip, authored only there.** A glTF animation's `extras.events` is a list of `{ name, time }`, time in seconds from the clip's start, with room for an `end` field so span events land without a format break. A moment belongs to the motion, and every character bound to a library shares it (`resource_management.md` §7, shared clip libraries). Names are unique within a clip; `0 ≤ time ≤ duration`.
- **The converter writes events, and cuts keep them.** `mixamo_to_gltf.py --event SOURCE:NAME=MS` attaches an event to a source clip before trims run, in milliseconds like `--trim`. `--trim` copies each event inside its kept range, shifted by the cut's actual start (rounded down to a key, as today); a windup cut ending before the moment carries none. An event at a range's start belongs to that range. The converter rejects an event outside its clip, a duplicate name and `--event` with `--clip-library`, as it does `--trim`.
- **The model loader carries events.** A loaded clip holds its events, so library clips bring them to every bound character; a character's own clip of the same name replaces the library clip whole, events included. A malformed `extras.events` warns and the clip loads without events (`resource_management.md` §7, a malformed `clipLibrary` warns).
- **Clip time runs on the game tick, not the animation clock.** An enemy's clip time is the time since the animation its path selects began playing, accumulated per AI tick and scaled by that state's playback rate. Activities sharing an animation state share its clip, as switching to the playing state does not restart it today. A looping state's clip time starts at the enemy's drawn phase, the per-instance offset its renderer and hit zones use, and wraps. A travel variant switch restarts clip time with the drawn clip, and the moment reads the variant clip's event (owner, 2026-11-13). Attack arbitration must stay deterministic (`entity_model.md` §7c, Arbitration), and the animation clock is not. This departs from one animation clock for all timing (`done/M10--skinned-animation-runtime`, `done/M10--skeletal-hit-zones`, `rendering_pipeline.md` Pose consumers): presentation leads or trails the moment by up to a frame, and without bound under the dev animation time scale.
- **`action.at` fires the attack at a named event.** The attack fires on the tick its clip time reaches the event's time, checked after that tick's transitions as attacks are today; that tick is its one chance per entry. Every existing gate still applies there — range, sight, facing, cooldown, turn — and a gate that fails is a whiff: nothing fires later in that entry (owner, 2026-11-13). An entry that leaves before the moment fires nothing, and so does one entering a non-looping clip already past it; a looping clip fires once per entry, at the next pass. A knockback launch, or a switch of the mesh off the path's animation other than a travel variant (an impact effect, a script reaction), cancels the pending moment for that entry, so a flinched or launched swing never connects (owner, 2026-11-14). A weapon attack starts its activation at the moment; its shots keep the weapon's schedule. Without `at`, firing is unchanged.
- **`at` implies the turn mark.** An activity whose own action carries `at` holds that attack's turn from entry as if marked `attackTurn` for it: an unmarked attack requests its turn only when its proposal forms, so its swing would play before the budget is consulted (`entity_model.md` §7c, Turns). The implied mark joins mark agreement: an explicit mark naming the same attack loads, one naming another rejects as disagreeing marks do today (owner, 2026-11-13; `index.md` §2, Designers over structure). Load rejects `at` on a selector row's action, since a row selects no animation and its mark would span the owner's whole life (owner, 2026-11-13), and on an activity whose path selects no animation.
- **A missing event warns and fires as today.** When `at` names an event the resolved clip lacks — a travel variant's clip included — or the state's clip did not resolve, the engine warns once when the entity's clips bind, naming entity class, state, clip and event, and the attack fires by today's rule (`index.md` §2, Designers over structure).
- **`brain.animationDone` reads that the selected animation has played through.** True from the tick after the clip time of a non-looping state reaches its clip's duration, so a moment on the clip's last tick fires before an exit on `animationDone` can pass (owner, 2026-11-14); always false for a looping state; true for a state whose clip did not resolve, which counts as played through as player one-shots do (`entity_model.md` §7b). It is entity-wide, not scope-relative like `timeInActivityMs`.
- **Host only; nothing crosses the wire.** Graph evaluation and attacks are host-only, so events add no wire field, version bump or digest entry. Clips stay in the compatibility digest's named gap (`networking.md`, covered set); a client-predicted use of events would have to close it first.
- **First consumers: the reference brute and the limitator.** `mixamo_brute` carries a measured `hit` on `punch`, `smash` and `swipe`, so `punch_strike`, `smash_strike` and `swipe_strike` inherit theirs. The brute's jab and slam fire `at: "hit"`. `mixamo_rifle` carries a measured `kick` on the looping `firing_rifle`, and the limitator's `shoot` fires `at: "kick"`, so its burst starts on the recoil (owner, 2026-11-13).
- **Non-goals.**
  - Span events and melee hit windows: roadmap follow-on (owner, 2026-11-13).
  - Clip-paced automatic fire, every pass of a looping moment firing a shot: roadmap follow-on; weapon cadence stays the weapon's (owner, 2026-11-13).
  - Descriptor-authored or overridden event times (owner, 2026-11-13).
  - Sounds or effects at events: events are host-only, and presentation runs on clients.
  - Multi-hit attacks.
  - Stance releases: the stances brief adopts `at` on its `release` when it builds; this brief changes no stances vocabulary.
  - Weapon gating: an event changes only when an attack fires, never which attacks exist, so it leaves the roadmap's Weapon-gated enemy attacks open.

### Scripting surface
```ts
// Library build: blender --background --python tools/mixamo_to_gltf.py -- \
//   ... --event punch:hit=620 --trim punch_windup=punch:467 --trim punch_strike=punch:467-
// punch_strike inherits `hit` at 0.153 s. The time is illustrative; the build measures it.
import { attackTurnFree, brain } from "postretro";

const jabs = {
  initial: "stalk",
  activities: {
    stalk: { animation: "walk" },
    jab: {
      animation: "attack_jab",                   // a state playing clip "punch"
      action: { attack: "jab", at: "hit" },      // fires when punch reaches `hit`;
    },                                           // holds the jab's turn from entry
  },
  transitions: {
    stalk: [{ to: "jab", when: brain.targetDistance.le(2).and(attackTurnFree("jab")) }],
    jab: [{ to: "stalk", when: brain.animationDone }], // the punch has played through
  },
};
```
The Luau mirror ships both: `at` in the descriptor types and parsers, `animationDone` in the prelude.

## Acceptance
### Automated
**Converter**
- [ ] `--event punch:hit=620` writes `hit` at 0.62 s into `punch`'s extras; a second `--event` on another clip lands there only.
- [ ] A range trim starting before the moment carries it, shifted by the range's actual start; a windup cut ending before it carries none. An event at a range's start that falls between keys lands at its offset from the key the start rounds down to; an event exactly on that key lands at 0 in the range and not in a windup ending there.
- [ ] An event past its clip's end, a duplicate name in one clip, and `--event` with `--clip-library` each reject with an error naming the flag.
- [ ] The Blender-free converter suite passes unchanged across the post-processing split, before any event code lands.
**Loader**
- [ ] A character bound to `mixamo_brute` resolves `punch_strike` with its shifted `hit`.
- [ ] A character's own clip named like a library clip keeps its own events, or none, never the library's.
- [ ] A malformed `extras.events` (not a list, a non-numeric time, a negative time) warns once and the clip loads with its keys and no events.
- [ ] An event carrying an `end` field loads with its name and time.
**Firing**
- [ ] With `at: "hit"` at 0.5 s, the attack does not fire on tick 29 after entry and fires on tick 30, a moment on a tick boundary firing on that tick; with `hit` at 0.51 s it fires on tick 31; without `at`, the same graph fires on its entry tick (P1).
- [ ] An event at time 0 fires on the entry tick.
- [ ] Out of range at the moment: no fire, and none later in that entry after the target steps back in; a fresh entry fires at its moment.
- [ ] Holding no turn for its attack at the moment: no fire, and none later in that entry after the turn is granted; a turn granted in the moment tick's own arbitration fires on that tick (P9).
- [ ] Leaving the activity before the moment fires nothing, including an exit row passing on the moment's own tick (P3).
- [ ] An event on a clip's last tick fires; `brain.animationDone` turns true on the next tick, and an activity leaving on it leaves after the fire (P4).
- [ ] An activity entered on a shared non-looping clip already past the moment fires nothing; one entered on the tick the clip reaches it fires on that entry tick (P5).
- [ ] Two activities on one animation state: the second activity's `at` fires at the moment measured from the first's entry, not its own.
- [ ] An activity entered on a looping state its predecessor has played past the moment fires at the next pass, not on entry; a looping travel variant restarts clip time at its drawn phase.
- [ ] An impact effect switching the enemy to `hit_reaction` before the moment, its attack activity unchanged: nothing fires that entry, even after the graph's animation is restored within the same gap between AI ticks or later; with no switch, the same swing fires (P8).
- [ ] An impact switch landing before the AI stage on the tick the enemy enters the activity does not cancel that entry; one landing after the enemy's compute on the moment tick (a hitscan, an earlier slot's contact hit) leaves that tick's fire standing (P6, P7).
- [ ] A knockback launch before the moment cancels it: nothing fires that entry, airborne or after landing, on a mesh without `jump_loop` and on a launch shorter than the fall delay (P15).
- [ ] A travel variant switch mid-entry restarts clip time, and the attack fires at the variant clip's event measured from the switch (P14).
- [ ] An `at` activity animating the graph's locomotion state, travelling at 1.5× its travel speed, fires at two-thirds of the event's time in wall time; each tick advances clip time by the rate the previous tick's pose stage set (P2).
- [ ] A looping clip fires once per entry, at the next pass of the moment from the enemy's drawn phase: not on entry, and not again on later loops; two enemies with different phases fire on different ticks.
- [ ] A weapon attack with `at` starts its activation at the moment; its later shots keep the weapon's steps.
- [ ] A missing event warns once per entity class and state when clips bind, and the attack fires on its entry tick as without `at`, for a resolved clip lacking the name, a travel variant's clip lacking it, and a state whose clip did not resolve.
- [ ] An activity with `at` and no `attackTurn` holds its attack's turn from entry, as a marked one does: a second enemy waiting on the same token is not granted before the moment.
- [ ] An explicit `attackTurn` naming the same attack as `at` loads; one naming another attack rejects; `at` on a selector row's action rejects; `at` on an activity whose path selects no animation rejects.
- [ ] The same seed and inputs, with no pose-dependent hit on the enemy, fire on the same ticks at 1, 2 and 4 ticks per frame and at animation time scales 1 and 0.25.
**Clip end**
- [ ] `brain.animationDone` is false through the tick the selected non-looping clip reaches its duration and true on the next; false throughout a looping state; true for a state whose clip did not resolve.
- [ ] Entering a non-looping state from another that has played through: `brain.animationDone` reads false on the entered activity's first evaluated tick, and the entered activity's `at` fires at its own clip's moment (P10).
- [ ] After an impact effect cancels the pending moment, clip time keeps running for the path's state: `brain.animationDone` turns true after the clip's duration from the state's start, and an activity leaving on it leaves (P11).
- [ ] A parent's guard and its leaf's guard read the same `brain.animationDone` on one tick.
**Surface and consumers**
- [ ] The Scripting surface example lowers identically in TypeScript and Luau and runs as a fixture: the jab fires at `punch`'s `hit` and leaves on `animationDone`.
- [ ] The reference brute's jab and slam fire at their clips' `hit`, and the limitator's first burst shot at `firing_rifle`'s `kick`.
- [ ] A limitator whose `shoot` whiffs at `kick` leaves `burst` and frees its `ranged` turn; one that fires leaves after its third shot, at every drawn phase.
- [ ] Grep gate: no wire field, protocol version or compatibility digest entry changes.
### Manual
- [ ] Re-exporting `mixamo_brute` and `mixamo_rifle` with their recorded commands reproduces the committed libraries, apart from the new events.
- [ ] Single player: the brute's jab and slam damage lands as the fist connects; a player who steps back before the fist arrives takes no hit; the limitator's first shot leaves on its recoil kick.
- [ ] Loopback co-op: the client sees the same timing, with no new warnings.

## Path
- Seams: `compute::evaluate` (transitions, then the Attack block) and the `activity_attack_count` latch; `graph_eval::animation_for_path`; `travel_variants`; `apply::apply_outcomes` switches the state; `launch` holds the launched state. `mesh_anim::state_elapsed`'s completion rule is the model for `animationDone`, not its clock.
- Loader: `parse_clip` and `AnimationClip` gain events; `append_library_clips` carries them. Mark validation sits beside `validate_attack_turn_marks`.
- Shape chosen: the brain accumulates clip time per tick for its path-selected state, and `at` fires inside the attack's own activity. Rivals: read `MeshAnimation::scaled_elapsed` on the animation clock — rejected for determinism; a `brain.eventReached("hit")` guard fact with a separate strike activity firing on entry — rejected for doubling every attack's activities and not fitting a stance's single release, though it can be added later beside `at`.
- First slice: `--event` on `punch`, the loader carrying it, and the brute's jab firing at it in a fixture; this falsifies the riskiest assumption, that the AI tick can reach the resolved clip and its rate cheaply.
- Moments and re-export commands are recorded in each library's `license.txt`; the source FBXs stay outside the repo. Correct the jab comment in `content/dev/scripts/reference-enemy.ts`.
- Docs: `docs/character-animation.md` (`--event`), the scripting reference (`at`, `animationDone`).
- `tools/mixamo_to_gltf.py` is past 800 lines. Split the glTF post-processing (`postprocess_gltf`, `trim_clip`, `sample_track` and the new events) into a sibling module first, behavior-preserving, in its own commit; Blender runs the script by path, so the module must import from the script's own folder.
- Which of the brute's strike and recover timers become `brain.animationDone` is the build's call, where the clip covers them. Today's jab leaves on `attacksFiredInActivity` and a 450 ms timer; a whiff never fires, so its exit drops `attacksFiredInActivity` and leaves on `animationDone`.
- Today's limitator `burst` leaves on `attacksFiredInActivity` and a 450 ms timer sized for a burst fired on entry. With `at: "kick"` its first shot waits up to one `firing_rifle` pass and a whiff never fires, so its exit drops `attacksFiredInActivity` and its timer covers a full pass plus the burst. `rifle_trooper` and `rifle_warrior` inherit it.
- Ordering pins and stage-order facts are in `research.md`.

## Open questions
- Event name rules beyond non-empty and unique (allowed characters, case) — **delegated**.
