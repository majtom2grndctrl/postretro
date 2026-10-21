# coop--timed-slot-projection — research

Moved from `coop--lossy-link-delivery/research.md` via `drafts/ui-tween-clock` (read at b0a5c0f; symbols rechecked at d6f3471).

- `hud.ts` `reloadMeter` binds `player.reloadProgress` with no tween, and visibility follows `player.reloadActive`.
- On a connected client, `PlayerHudStatePublisher::tick_for_role_and_report_sampled_weapon` skips reload publish. The value is the host's `WeaponComponent::reload_feedback_sample`: an endpoint stream, else `1 - remaining/total`. It is sent as `ReplicatedWireShape::WieldableSlotNumber` at `SNAPSHOT_TICK_INTERVAL` = 2 (~30 Hz, unreliable).
- `ClientStateApply` writes it straight into the store. Jitter reorders snapshots, and newest-wins drops the late ones. A loss leaves a ~66 ms hold.
- `WEAPON_COOLDOWN_SLOT` (`player.weaponCooldownMs`) shares the wire shape, so the executor checks whether any HUD binds it.
- `PresentationBindings::accept_presentation_clock` / `presentation_host_time` (client/bindings.rs): host anchor + RTT/2 + (local `script_time` − local anchor), re-anchored on every accepted snapshot (`client.rs`, snapshot accept). Rejected as the projection clock: arrival jitter moves it per snapshot and it leads the samples by half the RTT. The damaged-enemy roster in presentation/overlays.rs uses it only for expiry.
- Host resource timers advance per fixed sim tick: `tick_weapon_resources(registry, tick_dt)` → `advance_weapon_resource` (cooling after `cool_delay_ms`, regen after `regen_delay_ms`). Heat and cell also change per shot (`ShotResourceCost::Heat`, `::Cell` in sim/weapon/execution.rs).
- Slot send rule: `ServerStateReplication::produce_for_client` emits a FullBaseline, a Delta against the acked baseline, or nothing for a slot the client holds at the current baseline.
- Timed versus per-shot change: reload progress, cooldown countdown, cooling (after `cool_delay_ms`, or at once in overheat lockout) and regen (after `regen_delay_ms`) ramp per fixed tick. Heat and cell also step per shot, cooldown resets on fire, and per-shell reload progress wraps from 1 to 0.
- Endpoint samples: the owner projection's reload sample is the next unread endpoint when one waits. Started reads 0, active, although the host timer already advanced on the start tick. Completed reads 1, active, although the weapon is already idle or into its next shell. An endpoint is stamped at the tick of the snapshot that carries it, and the next snapshot carries the timer value (`WeaponComponent::reload_feedback_sample`).
- Sample spacing: the host stamps each snapshot with its frame's last tick and sends only on a cadence edge, so stamps run 1 to 4 ticks apart; the reload timer counts whole milliseconds, 16 or 17 per tick.
- `ClockEstimator` lerps its offset and smoothed RTT by 0.1 at every accepted probe (5 Hz) and reseeds outright on a low-RTT outlier, so `estimated_server_tick` minus half the RTT steps slightly, sometimes backward. A timeline built on it needs a slew and a never-rewind clamp.
- State apply can accept a snapshot and still reject its state batch whole (fingerprint, record or store validation), or hold a record back to request a baseline refresh; neither is a sample.
- Reload sounds and the overheat cue read the client's committed weapon projection, not the store. The client's cooldown reconcile reads the store's `player.weaponCooldownMs`; no HUD binds it.
- No cell grant exists: a cell has no reserve, pickups or reload, and grants cover health and ammo only. Jumps in a timed direction: a same-kind descriptor reload clamping heat down to a lowered overheatAt, a new weapon instance in the same host slot, or the host zeroing a client's cooldown at an eligible charged start. A same-kind descriptor reload otherwise keeps live values; a kind change rebuilds them only after that kind's `[slot]` absence.
- What the agreed-slope rule shows raw: a reload wrap, a cool delay ending mid-interval, a jump in the timed direction, a host cancel, a slot change and the active flag falling all break agreement.
- `StateApplyOutcome::default()` is returned for an empty batch, a batch of only held-id resends and a batch rejected whole, so intake cannot tell a held-value sample from no sample without a new signal.
- The clock estimator is never reset at level unload, manifest commit or demotion; only a new endpoint (rejoin) starts it fresh.
- Focus reachability, pointer hit-testing and accessibility predicates read the same UI snapshot values as bindings.
- Client demotion leaves `ClientStateApply` alone; its resets are level unload and committed manifest reload, and a rejoin builds a fresh apply.
- UI reads: `visibleWhen` predicates and Bar max read the same `slot_values` map as bindings.
- Timed-slot extrapolation contrast: E17 movers and `anim_start` replicate a schedule and run its closed form uncapped; M15 and E10 extrapolate a pose from velocity and hold after a cap.

