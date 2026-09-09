# Follow-up — let one texture asset serve entities with different draw params

> **Status:** deferred bug note. A content workaround already shipped (see below); recorded so the next particle/smoke-pass can address the root cause.
>
> **Origin:** surfaced while fixing the rocket trail rendering the plasma sprite. Root cause is in sprite-collection registration, not the content. Broader than the earlier "decouple `lifetime`" framing — `lifetime` was just the first of several params to collide.

## The problem

You cannot assign the same texture asset to two entities that want different draw/behavior params. A "sprite collection" is keyed by the texture **path**, and that key conflates two things that should be separate:

- **Asset identity** — frames, `frame_count`, `slot_mask`, dimensions. Genuinely shared across consumers.
- **A parameterized draw contract** — which folds in **four** per-consumer look params: loop period (from `lifetime`), `emissive`, `spec_intensity`, `spec_exponent`.

`resolve_sprite_collection_draw_contract` requires all four to be byte-identical across every consumer of one path. Any mismatch returns `Err`; the install loop logs a warning and `continue`s, skipping registration of the **whole** collection. At runtime the now-unregistered sprite falls through `resolve_fallback` to the first-registered collection — rendering the wrong sprite. That is the rocket-trail-shows-plasma bug. `lifetime` happened to fire first, but a shared texture differing only in `emissive` or spec would collide identically.

Observed symptom: rocket trail (`smoke_puff/smoke_puff_00.png`, lifetime 1.75) and enemy-rifle trail (same sprite, lifetime 0.45) collided; `smoke_puff` was skipped and the rocket trail rendered the plasma sprite (the default collection).

## Field classification

| Class | Fields | Status |
| --- | --- | --- |
| Asset-intrinsic (correctly shared) | frame arrays (diffuse/spec/normal), `frame_count` (`params.x`), `slot_mask`/shimmer (`params2.x`), dimensions | fine |
| Per-consumer (wrongly forced to match) | loop period (`params.z`, from `lifetime`), `emissive` (`params.w`), `spec_intensity` (`params.y`), `spec_exponent` (`params2.y`) | the bug |
| Per-instance (already correct) | position, age, size, rotation, opacity (`pack_sprite_instance`) | fine |

## Key nuance — two different "lifetimes"

1. **Sim lifetime** (`ParticleState.lifetime`, per-particle, drives fade/despawn) is **already per-particle** and must NOT be re-routed through registration.
2. **Registration lifetime** (`params.z`) only drives flipbook cadence: `frame_duration = lifetime / frame_count`, `frame_idx = floor(age / frame_duration) % frame_count`.

For a **single-frame** collection `frame_count == 1`, so `frame_idx` is always 0 — the registration lifetime has zero effect and the conflict check is purely spurious. `smoke_puff` is single-frame, so the check gated it for no reason.

## Solution directions

- **Immediate stopgap.** Skip the loop-period conflict check for single-frame collections (`frame_count == 1`). Fixes the observed case; explicitly a stopgap, not the real fix — it does nothing for `emissive`/spec collisions or multi-frame sprites.
- **B1 — parameterized collection id.** Key collections by `(asset_path + param-set)` via a synthetic id; dedup the GPU texture upload by `asset_path`; one draw-params uniform + bind group per id. Consumers must resolve to their collection id, not the raw sprite path (`sprite_to_collection` and the candidate-emitting paths must emit/route distinct ids). Cost: more draw calls per shared texture.
- **B2 — per-instance look params (truest "share asset, differ params").** Push loop period, `emissive`, `spec_intensity`, `spec_exponent` into the per-instance `SpriteInstance`; leave only asset-intrinsic `frame_count`/`slot_mask` per-collection. One texture array + one draw call serves all consumers. Cost: grows `SPRITE_INSTANCE_SIZE` past 32B (only one spare slot today); edits `billboard.wgsl` `SpriteInstance`/`SpriteDrawParams`, `pack_sprite_instance`, `build_draw_params`.

## Constraints any fix must respect

- One GPU texture array + one group-1 bind group per collection key; `register_collection` rejects duplicate keys and uploads a fresh array per key — so dedup the texture upload by asset path or VRAM doubles.
- One draw call + a 256B-aligned instance-buffer region per collection; minting more collections multiplies draws/regions.
- No shared mutable animation-cursor state exists (`frame_idx` is computed in-shader from per-instance age) — so per-instance cadence is **not** blocked by shared state.
- Billboard vertex stage is at 6/8 storage buffers — do NOT add a new vertex-stage storage buffer; widening the existing 32B instance stride is fine.
- Only one spare instance slot at 32B.

## Evidence / pointers

- `crates/postretro/src/startup/lifecycle.rs` — `SpriteCollectionCandidate` (~62–85), `resolve_sprite_collection_draw_contract` (~87–162, the conflict error), install loop (~1187–1258, maps `Err` to `log::warn!` + `continue`).
- `crates/postretro/src/scripting/builtins/data_archetype.rs` — `projectile_presentation_assets` (~469–530).
- `crates/renderer/src/render/smoke.rs` — `SpriteCollectionRegistration` (~36–47), `build_draw_params` (~473–491), `register_collection` (~892+), duplicate-key rejection (~908); and `crates/renderer/src/render/renderer_resources.rs:26`.
- `crates/renderer/src/shaders/billboard.wgsl` — `SpriteInstance` (~141–149), `SpriteDrawParams` (~158–164), `frame_idx` (~268–272), emissive/spec (~331/678/732).
- `crates/postretro/src/scripting/systems/particle_render.rs` — `register_sprite` (~50–59), `resolve_fallback` (~266–281, origin of the wrong-sprite render), `pack_sprite_instance` (~307–318); and `crates/render-cpu/src/fx/smoke.rs:33` (`SPRITE_INSTANCE_SIZE`).

## Workaround already shipped

`content/dev/scripts/enemy-rifle.ts`: the enemy-rifle trail was dropped wholesale — its `smoke_puff/smoke_puff_00.png` trail block (lifetime 0.45) was deleted — so the two no longer collide. Content-side only; this note is the durable engine follow-up.
