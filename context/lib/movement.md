# Player Movement

> **Read this when:** designing or extending player movement — states, abilities, or the author/tuning surface.
> **Key invariant:** movement is custom kinematic, engine-internal, and authored declaratively. The player is a capsule whose velocity the engine sets each tick — never a simulated rigid body, never script-driven per tick.
> **Related:** [Entity Model §5 update order, §7 collision, §7b component](./entity_model.md) · [Scripting](./scripting.md) · [Input](./input.md)

---

## 1. Foundation: custom kinematic, not simulated

Movement is hand-authored kinematic motion in the Quake/Doom lineage, not rigid-body dynamics.

- **Collision queries only.** `parry3d` provides shape-casts and intersection tests ("sweep this capsule along this vector — what does it hit?"). That is the whole physics dependency the player uses.
- **No `rapier3d`.** The rigid-body solver (forces, mass, momentum, restitution, constraints) is deliberately absent. Velocity is authored every tick, then collide-and-slid against world geometry. Nothing simulates where the player "ends up."

**Why.** Game-feel for a power-fantasy shooter wants *designed* motion curves, not emergent ones. Custom kinematics is exactly what lets a dash be tuned across a range — from a deterministic, reproducible designed burst (a configured speed for a configured duration, then control handed back; a solver fighting drag could not guarantee it) to a fluid, momentum-composing impulse — all authored, none emergent from a physics solver. Custom kinematics is also trivially deterministic in the fixed game-logic tick (no solver-iteration order to tame), and the genre's signature feels — air-strafing, bunny-hopping, the floaty-but-tight arc — are *artifacts* of a custom model, not of physical realism. Every modern movement-shooter (Doom Eternal, Titanfall/Apex, Ultrakill) uses this same custom-kinematic foundation; the modern part is the state set on top, not the physics.

