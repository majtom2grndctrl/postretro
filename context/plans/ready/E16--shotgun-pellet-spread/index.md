# Shotgun Pellet Spread (E16)

> **Milestone:** Epic 16 → Resolution Modes (the multi-pellet item
> `E16--weapon-state-machine/research.md` §5 assigned there).
> **Research notes:** `research.md` — fire-path/netcode/descriptor/RNG source
> inventory, vantage map, decision derivations.

## Goal

One trigger pull on a pellet weapon resolves N hitscan rays sampled uniformly in a
cone, end to end: descriptor stat → `effective()` → both fire paths → per-pellet
damage, zone multipliers, and impact-policy fires — in single-player, listen-host,
and connected-client co-op alike. The engine's first multi-ray resolution, on the
authority model client-authoritative combat already banked for it.

## Scope

### In scope

- **Descriptor stats:** `pelletCount` (default 1, validated 1..=32) and
  `spreadDegrees` (cone half-angle, default 0, validated finite 0..=45) on the
  weapon descriptor, threaded through `effective()`, hot reload, both VM parsers,
  and the generated SDK typedefs.
- **Uniform cone sampling:** a shared, pure, solid-angle-uniform cone sampler;
  per-shell deterministic seeding from replay-stable, spawn-order-stable state.
- **Local fire path generalized:** the single-player / listen-host path resolves N
  pellets per shell — per-pellet impact, damage, FX, and impact-policy fire.
- **Client fire path generalized:** the connected client samples and casts N rays
  once per frame and declares 0..N hit records; the host mints `AuthorizedShot`
  with the weapon's effective pellet count.
- **Tuning payload:** `pellet_count` + `spread_degrees` replicate to clients with
  apply-side clamping; epoch bump; fixture re-bless.
- **Dev-mod reference:** `reference_shotgun` becomes a real pellet weapon with
  per-pellet damage retuned, the demo walkthrough re-derived, and both demo
  targets' HP retuned to keep the staged down-then-gib beat.

### Out of scope

- **Authored pellet patterns.** Uniform cone only (owner decision). A fixed-pattern
  variant would be a new sampling mode beside the cone, not a rework of it.
- **Dynamic accuracy: bloom, recoil, ADS modifiers, `player.spread` HUD slot,
  crosshair spread ring.** The Weapon Feel roadmap item owns these; its
  composition seam is pinned in Direction.
- **Multi-shell activations (double-barrel, burst).** One trigger pull fires one
  shell. Pellet resolution is per-shell throughout, so a future shells-per-
  activation stat multiplies shells, never touches pellet sampling.
- **Per-pellet shot verdicts.** `ShotVerdict.hit_accepted` stays one bool per
  declaration — the shooter's hitmarker feedback stays per-shell (PvE bias:
  smooth client experience, cheap validation; nothing consumes a per-pellet
  verdict today).
- **Tracers / fire-FX replication.** No fire-FX replication path exists; other
  clients see a shot only through replicated consequences. Making a spread
  pattern visible to non-shooters is future presentation work
  (combat presentation substrate / viewmodel epics).
- **Damage falloff over distance.** A per-pellet range falloff curve is a
  Damage & Defenses–shaped stat axis; pellets inherit the existing binary
  in-range check.
- **Projectile pellets.** `ResolutionMode` has one variant (`Hitscan`); the
  projectile resolution mode is its own roadmap item.

## Direction

**Problem.** The engine resolves every shell as exactly one ray: the local path's
fire events carry at most one impact, and every `AuthorizedShot` is minted with
`pellet_count: 1` — while the wire, the host clamp, and the per-pellet impact-policy
dispatch shipped pellet-general and idle at N=1. Shotguns — the genre's signature
weapon, already carrying per-shell reload — hit like rifles.

**Prior commitments.**
- *Authority model already decided for pellets.* `networking.md` §Combat authority:
  hitscan, pellet spreads, and projectiles are one shape — client-declared hits
  against the rendered world, host-validated, "differing only in ray count and
  arrival timing." This spec changes ray count and nothing about authority.
- *The host clamp and per-pellet policy dispatch are shipped contracts.* Ingest
  takes at most `pellet_count` records, invalid records consume clamp budget, and
  the impact consumer runs after **each** accepted pellet so policy effects settle
  between pellets (regression-tested). This spec feeds them real counts; it must
  not reshape them.
- *`damage` per pellet is the no-change reading of shipped code.* Host ingest
  applies `AuthorizedShot.damage` once per accepted record; the local path builds
  its damage payload per impact. Owner decision aligns the stat's meaning with
  what the code already does — application logic is untouched, only authored
  numbers change.
- *Primitive surface is a contract* (`index.md` §2). New descriptor fields land
  with SDK types, validation, and typedefs in the same pass.
- *One cast per frame on the client.* The client casts on the first fire tick of a
  frame and retires additional ticks' shots with empty declarations. N pellets ride
  inside that one cast moment; the invariant is preserved, not renegotiated.
- *The determinism gate asserts spawn-order invariance, not just run-to-run
  equality.* The shipped harness compares labeled outcomes across reversed spawn
  orders, so pellet directions must not depend on `EntityId` or `NetworkId`
  allocation order — which rules entity-id bits out of the seed and shapes the
  recipe below.
- **Divergence, named:** the emitter precedent seeds its cone RNG from
  spawn-order-dependent inputs — emitter position bits ⊕ `EntityId::to_raw()`
  ⊕ a frame-step counter — reproducible run-to-run, but exactly the
  allocation-ordered identity the determinism gate's spawn-order-reversal
  assertion forecloses for weapons. This spec's sampling runs inside
  `simulate_tick`, which that gate replays, so its seed uses no entity id and
  no frame counter — deliberately different inputs while reusing the
  emitter's math.
- **Divergence, named:** `E18--trap-pools-seeded-arming` (whose SplitMix64 mixer
  Task 2 reuses) rules "RNG lives only in the install pass — never in per-tick
  evaluation." That rule's underlying invariant is replay determinism, which its
  install-time-only scoping guaranteed for trap pools. This spec does run seeded
  RNG per tick, and preserves the same invariant a different way: directions are
  a pure function of replay-stable weapon state (the shell counter + stable salt,
  Task 2), so replayed runs reproduce them exactly — verified by the determinism
  gate itself (AC 6).
