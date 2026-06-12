# anim-demo — M10 skinned-animation runtime demo

DEMO CONTENT exercising the per-entity animation path end to end: a
descriptor-declared animated-mesh entity is placed in a map, spawned, its model
uploaded, its clips resolved, and its pose sampled per instance — then switched
between declared animation states by a tag-targeted `setAnimationState`
reaction.

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
  of the same model as a control, and one `light`.

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

## What this demo proves

The descriptor → `components.mesh` → spawn → model upload → clip resolution →
per-instance sampling path, end to end:

- The grunt (origin `176 96 16`, facing the player) **spawns in its
  `defaultState` `idle`** and plays the model's clip on a loop. Seeing it stand
  there animating is the proof that a `components.mesh` descriptor was placed
  from the `.map`, materialized, had its model uploaded, its clip resolved, and
  its pose sampled per frame.

- The `levelLoad` `setAnimationState` reaction exercises the **state-switch
  path**: it changes the grunt's current state from `idle` to `alert`. The
  switch is recorded through the same validated `switch_animation_state` entry
  point the future AI / command-buffer layer will use.

## Why the state switch is a HARD CUT here, not a crossfade

`alert` declares `crossfadeMs: 250`, but you will **not** see a 250 ms blend from
this demo. A `levelLoad` reaction fires during level install, **before any frame
has rendered**, so the grunt's entry stamp is still *pending* when
`setAnimationState` runs. A switch out of a pending current state hits the
pending-stamp collapse (`scripting/components/mesh.rs`,
`switch_animation_state`): the never-rendered intermediate is dropped, it
contributes no outgoing fade, and the transition is a **hard cut**. The grunt is
already in `alert` by the first rendered frame.

This is correct, documented behavior — not a bug. The `crossfadeMs` field is
real and is unit-tested (`mesh.rs` tests: `resolve_pass_clears_fade_after_…`,
`…retains_fade_within_crossfade_window`, the smooth/snap interrupt cases). It
just cannot be observed from a pre-frame `levelLoad` trigger.

To see an actual 250 ms crossfade you must drive `setAnimationState` **after at
least one rendered frame**, so the current state's entry stamp is already
resolved when the switch lands. Two ways:

- A **runtime trigger** — the future Enemy-AI / command-buffer layer firing a
  switch during play. That layer does not exist yet, so this demo cannot show it.
- A **`progress` reaction** (`ProgressReactionDescriptor` in
  `sdk/types/postretro.d.ts`): it fires when tagged entities cross a kill ratio,
  which only happens during gameplay — i.e. well after rendering has begun — so a
  switch it drives would produce a real crossfade. (This demo has nothing to
  kill, so it uses the simpler `levelLoad` trigger and accepts the hard cut.)

## The `prop_mesh` control

The `prop_mesh` beside the grunt (origin `176 176 16`) carries **no animation
state**. A stateless mesh is not frozen in a rest pose — it samples clip 0 on a
`Loop::Wrap` policy against the animation clock (`MeshSampleParams::stateless` in
`render/mesh_instances.rs`), i.e. it **loops clip 0**.

Because the only skinned model shipped in the repo exposes exactly one clip, the
`prop_mesh` and the grunt's `idle` are sampling the *same* clip — so there is no
visible "animated vs. stateless" contrast with this single-clip asset. The
`prop_mesh` is a stateless **control**: with a multi-clip model the descriptor
entity would switch among distinct authored states while the `prop_mesh` stayed
locked on clip 0.

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

With distinct clips the `prop_mesh` (still on clip 0) and the grunt's switched
state would differ visibly — restoring the animated-vs-control contrast.

## Authoring notes / caveats

- **Player-start classname:** `player_spawn` (confirmed in
  `sdk/TrenchBroom/postretro.fgd` and `build_pipeline.md` §Built-in Classname
  Routing, where it is in the engine-special exclusion set).
- **Reaction trigger shape:** reactions are surfaced through `setupLevel`'s
  returned `LevelManifest` (`{ reactions }`), NOT through `setupMod`. The body
  used here is a `PrimitiveReactionDescriptor`
  (`{ primitive, tag, args }`); the `levelLoad` reaction name mirrors
  `arena-lights.ts` and fires once at level load — before the first frame, hence
  the hard cut documented above.
- **Descriptor placement:** a descriptor carrying `components.mesh` is directly
  map-placeable via `"classname" "anim_demo_grunt"`, resolved by the level
  loader's second dispatch sweep against `canonicalName`.