A real constraint solver may eventually earn a *scoped, opt-in* place (e.g. a deferred grapple's swinging rope), never as the movement foundation.

**View feel is render-side, not tick-side.** First-person view feel — head bob, strafe tilt, ambient sway, state-transition impulse, sustained slide dip/FOV, and the held vault pose — reads the followed pawn's movement, writes nothing back. It runs at render rate. Its integrator state lives engine-side, never on the movement component or the per-tick interpolation state — tick state stays position-only. Bob's vertical and lateral oscillators have independent frequencies measured in cycles per metre travelled; setting lateral frequency to half the vertical frequency produces the traditional figure-eight gait. State-transition impulses use one critically damped spring per closed impulse key — `normal`, `dash`, `crouch`, `slide`, `vault`; `Swim` kicks none. Their settle rates are limited to 0.1–240/sec and authored channel magnitudes to 180 degrees so every accepted descriptor stays numerically stable. A positional view-feel offset defines the effective render eye for the whole render stage: the view matrix, runtime cell lookup, portal-traversal apex, camera uniforms, and render diagnostics must all consume that same position. The audio listener reads it too: the eye is evaluated once per frame, ahead of the audio step, and shared (`audio.md` §3).

Sustained slide presentation reads the current native slide state and eases to neutral on every exit; transition impulses remain separate additive effects. Slide dip is measured below the normal movement eye and limited after bob to keep at least 0.05 m above the pawn’s feet. Both slide channels follow the resolved view-feel scale, including reduced motion, while their integrator keeps advancing. Entry: `view_feel.rs`.

The held vault pose (`viewFeel.vault`) is weighted by the vault's own progress, never a timer. Every field is optional: `pitch` and `roll` in [−90, 90] degrees (positive pitch looks up) and `fov` in [−90, 90] degrees, default 0; `easeIn` and `easeOut` shares of the climb in [0, 1], default 0.15 and 0.2, summing to at most 1. A bad value rejects the descriptor, as every `viewFeel` block does. Progress is the presented vault clock over the path's length (§4 Vault), so the envelope reaches neutral as the pawn reaches the top: smoothstep up over the first `easeIn`, hold, smoothstep down to exactly 0 over the last `easeOut`. A vault the pose was not already following — one a reconcile or a mid-vault view-feel reset shows in progress, or a progress rewind of more than 5% — eases in from neutral over the ease-in time. Leaving `Vault` any other way (abort, death, a lost block, a reconcile that ends it) releases from the last presented weight over the climb's time × `easeOut`, at least 0.1 s; a release and a new vault overlapping present the larger weight, so nothing steps. The pose turns the camera only: its pitch and roll join the render camera after bob, tilt, sway, impulses and slide, and its FOV joins the projection offset. The viewmodel re-applies the other view-feel offsets in camera space on top of the camera rotation it already follows, so it never receives the held pose, which would swing the weapon out of its placement for the whole climb. It follows the resolved view-feel scale and reduced motion as slide does. The render camera clamps the composed pitch (aim plus every view-feel term) to ±89°, so aim at the limit plus a pose or kick never flips the view; aim itself is unchanged. A descriptor refresh, hot reload included, clears slide and vault presentation to neutral with no release; a pawn change, no view-feel driver and level install reset all view-feel state.

**Sustained pose channel (decided, not yet built).** `viewFeel.pose` replaces the `viewFeel.slide` and `viewFeel.vault` blocks; a leftover key is rejected with a message naming its `pose` replacement.

- **Rows.** `slide` and `vault`, each with optional `pitch`, `roll`, `eyeDrop` and `fov`; a missing channel is 0. Other states get rows only when asked. `normal` never does: a pose there is a permanent offset, not a state cue. Rows evaluate independently and add. The 0.05 m eye-to-feet floor bounds their summed eye drop.
- **Strict block.** Unknown keys and inherited or `__index` entries reject the descriptor at every level, so a typo fails loudly.
- **Timing.** `slide` eases by `enterRate`/`exitRate` while the state's kind is current, so an exit with no edge still eases out. `vault` keeps the progress timing above, with literal `easeIn`/`easeOut` shares, so the pose reaches neutral as the pawn reaches the top. A rate on `vault` or a share on `slide` rejects.
- **Values under a release.** A row presents weight × values. A vault release fades the values it last presented, since the vault's geometry is gone. A release overlapping a new vault or a rewind keeps the larger weight and blends the two value sets by their weights, so a re-vault at a different ledge never steps; equal value sets present exactly as above.
- **Ledge focus.** `ledgeFocus { strength, maxPitch }`, vault row only, is a downward pitch offset toward the line from the rendered eye (after every row's eye drop) to the landing spot. Its reference is the aim pitch the pose's first frame on the vault renders, plus the row's literal pitch; a start, a join or a rewind each take one. While the reference is above the line the offset is `strength` of the gap, capped at `maxPitch`; a landing spot at or above the eye gives none. The offset is additive, so mouse pitch and yaw stay 1:1. Focus never turns yaw and ignores facing. On a mover, the landing spot follows the mover's presented pose; the reference does not turn with it.
- **Unchanged.** Pose angles turn the render camera only, never the viewmodel or aim, so under `allowFire` the reticle sits off the shot line by the presented pose. The ±89° pitch clamp, the FOV band, the view-feel scale, reduced motion and the refresh snap apply as above. Nothing rides the wire.

## 2. Author surface: declarative

Authors tune and compose movement through descriptor data — never per-tick imperative script.

States live natively in Rust. Authors tune enabled native states through descriptor data. They do **not** write per-tick movement callbacks.

**Why.** Movement runs every tick in the fixed game-logic step (`entity_model.md` §5, update order 1, before camera follow). Driving state logic through QuickJS/Luau per tick would add FFI cost and determinism risk on the hottest path. It also holds the standing invariant that movement is engine-internal — no script surface writes the movement component. Its one exception is a readonly per-player projection: `player.swimming` reads the movement state and `player.immersion` an output-only component the tick writes (`entity_model.md` §7b, `scripting.md` §5). No intent, reconcile replay or wire merge reads either back.

### The shape of the surface

In its full form the declarative surface is a **transition graph** assembled from three engine-owned, *closed* vocabularies. Closed does not mean small — a closed vocabulary can be arbitrarily expressive (`scripting.md` §11). What it forbids is author-shipped runtime code: the engine owns the evaluator, authors describe behavior as data. Tuning ships first; author-defined transitions follow across the series.

| Vocabulary | Author picks | Engine owns |
|---|---|---|
| **States** | which native states exist + their tuning | each state's per-tick velocity intent (Rust) |
| **Triggers** | a boolean combination (`all`/`any`) over a closed predicate set — `grounded`, `airborne`, `touchingWall`, `<input>Edge`, `speedAbove/Below`, `cooldownReady`, `chargesRemaining`, `elapsedMs` | predicate evaluation |
| **Carry-rules** | the velocity transform applied at a transition edge — `keepHorizontal`, `keepBoost`, `zero`, `projectOntoWallPlane`, `reflect`, `scale` | the transforms + momentum-conservation consistency (§6) |

A transition is one data row — `{ from, to, when, carry }`. The author wires native states into chains the engine never pre-built (dash → wall-run → wall-launch); the velocity math under each state and each carry-rule stays native.

**The line — one test:** a declarative element is anything the engine can evaluate each tick *from data alone*, with no author-shipped runtime code, no per-tick movement callback, and no reads outside the movement component. What is forbidden is a retained author function the engine calls each tick — that reintroduces the FFI/determinism cost on the hottest path and breaks the fixed-tick determinism and momentum-carry invariant. The constraint is on *who owns the evaluator*, not on how expressive the vocabulary is.

A richer authored surface — response curves, a velocity expression like `boost = f(speed, charges, grounded)` — is reachable *without* an author function: the **Typed Command Buffer** (`scripting.md` §11) lets the author describe such behavior as a typed, engine-evaluated IR that crosses the FFI as data. Movement adopts it for dash tuning: dash descriptor value fields accept a literal or an engine-evaluated expression over a movement-local input namespace mirroring the trigger vocabulary above, with the evaluation moment (dash entry vs per-tick) engine-pinned per field. Wider adoption (a transition graph, response curves) stays open; the command buffer is the canonical mechanism for it. New predicates and carry-rules are still added deliberately, vetted against the flexibility band (§3) — never via a general escape hatch.

## 3. The flexibility band

The author surface targets a *band* of expressiveness, not a point.

We don't couple what we build to a single use case. We distill movement mechanics to first principles and expose them through an approachable, composable API. A dash cooldown and a dash with charges each suit different games — the surface should be expressive enough that either is implementable.

| Edge | Rule |
|---|---|
| **Floor (min flexibility)** | A modder can compose the native states, declarative transitions, and tuning into movement *unlike* the specific game being built — without the engine having pre-built that exact mechanic. |
| **Ceiling (max generality)** | The vocabulary stays FPS-shaped. Never a content-agnostic movement/physics framework that caters to nothing. |

**Stress-test references.** Ultrakill and Neon White (slide-cancel chaining, dash routing, "movement *is* the game") are the **ceiling references**. The yardstick: can the declarative surface compose this? If the surface can reach that ceiling through composition, it is flexible enough.

**Guardrail.** Primitives stay in FPS vocabulary — states (crouch, slide, wall-run, vault), velocity intent, capsule, ground/air params, ability budgets, transitions. Never generic `apply_force(body)` or `register_update_callback`. Use the band to judge a proposed descriptor field: *FPS-flexible* (expands what authors compose in FPS terms) vs *generic framework creep* (a sandbox primitive that makes the common case — a crouch, a dash — as costly as the exotic one).

## 4. State-machine seam

The movement tick splits into two halves with a clean seam between them.

- **Shared physics substrate.** Collide-and-slide: sweep-and-slide, step-up, floor-push, ground-stick, contact/landing resolution. Runs regardless of state. Takes a desired velocity, returns the resolved position plus contact results. Behavior is fixed — states change *intent*, not collision.
- **Per-state velocity intent.** Each state authors the desired velocity for the tick (gravity, acceleration, friction, bursts). The current state is an explicit value on the movement component. A dispatch point runs the active state's intent, calls the substrate, then applies any returned transition.

**Contact flows forward, not sideways.** Collision queries go only through substrate-owned helpers. A state intent that needs surface contact (wall normal, floor normal) reads it from the substrate's contact result carried forward from the prior tick, never by querying collision itself. A stage that runs before the intent may cast from the tick-start pose, as the swim edge does for entry clearance and fluid-jump headroom and the vault's ledge probe does for its ledge. Reconcile replay recomputes such a cast on its first tick, so it needs no wire field. Crouch and slide stand-up clearance are the intent-time checks, made through a substrate helper. This holds the split: intents author velocity, the substrate owns collision.

**Per-state data ownership.** Each state's live data (timers, boosts) is owned through one uniform convention; the dispatch resolves the component-vs-active-state borrow once. A new state adds its data without widening the dispatch.

Walk/run/jump/air-control is the baseline `Normal` state. `Dash`, `Crouch`, `Slide`, `Swim` and `Vault` sit behind the same seam; wall-run will too. Ability budgets (air-jump, air-dash) refresh through one landing-refresh point so they reset uniformly.

### Environment resolution

The tick resolves the pawn's environment once, at its tick-start position, before the state intent: gravity (signed vertical), push (a vector), the winning fluid, its viscosity, immersion, and whether the tick eye is inside that fluid. Resolution is one pure function of position, capsule, the level's static environment volumes and the level gravity (`entity_model.md` §7, Environment Volumes). Host movement, client prediction and reconcile replay all call it through the tick, so they agree by construction. Intents receive the resolved value and never query it. The forward-contact rule above does not govern it: environment is a function of position and static data, not of a collision result.

Gravity keeps its meaning in every state. Push never defines "down": ground classification, the capsule, step-up, ground-stick, camera and navmesh keep +Y as up. Slide's slope boost reads gravity only, so push never feeds it.

### Swim

`Swim` is a native state, entered and left by immersion — the fraction of the capsule's vertical segment, feet to head, the winning fluid covers. The edge applies at the seam, after the protected layers are peeled and before the intent, so the entered state owns its first tick.

- **Hysteresis.** Any state enters at immersion ≥ `enterDepth`. `Swim` leaves below `exitDepth`; exactly at it stays. Between the two nothing toggles.
- **Standing capsule.** Entry is tested against the standing capsule's immersion. A crouched pawn (crouch or slide) stands up about the anchor its crouch entered with: feet when grounded, head when airborne. Feet-anchoring an airborne crouch would lift the capsule by twice the crouch shrink, so a diver crouching out of a floating block would stand straight back into it. The growth must pass a clearance sweep in the direction it grows; a blocked growth stays crouched, wades, and retries every tick.
- **Fluid jump.** Optional: without `fluidJumpVelocity`, jump at the surface only treads, and the pawn leaves water by wading out or, with a vault block, by mantling. With it, the jump fires only from `Swim` at tick start, with jump held, the tick eye — never the presented camera eye — out of the fluid, and a rise that clears the water: the ballistic rise under the tick-start gravity plus vertical push must uncover the capsule below `exitDepth`, and a headroom sweep must allow that rise. Otherwise jump at the surface only treads. A clearing jump wins over exit on the same tick; a reachable ledge wins over the jump (§Vault). It sets vertical velocity to `fluidJumpVelocity`, discards the vertical parts of the drift and knockback layers, leaves to airborne `Normal`, and arms a re-entry latch. The latch blocks swim entry until immersion first falls below `exitDepth`. Without the clearance rule a jump that could not leave the water would latch swim off with the pawn still submerged.
- **Locomotion.** The fluid's viscosity is the only drag while swimming, a linear fraction per tick on all three axes; a swimmer on the pool floor gets no ground friction. Acceleration toward the 3D wish (pitch-following, jump up, crouch down) only adds speed along the wish, so carried momentum is never clipped. With no vertical input the swimmer drifts at `sinkSpeed` scaled by resolved gravity over the default level gravity (−20.32 m/s²): zero gravity floats, upward gravity rises. While the tick eye is out the wish never points up, so a swimmer leaves the water by the fluid jump, a mantle (§Vault), or wading out. A pool-floor swimmer stays grounded for collision, not for its medium, and swim fires no `landed`.
- **No swim block.** The pawn never swims and walks fluid floors, a legitimate wade-only design. Host and offline warn once when a level with fluid volumes loads for such a player.

The `movement.swim` descriptor block is optional. A block failing validation is dropped with a warning naming the field, and the player wades; the rest of the descriptor loads.

| Field | Unit | Valid |
|---|---|---|
| `speed` | m/s target swim speed | finite, ≥ 0 |
| `accel` | m/s² toward the wish | finite, ≥ 0 |
| `sinkSpeed` | m/s with no vertical input, at default level gravity | finite, ≥ 0 |
| `enterDepth` | immersion fraction | (0, 1] |
| `exitDepth` | immersion fraction | (0, 1], strictly below `enterDepth` |
| `fluidJumpVelocity` | m/s upward | optional; when present finite, > 0 |

`Swim` and its fluid, the re-entry latch and the drift layer are wire-mirrored tick state: predicted, reconciled, and carried across movement-component rebuilds (hot reload, client tuning install).

### Vault

`Vault` is a native mantle: the pawn climbs onto a top surface above step height and stands on it, horizontal speed kept. Motion is authored velocity along a timed path, never a teleport, and the substrate still resolves collision. The `movement.vault` block is optional: without it the pawn never vaults and the ledge probe never casts. A block failing validation is dropped with a warning naming the field, as swim's is.

| Field | Unit | Valid |
|---|---|---|
| `maxHeight` | m above the feet, land reach | required; finite, above the step-up limit (`stepHeight` + 0.01 m) |
| `swimMaxHeight` | m above the fluid surface, fluid reach | required; finite, > 0 |
| `durationMs` | ms from entry to standing on the top | required; finite, > 0 |
| `from` | entry context → `"auto"` \| `"jump"` | contexts `air`, `ground`, `swim`; a missing key forbids entry from it. Default `{ air: "auto", swim: "auto" }`; a present `from` replaces it whole |
| `allowFire` | bool | default `false` |

**Entry.** A pre-intent stage runs after the swim seam, from the same tick-start pose, and only when the swim seam took no edge this tick, so swim entry beats a land vault on one tick. The entered vault owns its first tick. Contexts: `air` is airborne `Normal` or `Dash`, `ground` grounded `Normal` or `Dash`, `swim` is `Swim` with the tick eye out of the fluid. Crouch and slide never vault. `"auto"` fires when the horizontal wish pushes into a valid ledge; `"jump"` also needs jump held or buffered. Any entry clears the jump buffer, and a `"jump"` entry spends the jump. Entry folds knockback and drift into the entry speed and empties both protected layers, so carried-in knockback cannot ride the path. A swim-context entry arms the swim re-entry latch, so swim cannot recapture the rise; it clears as after a fluid jump. From `Swim`, a reachable ledge beats the fluid jump. Mid-vault, a swim edge still pre-empts a land-entered vault.

**Ledge probe.** Runs only on eligible ticks — block present, pawn not barred, context in `from`, horizontal wish, jump ready for a `"jump"` context — and casts nothing otherwise. At most 4 casts, all through the combined world, so movers count. Cost is per pawn, never per mover; the mover scan inside every combined cast is a separate cost.

1. Forward capsule sweep along the wish, over this tick's travel plus 0.1 m, so a fast pawn vaults before its own sweep stops it at the wall. Nothing ahead, or a walkable hit, ends the probe: open floor costs one cast.
2. Down ray past the wall onto the top. A non-walkable hit, or a top outside the reach band, refuses.
3. Upward capsule clearance sweep from the tick-start pose to just over the resting height on the top.
4. Forward capsule clearance sweep at that height to the landing point.

Sweeps 3 and 4 start from a known-clear pose, so together they prove the path's volume and the standing capsule's headroom on the top; collision has no overlap query. Land reach is a top strictly above the step-up limit and at most `maxHeight` above the feet; step-up and the probe share that one limit, so a ledge at it steps up and one just above vaults. Fluid reach is a top above the fluid surface, at most `swimMaxHeight` over it; the surface height comes from the resolved immersion, with no cast. A ledge on a mover whose up axis tilts more than about 0.8° refuses; a yaw-only mover vaults like a static ledge. Replay recomputes the probe on every replayed tick, so entry needs no wire field.

**Path.** The payload is the vault clock, the landing target and the start offset (entry feet minus target) in the target's frame — mover-local on a mover, world otherwise — the mover id, and the entry speed. The exit direction is the start offset's horizontal part, reversed. Each tick rebuilds the path from payload and tuning, and the velocity is the displacement to the next path point over the tick: no gravity, no input. Dash, jump and crouch are ignored, and no vault tick arms the jump buffer.

- **Clock.** Whole ticks, written fresh each tick, never accumulated. The path's tick count is the fewest whole ticks whose clock reaches `durationMs` — the same comparison the exit and the fire gate make, so all three end on one tick at every tick rate.
- **Shape.** The rise closes on a smoothstep. The forward move overlaps its end: a cubic from rest over the path's last share (0.4 for a still entry, shorter for a fast one), ending at the entry speed so the exit adds no jump. The rise finishes at least one path tick before the path does, so the eye rises monotonically and the pawn never stops at the corner.
- **Lip allowance.** Each tick's forward is capped by an allowance read at the depth the feet start that tick below their resting height. It keeps the whole tick's straight move a skin off the lip and inside the volume the probe cleared, so the path stays inside what the probe proves rather than the probe growing. At a very short duration the cap holds the forward back, the tick that would complete the path aims at the corner, and one more path tick carries the pawn onto the top.
- **Ride.** A tick that starts in `Vault` on a mover carries the pawn with the target's mover as a grounded rider is carried, and never with any other. Steering and the blocked check read the carried position, so the path holds at any mover speed and a rising lift is landed on. Mover release follows the same ride.
- **Blocked.** A pawn further than 0.1 m from the point it was steered to (knockback, a wall that moved in, a mover that dropped the lip), or at a non-finite distance, aborts. So does a target mover with no pose this tick (despawned).
- **Protected layers.** Vault ticks skip push entirely, entry, exit and abort ticks included: no drift accrues and no push cap applies. Knockback landing mid-vault still rejoins, moves the pawn off the path, and aborts it.
- **Exit.** Its own tick, after the path ends on the target: the intent restores the entry speed along the exit direction, closes any vertical gap to where the top now is, adds a small downward settle so the substrate finds the floor, and returns to grounded `Normal`. Ability budgets refresh through the landing-refresh point.

**Early ends.** Abort, death, a lost vault block on a client tuning install, and hot reload leave through one shared settle: `Normal`, base velocity zeroed (knockback and drift stand), swim re-entry latch cleared so a broken mantle lets the pawn swim again. Of these only abort emits `vault_ended`, as the exit and a swim pre-emption do. Abort is the one in-tick end, so on a moving target it keeps the mover's velocity through the release; a break from outside the tick has no pose to read and keeps none of it. Movement never reads health: before each movement tick the sim breaks a dead pawn's vault and bars it from entering one, on the host and in client prediction alike. The bar is caller input, not wire state; reconcile replay keeps the value the latest prediction left.

**Events.** `vault_started` and `vault_ended`, after the per-state naming; view-feel impulses key the state `vault`.

**Firing.** `allowFire` defaults to `false`: hands are on the ledge. With it off, a pawn refuses activation starts on any tick that starts in `Vault` with its clock below `durationMs`. Every fire path — host-local, host-remote, client-predicted — judges the pawn as its tick starts, so the entry tick, which starts in `Normal`, and the exit tick are open. A closed gate cancels the in-progress activation, as death does, with one opt-out: a charge whose action sets `charge.interruptedBy.vault: false` (`entity_model.md` §Components, Weapon vocabulary) keeps charging while it is still charging. Its release made mid-vault is held and emitted on the first open tick, stamped there, if the trigger is still up; pressing again forgets it. Once its charged shot fires, the gate cancels the rest as before. A fresh press refused by the gate alone is held, not consumed, and fires on the first open tick while the trigger stays held; a hold trigger restarts without a re-press. The host-remote lane's ordering of these rules is `networking.md` §Combat authority.

`Vault` and its payload are wire-mirrored tick state, predicted and reconciled; merge rejects a vault onto an unknown mover. `durationMs` stays local tuning. A client tuning install carries a vault while the new tuning keeps a vault block. Hot reload always ends it to `Normal`, unlike `Swim`, which a reload carries. Vault ticks, the exit tick included, take the wider prediction-correction band (`networking.md` §Snapshot apply ordering).

## 5. Design north-stars

| Reference | Borrow |
|---|---|
| **Doom Eternal** | Primary aesthetic match. High base speed, no ADS slowdown, dash-with-charges, double-jump, auto-mantle. Movement is a combat tool. |
| **Titanfall 2 / Apex** | Speed-preserving slide (converts sprint/downhill speed into a decaying boost rather than capping), timed wall-run, auto-mantle/vault. |
| **Ultrakill / Neon White** | Ceiling reference — can the surface compose it? (see §3) |

## 6. Cross-cutting policies

Two policies cut across every state and define the modern feel. Both are foundations, not per-state details. They belong at the transition and input seams, not in individual states.

- **Momentum conservation.** Velocity carry is owned by the dispatch point that applies the transition, never inside a state intent. A state that authors its own exit velocity transitions with the keep-all carry, which leaves it untouched: the vault's exit tick sets the restored entry speed itself, and vault entry applies no carry, since the vault intent authors its whole velocity. A transition's `carry` composes a horizontal rule and a boost rule over §2's closed vocabulary. Carried momentum above the run cap survives into the air only while the player gives no air-steer input or `air.bunny_hop` is set; otherwise airborne `Normal` re-clamps it.
- **Input forgiveness.** Coyote and jump-buffer windows are descriptor-tuned. The engine derives their edges once per tick; state intents consume those edges rather than raw button bits.

### Knockback

Combat adds authored velocity changes to a protected knockback layer on players and navigation agents. Normal acceleration, air-speed caps, AI steering, and dash/slide carry operate on locomotion; the protected layer rejoins before collision. Collision projects both layers and landing clears vertical knockback, so blocked pushes cannot return on the next tick. Gravity acts once on total motion; terminal fall limits consume downward knockback before altering baseline velocity.

Character descriptors author response scale, ground/air drag, and locomotion control during a shove. Drag removes a linear fraction per fixed tick (`max(0, 1 - drag * dt)`); zero drag preserves momentum. Control scales steering and acceleration while protected velocity remains, leaving jump/dash activation available. Upward impulses release ground attachment; mover launches preserve the prior base long enough to transfer its release velocity once. Mutable knockback velocity participates in player prediction and reconciliation. No runtime script callback or rigid-body solver is involved.

### Regional gravity and push

Gravity is one signed vertical scalar per body per tick: a gravity volume's value inside it, the level gravity outside. It keeps its meaning in every state, and upward level gravity with no push still never lifts a grounded body.

A push volume adds an acceleration in any direction. Its optional `force` (default 1, any finite value ≥ 0, no upper cap) scales what bodies feel: `push` is the nominal current that fluid surfaces and particles show, and every movement read — lift-off, drift accrual, the terminal clamp, the fluid-jump check — takes `push × force`, resolved once with the winner. Force 0 is a decorative current; above 1 hides a strong current under a calm surface. Terminal speed is the descriptor's and does not scale with force. It is static map data, never a script primitive that forces a body, so it stays clear of the §3 guardrail. Push lands after the state intent, in a second protected layer beside knockback — the **drift layer**:

- **Horizontal on land.** Outside water, horizontal push accrues in the drift layer. Intent speed caps and steering act on locomotion only, so an air cap never re-clips a wind. Vertical push joins locomotion directly; no intent caps vertical speed, and while grounded the floor bears it.
- **3D while swimming.** In `Swim` the whole push accrues in the layer, so a current settles at about push / viscosity in every direction (exactly, below) and swim steering never fights it. The layer's speed is capped at terminal velocity by magnitude. On every edge out of `Swim` its vertical part folds into vertical velocity, and the layer is horizontal again.
- **Decay by medium.** Ground friction while grounded (steady drift ≈ push / ground stop friction), the fluid's viscosity while swimming, nothing airborne: momentum carries out of a wind until landing.
- **Collision.** The layer rejoins before the one collision sweep, which projects it like knockback, so walls absorb it. While swimming, floors and ceilings never absorb its vertical part, so a downward current presses a swimmer into the floor and an upward one lifts it off once total vertical motion points up.
- **Lift-off.** A grounded dry body leaves the ground when the push points up harder than gravity pulls down, as a knockback launch does: grounded clears before the intent, and the substrate skips the upward clamp and ground-stick. Only gravity's downward part resists.
- **Terminal speed.** Speed along the push direction is capped at the descriptor's terminal fall velocity, so an updraft never accelerates a body forever. The excess comes from the drift layer first, then knockback, then locomotion, as the fall clamp does.

**Steady drift.** One shared function gives the drift a swimmer settles at in a steady current, and fluid surfaces drift at it (`rendering_pipeline.md` §7.3a), so look and feel agree at force 1 and any viscosity. Each tick decays the layer by `factor = max(0, 1 − viscosity·dt)`, then accrues `push·dt`, so it settles at `push·dt / (1 − factor)`, capped by magnitude at terminal velocity. That is about push / viscosity in a thin fluid, and exactly `push·dt` once viscosity·dt ≥ 1, where decay clears the layer before the push accrues. An inviscid fluid never settles below the cap: it drifts at terminal velocity along the push, or not at all with no cap. One exception comes from the decay's stop threshold, which zeroes a drift under 1 mm/s: when a swimmer entering from rest decays its first tick's `push·dt` below the threshold, it resets every tick and settles at `push·dt`, and the function returns that from-rest value.

The drift layer is predicted and reconciled like knockback velocity and rides the wire (`networking.md` §Game-logic-owned apply invariant). Push resolves by one winner like every regional field, never a sum: a zero push carves a calm pocket inside wind.

### Moving bases

Player grounded state is a `GroundRef`: `Airborne`, `World`, or `Mover(mover_id)`. Scripting-facing `grounded` stays a bool projection (`ground != Airborne`), and AI agent grounding remains separate.

The movement query surface combines static world collision with active mover colliders. When the player stands on `Mover(id)`, the substrate revolves the planted position by the mover's tick rotation about its origin, then applies its linear tick delta. Carry is predicted and reconciled with the pawn. `carry_yaw` is an authored mover toggle, default off: when set, world-up rotation carries view yaw on the owning client; pitch and roll never tilt the FPS camera. Leaving a mover preserves player-controlled velocity and adds linear base velocity plus tangential spin velocity (`angular_velocity × radius`) once. The player gains no angular momentum. Movers react to what they hit through a per-mover **block policy** — `displace` (default: push out of overlap, never yield), `reverse`, `stop`, or `crush` — applied to players and enemies. Because the reaction depends on live entity positions, the block decision is host-authoritative and reconciled, never pure-phase-predicted like mover motion (`networking.md`); crush deals damage through the entity damage chokepoint, with presentation and overkill effects left to mod policy. A mover never blocks, reverses for, or crushes a player *riding* it: a player grounded on a mover — or briefly airborne just after leaving it, for a bounded host-side grace — is not an obstruction to that mover. Only the free-riding contact is exempt; a rider carried into static geometry it cannot be pushed clear of (crushed against a ceiling) is still crushed. The grace is host-only decision state, never replicated.

### Teleport

One substrate chokepoint relocates a pawn: position, every velocity channel, a yaw delta, a movement-state reset and a presentation cut, alike on host, prediction and replay (`crates/physics/src/movement/teleport.rs`). It runs after the substrate step in every movement tick, reading the caller's snapshot of installed armed state for that tick. Teleporters are its first consumer (`entity_model.md` §7, Teleporter regions).

- **Firing.** A teleporter fires when the post-step body point lies inside an armed, unlatched region. Capsule overlap alone never fires. Overlaps pick one winner (`entity_model.md` §7). A pawn teleports at most once per tick, and the tick ends at the arrival pose.
- **One rotation rule.** Both modes rotate view yaw and every velocity channel — base, knockback, drift, and a dash or slide's banked boost — by one world-up yaw delta, on the teleport tick itself. Speeds and pitch are unchanged; roll is never touched. Classic keeps momentum, unlike Quake's fixed exit speed.
  - Classic: the body point lands on the destination origin; the delta is destination yaw minus the command's facing yaw.
  - Seamless: the delta is target yaw minus landmark yaw, and the body's offset from the landmark, vertical included, rotates onto the target.
- **Arrival state.** A vault aborts to `Normal` and reports `vault_ended`; its velocity is kept and rotated, never zeroed like a broken vault's. Slide and dash continue on the rotated velocity. The arrival never moves the capsule: a non-rising pawn whose feet rest on walkable static floor within `SKIN_DISTANCE` plus one map unit (0.0254 m) lands grounded on it, keeping its speed, so a walking or sliding shift takes no air tick. Any other arrival is airborne with its coyote jump spent; air jumps keep their budget. A mover under the arrival counts as no floor, and mover carry never follows a teleport.
- **Blocked arrival.** An arrival capsule that intersects world geometry, sits inside a solid or exterior cell, or intersects or sits inside a mover at its pose this tick refuses the teleport: no move, event or sound, and one warning per teleporter. No other winner is tried.
- **Arrival latch.** Arrival latches the pawn against every armed teleporter containing its body point. A latched teleporter drops out before the winner pick until the body point leaves it. Arming a disarmed teleporter clears every latch against it, so occupants fire; arming an armed one changes nothing.
- **Dead pawns.** The bar that keeps a dead pawn out of a vault (§4 Vault) keeps it from teleporting too.

**Teleport record.** Wire-mirrored tick state on the movement component, predicted and reconciled (`networking.md` §Snapshot apply ordering): a `u8` generation that wraps; the accumulated teleport yaw, wrapped to (−π, π] by the one function host and client share; the last teleporter's map-order index (`u16`, `0xFFFF` for none); and the arrival latches, sorted indices. Every consumer of a facing sampled before a teleport turns it by one rule, `facing_in_teleport_frame`: the pawn's current accumulated yaw minus the yaw the facing was sampled under (`networking.md` §Host input command queue).

## 7. Non-goals

- Rigid-body dynamics for the player (forces, mass, restitution, constraint solving).
- Per-tick script-authored movement (imperative callbacks). The author surface is declarative.
- Map-overridable per-archetype movement tuning — movement physics, swim and vault tuning included, is descriptor-owned. A map may only seed level-wide world gravity at load and place static regional gravity, push and fluid volumes and script-armable teleporters (`entity_model.md` §4).
- Rollback or deterministic lockstep. Pawn and mover prediction/reconciliation are in scope for authoritative co-op, but not rollback.
- Vault variants: hurdling to an obstacle's far side, mantling into a crouch-only gap, NPC vaulting (enemies use their own static-only controller), and a third-person vault pose (observers animate from velocity).
