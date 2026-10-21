# coop--timed-slot-projection

Brief · compact · reads: `context/lib/networking.md` §What gates, and what replicates instead, §Combat authority, §Presentation events vs. replicated state · `context/lib/ui.md` §3 · read at d6f3471

## Problem
Owner playtest under the dev lag proxy after PR #601: a connected client's reload meter fills in steps. Basis: observed defect, split out of `coop--lossy-link-delivery` and then `drafts/ui-tween-clock`. Cause: a client's store receives the host's sample about 30 times a second (`SNAPSHOT_TICK_INTERVAL` = 2), and the meter shows each sample raw. It steps on a clean link at any frame rate above 30; loss and jitter only lengthen the holds. The client has no rate to advance the value between samples. When done, a connected client's meters ramp smoothly wherever the host's value ramps, between samples and through lost or reordered snapshots, with no change to the host, single-player or any mod HUD.

## Decisions
- **The projection sets the display target.** For an enumerated slot on a connected client, the target the display value retargets to (`ui.md` §3) is the slot's projection, not the raw sample. The rest of §3 holds: a bound tween eases toward the moving target, and an untweened binding shows the projection. A mod HUD needs no change. The store slot keeps the host sample, so state crossings and the sample-derived reload sound edges (`networking.md` §Combat authority) fire once per host crossing.
- **Scope: every host-projected slot with a timed stretch.** Reload progress is the confirmed case; weapon cooldown, heat and cell follow. Not all of their change is timed: heat and cell move per shot, cooldown resets on fire and per-shell reload progress wraps from 1 to 0. Only reload, cooldown countdown, cooling and regen ramp on the host's fixed tick. The executor enumerates the slots from the state-slot table and reports the list in the plan of record.
- **The rate is derived on the client, from host-stamped samples.** The meter is presentational, so it takes the cheapest shape that looks right, not the authoritative one. Each accepted snapshot is a sample at its header's server tick. A snapshot with no record for the slot, or a resend at the held value, is a sample at the held value: the host sends nothing for a slot the client already holds at the current value (`ServerStateReplication::produce_for_client`). The slope between samples comes from the host's own values, never from local tuning, so `networking.md`'s no-local-fallback rule holds. Functional timing is unchanged: the host owns the reload timer and reserve transfer, and the store keeps its samples.
- **Extrapolate only on an agreed slope.** The projection advances only while the two newest intervals agree on the slope; otherwise the target is the newest sample. A ramp therefore smooths from its third sample, and a jump never extrapolates: a shot's heat step, a cooldown reset, a reload wrap or a cool delay going flat shows as the host's own sample. While firing, heat steps per shot as the host's meter does. A projected value never passes the host's reported endpoint. On a host cancel, a slot change or the active flag falling, the target moves to the host's value; a tween eases there as for any retarget.
- **The projection runs on the samples' own timeline.** It projects to the sample that would be arriving now, not to the host's current value. The clock is smooth: it never re-anchors on a snapshot's arrival, so arrival jitter never moves the meter. `presentation_host_time` fails both tests: it re-anchors on every accepted snapshot, and it leads the samples by half the RTT. On the sample timeline, a late packet only confirms the projection, and the meter stays in step with the raw `reloadActive` flag, its sounds and the vanish at full, which come from the same samples.
- **Bounded, then hold.** A derived slope is an extrapolation, so it follows the bounded-extrapolation-then-hold rule of `done/M15--p2-replication-timesync-lifecycle` and `done/E10--networked-enemy-authority-baseline`, not the uncapped closed forms of `done/E17--kinematic-platform-foundation` and `anim_start`, which replicate a schedule. Projection stops once the sample timeline runs more than a cap past the newest sample, and holds until the next. The cap is measured on the sample timeline, so it covers snapshot interval and jitter, not RTT. It is longer than M15's 100 ms because a 30 Hz slot stream holds about 66 ms on every single loss. During a real outage the meter freezes, which is honest; it never shows heat cooling while the client's own fire is going unheard.
- **Heat and cell are display, not simulation.** `networking.md` §Combat authority says connected clients never run the heat or cell update. The projection extends the host's own trend at the host's slope for display: no client heat state, no overheat latch, no gameplay read. §Combat authority is amended to say so. Client ammo, heat, cell and reload prediction stay out of scope.
- **Rivals rejected.**
  - A replicated rate or schedule. A replicated instantaneous rate is at most one snapshot interval better at a change; only a replicated schedule (value plus the time of the next change, as E17 and `anim_start` carry) is exact at a reload start or a cool delay ending. For a meter, a sample or two of raw display at each change is invisible, and it would cost a wire field, a `SNAPSHOT_VERSION` move and version coordination with `coop--lossy-link-delivery`.
  - Projecting to the host's current value. It leads the reload flag and its sounds by half the RTT, so the meter jumps at a reload start and sits full before vanishing.
  - Interpolating the samples on the remote-pawn interpolation timeline. It adds about 100 ms or more to the player's own meter, puts it behind the flag and the sounds, and makes it vanish short of full.
  - A separate client-local projected slot beside the raw one, as the damage bearing does (`networking.md` §Presentation events vs. replicated state). It keeps the raw value bindable but makes every HUD rebind to get a smooth meter. The quantity's meaning does not change, so the engine projects under the existing slot name.
  - A default tween on the HUD binding lags in every role and leaves an engine symptom to mod policy.
  - Advancing at the client's local tuning rate breaks `networking.md`'s no-local-fallback rule.
