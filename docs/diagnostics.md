# Postretro — Runtime Diagnostics

Quick-reference for the engine's diagnostic keyboard chords. All chords use **Alt+Shift** as the modifier. Chords fire on key-down only (no repeat); any extra modifier (Ctrl, Cmd) suppresses the chord.

---

## Chord table

| Chord | Action |
|-------|--------|
| Alt+Shift+\ | Toggle wireframe overlay |
| Alt+Shift+1 | Dump portal walk (one-shot, next frame) |
| Alt+Shift+V | Toggle vsync |
| Alt+Shift+[ | Lower ambient floor (−0.025, min 0.0) |
| Alt+Shift+] | Raise ambient floor (+0.025, max 1.0) |
| Alt+Shift+4 | Cycle lighting isolation mode |

**Layout note:** `[` and `]` are matched by physical key position, not character. On a non-US layout the key that sits where `[`/`]` are on a US board is what triggers the chord.

---

## Wireframe overlay — Alt+Shift+\

Toggles a culling-delta overlay on top of the scene. Chunks are color-coded:

- **Green** — rendered
- **Red** — frustum-culled
- **Cyan** — portal-culled

Useful for spotting over-culling or portal leaks.

---

## Portal walk dump — Alt+Shift+1

Writes the next frame's portal walk to the log: visited leaves, rejected portals, and reject reasons. One log entry per press. Redirect `RUST_LOG=info` output to a file if you need to diff walks across frames.

---

## Vsync toggle — Alt+Shift+V

Flips the surface present mode between `AutoVsync` and `AutoNoVsync`. Use this to compare vsync-pinned frametimes against real CPU cost when the frametime meter is pinned against the budget.

---

## Ambient floor — Alt+Shift+[ / ]

Adjusts the renderer's ambient floor at runtime in 0.025 steps. The floor sets a minimum brightness for all surfaces — raise it to see into shadow areas, lower it back to check true lighting behavior.

---

## Lighting isolation — Alt+Shift+4

Cycles through nine modes in order, wrapping back to Normal. Each press advances one step; the current mode is logged.

| # | Mode | What's visible |
|---|------|----------------|
| 0 | Normal | All terms — full production shading |
| 1 | NoLightmap | All terms except static lightmap |
| 2 | DirectOnly | Static lightmap + dynamic direct + specular |
| 3 | IndirectOnly | SH indirect + specular |
| 4 | AmbientOnly | Ambient floor only |
| 5 | LightmapOnly | Static lightmap atlas only |
| 6 | StaticSHOnly | Static SH probe grid only |
| 7 | AnimatedDeltaOnly | Animated-light SH delta only |
| 8 | DynamicOnly | Dynamic direct lights only |
| 9 | SpecularOnly | Specular only |

**Typical workflow for light leaks:** cycle through the `*Only` modes one at a time until the leak appears in exactly one mode. That mode identifies which lighting path is responsible — e.g. if the leak shows in `LightmapOnly` but not `DynamicOnly`, the problem is in the baked lightmap, not a runtime light.
