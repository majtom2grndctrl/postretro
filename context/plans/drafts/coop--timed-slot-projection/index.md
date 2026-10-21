# coop--timed-slot-projection

Brief · compact · reads: `context/lib/networking.md` §What gates, and what replicates instead, §Combat authority, §Presentation events vs. replicated state · `context/lib/ui.md` §3 · read at d6f3471

## Problem
Owner playtest under the dev lag proxy after PR #601: a connected client's reload meter fills in steps. Basis: observed defect, split out of `coop--lossy-link-delivery` and then `drafts/ui-tween-clock`. Cause: a client's store receives the host's sample about 30 times a second (`SNAPSHOT_TICK_INTERVAL` = 2), and the meter shows each sample raw. It steps on a clean link at any frame rate above 30; loss and jitter only lengthen the holds. The client has no rate to advance the value between samples. When done, a connected client's timed meters advance smoothly between samples and through lost or reordered snapshots, with no change to the host, single-player or any mod HUD.

## Decisions
- **The projection sets the display target.** For an enumerated slot on a connected client, the target the display value retargets to (`ui.md` §3) is the slot's projection, not the raw sample. The rest of §3 holds: a bound tween eases toward the moving target, and an untweened binding shows the projection. A mod HUD needs no change. The store slot keeps the host sample, so state crossings and the sample-derived reload sound edges (`networking.md` §Combat authority) fire once per host crossing.
- **Scope: every host-projected slot that advances on a timer.** Reload progress is the confirmed case; heat and cell follow. The executor enumerates the rest from the state-slot table, `player.weaponCooldownMs` first since it shares the wire shape, and reports the list in the plan of record.
- **Projection runs on host time with a replicated rate.** Projection = latest sample + rate × (host now − the sample's host time). Host now is `presentation_host_time`, the client's estimate of the host's script clock. The rate is replicated with the sample, never recomputed from local tuning (`networking.md` §What gates). The damaged-enemy roster's projection (`presentation/overlays.rs`) is the precedent, including its high-water guard against an anchor that regresses.
- **Projection rules.** Monotonic within one run, where a run is one uninterrupted stretch in one direction. A value whose trend can reverse, such as heat, starts a new run when the host sample reverses. On a new sample, a host cancel, a slot change or the active flag falling, the target moves to the host's value; a tween eases there as for any retarget. Never passes the host's reported endpoint.
- **Bounded, then hold.** Projection stops advancing once the sample is older than a cap that covers ordinary loss and jitter, and holds until the next sample. This follows the bounded-extrapolation-then-hold rule of `done/M15--p2-replication-timesync-lifecycle` and `done/E10--networked-enemy-authority-baseline`, with a longer cap than their 100 ms because a 30 Hz slot stream holds about 66 ms on every single loss. During a real outage the meter freezes, which is honest; it never shows heat cooling while the client's own fire is going unheard.
- **Heat and cell are display, not simulation.** `networking.md` §Combat authority says connected clients never run the heat or cell update. The projection extends the host's own trend at the host's rate for display: no client heat state, no overheat latch, no gameplay read. §Combat authority is amended to say so. Client ammo, heat, cell and reload prediction stay out of scope.
- **Rivals rejected.**
  - A rate the client derives from consecutive samples needs no wire change. Rejected: the host knows the exact moment a rate changes (a cool delay ending, a reload starting), while a derived rate learns it only from the next two samples, so every change shows the old slope for at least one sample interval.
  - A default tween on the HUD binding lags in every role and leaves an engine symptom to mod policy.
  - Advancing at the client's local tuning rate breaks `networking.md`'s no-local-fallback rule.
- **Independent of the UI clock.** Projection reads host time, built on the level clock `script_time`; UI time does not drive it. A tween bound to a projected slot eases on whatever UI time is, so this brief and `drafts/ui-tween-clock` land in either order.
- **Placement.** Projection runs in `postretro-netcode` on the state-slot apply path and hands the UI snapshot a projected target per enumerated slot. The UI module still writes no store slot. The rate is a wire field on the slot record; `SNAPSHOT_VERSION` moves if it changes a snapshot record, and the previous peer is refused at the handshake. Independent of `coop--lossy-link-delivery`'s protocol and wire bump; whichever lands second rebases its version.
- **Docs owed at promotion.** `ui.md` §3: the target bullet names the connected-client projection. `networking.md` §What gates: the timed-slot rate and its projection. `networking.md` §Combat authority: the heat and cell display sentence. `docs/scripting-reference.md`: one sentence that a timed meter advances smoothly on clients with no author change.
- **Non-goals.**
  - The UI clock, frontend tweens and Loading animation: `drafts/ui-tween-clock` owns them.
  - Loss recovery on the reliable channels: `ready/coop--lossy-link-delivery` owns it.

## Acceptance
### Automated
- [ ] Reproduces first. On the build this starts from, a connected client's reload-progress display under the profile changes only on frames that apply a sample, and the test is committed failing.
- [ ] Permit: on a connected client under loss and jitter, with production's newest-snapshot gate applied, each enumerated value advances monotonically through lost and reordered snapshots. Each frame's change is at most the replicated rate × frame time plus a small epsilon.
- [ ] Refuse: the target falls on a host cancel or slot change, never passes completion before the host completes, and the reload meter disappears when its active flag falls.
- [ ] A reversing value, such as heat rising while firing and then cooling, follows the host's direction change within one sample.
- [ ] Outage: with no sample for longer than the cap, the target holds at the cap's projection and does not move until the next sample. Permit: a single lost snapshot inside the cap leaves the advance unbroken.
- [ ] State crossings on an enumerated slot fire once per host crossing, never on the projection's advance.
- [ ] A tween bound to an enumerated slot eases toward the moving projection; with reduce motion on, it shows the projection that frame.
- [ ] Host and single-player displays are unchanged.
- [ ] No client heat or cell state is written: the host-sample store slots and the client weapon components match a build without the projection.
- [ ] A peer on the previous snapshot version is refused at the handshake, if the version moved. Permit: a peer on the new version is admitted.

### Manual
- [ ] Loopback recipe (`networking.md` §Manual loopback recipe), client conditioned at 45/60/5: the reload meter and a heat weapon's meter move smoothly, and the heat meter follows a fire-then-cool cycle.

## Path
- Seams: `hud.ts` `reloadMeter` (untweened bind of `player.reloadProgress`). `PlayerHudStatePublisher::tick_for_role_and_report_sampled_weapon`, whose connected-client branch skips reload. `WeaponComponent::reload_feedback_sample`. `ReplicatedWireShape::WieldableSlotNumber`, shared by reload progress and weapon cooldown. `ClientStateApply`. `PresentationBindings::presentation_host_time`, with consumers in mesh_anim.rs and overlays.rs.
- Harness: `state_slot_loss_harness_test.rs` lacks production's newest-snapshot gate; add it first.
- Rate shape: `done/coop--networked-by-default` Decisions 8–9 replicate a host start time (`anim_start`) and let the client derive age, which is the closest precedent for the delegated choice below.
- First slice: the gated harness and the failing reproduction row.
- `research.md` has the symbols.

## Open questions
- Replicated rate shape: total duration, remaining time, start time or rate. — **delegated**: pick from the enumerated slots and report it in the plan of record.
- The hold cap's value. — **delegated**: at least two snapshot intervals plus the profile's jitter; report it in the plan of record.