- **Divergence, named:** `networking.md`'s durable posture — "General posture
  for engine randomness: host-only, load-time, consequences-only — never
  per-tick or client-side, never shared-seed re-sim." This spec runs
  deterministic weapon RNG per tick and on the client. The posture's
  underlying commitments all hold: no roll crosses the wire, no client re-runs
  another machine's roll (each machine samples only its own pawn's fire), and
  replay determinism is preserved by the pure-function seed. At promotion,
  that `networking.md` sentence gains the carved exception — a promotion-time
  lib update, per the drafting rule that `context/lib/` changes land at
  promotion.

**Placement.** Spread lives on the weapon descriptor behind `effective()` — the
stat seam the roadmap reserves for accuracy axes — not on the camera/aim layer and
not as runtime-writable weapon state. The aim direction stays the pure screen-center
`aim_ray()`; pellet sampling perturbs per-pellet ray directions *around* that axis
at resolution time. **The Weapon Feel seam:** future dynamic accuracy (bloom,
recoil, ADS) composes by either moving the axis before sampling or scaling the
effective spread via the modifier layer — pellet sampling reads whatever axis and
spread it is handed and needs no rework. One reassignment, stated: the
roadmap's Weapon Feel bullet owns "per-weapon accuracy stats… behind the
`effective()` seam", and `spreadDegrees` is exactly that — this spec pulls the
*static* accuracy stat forward into Resolution Modes because the pellet fan
cannot ship without it; Weapon Feel's bullet narrows to the dynamic axis
(bloom, recoil, ADS, the HUD slot and ring). The roadmap edit lands at
promotion.

**Alternatives rejected.**
- *Authored per-pellet patterns (ring/star fixed offsets).* Rejected by owner:
  cone-only. Patterns also fork the authoring surface (a pellet-offset table per
  weapon) for feel gains a cone approximates at this fidelity.
- *Divide-damage semantics (`damage` = shell total, engine divides by count).*
  Rejected: introduces rounding, makes per-pellet zone/crit math illegible, and —
  decisive — contradicts the shipped per-record application, which would need a
  divide inserted at every apply site.
- *Host-sampled directions replicated to the client.* Rejected: the host casts no
  rays for remote fire and never needs directions; replicating them adds wire
  surface to make the client match state nobody else can see (no fire-FX
  replication exists). Client-sampled is the client-authoritative-HIT shape.
- *Reusing the emitter's sampler in place.* Rejected: it is private, welded to
  `EmitterBridgeState`, and its seeding is welded to the entity-id and frame-step inputs the determinism gate forecloses. Extract the pure math instead.
- *Seeding from a sim tick + weapon entity id.* Rejected twice over: no tick
  reaches the local weapon stage today (threading one crosses ~41
  `simulate_tick` / `run_local_weapon_command` call sites for no other consumer),
  and entity-id bits vary with spawn order, which would fail the shipped
  spawn-order-reversal determinism assertion. The per-weapon shell counter
  (owner decision) needs neither.

**Foreclosures / one-way doors.** The stat names and semantics (`pelletCount`,
`spreadDegrees` as half-angle degrees) are the authoring contract and are expensive
to rename after mods author against them. The tuning-payload epoch bump is a
one-way version event but a routine, precedented one. Nothing else here is hard to
reverse: sampling internals, seed recipe, and FX cadence are behind engine seams.

## Ordering pins

Scenarios the tasks must hold and the test work asserts directly.

