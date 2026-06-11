// DEMO CONTENT — the data script for `anim-demo.map` (M10 skinned animation).
//
// Reactions are surfaced through `setupLevel`'s returned `LevelManifest`
// (scripting.md §2 — the data context), NOT through `setupMod`. The map wires
// this file in via its worldspawn `data_script` KVP; the engine runs
// `setupLevel(ctx)` at level load and drains `{ reactions }` into the
// per-level reaction registry.
//
// The reaction is a tag-targeted Primitive (PrimitiveReactionDescriptor): it
// invokes the `setAnimationState` mesh primitive on every entity tagged
// `demo_grunt`, switching it to the `alert` state. The grunt spawns in its
// `defaultState` ("idle", looping); at level load this reaction switches it to
// `alert` and the animation runtime crossfades over the state's 250 ms window.
//
// Trigger shape: a `levelLoad`-named reaction fires once when the level loads
// (mirrors `arena-lights.ts`). A one-shot levelLoad switch is the cleanest
// observable transition available without an AI / timer system: the demo's
// whole point is to make the descriptor → component → crossfade path visible,
// and a level-load switch does exactly that. (The `prop_mesh` placed alongside
// the grunt carries no animation state and stays a stateless mesh for contrast.)

import { type NamedReactionDescriptor, defineReaction } from "postretro";

export function setupLevel(_ctx: unknown): { reactions: NamedReactionDescriptor[] } {
  const reactions: NamedReactionDescriptor[] = [];

  // Tag-targeted Primitive reaction: one descriptor, applied to every entity
  // tagged `demo_grunt`. `args.state` names a state declared on the
  // `anim_demo_grunt` descriptor's `components.mesh.animations`.
  reactions.push(
    defineReaction("levelLoad", {
      primitive: "setAnimationState",
      tag: "demo_grunt",
      args: { state: "alert" },
    }),
  );

  return { reactions };
}
