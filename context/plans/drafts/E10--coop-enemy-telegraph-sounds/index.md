# E10--coop-enemy-telegraph-sounds

Brief · compact · Epic 10 · reads: `context/lib/networking.md` §Presentation events vs. replicated state · `context/lib/audio.md` §4 · `context/lib/entity_model.md` §7c · read at d0dbf78

## Problem
Co-op clients never hear an enemy's tell, an observed defect. An activity's `sound` and a contact (melee) attack's `sound` play only on the host: behavior-entry sounds play from the host's AI events, which a client never runs, and observer weapon cues skip any emission without a shot, so melee never reaches a client. When this is done, every participating peer hears each activity and contact sound once, at the enemy's position on the host, as an `onEnter` `playSound` is heard today. Split out of `ready/E10--enemy-stances-and-telegraphs`, whose stance sounds ride this route; it builds first.

## Decisions
- **Sounds ride the `onEnter` route.** An activity's `sound` and a contact attack's `sound` reach each participating client as a positioned `PlaySound` presentation command at the enemy's host-captured origin, the route an `onEnter` `playSound` already takes (`networking.md` §Presentation events vs. replicated state). The host plays its own once, as today. Host-side capture keeps the sound where the host heard it, whatever the client's interpolated pose.
- **Unreliable on purpose.** Presentation is unreliable, so a dropped packet can lose a sound. The enemy's pose replicates as animation state and still tells. No new message kind and no version bump.
- **Takes the enemy slice of roadmap E12 step 4**, on the payload that step extends.
- **Weapon attack sounds stay on `WeaponCue`.** They already reach clients; the enemy route never duplicates them.
- **Non-goals:**
  - **`onEnter` reactions.** Unchanged; a client receiving a sound runs no reaction.
  - **Catch-up sounds.** A client that materializes an enemy mid-activity hears nothing for that entry: a sound is an event, not state.

## Acceptance
### Automated
- [ ] On a loopback client, an activity's `sound` and a contact attack's `sound` play exactly once per host event, and the host plays each exactly once, with or without a server (pin P15).
- [ ] An enemy weapon attack's sound still plays exactly once on every peer (pin P10).
- [ ] An A → B → A sequence inside one snapshot interval plays A's sound twice on each peer (pin P7).
- [ ] A sound plays once at the host-captured position whether or not the client holds the enemy, and never replays (pin P9).
- [ ] An enemy killed in the apply pass that moves it into a sounding activity plays no entry sound on any peer (pin P11). One that enters it and is killed after the AI pass on the same tick plays the sound once on every peer (pin P19).
- [ ] A client receiving the sound runs no `onEnter` reaction.
- [ ] A client that begins participating on the tick an enemy enters a sounding activity plays that sound at most once, and only from its command. Materializing an enemy mid-activity plays no sound (pin P14).
- [ ] Grep gate: no server message variant is added, and `PROTOCOL_ID`, `WIRE_VERSION` and `SNAPSHOT_VERSION` are unchanged.
### Manual
- [ ] Co-op loopback: the client hears the brute's activity sounds and its jab and slam contact sounds, each once, from where the brute stands.

## Path
- **Today.** `AiCue::Entered` plays the entered leaf's `sound` from the host's AI events (`sound_events/descriptors.rs`); clients never run `simulate_tick`, so their queue is empty. `publish_observer_weapon_cues` (`weapon_observers.rs`) skips emissions without a `shot_id`, so contact sounds never leave the host; a weapon attack's sound rides `WeaponCue.additional_sound`.
- **Route.** `SystemReactionCommand::PlaySound` maps to `PresentationCommand::PlaySound` (`crates/netcode/src/presentation_commands.rs`); `route_player_presentation` sends it per client through `send_presentation` and also queues it locally. `resolve_recipients` (`system_commands.rs`) sends a host-only sound with no audience to Everyone. The AI emission's `Emitter::Entity` origin is the captured position.
- **Host double-play.** `route_player_presentation` re-pushes an Everyone `PlaySound` to the host's local queue, and with no server pushes every command locally, while the Audio stage already plays the AI emission. Send the routed copy to clients only, or drop the emission copy (P15).
- **First slice.** A brute contact sound heard once on a loopback client and once on the host.
- Pin table: `research.md`.

## Wire format
**No new message.** Activity and contact sounds ride the existing `PresentationCommand::PlaySound { sound, bus, at }` on the Presentation channel, `at` set to the host-captured origin, to every participating client. No `PROTOCOL_ID`, `WIRE_VERSION` or `SNAPSHOT_VERSION` change.