| # | Scenario | Ordering / input | Expected outcome |
|---|---|---|---|
| 1 | Legacy weapon, no new fields authored | `pelletCount` absent → 1, `spreadDegrees` absent → 0 | Behavior byte-identical to today on every path: one ray, exactly on axis, one impact, one record max |
| 2 | `spreadDegrees: 0` with `pelletCount: 8` | zero cone | All 8 rays exactly the (normalized) aim axis; 8 records/impacts on the same target legal |
| 3 | Multi-tick client frame | 2+ fire ticks resolve in one frame | First tick samples and casts all N pellets; each additional tick sends an empty declaration retiring its shot — never a second sample/cast |
| 4 | Declaration larger than pellet count | client declares > effective count | Host applies at most `pellet_count` records; invalid records still consume clamp budget (shipped behavior, now exercised at authored counts) |
| 5 | All pellets miss entities — client | N world/no hits | Client declares an empty record list (valid, retires shot) |
| 6 | Two identical sim runs, multi-pellet weapon | same inputs, same ticks | Recorded per-pellet impact points, weapon events, and deaths identical run-to-run |
| 7 | Consecutive shells from one weapon | shell counter advances between shells | Different pellet direction sets per consecutive shells — verified at the RNG level by Task 2's seed test and at shell level by Task 4's consecutive-shell recorded-impact comparison |
| 8 | Hot reload edits `pelletCount`/`spreadDegrees` | refresh mid-session | Live component reseeds both stats; cooldown/magazine/state and the shell counter persist; an already-open `AuthorizedShot` keeps the count it was minted with |
| 9 | All-pellets-headshot declaration | N records, all zone-tagged | Each record scales by its own zone multiplier — full multiplication accepted (PvE decision) |
| 10 | Pellet target lacks a NetworkId client-side | record dropped before send | Fewer records declared; remaining records validate normally |
| 11 | All pellets miss entities — local path | N world impacts | Per-impact sequence runs per world impact (FX at each point); no damage applied |
| 12 | Mid-shell target despawn — local path | pellet k's `on_impact` policy despawns target E; pellets k+1..N also resolved onto E | Host parity: each later pellet whose entity target no longer resolves skips damage and `on_impact` (impact FX may still spawn — the host path never spawns FX, so parity does not constrain presentation); world-hit pellets unaffected |
| 13 | Spawn-order-reversed determinism run | same command stream; harness entities spawned in reversed order | Identical labeled per-tick outcomes — pellet directions must not depend on `EntityId`/`NetworkId` allocation order |
| 14 | one pawn wields two instances of the same archetype in different inventory slots; both fire | each fired on its own tick after an active-slot switch — only the active wieldable fires | Distinct fans — the slot index in the salt disambiguates; cross-machine instances never share a sampler (each machine casts only its own pawn's rays), so no owner id is needed in the seed |
| 15 | Hot reload lowers `pelletCount` 8→4 with a connected client mid-RTT | host refreshes + resends payload; client predicts at 8 for up to one RTT | Shots minted after the reload clamp declarations at 4 (excess records consume budget, shipped clamp); a shot minted before it ingests at 8 (pin 8); the client's stat converges on payload apply — no verdict rejection required, no desync |
| 16 | Tuning payload carries out-of-range values | payload arrives with `pellet_count` 0 or huge, `spread_degrees` negative/NaN | Client clamps on apply to `1..=MAX_PELLET_COUNT` and finite `0..=45` — never casts 0 or >32 rays |
| 17 | Client multi-tick Auto frame | 3 fire ticks selected in one frame (the tick-selection loop breaks only for Semi) | Shell counter advances by exactly one (once per cast); ticks 2–3 send empty declarations; the client counter may lag the host's shell tally with no observable effect |
| 18 | Host mints a shot for a remote pawn | `run_remote_weapon_commands` accepts a remote fire | The remote weapon's shell counter does not advance host-side — the host casts no rays; only the declaring client's counter advances |
| 19 | Same-tick fire + completed weapon switch, local path | shell fires on the tick a lower completes and the inventory repoints, in one `run_local_weapon_command` call | All N pellets credit the firing weapon's snapshot, taken before the impacts loop — never the incoming weapon; the credit snapshot is captured after `tick_weapon_machine`, before the repoint block |
| 20 | Policy swaps or despawns the firing weapon mid-shell, local path | pellet k's `on_impact` changes the active wieldable | Pellets k+1..N apply with the pre-loop credit snapshot and full damage — matching the host's fire-time-snapshotted `AuthorizedShot` |
| 21 | Target reaches 0 HP mid-shell, corpse keeps `HealthComponent` | 8 pellets on one target; 0 HP at pellet 3, no despawn | Pellets 4–8 still validate and apply on both paths (the check is component presence, not liveness); exactly one death event for the shell |
| 22 | Hot reload raises `pelletCount` 4→8 with a connected client mid-RTT | host resends payload; client predicts at 4 for up to one RTT | Declarations of ≤4 records accepted whole (4 ≤ 8 — no clamp, no rejection); that shell deals proportionally less damage for at most one RTT; the client converges on payload apply |
| 23 | Client weapon rematerialized by tuning payload | slot goes shotgun → rifle → shotgun across payloads (despawn + fresh spawn) | `shells_fired` is instance-lifetime state and resets to 0 — the re-acquired weapon repeats its archetype's first fan; accepted, no engine consequence |
| 24 | Determinism harness records a shell with a mid-shell despawn | pellet 3's policy despawns the target; pellets 4..N skipped by the liveness re-check | The recorded per-pellet impact set is the cast set, not the applied subset — the compared tick shape is stable regardless of policy side effects; captured off `events.impacts` before the first apply |
| 25 | Client frame authorizes no fire tick | tick selection returns empty; `advance_client_fire_state` still runs on the no-tick branch and may return true | `shells_fired` does not advance; no `PelletRng` is constructed; no directions consumed |
| 26 | Client cast whose input command fails to send | `resolve_client_fire` returns `Some` (counter and cooldown written), then the send-tick mismatch path returns early | Counter stays advanced with no declaration sent — accepted; the next shell's fan is the following counter position |
| 27 | Impact policy displaces the shooter mid-shell | co-op: the host recomputes eye/LOS/range per record after each `on_impact`; single-player: those checks are deliberately not mirrored | Host rejects every remaining record once the shooter moves past `range × HIT_RANGE_TOLERANCE` (each still consumes clamp budget; no early break); single-player applies all N — accepted divergence, PvE |
| 28 | Level transition with a partly-fired weapon | `shells_fired = 5`; level change; weapon respawns from descriptor (no `WeaponComponent` persistence) | Counter resets to 0; the first shell of the new level repeats the archetype's first fan — deterministic, accepted |
| 29 | Drop to world, re-pickup into a different slot | the weapon entity survives; slot changes 0 → 1 | Counter keeps its value (entity-lifetime state); the fan still changes because the slot salt changed — both halves asserted |
| 30 | Two provenance-less weapons, same slot and counter | neither carries `DescriptorProvenance` nor authors `creditSource` | Identical fans — the fallback salt is the shared constant; degenerate by design, unreachable from production spawn routes |
| 31 | Hot reload edits `creditSource` on a provenance-less weapon | refresh changes only `creditSource` | The fallback salt input moves and the fan re-rolls — accepted; dev-only configuration off the authored path |
| 32 | Client-local descriptor refresh after a payload apply | refresh reseeds `pellet_count`/`spread_degrees` from the local descriptor; the host payload is unchanged, so the value-dedup suppresses a resend | Client stats follow the local descriptor until the next payload change — the same exposure shipped replicated stats (`range`, `cooldown_ms`) have; accepted precedent |
| 33 | Zero-spread and single-pellet shells still advance the counter | 3 shells at spread 0, then hot reload sets `spreadDegrees: 4`, then shell 4 fires | Shell 4 samples at counter position 3, not 0 — the increment is unconditional on a resolved shell; no skip-RNG-at-zero-spread optimization |

## Acceptance criteria

- [ ] A weapon authored `pelletCount: 8`, `spreadDegrees: 4` fired once in
  single-player: consumes ammo cost once, produces up to 8 impacts inside the
  cone, applies per-pellet damage with per-pellet zone multipliers, and fires the
  impact-policy consumer once per pellet impact, effects settling between pellets
  — including ordering-pin rows 11, 12, and 19–21.
- [ ] The same weapon in co-op: the firing client casts 8 rays at the rendered
  pose and declares 0..8 records; the host mints the shot with pellet count 8,
  clamps and validates per record, and applies per-record damage — the
  host-ingest halves (pins 4, 9, 10, and the ingest halves of 15 and 22)
  asserted in the host-side ingest tests, the client halves (pins 3, 5, the
  predicted ray count, and pin 16's clamp) in the client fire and
  payload-apply tests.
- [ ] Every shipped weapon authored without the new fields behaves identically to
  today on all paths (over the full content set this is a review/grep gate; the
  runnable core follows), asserted by a named regression: one impact, resolved along
  the unperturbed axis, with `event_names()` containing `"impact"` exactly once
  (ordering pin 1). The weapon script events `dry_fire` / `activate` / `impact`
  keep their at-most-once-per-shell cardinality.
- [ ] Descriptor validation rejects `pelletCount: 0` and `> 32`, and
  `spreadDegrees` negative or `> 45`, each with a load error naming the
  field; both VM parsers reject identically. A non-finite `spreadDegrees` is
  rejected by both VM conversion walkers before deserialization, asserted
  against the conv-layer path message.
- [ ] Zero spread means exact axis: ordering pin 2 asserted at the sampler and by
  a fired-shot test — an authored `pelletCount: 8`, zero-spread weapon produces 8 impacts
  all resolved along the exact aim axis.
- [ ] The determinism harness drives a multi-pellet, nonzero-spread weapon
  (now carrying `DescriptorProvenance`) through `simulate_tick`, records
  per-pellet impact points into the compared tick shape, and asserts: two
  identical runs match, the spawn-order-reversed run matches, two consecutive shells record different impact sets, and a second same-archetype
  instance in another slot, fired after an active-slot switch, records a
  different fan (pins 6, 7, 13, 14, 24).
- [ ] A connected client receives `pellet_count` and `spread_degrees` in the
  tuning payload, clamps them on apply (pin 16), and predicts N-ray fire from
  them; the payload epoch is bumped and the committed fixture re-blessed.
- [ ] Hot-reloading a weapon descriptor's `pelletCount` or `spreadDegrees`
  reseeds the live component per ordering pin 8 (two tests: component-level
  beside the refresh coverage, shot-level beside the ingest tests — the
  open-shot half proves the fire-time snapshot).
- [ ] A fixed-seed emitter test records the direction sequence before the
  sampler extraction and asserts it unchanged after (the emitter's
  degenerate-case early returns still consume zero draws), and the shared
  sampler has its own uniformity check. The fixed-seed gate is a
  golden-vector test; capturing its expected values before the extraction is
  a review obligation, not machine-checked.
- [ ] The generated `.d.ts` and `.d.luau` typedefs document both fields as
  optional with their ranges and defaults; the committed-typedef tests pass,
  including the rewritten per-pellet `damage` doc string.
- [ ] `reference_shotgun` fires 8 pellets per shell with per-pellet damage
  retuned; no residual 12-damage claim remains in `combat-demo.README.md`,
  `target-dummy.ts`, `combat-lifecycle.ts`'s derivation comment, or the two
  SDK `reference_enemy` authorings, and every shot-count / HP figure
  re-derives from the new per-shell total at the walkthrough's stated
  distance (a review/grep gate, not a runnable test); the two SDK
  authorings keep passing their shipped parity gate.

## Tasks

### Task 1: Descriptor stats + effective() + SDK surface

Add `pellet_count: u32` (`#[serde(default = "default_pellet_count")]` with a
`const fn` returning 1 in the
existing `default_cost_per_shot` style — plain `#[serde(default)]` would yield 0
and fail the validator) and `spread_degrees: f32` (plain
`#[serde(default)]` — 0.0 is the intended default; the asymmetry with
`pellet_count` is deliberate) to `WeaponDescriptor`
(`crates/foundation/src/data_descriptors/types/combat.rs`), validated in
`WeaponDescriptor::validate`: `pelletCount` in `1..=32` via a
`pub const MAX_PELLET_COUNT: u32 = 32` in the same module, `spreadDegrees` finite
and in `0.0..=45.0` (cone half-angle, degrees); errors are
`DescriptorError::InvalidShape` naming the wire field, matching the existing
`fireRateMs` error style. The struct's existing
`#[serde(rename_all = "camelCase")]` supplies the `pelletCount`/`spreadDegrees`
wire keys — no per-field rename. The mirrored `WeaponComponent` fields carry
`#[serde(default)]`, matching every post-original live-state field.
**Blast radius:** `WeaponDescriptor` derives no
`Default`, and none is added — every struct literal in the workspace (~37 sites
— a 38th grep hit is the struct definition itself
across `foundation`, `entities`, `scripting-core`, and `postretro` — netcode,
sim, scripting, main, mod_digest, observability, mostly test fixtures) gains the
two fields, using the default values. Thread both through `WeaponComponent` and
`EffectiveStats` in `crates/entities/src/components/weapon.rs` (passthrough, like
every existing stat) and add both to the authored-tuning set
`refresh_from_descriptor` reseeds — live fire/reload state stays preserved as
today. Task 2's `pub shells_fired: u32` counter field also lands here — all three
new `WeaponComponent` fields are one edit, each `#[serde(default)]` — and
`shells_fired` joins the preserved-live-state comment trailing
`refresh_from_descriptor`, since it is preserved by omission. Both VM parsers (`scripting-core/src/data_descriptors/js/entity.rs`,
`lua/entity.rs`) are serde-driven and need no reader edit; extend the shared parse
tests in `scripting-core/src/data_descriptors/tests/entity.rs` with authored,
defaulted, and each rejected-range case, parameterized over both VMs. The
non-finite `spreadDegrees` case never reaches the validator: both VM
conversion walkers reject NaN/Infinity before deserialization with the
conv-layer path message (``non-finite number at `spreadDegrees` ``), so that
case asserts the conv-layer message, and `WeaponDescriptor::validate`'s
`is_finite` check is defense-in-depth for non-VM ingresses only. Add a
weapon-refresh test beside the existing weapon coverage in
`crates/scripting-core/src/refresh_plan.rs` asserting a descriptor edit to either
stat reseeds the live component while cooldown/magazine/state and
`shells_fired` persist (ordering pin 8). Register both fields on the
`WeaponDescriptor` type in
`crates/postretro/src/scripting/primitives/mod.rs` as optional
(`"pelletCount?"`, `"spreadDegrees?"`, the `lowerMs?`/`raiseMs?` pattern) with
doc strings stating default, range, and half-angle semantics; regenerate typedefs
(`cargo run -p postretro --bin gen-script-types`) so `sdk/types/postretro.d.ts`,
`.d.luau`, and the typedef test fixtures update together. The shipped `damage`
doc string ("Base damage payload per resolved shot") is rewritten in the same
registration pass to state per-pellet semantics — "per pellet; a shell's total
is damage × pelletCount" — since the generated SDK would otherwise document
the shell-total reading this spec rejects.

### Task 2: Shared cone sampler + per-shell seed

Create a small pure module (suggest `crates/postretro/src/weapon/spread.rs`):
`sample_cone_direction(axis: Vec3, half_angle_rad: f32, u1: f32, u2: f32) -> Vec3`
— solid-angle-uniform inverse CDF (`cos θ = 1 − u·(1 − cos α)`, uniform azimuth,
orthonormal basis). Full degenerate-input contract, preserving the emitter
behavior it absorbs: the axis is normalized via `normalize_or_zero` with a
`Vec3::Y` fallback for a zero axis; spread is clamped to `>= 0`; at
`half_angle_rad <= f32::EPSILON` the function returns the **input axis
unmodified** when it is finite and non-zero — only the degenerate zero-axis
branch normalizes and falls back to `Vec3::Y` — because `normalize_or_zero` on
an already-unit axis is not the bit-identity (it multiplies by
`length_recip()`), and a legacy shell routed through the sampler must resolve
byte-identically to today's straight-through cast (the zero-spread exactness
pin; also required because the entity ray cast
debug-asserts a finite, non-zero direction and assumes unit length so `toi` is a
distance — the local sim path normalizes aim via `normalize_aim_direction` and
the client path's `Camera::aim_ray` returns a normalized direction, so for both
"normalized axis" is the axis). The emitter keeps its zero-axis and zero-spread
early returns *ahead of* its RNG draws and delegates only the post-draw math — a
signature taking pre-drawn uniforms would consume two draws in exactly the
degenerate cases that consume zero today, shifting every subsequent particle's
stream. Port the math from the emitter's private sampler in
`crates/postretro/src/scripting/systems/emitter_bridge.rs` and refactor the
emitter to delegate through its own RNG — emitter seeding policy (position ⊕ entity-id ⊕ frame-step — spawn-order-dependent, unsuitable for the sim path) is untouched, and a new fixed-seed emitter test records the
direction sequence before the refactor and asserts it unchanged after (the
existing mean-direction distribution test is draw-order-insensitive and is not
the gate).

Seeding: a **per-weapon shell counter** — a monotonic `shells_fired: u32` on
`WeaponComponent` live state (the field itself lands with Task 1's component
edits; preserved by `refresh_from_descriptor` by omission, zeroed at spawn),
incremented once per resolved shell. The two increment sites live in
`weapon/mod.rs` and belong to Task 3, which owns both functions' signatures;
Task 2 ships the pure pieces only — `sample_cone_direction` and `PelletRng`
plus a seed helper whose signature takes plain values,
`(shell_counter: u32, salt_name: &str, slot: usize)`, never registry types.
`PelletRng` wraps
the `SplitMix64` mixer
from `crates/postretro/src/trigger_pools.rs` — promote its private `next_u64` to
`pub(crate)` and update its doc comment to name the second consumer (no rand
dependency). Seed = SplitMix64 mix of the shell counter and a **spawn-order-stable
instance salt**: the weapon's canonical name (read from the weapon
entity's `DescriptorProvenance`; fallback when provenance is absent: the
component's `credit_source`) with its string bytes folded through the same
`SplitMix64` mixer — never a `RandomState`/`HashMap` hasher, which is
per-process randomized and would break replay — plus the pawn's active
inventory slot index. No owner
id in the seed — none is reachable from either fire path, and none is needed:
each machine samples only its own local pawn's fire (the host casts nothing for
remote pawns), so two owners' fans never share a sampler; instances in different
slots of one pawn are disambiguated by the slot. Never `EntityId`/`NetworkId`
bits (allocation-ordered; would break the spawn-order-reversal determinism
assertion) and never wall clock. Pellet i consumes sequential draws. Uniforms derive as the emitter's `next_f32`
does — top 24 bits over 2^24, in `[0, 1)`. The sampler takes `u2` as the raw
uniform and multiplies by TAU internally; the emitter's delegation adapts its
existing outside-TAU draw to that API. The provenance-absent fallback is
degenerate by design: a weapon with neither a canonical name nor an authored
`creditSource` salts as the shared `weapon.unknown` constant, leaving the slot
as the only disambiguator — unreachable from both production spawn routes,
which resolve descriptors by canonical name (pins 30, 31). Unit tests: zero-spread
exactness, unit-length output, degenerate-axis fallback, seed determinism (same
seed → same sequence; counter+1 → different; two salts, same counter →
different), and a distribution check in the style of the emitter's
`spawn_directions_distribute_within_cone`.

