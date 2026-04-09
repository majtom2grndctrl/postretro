# Task 04: Material Derivation

> **Phase:** 3 — Textured World
> **Dependencies:** task-02 (face metadata with texture names).
> **Produces:** material enum assignment per face, attached to per-face metadata. Consumed by Phase 4+ (footstep sounds, emissive rendering bypass, surface interactions).

---

## Goal

Derive material type from texture name prefix for every face. Build the material enum with initial variants and the prefix-to-material lookup table. Attach material assignments to per-face metadata. Log warnings for unknown prefixes. Material behaviors are stubs in this phase — later phases consume the enum values.

---

## Implementation Guidance

### Prefix parsing

Parse the first `_`-delimited token of each texture name as the material prefix.

Examples:
- `metal_floor_01` → prefix `metal`
- `concrete_wall_03` → prefix `concrete`
- `neon_sign_01` → prefix `neon`

Edge cases:
- No underscore in name (e.g., `lava`): the entire name is the prefix.
- Empty name: default material. Should not occur in valid BSP data.
- Name starts with underscore (e.g., `_trigger`): tool texture, not a rendered surface. Tool texture detection may already be handled by Phase 1's face filtering.

### Material enum

Initial variants. This list grows as content requires — the engine provides the mechanism, not an exhaustive table.

| Variant | Prefix | Properties |
|---------|--------|------------|
| Metal | `metal` | ricochet: true |
| Concrete | `concrete` | — |
| Grate | `grate` | — |
| Neon | `neon` | emissive: true |
| Glass | `glass` | — |
| Wood | `wood` | — |
| Default | (unknown) | fallback for unrecognized prefixes |

Each variant carries property flags. For Phase 3, only the `emissive` flag matters (consumed by Phase 5's emissive rendering bypass). Other properties (footstep sound, ricochet, decal type) are placeholder fields populated by later phases.

### Prefix lookup table

Static or const-initialized mapping from prefix string to material enum variant. A simple match statement or small HashMap. Checked once per unique texture name at load time.

### Per-face material assignment

For each face:
1. Get the face's texture name (from per-face metadata or BSP data directly).
2. Parse the prefix.
3. Look up the material.
4. Attach the material variant to the face's metadata.

### Warnings

Log one warning per unknown prefix, not per face. Deduplicate with a `HashSet<String>` of warned prefixes.

Format: unknown prefix, texture name, and "using default material."

### Module placement

Material derivation is a loader-side concern. Runs during BSP load, attaches results to face metadata. Does not touch the renderer or wgpu.

---

## Key Decisions

| Item | Resolution |
|------|------------|
| Prefix delimiter | First `_` in texture name. |
| No-underscore names | Entire name is the prefix. |
| Unknown prefix | Default material + deduplicated warning. |
| Emissive flag | Carried on the material enum variant. Consumed by Phase 5. |
| Table format | Static match or small HashMap. Grows as content requires. |
| Warning deduplication | HashSet of warned prefixes. One warning per unique unknown prefix. |

---

## Acceptance Criteria

1. Every face has a material assignment in its metadata.
2. Known prefixes map to the correct variant. `metal_floor_01` produces Metal, `neon_sign_01` produces Neon with emissive flag set.
3. Unknown prefixes produce Default material. Warning logged once per unique unknown prefix.
4. Texture names without underscores are handled (entire name is prefix).
5. Material enum and derivation logic contain zero wgpu imports.
6. Adding a new variant and prefix mapping is a two-line change (new enum variant + new match arm).
