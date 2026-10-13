# Vault Dev Stations

Manual QA map for `movement--vault`: graded ledges, headroom, mover ledges and a
pool. The map is one sealed room. Start at the west end and work east through
the stations. Every ledge riser faces west, toward the spawn.

Build a local artifact on demand:

```bash
cargo run -p postretro-level-compiler -- content/dev/maps/vault-test.map -o /tmp/vault-test.prl
```

Do not commit generated `.prl` files.

The tuning is content/dev's player (`scripts/player.ts`):

- standing capsule 2.0 m, `stepHeight` 0.5 m, jump peak about 1.16 m;
- `vault.maxHeight` 1.6 m, `vault.swimMaxHeight` 0.9 m, `durationMs` 800;
- `from: { air: "auto", swim: "auto" }` and `allowFire: false`;
- `viewFeel.vault` holds a -6° look down, easing in over the first 15% of
  the climb and back over the last 20%.

The expectations below follow from those numbers. If you retune the player,
re-derive them.

## Stations

- **Graded ledges (x 600–900).** Six ledges side by side along y, at 0.4,
  0.6, 1.0, 1.5, 2.2 and 3.0 m. The 0.4 m ledge steps up and never vaults. The
  0.6, 1.0 and 1.5 m ledges vault on a jump: the jump leaves the ground, and
  air entry is `auto`. Ground entry is off, so walking into any of them
  without a jump does nothing. The 2.2 m ledge vaults only near the jump's
  peak. The 3.0 m ledge refuses, because the peak cannot bring it within 1.6 m
  of the feet.
- **Headroom (x 1200–1500).** Two 1.0 m ledges with a ceiling slab over each
  top: 1.8 m clear on the south ledge, 2.2 m on the north. The south ledge
  refuses, since the standing capsule does not fit. The north ledge vaults.
- **Mover ledges (x 1900–2200).** Two 1.2 m `kinematic_mover` blocks, tagged
  `vault_station`.
  - `vault_still` (south) does not start, so it is a mover ledge that is not
    moving.
  - `vault_rising` (north) ping-pongs 1 m up and down at 0.5 m/s with a 500 ms
    wait at each end.
- **Pool (x 2500–2950).** A water basin with 2.0 m walls and its surface at
  1.6 m. Get in by jump-vaulting the west wall. The east deck has three
  segments, at 0.3, 0.8 and 1.2 m above the surface. From the water, the
  0.3 m and 0.8 m segments mantle (swim entry). The 1.2 m segment is above
  `swimMaxHeight`, so it needs a fluid jump first; the jump's arc then
  air-vaults onto it. The deck drops back to the floor on its east side.

## Manual Checks

### N1: feel, on the dev station

1. At each graded ledge, run and jump into the riser. The vault should read
   as one motion: a rise that rolls forward over the lip as it finishes, with
   no stop at the corner. Watch for a camera pop at entry or exit, a hitch at
   the top, or a stall at the lip. The move onto the top should speed up into
   your running pace and carry it off the ledge, with no slow crawl followed
   by a lurch. The view should never clip into the riser or the lip.
2. Watch the camera through each vault: it should dip about 6° toward the
   ledge as the climb starts, rest there through the climb, and level out as
   you step onto the top, never snapping. A vault cut short (dying
   mid-climb, or a co-op correction in N2) must still ease the view back
   level. Turn reduced motion on and confirm no dip at all.
   The weapon must stay where it sits in view during the dip.
3. On the 0.4 m ledge, confirm it is an ordinary step-up with no vault dip.
   On the 3.0 m ledge, confirm the jump just hits the wall.
4. Chains. Each should flow with no hitch between moves.
   - From the 1.0 m ledge's top, jump straight into the 1.5 m ledge beside
     it. The second vault should start cleanly from the first one's landing.
   - Slide toward the 1.0 m ledge, jump out of the slide, and vault.
   - Dash into the 1.0 m ledge, both from the ground and airborne. A dash
     reads ground or air entry by whether it is grounded. Ground entry is
     off, so a grounded dash must not vault and an airborne one must.
5. At the headroom station, the south ledge must refuse with no pop or
   clipping into the slab. The north ledge must vault.
6. On `vault_rising`, vault at the bottom, at the top and mid-travel in both
   directions. You should land planted on the block and ride it. You should
   not land floating above it, sunk into it, or snapped sideways.
7. In the pool, mantle out onto the 0.3 m and 0.8 m segments. On the 1.2 m
   segment, fluid-jump and catch the lip. Then swim under the deck lip and
   confirm nothing mantles while your eye is underwater.
8. Hold fire through a vault. Nothing fires until the vault ends
   (`allowFire: false`), and the held trigger fires on the first tick after
   it ends. Then set `allowFire: true` in `scripts/player.ts` and repeat: you
   should fire throughout.

### N2: co-op client at about 100 ms

Host this map and join it as a loopback client. Follow "Testing the
conditioned link" and its manual loopback recipe in
`context/lib/networking.md`, with `vault-test.prl` in place of
`campaign-test.prl`. Shape the loopback link to about 50 ms one-way (about
100 ms round trip).

1. As the client, vault each static graded ledge and the still mover ledge.
   There should be no visible correction: no rubber-band at the lip and no
   snap on landing.
2. On `vault_rising`, a small correction at exit is acceptable: the automated
   harness bounds it at about 0.07 m for a 320 ms vault (the dev player's
   800 ms vault has not been measured). A visible teleport is not.
3. Hold fire through a client-side vault on a static ledge. The client must
   not show a shot, muzzle flash or ammo tick that the host then takes back.

These checks are visual and in-engine. Compiling the map only proves the
content is valid. The automated vault suites cover the arithmetic, so these
checks are about feel and how a correction looks.
