# coop--timed-slot-projection

Brief · compact · reads: `context/lib/networking.md` §What gates, and what replicates instead, §Combat authority, §Presentation events vs. replicated state · `context/lib/ui.md` §3 · read at d6f3471

## Problem
Owner playtest under the dev lag proxy after PR #601: a connected client's reload meter fills in steps. Basis: observed defect, split out of `coop--lossy-link-delivery` and then `drafts/ui-tween-clock`. Cause: a client's store receives the host's sample about 30 times a second (`SNAPSHOT_TICK_INTERVAL` = 2), and the meter shows each sample raw. It steps on a clean link at any frame rate above 30; loss and jitter only lengthen the holds. The client has no rate to advance the value between samples. When done, a connected client's timed meters advance smoothly between samples and through lost or reordered snapshots, with no change to the host, single-player or any mod HUD.

## Decisions
- **The projection sets the display target.** For an enumerated slot on a connected client, the target the display value retargets to (`ui.md` §3) is the slot's projection, not the raw sample. The rest of §3 holds: a bound tween eases toward the moving target, and an untweened binding shows the projection. A mod HUD needs no change. The store slot keeps the host sample, so state crossings and the sample-derived reload sound edges (`networking.md` §Combat authority) fire once per host crossing.
- **Scope: every host-projected slot that advances on a timer.** Reload progress is the confirmed case; heat and cell follow. The executor enumerates the rest from the state-slot table, `player.weaponCooldownMs` first since it shares the wire shape, and reports the list in the plan of record.
- **The rate is derived on the client, from host-stamped samples.** The meter is presentational, so it takes the cheapest shape that looks right, not the authoritative one. Each accepted snapshot is a sample at its header's host `script_time`. A snapshot with no record for the slot, or a resend at the held value, is a sample at the held value: the host sends nothing for a slot the client already holds at the current value (`ServerStateReplication::produce_for_client`), so a flat stretch reads as flat within one snapshot. The rate is the slope between the two newest samples. Projection = newest sample + rate × (host now − its host time), with host now from `presentation_host_time`. The slope comes from the host's own samples, never from local tuning, so `networking.md`'s no-local-fallback rule holds. The damaged-enemy roster (`presentation/overlays.rs`) is the precedent for host-time projection, including its high-water guard against an anchor that regresses. Functional timing is unchanged: the host owns the reload timer and reserve transfer, and the store keeps its samples.
- **Projection rules.** Monotonic within one run, where a run is one uninterrupted stretch in one direction. A value whose trend can reverse, such as heat, starts a new run when the host sample reverses. On a new sample, a host cancel, a slot change or the active flag falling, the target moves to the host's value; a tween eases there as for any retarget. Never passes the host's reported endpoint.
- **Bounded, then hold.** A derived slope is an extrapolation, so it follows the bounded-extrapolation-then-hold rule of `done/M15--p2-replication-timesync-lifecycle` and `done/E10--networked-enemy-authority-baseline`, not the uncapped closed forms of `done/E17--kinematic-platform-foundation` and `anim_start`, which replicate a schedule. Projection stops once host now is more than a cap past the newest sample's host time, and holds until the next. Sample age is measured against a host-time estimate that already includes half the RTT, so the cap is sized from snapshot interval and jitter, not RTT. It is longer than M15's 100 ms because a 30 Hz slot stream holds about 66 ms on every single loss. During a real outage the meter freezes, which is honest; it never shows heat cooling while the client's own fire is going unheard.
- **Heat and cell are display, not simulation.** `networking.md` §Combat authority says connected clients never run the heat or cell update. The projection extends the host's own trend at the host's rate for display: no client heat state, no overheat latch, no gameplay read. §Combat authority is amended to say so. Client ammo, heat, cell and reload prediction stay out of scope.
- **Rivals rejected.**
  - A replicated rate or schedule. A replicated instantaneous rate is at most one snapshot interval better at a change; only a replicated schedule (value plus the time of the next change, as E17 and `anim_start` carry) is exact at a reload start or a cool delay ending. For a meter, about 33 ms of old slope at each change is invisible, and it would cost a wire field, a `SNAPSHOT_VERSION` move and version coordination with `coop--lossy-link-delivery`.
  - Interpolating the samples on the remote-pawn interpolation timeline. It adds about 100 ms or more to the player's own meter, puts it out of step with the raw `reloadActive` flag and the sample-derived reload sounds, and makes it vanish short of full.
  - A default tween on the HUD binding lags in every role and leaves an engine symptom to mod policy.
  - Advancing at the client's local tuning rate breaks `networking.md`'s no-local-fallback rule.
