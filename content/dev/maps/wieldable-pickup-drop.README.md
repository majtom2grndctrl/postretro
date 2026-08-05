# wieldable-pickup-drop

Manual E16 fixture for world wieldable acquisition and drop.

```bash
cargo run -p postretro-level-compiler -- content/dev/maps/wieldable-pickup-drop.map -o content/dev/maps/wieldable-pickup-drop.prl
cargo run -p xtask -- run content/dev/maps/wieldable-pickup-drop.prl
```

The player starts with the reference shotgun in slot 1 and pistol in slot 2.
Both are touchable, visible when dropped, and retain their weapon state.

1. Walk forward into the first SMG. It uses `auto` mode and disappears on the
   enter edge as it fills an empty inventory slot.
2. Continue to the second SMG. It uses `press` mode. It remains in the world
   until you press **E** while in range.
3. Press **2** to select the pistol, then **G** to drop it. The world mesh
   appears in front of the player. Step away and walk back into its radius to
   re-acquire it through `auto` mode.
4. Press **1** to select the shotgun, then **G**. Its dropped mesh remains
   until you stand in range and press **E**, exercising `press` re-acquisition.

For a co-op check, run the same PRL with `--host` on one machine and connect a
second client. Both map pickups and dropped weapons should appear once, vanish
after host-authoritative acquisition, and leave no duplicate held world item.
