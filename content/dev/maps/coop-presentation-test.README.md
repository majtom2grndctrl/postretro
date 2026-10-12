# Co-op presentation test

A sealed dev room for the co-op checks in `coop--networked-by-default`. The map
uses `scripts/coop-presentation-test.ts`; its descriptors are registered by
`content/dev/start-script.ts`. No model or sound download is needed.

From a project containing this dev content, with the SDK tools on `PATH`:

```bash
prl-build content/dev/maps/coop-presentation-test.map \
  --baked-root baked --cache-dir .build-caches/prl-cache \
  -o content/dev/maps/coop-presentation-test.prl
postretro-tool run maps/coop-presentation-test.prl --host 27015
postretro-tool run maps/coop-presentation-test.prl --connect 127.0.0.1:27015
```

For a checkout build, use `target/debug/prl-build` and
`target/debug/postretro-tool run --install-root .` with the same arguments.
Build the authoring engine, compiler and tool first. A remote peer uses the
host's address in place of `127.0.0.1`. Both peers need the same built content.

## Stations

Coordinates below are TrenchBroom editor units, with Z up.

- West spawn `(128,224)`: pistol loadout. The **blue pressure plate** occupies
  `(192..320,160..288)`. Enter fires exactly four reactions: an alarm heard by
  everyone, a flash for that edge's activators, a shake for everyone currently
  inside, and an activator-only click. Step out and back in to repeat. Swap
  host/client activators, then stand inside together; listen for one playback
  per recipient.
- **Green atmosphere plate** `(192..320,416..544)` fires once. PRL lights fade for five
  seconds, fog density and every scalar fog family change and a typed fog fade
  runs, map smoke stops and its spin tween advances, descriptor-only smoke
  stops, both descriptor light kinds pulse, the mesh-only knight switches from
  `Idle` to `Walking_A`, and the door opens. Join another client after firing;
  it should see the current state and animation age, including finished fades.
- The **door** at `(968,368)` rises vertically. Its `open_event` is `door.open`,
  using the dev positioned-sound reaction `playSound(..., { at: on.emitter })`.
  Both clients hear it at the door rather than at their camera.
- The **death enemy** at `(1120,192)` uses the shipped reference enemy. Its
  singleton progress threshold fires a PRL pulse and a global red flash on
  death. Compare the looping pulse on both screens at the same moment.
- Ten **target dummies** stand in rows `Y=576` and `Y=688`, `X=512..1024`.
  Pistol torso kills award 10 XP only to the killer, play `Death_A`, then
  disappear after 1.8 seconds on both peers. Ten kills reach 100 XP for one
  player. The **invisible target** above the purple floor marker at `(1152,448)` has a 0.8 m wide, 1.8 m tall
  authored hitbox; aim at torso height there. It has no visual body by design.
- **Retained-Health weapon** `coop_health_weapon` lies at `(128,96)`. Before
  pickup, use the small one-shot pad `(64..112,64..128)` to remove 12 of its
  48 HP. Pick it up with use, fire two shots, holster, re-equip, then drop.
  The world copy disappears on pickup and returns at the drop pose on both
  peers. Its magazine should retain the two spent rounds. Three subsequent
  12-damage pistol body hits remove it; needing four indicates Health reset.
  Do this once per fresh map; the damage pad does not repeat.
- **Heat weapon** `coop_heat_test` lies at `(128,384)` and uses the use button.
  Hold fire to reach 100 heat in four rapid shots. Only its owner takes 5 scald
  damage and hears the hiss; it cools in about 5 seconds, then that owner hears
  the cooled sound. Holstering also cools it.

Compare floating damage numbers and recently damaged enemies' health/shield
bars on both screens for host and client hits. Repeat the reported enemy-on-enemy
fight on `splash-damage-demo`: numbers and the selected bars should appear on
both screens. Join or rejoin while bars are live and confirm they retain only
their remaining lifetime. Expired or dead enemies must not regain bars. Record
other hit effects separately if they differ. Shared feedback does not change
who earns XP or receives private player feedback. Swap host/client roles.

## Player events and accommodations

The map includes `player-events.ts`: the guarded XP milestone (`XP >= 100` and
`level < 2`) gives only that player the fanfare, gold flash and shake. Its Luau
twin carries the same named reactions and fire lists. The complete map setup
also has a twin, `scripts/coop-presentation-test.luau`; to play that variant,
copy the `.map` to a separate fixture file, change only its `data_script` to
`../scripts/coop-presentation-test.luau`, and compile the copied map. Start with fresh or
backed-up dev progression if the milestone has already fired; do not delete
personal saves. After the guarded milestone, extra kills should not repeat it.

Repeat scald cycles until the weapon owner's Health crosses below 25, checking
that only that player's low-health vignette appears without network latency.
Starting at 100 HP takes sixteen 5-damage scalds; stop at 20 HP. On a client, enable the flash limiter and
reduce motion, then repeat the blue plate flash/shake and enemy global flash.
Record the configured values and check that forwarded cues honor them.

Record platform, build SHA, host/client roles, content identity, join timing,
progression starting values and accommodation settings with M1–M4 results.
Automated CPU checks cover the contracts; this map remains the owner-run visual
and audio check across Mac and Windows.
