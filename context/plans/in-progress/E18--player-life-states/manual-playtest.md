# E18 client visual proof

Status: outstanding. No visual result is inferred from CPU tests.

Use this branch's dev-tools engine and freshly compiled dev content. Build the
engine, `scripts-build`, `prl-build` and `postretro-tool` before the run; the
coordinator checks for active `rustc` or `prl-build` before every build and
defers when another session is building. Run from the workspace root.

```sh
target/debug/prl-build content/dev/maps/coop-presentation-test.map \
  --baked-root baked --cache-dir .build-caches/prl-cache \
  -o content/dev/maps/coop-presentation-test.prl
```

The compiler also needs the same-directory `scripts-build` helper. In three
terminals, use separate app names to keep these runs' settings/progression
separate:

For simultaneous controls, run the owner on a second machine with matching
content and replace `127.0.0.1` with the host's reachable address. An alive
teammate can then operate recovery while the owner keeps running.

```sh
RUST_LOG=info target/debug/postretro-tool run --install-root . \
  maps/coop-presentation-test.prl --host 27015 --app-name Postretro-E18-Host
```

```sh
POSTRETRO_NET_LAG=75 RUST_LOG=info target/debug/postretro-tool run --install-root . \
  maps/coop-presentation-test.prl --connect 127.0.0.1:27015 --app-name Postretro-E18-Owner
```

```sh
RUST_LOG=info target/debug/postretro-tool run --install-root . \
  maps/coop-presentation-test.prl --connect 127.0.0.1:27015 --app-name Postretro-E18-Observer
```

Condition only the owning client. Its proxy applies 75 ms in each direction,
about 150 ms RTT. Repeat with `POSTRETRO_NET_LAG=45,60,5` (or Standard in the
client's Network tab) for delay, jitter and 5% loss. Do not condition the host
too: that would add delay again.

1. On the owner, compare an ordinary crouch-key press/release with the life-state
   transitions below. Record frame rate and crouch/eye behavior.
2. Run through the red pad at editor X=352..416, Y=160..288. Keep moving while
   crossing. The owner should ease into crouch/crawl without a position or view
   snap; fire still works. The down bar and vignette appear only for that owner.
   Switch, Use, pickups and new Touch entries should be refused. The other two
   players stay alive and can act normally.
3. Before 15 seconds, have an alive teammate press Use inside the blue recovery
   pad at X=352..416, Y=416..480. The runner eases back to full speed and normal
   eye control, without a teleport cut. The bar clears. Compare each eye-height
   change with the crouch-key reference.
4. Repeat the down run and let the full timer expire. At the 15-second gameplay
   deadline the owner respawns at a free seat/map placement, with restored HP,
   retained inventory and no carried motion. This path cuts the owner's view;
   the observer sees a cut too. No teleporter arrival cue plays.
5. Watch or record the owner's bar at a frame rate above 60 Hz. It must advance
   on every positive-duration game frame, including zero-tick frames and while
   snapshots/corrections arrive. It must neither hold nor move backwards during
   one down state. It may reset on a real state/pawn/teleport-generation reset.
6. Repeat steps 2–5 with jitter/loss, then swap client roles. Join the observer
   while the first client is down: the teammate should already appear crouched.

| Check | Result | Evidence / settings |
|-------|--------|---------------------|
| AC17 down from full run: smooth correction and eye ease | outstanding | |
| AC17 return to alive: smooth correction and eye ease | outstanding | |
| AC17 respawn is the life-state cut | outstanding | |
| AC18 continuous owner bar, 150 ms RTT | outstanding | |
| AC18 continuous owner bar, jitter/loss | outstanding | |
| Supporting crawl/fire/other-pawn and late-join observations | outstanding | |

Record build SHA, platform, impairment, roles, frame rate, accommodation
settings, and observations or clip paths. A failing visual result returns to
implementation; a passing result still needs the owner's separate
"land the plane" instruction.
