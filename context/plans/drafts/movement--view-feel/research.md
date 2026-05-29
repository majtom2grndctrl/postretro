# movement--view-feel — research notes

Background for the spec. Not normative; the spec captures the decisions this informed.

## Three distinct motion types

The literature and shipped games treat these as separate systems, not one effect:

| Type | Driver | Mechanism | Conveys |
|---|---|---|---|
| **bob** | step cycle / distance travelled | periodic oscillation | footing, gait |
| **tilt** | lateral (strafe) velocity | roll, spring-settled | weight, agility on direction change |
| **sway** | ambient / creature nature | continuous noise | what the character *is* (alien, drunk, heavy) |

Unifying them under one mechanism is a known mistake — bob/tilt are reactive, sway is not.

## Strafe friction vs. view tilt (scope boundary)

The "floaty strafe" complaint that started this has two halves:
- **Mechanical** — velocity decays too gradually on direction change. That is ground-friction tuning in the `Normal` movement state (Quake `pm_friction` / `pm_stopspeed` model). **Not this spec** — belongs in a separate `Normal`-tuning spec.
- **Visual** — the camera doesn't *read* as a sharp direction change. View tilt addresses the read. This spec.

## Prior art

**Quake 1 view roll** (`V_CalcRoll` in `view.c`): dot velocity against the right vector, scale linearly. Two cvars: `cl_rollangle` (max degrees, default 2.0), `cl_rollspeed` (lateral speed for full roll, default 200). Half-Life inherited it. This is the direct ancestor of `tilt`.

**Half-Life view bob**: sine oscillation driven by ground speed; vertical and lateral components. Kept separate from weapon sway in the codebase — independently tunable.

**Destiny (Bungie, GDC 2015 "The Art of First Person Animation")**: camera *leads* direction changes then overcorrects on landing (boxing-reference). Framed camera motion as a dial: too little = floating, too much = nausea (~10% of players). Separate locked camera for the first-person weapon (~74–77° FOV) so player FOV changes don't distort the viewmodel. Reticle placed low to keep the center "combat corridor" clear. The lead-and-settle behavior is what an under-damped spring produces for free.

**Titanfall 2 (Gamedeveloper.com controls interview)**: free-bobbing crosshair caused nausea; fix was a *screen-fixed* crosshair while the view bobs. Wall-run tilt fired on the *anticipation* jump, before the run — timing of the tilt mattered as much as the amount.

**Spring-as-character-weight**: "Instant Game Feel — Springs Explained" (Game Developer) explicitly uses low spring frequency = heavy/loose, high = snappy, with "dinosaur/ogre" as the worked example. Opsive Ultimate Character Controller and NeoFPS (Unity assets) expose per-character spring presets. No AAA postmortem names stiffness+damping as a character-archetype vocabulary, but the pattern is well-established in toolkits and community writing.

**Frame-rate-independent springs**: Ryan Juckett "Damped Springs" (analytic critically-damped step) and Daniel Holden "Spring-It-On" (theorangeduck) are the canonical references. Naive Euler integration makes spring feel framerate-dependent — must use the analytic/semi-implicit step. A `damped-springs` Rust crate exists; hand-rolling the analytic formula is simple enough that no dependency is required.

**Ambient sway vocabulary**: Unity Cinemachine calls the channel *Noise* (6DOF: per-axis amplitude + frequency, plus global amplitude/frequency gain). Cyberpunk 2077 labels it "additive/secondary camera movement." FPS communities call the character-expressive version *sway*. Implementation is Perlin/value noise per axis (continuous → smooth wander) or summed incommensurate sines (cheaper, deterministic, no table). Squirrel Eiserloh's GDC "Juicing Your Cameras With Math" is the origin of noise-driven (vs. white-noise) camera motion.

## Accessibility

Constant ambient camera motion is a recognized motion-sickness trigger (Xbox Accessibility Guideline 117). Best practice is a per-effect *intensity slider* (0–100%), not just on/off — Cyberpunk, Halo Infinite, ESO, and DOOM: The Dark Ages (2025 Shacknews accessibility award) all expose separate head-bob / weapon-bob / screen-shake toggles. DOOM TDA calls out disabling head bob specifically as a motion-sickness mitigation.

For this engine: the *author* surface (descriptor) sets per-class feel and can disable any motion by omission. The *player* accessibility override is a separate runtime global scale (0–1) that multiplies all view-feel output — its UI belongs to the M13 settings menu, so this spec only lands the seam (one engine value, default 1.0).

## Why render-rate, not tick-rate

Existing pattern (`frame_timing.rs`, `main.rs:848-856`): camera *position* is tick-state interpolated; yaw/pitch are applied render-rate so mouse motion is never lost on zero-tick frames. View feel is most naturally render-rate too — spring and phase advance by `frame_dt`, reading the pawn's latest velocity as an input. This keeps the tick state minimal (no bob/roll added to `InterpolableState`/`push_state`) and matches how look angles are already handled.