## Ordering pins

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | Reload start, first sample | Host starts at tick t0; the next cadence sample replays the Started endpoint (0, active) at a tick ≥ t0 | Target is 0 on the frame `reloadActive` rises; no extrapolation |
| P2 | Reload start, second sample | First timed sample after the Started replay | Target is that sample; one interval, no slope |
| P3 | Ramp onset phase | Started replay or cool-delay end falls between sample ticks; first interval short or partial | First interval disagrees; smoothing starts once two full intervals agree |
| P4 | Reload cancel mid-ramp | Projection ahead of newest sample by ≤ slope × cap; fall sample (0, inactive) arrives | Target is the host's fall value on the arrival frame; no complete edge |
| P5 | Cancel near full | Host cancels at progress ≥ 1 − slope × cap; projection already at 1 before the fall arrives | Cancel edge, no complete sound; client pull reads the raw progress < 1 as reloading |
| P6 | Reload completion | Completed endpoint (1, active) applies; a later snapshot carries (0, inactive) | Target reaches 1 no earlier than one-way delay less one tick after the host completes, no later than the frame applying Completed; when Completed is lost, no later than one tick of timeline time past the completion tick unless the fall applies first; never above 1 |
| P7 | Completion and fall in one frame | Both snapshots drain in one frame | Samples taken in sequence order; target ends at the fall value; exit fade captures the host's fall value |
| P8 | Fall overtakes Completed endpoint | Completed snapshot reordered behind the fall snapshot | Newest gate drops it; not a sample; target is the fall value |
| P9 | Per-shell wrap | Samples rise toward 1; the next sample after the shell boundary is small | Clamp at 1; target drops to the post-wrap sample on its frame; no extrapolation until two intervals on the new shell agree |
| P10 | Regular fire at ≥ snapshot cadence | A shot every ≤ 2 ticks; each interval steps heat (or cell) by the same amount | Each step moves against the slot's timed direction, so nothing extrapolates; target is the sample at any fire rate |
| P11 | Two shots in one interval | Heat rises by 2 × heatPerShot in one sample | Jump; target is the sample |
| P12 | Cool delay ends between samples | Flat, partial-slope, full-slope intervals | Hold at newest until two full intervals agree; target never below the newest sample before then |
| P13 | Overheat lockout | Crossing shot latches at overheatAt; cooling starts with no delay | Heat smooths toward 0; never below 0; reaches 0 no earlier than one-way delay less one tick after the host's heat does, no later than the first latch-down sample's frame |
| P14 | Cell regen to capacity | Regen ramp toward `cellCapacity` | Clamp at the `cellCapacity` sample naming the same host slot; with no such sample, no projection |
| P15 | Cooldown countdown, then reset | Countdown reaches 0; next shot resets it | Clamp at 0; reset is a jump, target is the sample |
| P16 | Weapon switch mid-reload | First sample naming a new host wieldable slot | History resets; target is the new slot's sample; old slot's slope never applied |
| P17 | Switch away and back | A sample of another slot arrives in between | History resets on return |
| P18 | Lost snapshot at a slope change | The sample carrying the change is lost | Old slope continues until the next sample or the cap; error ≤ slope × cap; the gap interval disagrees, hold until two agree |
| P19 | Single loss mid-ramp | One snapshot lost; the next arrives inside the cap, up to 60 ms late | Slope per server tick across the gap agrees; advance unbroken, never retreats |
| P20 | Reordered snapshot | Older sequence arrives after a newer one | Dropped by the newest gate; not a sample |
| P21 | Duplicate snapshot | Same sequence twice | Not a sample |
| P22 | No record for the slot | Host omits an acked, unchanged slot | Sample at the held value, at the header tick |
| P23 | Resend at the held id | Flat value, ack in flight; apply skips the record | Sample at the held value |
| P24 | Snapshot with zero state records | Every slot acked; apply returns before the record loop | Held-value sample for every enumerated slot |
| P25 | Left-out delta | Delta against a missing or regressed baseline; refresh requested | Not a sample for that slot |
| P26 | Rejected state batch | Fingerprint or validation rejects the whole batch | Not a sample for any slot |
| P27 | Deferred or undecodable snapshot | Dropped before replication accepts it | Not a sample |
| P28 | Time-sync reseed mid-ramp | Estimator adopts offset and RTT outright | Timeline slews, never rewinds; target never retreats and stays within the base bound |
| P29 | Time-sync fold on an echo | Offset and RTT take an EWMA step | Timeline slews; per-frame change stays within the base bound |
| P30 | Time sync without a sample | Snapshots arrive before the first echo | Target is the newest raw sample |
| P31 | Level unload mid-ramp | Client state-slot apply resets | History clears; first samples on the new level show raw |
| P32 | Demotion or rejoin mid-ramp | Held: no snapshots; history and timeline clear at demotion, and rejoin builds a fresh apply | Holds at the cap until the clear; first samples afterwards show raw |
| P33 | Cap expires, then a sample | No sample for longer than the cap | Target holds at min(newest + slope × cap, bound); next sample retargets on its frame (exempt from the base bound); resumes when the gap interval agrees |
| P34 | Frame with zero snapshots | No sample this frame | Target advances on the timeline |
| P35 | Frame with two snapshots | Production drains and applies each in order | Both are samples in sequence order; store shows the last |
| P36 | Client frames below 30 Hz | At least one sample every frame | Target is the projection at the frame's timeline time, not the newest raw sample |
| P37 | Sample behind the projection | Whole-ms carry or arrival lag puts a sample slightly under the projection | Target holds, never retreats within a ramp |
| P38 | Irregular sample spacing | Host frames run 1–3 ticks; a one-tick frame off the cadence sends nothing, so stamps run 1 to 4 ticks apart | Slope per server tick; spacing alone never breaks agreement |
| P39 | Held values through a cool delay | Heat flat after a shot for longer than an ack round trip | Held-id resends, then omitted records, are held-value samples; target stays at the host's value |
| P40 | Client pull at reload end | Projection at 1; newest raw progress < 1 | Presents silent; cell dry-fire reads the raw charge |
| P41 | State crossing between samples | Host crosses a threshold and back between two samples | Nothing fires; a crossing in the applied samples fires once, at its sample |
| P42 | Reduce motion mid-ramp | Switch turns on while the target moves | Tween shows the projection that frame |
| P43 | Installed tuning differs from host | Client's installed reload time or cool rate differs from the host's live rate | Slope is the host samples' slope |
| P44 | Jump in the timed direction | A descriptor reload lowers overheatAt and clamps heat down, a new weapon instance enters the same host slot, or the host zeroes cooldown at a charged start | Intervals disagree; target is the sample. A kind change rebuilds heat at 0 or cell full only after that kind's `[slot]` absence, so it shows raw under P47 |
| P45 | Time sync's first sample mid-ramp | Snapshots apply before the first echo (join or rejoin); the echo lands while two intervals agree | Timeline starts at that echo with no clamp from the estimator's pre-sync passthrough; target never below the raw sample it showed; that frame exempt from the base bound; projection advances within two intervals |
| P46 | Sample ahead of the timeline | An agreeing sample applies while the timeline is still short of its tick (low-jitter arrival, estimate running late) | Target stays on the projection at the timeline, below the sample, within the base bound; a sample at the slot's bound sets the target to the bound on its frame |
| P47 | Heat or cell absence mid-ramp | A `[slot]` absence applies (the host's active weapon is of another kind) | Target clears on that frame, as the store does; no slope spans it; the next numeric sample shows raw |
| P48 | Rejoin to a host whose ticks run lower | New session's samples carry ticks below the old timeline | Timeline resets with the history; no never-rewind clamp carries over; projection resumes once two intervals agree |
| P49 | Post-cap sample below the held target | Outage past the cap; the late sample sits under min(newest + slope × cap, bound); gap interval agrees | On its frame the target is the projection from that sample, never the bare sample; no drop-then-jump |
| P50 | Capacity lags a new slot's cell | Cell names the new slot while capacity still names the old one (left-out delta) | No projection; cell history kept; projects once capacity for that slot applies and two intervals agree |
| P51 | Agreement breaks below the projection | A ramp slows or stops away from its bound (lost slope change, empty-slot switch, a held input tick on remote cooldown) | Target retargets to that sample on its frame, a retreat of at most a step for the interval ending at that sample; exempt from the base bound |
| P52 | Agreement onset | Target shows the newest raw sample; the next sample makes two intervals agree, typically after the timeline has passed its tick | On that frame the target steps to the projection, in the timed direction only, by at most a step; that frame exempt from the base bound |
| P53 | Onset with the raw sample ahead of the timeline | The raw sample shown has a tick the timeline has not reached; the agreeing sample arrives, so the projection from the newest fed sample sits behind the raw one | Target never moves against the timed direction from the raw sample it showed; holds until the projection passes it |
| P54 | Sample feeds mid-ramp | Timeline reaches an agreeing sample's tick, or it arrives late; whole-ms carry puts it up to 1 ms of timer off the projection line, and its interval's slope differs within tolerance | Projection re-bases on it without retreating; that frame's change stays within the base bound plus slope tolerance × slope × (its interval + the arrival allowance) |
| P55 | Host frame runs several ticks | A catch-up frame runs 2–3 ticks; the reload completes, or heat reaches 0, on an earlier tick of that frame | Timing bounds measure from that tick's due time on the virtual clock, not the frame that ran it |
| P56 | Started snapshot lost | The first active sample is a timer value | On the rise frame the target is that sample; no extrapolation (one interval) |
| P57 | Waiting sample at a slot change or break | An agreeing sample applied ahead of the timeline is still waiting when a sample naming a new host slot, or one that breaks agreement, applies | The waiting sample never feeds; target is the new or breaking sample, and no later frame returns to the old line |
| P58 | Four-tick stamp gap | Host frame ends on an even tick; the next runs one odd tick and sends nothing; the next runs three ticks and stamps four on; a snapshot is lost between two such gaps | Slope per server tick; spacing alone never breaks agreement; the next sample arrives inside the cap and the advance is unbroken |