### Task 3: Pellet-general shot resolution (both paths)

In `crates/postretro/src/weapon/mod.rs`, generalize both resolution sites over
pellet count and spread, sourced from `effective()`; call sites convert with
`spread_degrees.to_radians()` — the sampler's parameter is radians only. Change
`WeaponFireEvents.impact: Option<WeaponImpact>` to `impacts: Vec<WeaponImpact>`
and update `event_names()` to push `"impact"` at most once per fire when the list
is non-empty (script-event cardinality unchanged); update every `events.impact`
reader — the `run_local_weapon_command` borrow site in
`sim/weapon_stage/commands.rs`; the tests in `sim/weapon_stage.rs`, which call
`tick_resolved_component` directly at four sites beside the `#[cfg(test)]`
wrappers; the ~10 `events.impact` assertions in `weapon/mod.rs`'s own test
module; and the `WeaponImpact` "Not `Copy`" audit comment above the struct,
which names the single-literal/single-borrow shape this task replaces (Task 3
edits these files ahead of Task 4's Phase 3 work; the phases are sequential, so
no conflict).
`fire_hitscan` samples N directions (`sample_cone_direction` over a `PelletRng`
seeded per Task 2's recipe; `run_local_weapon_command` holds the pawn, the
weapon entity id, and the `Inventory` active slot, and reads the canonical name
off the weapon's `DescriptorProvenance` — these seed inputs thread as new
parameters through `tick_resolved_component` into `fire_hitscan`, all
`run_local_weapon_command` call sites updating — two production sites in
`sim/mod.rs` (the main sim call and the equip-only pass) plus ~17 test call
sites in `sim/weapon_stage.rs` — and the
`#[cfg(test)]` `tick`/`tick_resolved` wrappers update with them (their call
sites are inside `weapon/mod.rs`'s own test module — `sim/weapon_stage.rs`
calls `tick_resolved_component` directly)) and resolves each pellet
independently through the existing `resolve_nearest_hit`, producing one
`WeaponImpact` per pellet that hit anything (world or entity), each carrying its
own point, normal, target, and zone; `WeaponActivation` keeps the unperturbed
aim axis, one per shell. `resolve_client_hitscan` samples the same way; the same salt inputs are
threaded as new `resolve_client_fire` parameters from its caller in `main.rs`
— the once-per-frame
cast *structure* (first-tick-only cast, empty-declaration retirement for
additional ticks) is unchanged; only the argument list grows. Task 3 owns
both `shells_fired` increments: `tick_resolved_component` (local) and
`resolve_client_fire` (client — after `advance_client_fire_state` returns
true, before the hitscan resolve), each passing the pre-increment value down
as the seed input. `main.rs`'s no-tick `advance_client_fire_state` branch
must not advance it (pin 25); a cast whose input command fails to send keeps
its increment (pin 26); `run_remote_weapon_commands` never increments (pin
18); the increment is unconditional on a resolved shell — single-pellet and
zero-spread shells advance it (pin 33). The salt fallback chain restated for
this task's wiring: provenance canonical name → the component's
`credit_source` → the shared `weapon.unknown` constant. On a connected
client the stats read the local component, which a later phase populates
from the tuning payload — Task 3 adds no payload plumbing. It returns one
`LocalHitRecord` per pellet whose nearest hit is an entity — the existing
entity-only record rule, per pellet. Named regressions in this task: an
unauthored weapon (`pellet_count` 1, `spread_degrees` 0) resolves byte-identically
to today through both functions — one impact, unperturbed axis, `"impact"` once
(pin 1) — and a `pellet_count: 8`, zero-spread weapon produces 8 impacts all on
the exact axis (pin 2, the fired-shot half of AC 5). Two more named tests here:
a multi-tick Auto frame (2+ fire ticks in one frame — one cast, one counter
increment, empty declarations retiring the rest; pins 3, 17) and an
all-pellets-miss shell producing a valid empty declaration (pin 5). Ammo cost
and cooldown stay per-shell in the weapon state machine, which no task touches
— pellet count never reaches it.

