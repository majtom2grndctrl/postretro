# E10--coop-enemy-telegraph-sounds — research

Pins split from `E10--enemy-stances-and-telegraphs` (2026-11-15); ids keep their numbers there. Seams and the double-play finding: brief §Path.

| Id | Name | Scenario | Expected |
|---|---|---|---|
| P7 | A → B → A inside one snapshot interval | Activity A, with a sound, is entered, left and re-entered between two snapshots. | Each peer plays A's sound twice. The client's clip follows the latest replicated state. |
| P9 | Sound without a live client entity | A sound reaches a client before the enemy's snapshot, after it despawned, or while the client holds it. | Plays once at the host-captured position whether or not the client holds the enemy, never at the client's interpolated pose, and never replays. |
| P10 | Weapon attack sound | An enemy weapon attack carries a sound. | It rides `WeaponCue.additional_sound` and plays once. The enemy cue never duplicates it. |
| P11 | Killed in the pass that enters a sounding activity | An earlier outcome in the apply pass kills the enemy that compute moved into an activity with a `sound`. | No entry sound and no `PlaySound` command on any peer. |
| P14 | Sounding activity entered on a client's join tick | On tick N an enemy enters an activity with a `sound`; the client begins participating that frame, before or after the host publishes tick N's `PlaySound` command. | A command published after participation began plays once, at the captured position if the enemy is not yet materialized (P9). One published before is never delivered. Materializing the enemy mid-activity plays no sound. |
| P15 | Sound on the host | The Audio stage plays an activity's or contact attack's `sound` from its AI emission; a `PlaySound` routed to every peer is also pushed to the host's local queue (always, with no server). | The host plays it once, with or without a server: the routed copy goes to clients only, or the emission copy is dropped. |
| P19 | Entry, then death later in the tick | Compute moves the enemy into an activity with a `sound` on tick N; a Weapons-stage hit kills it on tick N. | The sound plays once on every peer, at the captured origin. |
