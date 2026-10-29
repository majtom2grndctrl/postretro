# Postretro — Runtime Diagnostics

Quick-reference for the engine's diagnostic keyboard chords, plus simulated network lag for co-op testing. All chords use **Alt+Shift** as the modifier. Chords fire on key-down only (no repeat); any extra modifier (Ctrl, Cmd) suppresses the chord.

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
| Alt+Shift+\` | Toggle the Diagnostics panel (SDK engine, in a level) |

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

---

## Simulating network lag

The SDK engine can add delay, jitter, and packet loss to a co-op session, so
you can feel how your mod plays over a real connection while host and client
both run on one machine. It works on every platform and needs no admin rights.
Player builds made with `dist` do not include it.

Lag is off until you ask for it. Two ways to set it:

- **At launch**, with the `POSTRETRO_NET_LAG` environment variable:
  `delay_ms,jitter_ms,loss_percent`. You can leave off trailing values: `75`
  is 75 ms of delay only, `75,30` adds up to 30 ms of jitter, and `75,30,5`
  also drops 5% of packets. Delay and jitter can each be 0–5000; loss can be
  0–100, but at 100 a client never connects. If the value can't be read or is
  out of range, the engine logs a warning and runs without lag.
- **Mid-session**, in the **Network** tab of the Diagnostics panel
  (Alt+Shift+backtick toggles the panel once you are in a level). Sliders
  cover delay 0–500 ms, jitter 0–250 ms, and loss 0–50%. A launch value
  beyond a slider's range is kept until you drag that slider. **Standard**
  sets 45 ms delay, up to 60 ms jitter, and 5% loss, and **Off** clears it.
  The tab also shows whether you are the host or a client and, on a client,
  the measured round trip. In single-player it shows "Not in a networked
  session".

Settings apply to this machine's traffic, in **each direction**. Delay is
added on the way out and again on the way back, so a 75 ms delay adds roughly
150 ms to the round trip. Set lag on one side only, usually the client; if you
set it on both, the effects add up. Lag set on a host applies to every client
that joins it.

A typical two-terminal test from the bundle root:

```bash
# Terminal 1: host
bin/postretro-tool run maps/<name>.prl --host

# Terminal 2: client with simulated lag (bare --host listens on port 27015)
POSTRETRO_NET_LAG=75,30,5 bin/postretro-tool run maps/<name>.prl --connect 127.0.0.1:27015
```

A `--connect` client needs no map: it loads whatever the host is running. It
must run the same mod as the host, though. `postretro-tool run` and the
launcher already pass the project's mod; when you start the engine yourself,
add `--mod <folder>`, naming the folder under `content/` that holds the
host's mod.

On Windows PowerShell, set the variable first with
`$env:POSTRETRO_NET_LAG = "75,30,5"`, then run the client command without the
prefix. The variable stays set for the rest of that PowerShell window, so a
later host started there is lagged too; clear it with
`Remove-Item Env:POSTRETRO_NET_LAG`.
