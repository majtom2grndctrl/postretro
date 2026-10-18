# movement--mover-pinned-rider — research

Read at c1cad7e. Read-only audit; no build or test was run. Symbols cite `crates/physics/src` unless noted. "Inferred" marks player experience reasoned from the code, which the brief's reproduce-first fixtures confirm.

## How a mover moves a player today

Order inside one player movement tick (`integrate_collision`, movement/substrate.rs):
1. **Carry** (`apply_mover_carry`, movement/mover_carry.rs): when the carry ground is `Mover(id)`, revolve the position about the mover pivot by the tick rotation, then add `tick_delta`. Unswept, with no static test of any kind. Carry ground is `previous_ground`, or the vault target's mover while vaulting (movement/mod.rs, `carry_ground`).
2. **Displace** (`displace_from_movers`): the deepest mover push on the capsule, then up to four settle iterations. Each push goes through `unblocked_mover_displacement`. If `mover_push_is_blocked_by_static` (a skin cast of the capsule along the push normal for the full push depth) hits anything, the whole push is refused and the position is left unchanged. The push is all-or-nothing: static geometry 1 cm away refuses a 5 cm push entirely. For a rider, its own mover's swept push is excluded (`deepest_mover_push_penetration_excluding_swept`); only a resting overlap with it counts.
3. **Checked start** is sampled here (`checked_start`), so the overlap check never sees carry or displacement (`movement.md` §1).
4. Own movement: slide sweeps against static and movers (`cast_capsule_query`), step-up, snap-to-floor.

Host blocking pass (`run_mover_blocking_pass`, kinematic_mover/blocking.rs), after player movement, triggers and agent steering (`simulate_tick`, crates/sim/src/sim/mod.rs; `entity_model.md` §5):
- **`displace` movers never enter the pass** (the snapshot filter is `policy != Displace`).
- **`stop` / `reverse`:** any leading contact reacts (`leading_mover_contact_penetration`: this tick's contact, then the next tick's preview pose). A player grounded on the mover, or airborne inside `RIDER_GRACE_MS` (150 ms) of leaving it, is skipped outright. That skip has no pin test.
- **`crush`:** a player is pinned when this tick's push from that mover (swept push included, so a rider's own platform counts) is blocked by static (`mover_push_is_blocked_by_static`). Grace does not apply.
- **Agents:** stop/reverse react to any contact; crush treats any push overlap as pinned; displace ignores them. Agents are never carried or displaced: their controller is static-only (`movement.md` §6, §7).
- **Stop hold:** the replicated `blocked` flag. The driver publishes a zero-motion pose, spin included (`blocked_mover_pose`, kinematic_mover.rs). The pass re-checks against `preview_next_mover_pose(.., ignore_stop_hold = true)` and resumes when clear. The stale-hold sweep at the top of the pass clears `blocked` on any mover whose policy is not `stop`.
- **Reverse:** `reanchor_direction`, which clears `target_segment` and restarts the mover. It only reverses when the tick-end heading points into the contact normal. A pathless `reverse` degrades to `stop` (`effective_block_policy`).

Networking: block policy, cadence and grace are host-only, off the wire and off the content digest. Clients reconcile only the resulting phase: `blocked`, direction and segment (`networking.md` §Current contract, the mover prediction and block reaction paragraphs). Clients predict mover motion from phase (mover history in crates/netcode/src/reconcile.rs) and predict their own pawn's carry with the same substrate.

## Every way a mover leaves a player inside or against static geometry or another mover

