# Baked Promoted-Light Depth — Cost Note

**Date investigated:** 2026-09-02
**Status:** Cost note, not a draft plan. Records why the promoted-depth cache
could move from runtime to bake time, what that would buy, and why it is not
worth doing until a specific symptom appears.

> **Read this when:** promotion churn shows up in playtesting as a hitch,
> when sizing PRL storage for a large map, or when a spec proposes touching
> the promoted-depth cache's fill path.
> **Key fact:** the cache fill is not per-frame work. It runs only when a
> slot's assignment changes, so baking it changes hitch behaviour, not
> steady-state bandwidth or ALU.

---

## What the cache is

A promoted static light's slot in the shadow pool is backed by a
promoted-depth cache layer: the light's-eye depth of the static world,
rendered once when the light takes the slot. It is not a shadow projected
onto any surface. It is the raw material a receiver holds its own position
up to when asking "is there world between me and this light?"

After `promoted-shadow-entity-only-depth` (in `ready/` at time of writing),
only entity receivers ask that question at runtime. World receivers already
hold the answer in the lightmap's baked visibility. The cache is therefore the
runtime's sole source for world-onto-entity occlusion at near-tier
resolution, and it stays.

The cache is sized to the promotion budget, not the map:

| Pool | Layers | Resolution | Bytes |
|---|---|---|---|
| Spot | `MAX_PROMOTED_SPOT` = 8 | 1024² `Depth32Float` | 32 MiB |
| Cube | `MAX_PROMOTED_CUBE` = 2 × 6 faces | 512² `Depth32Float` | 12 MiB |

Filling a layer is the cold path: a shadow cone cull dispatch over the world
BVH gated by the light's frustum, then a world depth draw into the layer.
The layer is then warm and reused until the slot changes hands.

## Why it could be baked

A layer's content is a pure function of two baked inputs — the static world
geometry and the light's fixed pose. Nothing at runtime changes either. That
is the same argument the engine makes for lightmaps and SH probes, so the
compiler could render every promotable light's depth once and store it in
the PRL as a new section.

What a bake would delete at runtime:

- The cold fill pass and its cull dispatch.
- The promoted arm of the shadow cone cull.
- The warm/cold layer state machine (`plan_frame`, layer assignment,
  invalidation on slot reassignment).

A promotion would become a disk-to-GPU upload into the slot's cache layer.

One freedom the entity-only-depth plan opens: once the pool slot no longer
holds world depth, the cache need not match the pool's resolution. Each tap
compares against the two textures independently, so a baked layer could be
smaller than 1024² if the static shadows on entities still read acceptably.

## Why it is a trade

Runtime storage is bounded by the budget: at most 10 lights, 44 MiB. Baked
storage is bounded by the map: every compiler-selected promotable light
(`entity_shadow_select` in the level compiler) needs a layer whether or not
it ever promotes.

| Light kind | Bytes per light at today's resolution |
|---|---|
| Spot, 1024² `Depth32Float` | 4 MiB |
| Point, 6 × 512² `Depth32Float` | 6 MiB |

A map with a hundred promotable spots carries about 400 MiB of depth in its
PRL. Depth compresses poorly. That sits against the near-instant-boot,
tiny-footprint northstar, and against load time on the primary dev hardware.

Mitigations, each a design of its own:

- 16-bit depth (halves storage; compare precision at the far plane needs
  checking against the caster bias).
- Lower baked resolution, using the freedom above.
- Bake only for lights the compiler ranks as likely to promote, keeping the
  runtime fill as the fallback for the rest. This keeps both paths alive.
- Stream layers from disk on promotion rather than loading all at install.

## Why not now

The owner's cost axis is per-frame bandwidth and ALU, counted in operations.
The cold fill is not per-frame: at steady state a runtime cache and a baked
cache cost the same per frame. What a bake removes is a spike on promotion
changes — a hitch concern. No playtest has reported one. If promotion churn
ever presents as a stutter, this note is the starting point, and the
per-frame arithmetic above is unchanged by anything the entity-only-depth
plan ships.

Related: the pool itself allocates `SHADOW_POOL_SIZE` = 96 slots at 1024²
`Depth32Float` eagerly, 384 MiB, against a ceiling near 19 occupied slots on
`campaign-test`. That is allocation, not per-frame traffic, and is unclaimed
by any spec.
