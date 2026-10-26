# Host switch timing — manual acceptance

AC31 remains outstanding and blocks landing. Record results for both profiles; automated tests do not supply this visual proof.

## Fixture

Use this feature checkout and its own target directory. Temporarily add `lowerMs: 600` and `raiseMs: 250` inside `components.weapon` in the three dev descriptors:

- `content/dev/scripts/reference-shotgun.ts`
- `content/dev/scripts/reference-pistol.ts`
- `content/dev/scripts/reference-rifle.ts`

These occupy the first three player loadout slots. Start every peer after making the same edits so their authored timing agrees. Keep these fixture edits out of the feature commit; remove only these added fields after testing.

## Preparation

From the workspace root, build the authoring engine, tool and scripts sidecar in the standard dev profile:

```sh
CARGO_INCREMENTAL=0 cargo build -p postretro -p postretro-tool -p postretro-script-compiler --features postretro/dev-tools --bins
```

This worktree does not initially contain the ignored campaign PRL. If absent, compile it once (this is a content bake):

```sh
CARGO_INCREMENTAL=0 cargo run -p postretro-level-compiler --bin prl-build -- content/dev/maps/campaign-test.map -o content/dev/maps/campaign-test.prl
```

The tool launches already-built helpers. `--install-root .` supplies the checkout's engine assets; the level path is relative to the dev mod.

## Peers

Listen host:

```sh
RUST_LOG=info target/debug/postretro-tool run --install-root . maps/campaign-test.prl --host
```

Shooter (replace `<port>` with the host's reported port):

```sh
POSTRETRO_NET_LAG=45,60,5 RUST_LOG=info target/debug/postretro-tool run --install-root . maps/campaign-test.prl --connect 127.0.0.1:<port>
```

Second observing client:

```sh
RUST_LOG=info target/debug/postretro-tool run --install-root . maps/campaign-test.prl --connect 127.0.0.1:<port>
```

Condition the shooter only. Its proxy shapes both directions. Keep the observer facing the shooter; compare both windows or a recording. Weapon models change at repoint; this brief adds no lower/raise animation.

## Actions and expected results

1. Switch among the three fixture weapons. The outgoing weapon stays selected during the lower. The observer changes once, near the shooter's repoint, with no early incoming model.
2. Switch, then drop midway through the lower. The outgoing weapon lands in the world; the shooter and observer end on the incoming weapon. Recover the dropped weapon and confirm it can fire.
3. Fire enough rounds to permit reload. Switch and press reload during lower, then repeat during raise. Neither press starts a reload or transfers reserve. A fresh press after raise starts a reload. Also begin a non-blocking reload, switch away, then return: no resumed partial reload.
4. Rapidly retarget A→B→C during one lower. Both peers end on C. The observer sees one change to C, with no B flash and no stuck selection.
5. Hold fire through a switch. The first shot after raise works; neither lower nor raise permits an early shot.

Restart the shooter with `POSTRETRO_NET_LAG=45,60,10` and repeat all actions. Keep host and observer unconditioned.

## Result record

| Profile | Switch / observer | Mid-lower drop / recovery | Reload lower / raise | Retargets | Held fire | Result |
|---|---|---|---|---|---|---|
| 45/60/5 | outstanding | outstanding | outstanding | outstanding | outstanding | outstanding |
| 45/60/10 | outstanding | outstanding | outstanding | outstanding | outstanding | outstanding |

Record tested commit, machine, authored durations and any visible mismatch. Report the result before requesting landing. Remove the temporary timing fields and stop all three processes afterward.