### Task 4: Local-path per-pellet consumption + authorized-shot mint

In `crates/postretro/src/sim/weapon_stage/commands.rs`: `run_local_weapon_command`
snapshots `(weapon_id, effective().credit_source.to_string())` immediately
after the `tick_weapon_machine` call — before the `machine.lowered`
switch-repoint block and before the component moves back into the registry,
mirroring `AuthorizedShot`'s fire-time capture; never obtained by re-resolving
`Inventory::active_wieldable` — then loops Task 3's `impacts`
in order, applying each pellet through `spawn_impact_effect_at` and the
snapshot-accepting `apply_authorized_weapon_impact_damage` (its `damage_amount`
argument comes from each pellet's own `ActivationOutcome::Hit(payload).amount`,
as the shipped local path already reads it — not from a pre-loop
`effective().damage`), then `on_impact` — `apply_weapon_impact_damage`, whose
per-call active-wieldable re-resolution would re-credit pellets to an incoming
weapon after a same-tick switch or a policy-driven swap mid-shell, is not
called from this path at all (pins 19, 20). The
impact-policy consumer runs after each pellet exactly as the host ingest
already does for remote pellets. Host parity governs the mid-shell edges (pins
12, 21): before each pellet's apply, the loop re-checks `registry.exists(target)`
**and** `get_component::<HealthComponent>(target).is_ok()` for the entity
target, and that the firing pawn still resolves — either failing skips
damage and `on_impact` for that pellet,
silently (no per-pellet warn spam), mirroring the host's per-record checks in
`ingest_hit_declaration`. A target at 0 HP whose corpse still carries
`HealthComponent` keeps applying (host parity). The host's world-LOS and range
re-checks are deliberately not mirrored — the local points come from a
same-tick cast. World-hit pellets are unaffected, and FX is unconstrained by
parity (the host path spawns none). A world-hit pellet runs the full sequence
including `on_impact`, matching the shipped single-impact behavior — up to
`MAX_PELLET_COUNT` policy dispatches per wall shot is the accepted consequence
(pin 11). A local analogue of the shipped
"policy effects settle between pellets" regression asserts this observable,
including both mid-shell despawn cases — target and shooter. Further named
tests here: same-tick-switch and policy-driven-swap credit tests (each
repointing to a weapon with a distinct `creditSource` and asserting every
damage record names the outgoing one; pins 19, 20), a world-hit-only shell
(per-pellet FX and `on_impact`, no damage; pin 11), a 0-HP-corpse shell
asserting continued application and exactly one death event (pin 21), a
remote-mint counter-unchanged assertion (pin 18), one ammo debit for an
8-pellet shell, and per-pellet zone multiplication on the local path — its
zone site is distinct from the netcode one. In the same file,
`run_remote_weapon_commands` mints `AuthorizedShot` with
`pellet_count: effective.pellet_count as usize` from the weapon's `effective()`
instead of the literal 1 — the ingest clamp, zero-count rejection, and per-record
validation in `netcode/mod.rs` are consumed unchanged. The mint does not advance
the remote weapon's shell counter — the host casts nothing for remote fire;
only the declaring client's own counter advances, once per cast (a client
resolving multiple fire ticks in one frame increments once, and its counter
may lag the host's shell tally — unobservable, since the host never samples
directions; pins 17, 18). Extend
`sim/determinism_tests.rs`: record per-pellet impact points (or a hash of them)
into the compared tick shape — no existing field of it observes ray
directions. The recorded set is the *cast* impacts, including pellets the
liveness re-check later skips, so the comparison is stable against policy side
effects (pin 24). The harness gains: a `DescriptorProvenance` on its weapon (the
canonical-name salt input — today it is spawned without one, which would
collapse the salt to its fallback inside the very gate AC 6 relies on); a
second same-archetype weapon in another slot, fired on later ticks after an
active-slot switch (only the active wieldable fires; two slots cannot fire in
one tick), whose recorded fan must differ (pin 14) — both harness weapons
carry `DescriptorProvenance` with the same canonical name, so the slot index
is the differentiator pin 14 asserts; a backstop wall behind the target — the shipped harness aims
horizontally over a flat floor a horizontal ray never hits, so off-target
pellets would record nothing and an N-impact anchor would be unfalsifiable;
and a positive anchor asserting a shell records exactly N impacts. Capture
plumbing, three hops: `run_local_weapon_command`'s return grows the per-shell
impact set (the same ~19 call sites Task 3 enumerates absorb the tuple
change), `TickEvents` carries it, `RecordedTick` compares it — cloned or
hashed off `events.impacts` before the first pellet applies, so no `on_impact`
side effect can influence it (pin 24). Then drive a multi-pellet,
nonzero-spread weapon through `simulate_tick` and assert run-to-run equality,
spawn-order-reversed equality, and that two consecutive shells record
different impact sets (pins 6, 7, 13; AC 6).

