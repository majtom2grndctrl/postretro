# Manual proof: player revive and assist

Owner-run. The automated suite covers the host rules, the prediction maths and the dev-mod
policy on a CPU-only tick loop; it cannot show whether the bars look right at 150 ms RTT or
whether a held E feels like one gesture. Do not infer a pass from the tests. Record every run
below in the PR.

Covers: AC9 (both seats), AC8 (tap-for-Use beside a downed teammate), the AC11 bars, and the
Task 3 manual-proof list (revive given and received, release or range loss mid-hold, a
self-revive kill).

## Setup

Build from the checkout root (tools compile nothing):

```sh
cargo build -p postretro -p postretro-level-compiler -p postretro-tool -p postretro-script-compiler --features postretro/dev-tools
target/debug/prl-build content/dev/maps/coop-presentation-test.map --baked-root baked --cache-dir .build-caches/prl-cache -o content/dev/maps/coop-presentation-test.prl
```

Two terminals:

```sh
# Host (listen server). Note the port it reports.
RUST_LOG=info target/debug/postretro-tool run --install-root . maps/coop-presentation-test.prl --host 27015

# Client, conditioned. 75 ms each way through the proxy is 150 ms RTT.
POSTRETRO_NET_LAG=75 RUST_LOG=info target/debug/postretro-tool run --install-root . maps/coop-presentation-test.prl --connect 127.0.0.1:27015
```

Condition only the client. Confirm the info line for the seed and the Network tab (dev-tools)
show the delay before starting. Repeat the whole sheet once with `POSTRETRO_NET_LAG=45,60,5`
(jitter and loss). Leave `assist` on its engine default: do not rebind it, and do not save
player bindings that move Use or `assist` off E.

Roles: run the sheet twice, swapping who is host and who is client, so each machine is
helper and target once.

Getting downed: run through the red down pad at editor coordinates (352..416, 160..288). It
zeroes only the entrant's HP. Leave the pad before the bleed-out finishes so it can fire again
(it repeats on re-entry). Bleed-out is 15 s. Revive reach is 1.5 m and the hold is 3000 ms; the
hold starts when `assist` resolves, about 0.2 s after key-down, so a full revive is about 3.2 s
of holding E.

Record for each run: build SHA, platform, host/client roles, actual frame rate, lag settings,
the Network tab profile, and pass or fail with what was seen.

## Steps

1. **Revive given and received (AC9, AC11).** Target goes down. Helper walks within 1.5 m and
   holds E to completion.
   - Helper: the "Reviving" bar (a bare bar near screen centre, no label) starts moving on the
     first held tick after `assist` resolves, with no 150 ms wait, and fills smoothly each
     frame, not in host-sample steps. It never drops while the hold continues.
   - Target: the "BEING REVIVED" bar rises from the first host sample, never decreases while
     the helper holds, and does not jump ahead of the helper's bar by more than a short lag.
   - On completion the target stands up with 30 health, the bleed-out bar and red vignette
     clear, and the helper's XP readout rises by 25. No success effect shows on the helper
     before the target actually stands up.
2. **Release mid-hold.** Helper holds for about half the time, then releases E. The helper's
   bar returns to 0 with no success cue. The target's bar returns to 0. Holding again starts
   from 0.
3. **Step out of range mid-hold.** Helper holds, then walks beyond 1.5 m while still holding.
   Helper bar returns to 0 within a round trip, no success cue, no revive. Walk back in while
   holding: it restarts from 0 and a full hold still completes it.
4. **Target revived elsewhere (AC9 reset).** While the helper holds, let a second route revive
   the target (the blue recovery Use pad at (352..416, 416..480) heals everyone and enters
   `alive`, so have a third participant or the target's own teammate trigger it, or let the
   bleed-out expire and respawn the target). The helper's bar returns to 0 with no success cue.
5. **Self-revive kill (second wind).** Target goes down, then kills an enemy while down (the
   target dummies at Y=576 and Y=688, or the death enemy at (1120, 192); the pistol still fires
   while down). Target stands up with 40 health and gains 50 XP. The bleed-out clears. The other
   player gains nothing. Repeat with the target alive: killing an enemy upright pays nothing
   extra.
6. **Kill and hold in the same moment (O3).** Helper holds E close to completion while the
   target makes a killing shot. Whichever lands first, the target ends alive exactly once. If
   the kill won, the helper does not gain the 25 XP and the target does not get 30 extra
   health.
7. **Tap-for-Use beside a downed teammate (AC8).** With the target down and the helper within
   reach, tap E (release well under 0.2 s) while looking at a Use target such as a pickup.
   Use fires once on release, the revive does not start, and no `assist` bar appears. Then hold
   E past 0.2 s with no Use target: `assist` starts and Use never fires. Check both keyboard
   E and gamepad west.
8. **Both seats (AC9).** Run steps 1 to 3 from each seat. In each, confirm the helper's bar is
   predicted (starts at once) and the target's bar is projected (no backward step, no more than
   twice the host slope between frames). Note any visible snap, sawtooth or flicker when the
   host corrects the helper's bar.
9. **Two helpers on one target.** Both other players hold E on a downed third (needs three
   peers; skip if unavailable and say so). Exactly one completion fires, the second helper's
   bar resets, and only the first helper is paid.
10. **Reduce motion / flash limiter.** With reduce motion on, the bars still advance; no
    effect should flash.

## What would fail the proof

- A helper bar that waits a round trip before moving, or drops to 0 and recovers while the hold
  continues.
- A target bar that steps backward during a continuous hold.
- Any success cue (sound, flash, XP) on the helper before the target stands up.
- Use firing during a hold, or `assist` starting from a tap.
- A second completion on one target, or a paid assist after the target self-revived.