| # | Case | Policies affected today | Mechanism | Today's outcome (host and client alike) |
|---|---|---|---|---|
| 1 | Rider lifted into a ceiling (headroom at the top stop lower than lift top plus capsule) | displace, stop, reverse | Unswept carry. Displace ignores the rider's own platform. Stop/reverse skip the rider. | The head goes into the ceiling and keeps going while the lift moves. Pinned by `a_pawn_carried_into_a_static_ceiling_rides_on_and_never_trips_the_assertion` (movement/overlap_tests.rs), which asserts the pawn ends above a single-plane ceiling. |
| 2 | Rider overhanging the platform edge, lift rising past an upper-floor slab edge | same | same | The capsule overhang passes through the slab while the lift moves. It is transient if the lift clears the slab, and persistent if the lift stops inside it. |
| 3 | Rider carried horizontally or by rotation into a wall, pillar or arch (a platform under a low arch, a carousel rim under a column) | every policy, crush included | Unswept carry. The own platform's push is tangential, so crush's push-based pin never fires (inferred). | The rider is carried through the wall. Under crush, no damage (inferred). |
| 4 | Non-rider squeezed against static by a translating mover: a door closing on a pawn at the jamb, a slider into a wall, a lift descending onto a pawn standing under it | displace | The push is refused when blocked; the mover keeps moving into the capsule. | The pawn is left overlapping the mover. A thin mover passes through it, so it ends clear on the far side. A thick one that stops ends with the pawn inside or wholly within its hull (`a_pawn_pushed_out_of_a_mover_toward_a_static_wall_does_not_trip_the_assertion` leaves the pawn 0.3 m inside the face, asserting only x < 1). |
| 5 | Same, with a rotating door | displace | Rotational swept push, refused the same way | Same as 4. |
| 6 | Airborne former rider caught by its rising lift against a ceiling inside the grace window | displace (push refused); stop/reverse (grace skip) | | Left overlapping the lift, under the ceiling. |
| 7 | Vaulting body riding a rising vault-target mover into a ceiling | as 1 | Carry uses the vault mover. The pass reads `movement.ground`, not the vault mover. | As 1 (inferred). |
| 8 | Body pinned between two movers: a default double door whose leaves meet on a pawn, or a lift rising into a closed hatch mover | displace (stop/reverse react to the leading contact first) | `mover_push_is_blocked_by_static` ignores other movers. `displace_from_movers` settles against every mover's resting overlap, so the body is pushed into the other leaf and back for up to four iterations. | The pawn ends inside one leaf. Leaves that stay closed (a `once` door) never free it. That is the same softlock with no static geometry involved, so the brief counts another mover as a pin blocker. |

Thin or fast movers: the swept push (`sweep_mover_against_capsule`) detects a face crossing the capsule within a tick (`swept_mover_push_detects_thin_mover_crossing_capsule`). `point_inside_mover` serves only teleport arrival and `capsule_fits_combined`, not displacement. At the content speeds read (≤ 15 m/s, 0.25 m per 60 Hz tick, below a 0.8 m capsule diameter) the swept push sees every crossing.

## What the player experiences (inferred)

- **Embedded in a static ceiling (cases 1, 2, 7):**
  - Horizontal input still moves: an embedded skin cast blocks only closing motion, and tangent or separating motion passes (collision/skin_cast.rs, "rate >= 0 → no hit").
  - Jump closes on the ceiling: a TOI-0 hit, so no rise.
  - Crouch shrinks from the feet while grounded (`movement.md` §4 anchor rule), so it can lower the head clear. Stand-up then refuses, because the anchored probe sweeps up into the ceiling.
  - The camera can sit above the ceiling face, showing the void or the room above.
  - When the core passes a triangle, the skin cast defers to parry's cast and behaviour is unpinned.
  - A lift that later descends carries the rider out. A one-way lift, or one that stops at the top, leaves the rider embedded. That is the softlock.
  - The overlap check stays silent on every such tick, because the tick starts embedded (`movement.md` §1).
- **Inside a mover (cases 4, 5):**
  - Walking away from the mover face separates, so it is allowed.
  - Walking into it is blocked.
  - Inside a closed door hull every direction closes on an inner face, so the pawn waits for the door to open.
  - A `once` door that never reopens is a softlock.
- **Co-op:**
  - Carry and displacement are deterministic on host and client, so both peers embed identically and no correction fires.
  - The client's camera shows the embed exactly as the host's pawn has it.
