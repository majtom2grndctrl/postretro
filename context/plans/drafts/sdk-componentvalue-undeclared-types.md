# Follow-up — SDK `ComponentValue` references undeclared `ParticleState` / `SpriteVisual`

> **Status:** bug note, not a spec. A small, self-contained type-generation gap to fix.
>
> **Origin:** surfaced while scaffolding the M10 skinned-animation demo content (`content/dev/maps/anim-demo.*`). Running `tsc --noEmit` against `content/dev/scripts/tsconfig.json` fails — independent of the demo content, which references none of these types.

## The gap

The generated SDK typedef emits a `ComponentValue` union (and `ComponentKind`) with arms for `particle_state` and `sprite_visual`, but the generator never emits the `ParticleState` / `SpriteVisual` interface types those arms reference:

```ts
// sdk/types/postretro.d.ts:14-15
export type ComponentKind = "transform" | "light" | "billboard_emitter"
  | "particle_state" | "sprite_visual" | "fog_volume";
export type ComponentValue =
    ({ kind: "transform" } & Transform)
  | ({ kind: "light" } & LightComponent)
  | ({ kind: "billboard_emitter" } & BillboardEmitterComponent)
  | ({ kind: "particle_state" } & ParticleState)   // ParticleState never declared
  | ({ kind: "sprite_visual" } & SpriteVisual)     // SpriteVisual never declared
  | ({ kind: "fog_volume" } & FogVolumeComponent);
```

`tsc --noEmit` → two errors: `Cannot find name 'ParticleState'` / `Cannot find name 'SpriteVisual'`. Check the `.d.luau` twin for the same gap.

## Impact

- **Not blocking, not from the mesh work.** The engine's authoritative bundler (`scripts-build` / `postretro-script-compiler`) compiles content fine, and the committed SDK drift test (`committed_sdk_types_match_current_registry`) passes — the committed `.d.ts` *is* what the generator produces, so the gap is in the generator, and it predates the M10 animation work (which added `MeshDescriptor`, not these particle/sprite types).
- **But** it breaks editor/`tsc` type-checking of mod scripts — any author who type-checks `content/dev/scripts` hits these two errors with no offending code of their own.

## Fix direction (pick one)

1. **Emit the two missing interface types** in the generator (`scripting/typedef.rs` + the `primitives/` type registration) so the union resolves. Use this if `particle_state` / `sprite_visual` are meant to be author-visible component shapes.
2. **Drop the two arms** from `ComponentKind` / `ComponentValue` if they are not author-facing. The SDK already documents `particle` and `sprite_visual` as engine-managed (`worldQuery` returns `[]` for them), which suggests scripts never hold these component values directly — in which case omitting them from the script-facing union is the cleaner fix.

Either way, regenerate `sdk/types/postretro.d.ts` / `.d.luau`, keep the drift test green, and add a `tsc --noEmit` smoke check over `content/dev/scripts/tsconfig.json` to a content/CI step so this can't regress silently.