- **Independent of the UI clock.** Projection reads host time, built on the level clock `script_time`; UI time does not drive it. A tween bound to a projected slot eases on whatever UI time is, so this brief and `drafts/ui-tween-clock` land in either order.
- **Placement.** Sample intake runs in `postretro-netcode` where an accepted snapshot's state records apply. Projection runs per frame, every frame including those with no snapshot, beside the overlay projection (`update_client_overlay_anchors`), and hands the UI snapshot a projected target per enumerated slot. The UI module still writes no store slot. The wire is unchanged: no field, no version move.
- **Docs owed at promotion.** `ui.md` §3: the target bullet names the connected-client projection. `networking.md` §What gates: the client-derived timed-slot projection. `networking.md` §Combat authority: the heat and cell display sentence. `docs/scripting-reference.md`: one sentence that a timed meter advances smoothly on clients with no author change.
- **Non-goals.**
  - The UI clock, frontend tweens and Loading animation: `drafts/ui-tween-clock` owns them.
  - Loss recovery on the reliable channels: `ready/coop--lossy-link-delivery` owns it.

## Acceptance
### Automated
- [ ] Reproduces first. On the build this starts from, a connected client's reload-progress display under the profile changes only on frames that apply a sample, and the test is committed failing.
- [ ] Permit: on a connected client under loss and jitter, with production's newest-snapshot gate applied, each enumerated value advances monotonically through lost and reordered snapshots, including frames with no snapshot. Each frame's change is at most the host's true rate × frame time plus a small epsilon.
- [ ] A flat stretch reads flat: when the host value stops changing, as in heat's cool delay, the projection stops within one snapshot interval of the host and never overshoots by more than one interval's slope. Permit: an accepted snapshot that carries no record for the slot counts as a sample at the held value.
- [ ] Refuse: the target falls on a host cancel or slot change, never passes completion before the host completes, and the reload meter disappears when its active flag falls.
- [ ] A reversing value, such as heat rising while firing and then cooling, follows the host's direction change within one sample.
- [ ] Outage: with no sample for longer than the cap, the target holds at the cap's projection and does not move until the next sample. Permit: a single lost snapshot inside the cap leaves the advance unbroken.
- [ ] State crossings on an enumerated slot fire once per host crossing, never on the projection's advance.
- [ ] A tween bound to an enumerated slot eases toward the moving projection; with reduce motion on, it shows the projection that frame.
- [ ] Host and single-player displays are unchanged.
- [ ] No client heat or cell state is written: the host-sample store slots and the client weapon components match a build without the projection.
- [ ] The wire is unchanged: `SNAPSHOT_VERSION`, `WIRE_VERSION` and the application protocol keep their values, and no slot record gains a field.

### Manual
- [ ] Loopback recipe (`networking.md` §Manual loopback recipe), client conditioned at 45/60/5: the reload meter and a heat weapon's meter move smoothly, and the heat meter follows a fire-then-cool cycle.

## Path
- Seams: `hud.ts` `reloadMeter` (untweened bind of `player.reloadProgress`). `PlayerHudStatePublisher::tick_for_role_and_report_sampled_weapon`, whose connected-client branch skips reload. `WeaponComponent::reload_feedback_sample`. `ReplicatedWireShape::WieldableSlotNumber`, shared by reload progress and weapon cooldown. `ClientStateApply`. `PresentationBindings::presentation_host_time`, with consumers in mesh_anim.rs and overlays.rs.
- Harness: `state_slot_loss_harness_test.rs` lacks production's newest-snapshot gate; add it first.
- First slice: the gated harness and the failing reproduction row.
- `research.md` has the symbols.

## Open questions
- The hold cap's value. — **delegated**: at least two snapshot intervals plus the profile's jitter, measured against host time; report it in the plan of record.