- **After the fix (the brief's direction):**
  - The host holds or reverses the mover one tick ahead.
  - The client still predicts the mover moving until the snapshot carrying the new phase arrives, so for that window its own pawn is carried into the ceiling as today. The reconcile then replays against the held mover and snaps the pawn back out.
  - The correction is bounded by the mover's travel over the client's prediction lead. That is the same correction class a `stop` door closing on a client pawn already takes.
  - The loopback harness (crates/netcode/src/predict_reconcile_harness_test_fixtures.rs, `host_simulate_tick`) runs the mover driver but not the blocking pass, so it cannot show this today.

## Content exposure

`block_policy` is unauthored, so `displace`, on every mover in spawner-test, vault-test, coop-presentation-test, teleporters (all three), switch-demo, closet-reveal and kinematic-platform (six movers: two ping-pong platforms at 15 m/s, three more ping-pong movers, and a spinning carousel), plus one mover in stress-env-volumes. campaign-test and the stress-warren maps author `reverse` or `crush`. `docs/` has no movers or block-policy section (`docs/level_design.md`). The FGD's only help text is "Host-authoritative response when this mover contacts an entity" (sdk/TrenchBroom/postretro.fgd).

## Prior commitments this brief touches

- `done/E17--doors-blocking-movers` AC4: "the unauthored `displace` default behaves byte-for-byte as today: … the push no-ops (no damage, no mover reaction) when blocked." Its own Problem paragraph named the softlock this leaves ("a closing mover soft-locks a pinned player") and answered it with authorable policies, not a changed default.
- `done/E17--rider-not-a-blocking-contact`, out of scope: "A stop mover carrying a rider into a ceiling does not auto-stop to protect them; the reactive path has no static-pin logic and gains none here." Also rejected: "Unify the reactive and crush arms on the relief test", because stop/reverse must still fire on pushable obstructions. The brief keeps that rule: ordinary contacts react as before, and only the rider skip gains a pin test.
- `movement.md` §6: displace is "push out of overlap, never yield"; "a mover never blocks, reverses for, or crushes a player riding it … Only the free-riding contact is exempt."
- `movement.md` §1, Not covered: names this brief's softlock.
- `done/movement--static-overlap-invariant`: carry and displacement are exempt from the overlap check, "policed by mover policy".
- `context/research/enemy-mover-interaction.md` §8: slice 1 moves agents onto a shared mover-contact substrate and uses "the player's pin predicate" for agent crush. The predicate this brief defines is the one agents inherit.

## Prior art

From `context/research/enemy-mover-interaction.md` §2 and id/Valve source, not re-read this session:
- **Quake 1:**
  - `SV_PushMove` (sv_phys.c) moves the pusher and its riders, tests each pushed entity's position, and on a block restores every position (the pusher does not advance this frame). It then calls the pusher's `blocked`.
  - `door_blocked` damages, then reverses unless `wait < 0`.
  - `plat_crush` damages 1, then reverses.
- **Quake 2:** `door_blocked` (g_func.c) damages, then reverses unless `DOOR_CRUSHER`.
- **Doom:** `T_MovePlane` moves the plane, runs `P_ChangeSector`, and undoes the move when a thing does not fit and the plane is not crushing.
  - Normal doors reverse.
  - `close`/`blazeClose` doors keep pressing.
  - Non-crushing plats reverse.
- **Source:** `CBaseDoor::Blocked` damages, then reverses unless `forceclosed`.

Pattern: every classic engine's default pusher refuses to move into an entity it cannot push clear of. It unwinds that frame, then holds or reverses. Only an opt-in crusher keeps pressing. PostRetro's `displace` default is the outlier. Its carry is body-side and predicted, so the unwind becomes a one-tick preview on the host (the existing stop-hold preview), not an undo stack.

## Rivals weighed

| Shape | For | Against |
|---|---|---|
| **Mover yields to a pinned body (chosen)** | Only the mover can create free space when a body sits between it and static geometry. Uses the existing replicated hold and reverse phase, so no wire change. Matches classic defaults. Leaves every unpinned case untouched. | Changes the default policy where a pin occurs. The client mispredicts the mover until the hold arrives. |
| Body-side static depenetration (Quake `SV_CheckStuck` nudge, Source unstick) | Catches any cause | A pinned body has no free pose nearby: both sides are solid. A nudge can push through thin walls or out of the level. It is the rescue the landed invariant brief rejected for own movement. |
| Swept carry (clip the carry at static) | The client never embeds during the latency window | The platform then rises into the rider's feet, and the displacement is refused, so the body ends inside the mover instead. It still needs the mover to yield, so it adds a mitigation, not a fix. It changes the carry exemption in `movement.md` §1. Rejected by the owner. |
| Change the FGD default to `stop` or `reverse` | No new semantics | `stop`/`reverse` react to every pushable contact, so a default slider that pushes a pawn across open floor would halt instead. Every unauthored mover in content and in mods changes behaviour, not only the pinned case. |
| Load-time headroom check on a lift's stops (`/validate-plan`) | Catches the authored-too-low stop before play | Complement, not rival: misses doors, overhangs, arches and carousels. The once-per-mover warning covers it at runtime. |
| Partial advance: move the mover up to the pin, not a whole-tick hold (`/validate-plan`) | Smaller gap left by a hold | A per-tick fraction would ride the wire; the gap a hold leaves is at most one tick's travel (0.25 m at 15 m/s) |
| Client predicts the yield for its own pawn | No latency correction | Block policy and `setBlockPolicy` would have to enter the digest and the wire (`networking.md`: "a deterministic input to client prediction belongs in the hash"). That is a one-way door for a latency polish. |

## Pin predicate detail

- **Rider:** the body the mover carries, keyed on carry ground (grounded on the mover, or vaulting onto it). It is pinned when its next carry would end inside static geometry or another mover. That is the capsule swept from its current position to the carried position (the revolve-then-translate end; the chord approximates a per-tick arc).
- **Airborne inside the grace window:** the body is not carried, so it falls under the non-rider test.
- **Non-rider:** pinned when the mover's push on it is blocked by static geometry (the existing `mover_push_is_blocked_by_static`) or by another mover's collider (case 8).
- **Rider blockers:** the rider sweep counts the same blockers, the rider's own mover excluded.
- **Both tests run twice:** against this tick's pose (a pin already made) and against the preview of next tick's pose (a pin about to be made).
- **Exactness:** the host's preview equals the next tick's driver step. Trigger commands have already mutated phase, commands from the frame-end script drain are re-tested before the next mover tick (Q7), and the next carry runs from the same position the pass sees, because carry is the first thing the next movement tick does. So on the host a rider is never carried into static geometry, barring its own swept movement, which the invariant already covers. Another mover is tested at its current pose, so a pin against a second moving mover can be caught one tick late. That is within the brief's one-tick-of-travel bound.

## Implementation notes for the executor

- `reverse` needs a contact normal for its heading test. For a pin, use the push or carry direction; `displace` uses the same test.
- The stop hold is gated on `policy == Stop` in four places in the pass, not one: the stale-hold sweep, the preview's `ignore_stop_hold` argument, `policy_is_active_this_tick`'s held-mover arm, and the `blocked = stop_contact` write with its onset event. A `displace` or `reverse` hold needs each. Missing the preview or the activity arm drops a held mover from the snapshot, so it never resumes.
- movement/mod.rs is past 6000 lines.
- Dev map: the content maps were not checked for pin geometry. A new `content/dev/maps/` map (low-ceiling lift, overhang lift, jamb door, double door, descending lift, rotating door, low-arch platform, carousel and pillar) gives the manual rows a place.

## Ordering pins (`/review-brief`)

Host tick order (`entity_model.md` §5): mover tick → player movement → triggers (commands mutate phase) → … → agent steering → auto-close timers → blocking pass.

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| Q1 | auto-close timer expires on a door a pawn pins | auto-close sets the closing course and clears `blocked` → blocking pass previews the new course | a course into the pin yields again that tick; a course away from it moves; never into the pawn |
| Q2 | `start` or `reverse` trigger command on a held, pinned mover | trigger tick clears `blocked` (`apply_mover_command`) → blocking pass previews the commanded course | a course into the pin yields again that tick (the blocked event may re-fire, as a stop hold's does today); a course away from it moves; never into the pin |
| Q3 | pinned pawn despawns or its client disconnects while the mover holds | despawn → next blocking pass finds no pinned body | the hold releases; the mover moves on the following tick |
| Q4 | `setBlockPolicy` while held: displace→crush, or crush→displace mid-pin | trigger-stage command → pass; or frame-end drain command → host re-test before the next mover tick (Q7) | to crush: resumes the next tick and crushes; to displace: yields the next tick |
| Q5 | preview pins on tick N | pass on N sets hold or reversal → mover tick N+1 → carry on N+1 | hold: zero delta, so the rider ends N+1 where it ended N; reversal: the carry moves the rider away from the pin |
| Q6 | `reverse` mover pinned by spin, or with its path heading off the pin | pass finds the pin → heading test fails | holds, so the pin never goes unanswered |
| Q7 | a script reaction, sequence step or trigger `wait` tail runs `moverStart`, `moverReverse`, `moverGoToPathNode` or `moverSetSpinRate` on a mover holding for, or one tick from pinning, a rider | blocking pass (last stage of the frame's last tick) → frame-end script drain applies the command (`Start`/`Reverse`/`GoToPathNode` clear `blocked`) → next mover tick runs a course no pass previewed | a course into the pin never moves; the rider never ends a tick inside static geometry; a course away from the pin moves |
| Q8 | `displace` or `reverse` mover pinned ahead with a second body pinned on its reversed course (a rider against the ceiling, a pawn under the lift) | pass previews only the current course → a reversal commits a course no pass previewed → next mover tick runs it | holds: never reverses onto the second body, never flips on successive ticks; one blocked event at onset |