### Task 5: Tuning payload + client prediction stats

Replicate the two stats to clients so predicted fire casts the real pellet fan.
`WieldableTuningPayload` (`crates/postretro/src/netcode/tuning_payload.rs`) gains
`pellet_count: u32` and `spread_degrees: f32`; bump `TUNING_PAYLOAD_EPOCH` 3 → 4;
re-bless `netcode/tests/fixtures/tuning_payload.expected.json` via
`POSTRETRO_BLESS_COMPATIBILITY_FIXTURES=1` (semantic change — the epoch bump is
the honest path the fixture's own failure message prescribes). The bump also
lands in two test surfaces the fixture re-bless does not touch:
`payload_rejects_previous_merge_semantics_epoch` hardcodes `expected: 3` and
updates to the new epoch, and every `WieldableTuningPayload` struct literal
gains the new fields — six sites, all test helpers: two in `weapon_slots()`
(`tuning_payload.rs`), one in `netcode/mod.rs`, three in
`scripting/builtins/net_descriptor.rs`. Populate it in
`tuning_payload_for_pawn` (`netcode/mod.rs`) from the slot's live
`WeaponComponent`. `apply_net_wieldable_tuning`
(`scripting/builtins/net_descriptor.rs`) is the **single write chokepoint** — it
writes both fields onto the client's local component beside `range`/`cooldown_ms`,
and `materialize_net_local_wieldable_at_slot` reaches it by delegation, so no
other file needs a write site. The payload is a separate ingress that bypasses
descriptor validation, so the chokepoint **clamps on apply**: `pellet_count` to
`1..=MAX_PELLET_COUNT` (already reachable as `postretro_foundation::MAX_PELLET_COUNT`
through the crate's glob re-exports — no export edit), `spread_degrees`
to finite `0.0..=45.0` (pin 16). The host-side value is always
descriptor-validated finite, so `last_sent_tuning`'s equality dedup stays
well-defined — pin 16 is a client-side ingress guard only. A client-local descriptor refresh
reaching `refresh_from_descriptor` can overwrite the replicated stats until
the next payload change — the identical exposure the shipped replicated
stats (`range`, `cooldown_ms`) already have; the new stats deliberately
follow that shipped precedent rather than growing a carve-out here (pin 32).
`damage` stays
unreplicated — the host owns
damage; the client needs count and cone, not amounts. Two test surfaces,
split by module — the shipped test module has no host↔client loop, so no
single end-to-end test exists to write. Host side, beside the shipped
pellet regressions in the `netcode/mod.rs` test module (its ingest fixture
is host-only; the shipped authorization-time-facts-after-switch test is the
precedent): a `pelletCount: 8` weapon's declaration applies up to 8 accepted
records with per-record damage and zone multipliers, plus pins 4, 9, and 10,
and the ingest halves of pins 15 and 22 — realized by mutating the live
`WeaponComponent`'s `pellet_count` between the mint and the ingest, since the
fire-time snapshot lives on `AuthorizedShot`. Client side, beside
`apply_net_wieldable_tuning` in `net_descriptor.rs`: pin 16's apply-clamp and
the predicted N-ray count. The clamp writes the base component fields —
there is no override layer for these stats — which is exactly the pin-32
exposure accepted above.

### Task 6: Dev-mod reference + walkthrough

Retune `content/dev/scripts/reference-shotgun.ts` into a real pellet weapon:
`pelletCount: 8`, `spreadDegrees: 4`, `damage` dropped to a per-pellet value
(suggest 3.0 — shell total 24; comment the per-pellet semantics inline). Five files are arithmetic-locked to 12 damage or the HP it implies: the walkthrough at
`content/dev/maps/combat-demo.README.md` (dummy hit counts, the 60 HP enemy's
five-shot down, the −12 gib crossing, magazine budgets), the damage comment in
`content/dev/scripts/target-dummy.ts`, the derivation comment above
`FINISHER_OVERSHOOT = -12` in `content/dev/scripts/combat-lifecycle.ts`, and
the `reference_enemy` health block authored twice in the SDK —
`sdk/behaviors/reference/entities.ts` and
`sdk/behaviors/reference/entities.luau`, which must move together or the
shipped parity gate
`the_shipped_reference_enemy_graph_is_identical_in_both_authorings` fails —
this task owns re-deriving every numeric claim in all five. The gib predicate is
`.le(FINISHER_OVERSHOOT)`, so a shell landing exactly at −12 gibs. Anchor the
walkthrough at a stated point-blank distance where all 8 pellets connect so
shot counts stay deterministic, and retune both targets' HP so the staged
down-then-gib beat survives per-shell totals of 24 — the killing shell must
land above −12 with a later shell crossing it (suggest raising the shipped
30 HP dummy to 40: 16 → −8 downs → −32 gibs; and the shipped 60 HP enemy —
authored in the two SDK files — to 70: 46 → 22 → −2 downs → −26 gibs; at 60
the enemy would down-and-gib in one shell at exactly −12, erasing the
walkthrough's death/recovery beat). Exact numbers are Task 6's dev-fixture
tuning choice within these constraints. Leave `reference_pistol`
and both wieldable fixtures unauthored — their unchanged behavior is asserted
by Task 3's named pin-1 regression, not assumed.

## Sequencing

**Phase 1 (concurrent):** Task 1 (descriptor surface), Task 2 (sampler + seed) —
no dependency between them. Task 1 owns every `crates/entities/src/components/weapon.rs`
edit — including Task 2's counter field. One-line overlap remains: Task 2's
`mod spread;` declaration lands in `weapon/mod.rs`, a file Task 1's literal
sweep also touches — a trivially mergeable line; otherwise disjoint.
**Phase 2 (sequential):** Task 3 — thin slice through resolution; consumes Task 1's
stats and Task 2's sampler, falsifies the fire-events reshape and seeding
assumptions before the consumption fan-out.
**Phase 3 (concurrent):** Task 4 (weapon-stage consumption + mint), Task 5
(netcode payload + client stats) — both consume Task 3; disjoint files
(`sim/weapon_stage/` + `sim/determinism_tests.rs` vs `netcode/` +
`scripting/builtins/`).
**Phase 4 (sequential):** Task 6 — consumes all of it.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| pellet count | `WeaponDescriptor.pellet_count: u32`; `EffectiveStats.pellet_count`; `AuthorizedShot.pellet_count: usize` (existing) | descriptor key `pelletCount`; tuning payload JSON `pellet_count` (snake_case, matching `cooldown_ms`) | `pelletCount?: number` (default 1) | same | n/a |
| spread cone | `WeaponDescriptor.spread_degrees: f32`; `EffectiveStats.spread_degrees` | descriptor key `spreadDegrees`; tuning payload `spread_degrees` | `spreadDegrees?: number` (default 0, half-angle) | same | n/a |
| pellet cap | `pub const MAX_PELLET_COUNT: u32 = 32` (foundation, beside the validator; also the client apply-clamp bound) | enforced at descriptor validation and payload apply — no wire count field exists | documented in typedef doc string | same | n/a |
| shell counter | `WeaponComponent.shells_fired: u32`, `#[serde(default)]` (as are the mirrored `pellet_count`/`spread_degrees` component fields) | not on the descriptor or tuning payload — engine live state | — (not author-facing) | — | n/a |

No new wire messages, and no `WIRE_VERSION` bump despite `networking.md`
§Version gates' field-addition rule: the tuning payload is JSON the net crate
carries opaquely, not a bitcode message layout — the payload's own epoch is its
gate. `HitDeclaration` already carries 0..N records with no count field.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| One authorized fire accepts ≤ `pellet_count` records; invalid records consume budget; zero count rejects hits | shipped ingest (`ingest_hit_declaration`) | Task 4's mint feeds it real counts — the clamp/validation shape must not be edited | AC 2, pins 4, 10, 15 |
| Impact-policy consumer runs after each pellet, effects settling between pellets, on **both** paths — with a dead-target pellet skipping damage + policy on both | shipped ingest (remote); Task 4 (local loop + liveness re-check) | any batching of the local apply loop, or an unconditional `on_impact`, re-opens the stale-policy-state bug the shipped regression pinned; per-call credit re-resolution after a same-tick switch — the loop applies through the pre-loop snapshot | AC 1, 2, pin 12, 19–21 |
| Unauthored weapons behave byte-identically (count 1, spread 0 ⇒ exact-axis single ray) | Task 1 (named default fn = 1), Task 2 (zero-spread exactness), Task 3 (named pin-1 regression) | plain-`default` drift (u32 → 0 fails validation), or any sampler that perturbs at zero spread | AC 3, 5, pins 1, 2 |
| Pellet directions are a pure function of replay-stable, spawn-order-stable state | Task 2 (shell counter + canonical-name/slot salt), Task 3 (seed threading) | any `EntityId`/`NetworkId`/wall-clock/global-counter seeding reaching `simulate_tick`; any owner-id salt input (unreachable from either fire path) | AC 6, pins 6, 7, 13, 14, 25–26, 33 |
| Client casts pellets once per frame; additional fire ticks retire via empty declarations | shipped client fire loop | Task 3 must resolve N inside the existing single cast; Task 5 must not add a second cast site | AC 2, pin 3 |
| `damage` means per-pellet on every path; shell total is damage × count | shipped per-record/per-impact application; owner decision | any future divide-by-count "normalization" at an apply site | AC 1, 2, 11 |
| Ammo cost and cooldown consume per shell, never per pellet | shipped weapon state machine (untouched by this plan) | any pellet loop reaching the machine or the ammo debit | AC 1 |
| Weapon script events (`dry_fire`/`activate`/`impact`) fire at most once per shell | Task 3 (`event_names` over the list) | per-pellet event emission would N× audio and script dispatch | AC 3 |

## Script syntax examples

```ts
// Proposed design — the weapon block Task 6 lands inside the existing
// defineEntity(...) call in reference-shotgun.ts; the entity's mesh /
// touchable / model fields are unchanged and elided here.
// damage is PER PELLET: this shell totals 8 × 3 = 24 on a full connect.
weapon: {
  damage: 3.0,            // per pellet
  pelletCount: 8,         // 1..=32; default 1
  spreadDegrees: 4,       // cone half-angle; default 0 = laser-exact
  range: 64.0,
  fireRateMs: 700.0,
  fireMode: "semi",
  resolution: "hitscan",
  resource: {
    kind: "ammo", type: "shells.buck",
    magazine: 8, reserve: 32, reloadMs: 450, reloadStyle: "perShell",
  },
},
```

## Open questions

- **Local-path world-impact FX volume.** Up to 32 pellets × the existing
  9-particle burst per world impact is bounded but untuned; if playtest shows it
  noisy, thinning is a presentation tweak inside `spawn_impact_effect_at`'s
  caller, not a contract change. Default: ship unthinned.
- **Zone-multiplier stacking at high counts.** Pin 9 accepts full per-pellet
  multiplication (PvE). If a future balance pass wants a per-shell multiplier
  cap, it belongs in the Damage & Defenses milestone, not here.
