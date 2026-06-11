# anim-demo — M10 skinned-animation runtime demo

DEMO CONTENT exercising the per-entity animation state / crossfade path end to
end: a descriptor-declared animated mesh entity is placed in a map and switched
between animation states by a tag-targeted `setAnimationState` reaction.

## Files

- `content/dev/scripts/anim-demo-grunt.ts` — `defineEntity({ canonicalName:
  "anim_demo_grunt", components: { mesh: { ... } } })`. Declares the per-entity
  animation-state map (`idle`, `alert`) and `defaultState: "idle"`. Registered
  into the mod via `content/dev/start-script.ts`'s `setupMod()` `entities` array.
- `content/dev/scripts/anim-demo-reaction.ts` — the level **data script**
  (`setupLevel`). Returns a `levelLoad` Primitive reaction that fires
  `setAnimationState { state: "alert" }` against entities tagged `demo_grunt`.
  Wired into the map via the worldspawn `data_script` KVP.
- `content/dev/maps/anim-demo.map` — a minimal walkable room with a
  `player_spawn`, the animated `anim_demo_grunt` entity, a stateless `prop_mesh`
  of the same model for contrast, and one `light`.

## Compile

```bash
# Compile the map (also compiles + embeds the data script via its data_script KVP)
cargo run -p postretro-level-compiler -- content/dev/maps/anim-demo.map -o content/dev/maps/anim-demo.prl
```

The mod entry script (`start-script.ts`, which imports the grunt descriptor) is
auto-compiled by the engine at startup in debug builds. To bundle it manually:

```bash
cargo run -p postretro-script-compiler --bin scripts-build -- --in content/dev/start-script.ts        --out /tmp/start-script.js
cargo run -p postretro-script-compiler --bin scripts-build -- --in content/dev/scripts/anim-demo-reaction.ts --out /tmp/anim-demo-reaction.js
```

## Run

```bash
cargo run -p postretro -- content/dev/maps/anim-demo.prl
```

## What to look for

- The grunt (at origin `176 96 16`, facing the player) **spawns in `idle`** — a
  looping playback of the model's single clip.
- At level load the `levelLoad` reaction switches the grunt to **`alert`**: the
  animation runtime **crossfades over 250 ms** (`crossfadeMs: 250`,
  `interrupt: "smooth"`) from the running idle timeline into the alert state.
  Because both states reuse the one shipped clip, the crossfade blends two
  divergent clip-local poses of the same clip — the transition is the visible
  proof of the descriptor → mesh component → state-switch → crossfade path.
- The `prop_mesh` beside it (origin `176 176 16`) carries **no animation state**
  and stays a stateless mesh (rest / first-frame pose) — the side-by-side
  control showing the animated vs. stateless paths.

## Multi-clip-model swap

Both `idle` and `alert` intentionally point at the same clip, because the only
skinned model shipped in the repo
(`content/dev/models/decraniated_low_poly_retro_pixel/scene.gltf`) exposes
exactly one clip, named `mixamo.com`. To make the two states play *distinct*
animations:

1. Drop a multi-clip glTF under `content/dev/models/<your_model>/`.
2. In `anim-demo-grunt.ts`, point `model` at
   `models/<your_model>/<file>.gltf`.
3. Give each state a DISTINCT authored clip name from that model, e.g.

   ```ts
   animations: {
     idle:  { clip: "Idle",  loop: true },
     alert: { clip: "Alert", loop: false, crossfadeMs: 250, interrupt: "smooth" },
   },
   defaultState: "idle",
   ```

   `clip` is resolved against the model's clip metadata at level load.

## Authoring notes / caveats

- **Player-start classname:** `player_spawn` (confirmed in
  `sdk/TrenchBroom/postretro.fgd` and `build_pipeline.md` §Built-in Classname
  Routing, where it is in the engine-special exclusion set).
- **Reaction trigger shape:** reactions are surfaced through `setupLevel`'s
  returned `LevelManifest` (`{ reactions }`), NOT through `setupMod`. The body
  used here is a `PrimitiveReactionDescriptor`
  (`{ primitive, tag, args }`); the `levelLoad` reaction name mirrors
  `arena-lights.ts` and fires once at level load. A one-shot level-load switch
  is the cleanest observable transition available without an AI / timer system.
- **Descriptor placement:** a descriptor carrying `components.mesh` is directly
  map-placeable via `"classname" "anim_demo_grunt"`, resolved by the level
  loader's second dispatch sweep against `canonicalName`.