- **Independent of the UI clock.** The sample timeline comes from the time-sync estimate, not UI time. A tween bound to a projected slot eases on whatever UI time is, so this brief and `drafts/ui-tween-clock` land in either order.
- **Placement.** Sample intake runs in `postretro-netcode` where an accepted snapshot's state records apply. Projection runs per frame, every frame including those with no snapshot, beside the overlay projection (`update_client_overlay_anchors`), and hands the UI snapshot a projected target per enumerated slot. The UI module still writes no store slot. The wire is unchanged: no field, no version move.
- **Docs owed at promotion.** `ui.md` §3: the target bullet names the connected-client projection. `networking.md` §What gates: the client-derived timed-slot projection. `networking.md` §Combat authority: the heat and cell display sentence. `docs/scripting-reference.md`: one sentence that a timed meter advances smoothly on clients with no author change.
- **Non-goals.**
  - The UI clock, frontend tweens and Loading animation: `drafts/ui-tween-clock` owns them.
  - Loss recovery on the reliable channels: `ready/coop--lossy-link-delivery` owns it.

## Acceptance
### Automated
- [ ] Reproduces first. On the build this starts from, a connected client's reload-progress display under the profile changes only on frames that apply a sample, and the test is committed failing.
- [ ] Permit: on a connected client under loss and jitter, with production's newest-snapshot gate applied, a ramp (reload, cooldown countdown, cooling, regen) smooths from its third sample. Within it, the target advances monotonically through lost and reordered snapshots, including frames with no snapshot, and each frame's change is at most the host's slope × frame time plus a small epsilon.
- [ ] Arrival jitter never moves the meter: on a ramp under 60 ms jitter, the target's per-frame change stays within that bound on the frames a snapshot arrives.
- [ ] Refuse: a jump never extrapolates. A shot's heat step, a cooldown reset, a reload wrap and a cool delay going flat each show the host's sample, and the target never overshoots it by more than the slope over one interval plus the profile's jitter.
- [ ] Permit: an accepted snapshot that carries no record for the slot counts as a sample at the held value, so a flat stretch stops the projection within one snapshot interval.
- [ ] Refuse: the target falls on a host cancel or slot change, never passes completion before the host completes, and the reload meter disappears when its active flag falls.
- [ ] In step: at a reload start the meter shows nothing before the `reloadActive` flag rises, and at the end it reaches full no more than the profile's jitter before the raw full sample arrives.
- [ ] Outage: with no sample for longer than the cap, the target holds at the cap's projection and does not move until the next sample. Permit: a single lost snapshot inside the cap leaves the advance unbroken.
- [ ] State crossings on an enumerated slot fire once per host crossing, never on the projection's advance.
- [ ] A tween bound to an enumerated slot eases toward the moving projection; with reduce motion on, it shows the projection that frame.
- [ ] Host and single-player displays are unchanged.
- [ ] No client heat or cell state is written: the host-sample store slots and the client weapon components match a build without the projection.
- [ ] The wire is unchanged: `SNAPSHOT_VERSION`, `WIRE_VERSION` and the application protocol keep their values, and no slot record gains a field.

### Manual
- [ ] Loopback recipe (`networking.md` §Manual loopback recipe), client conditioned at 45/60/5: the reload meter moves smoothly; a heat weapon's meter steps per shot while firing, then cools smoothly.

## Path
- Seams: `hud.ts` `reloadMeter` (untweened bind of `player.reloadProgress`). `PlayerHudStatePublisher::tick_for_role_and_report_sampled_weapon`, whose connected-client branch skips reload. `WeaponComponent::reload_feedback_sample`. `ReplicatedWireShape::WieldableSlotNumber`, shared by reload progress and weapon cooldown. `ClientStateApply`. Host timers: `advance_weapon_resource` (cooling, regen) and the weapon stage's reload.
- Sample timeline: one way is the time-sync estimate (`ClockEstimator::estimated_server_tick`) minus half the smoothed RTT. Another is smoothing each slot sample's arrival lag against its recent minimum, as the snapshot-lag law in `networking.md` §Time sync does. Either meets the Decision; a time-sync reseed shifts it once, which the hold rule absorbs.
- Harness: `state_slot_loss_harness_test.rs` lacks production's newest-snapshot gate; add it first.
- First slice: the gated harness and the failing reproduction row.
- `research.md` has the symbols.

## Open questions
- The hold cap's value. — **delegated**: at least two snapshot intervals plus the profile's jitter, measured on the sample timeline; report it in the plan of record.
