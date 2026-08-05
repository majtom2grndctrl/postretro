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
  per-shot deterministic seeding from sim-visible state.
- **Local fire path generalized:** the single-player / listen-host path resolves N
  pellets per shell — per-pellet impact, damage, FX, and impact-policy fire.
- **Client fire path generalized:** the connected client samples and casts N rays
  once per frame and declares 0..N hit records; the host mints `AuthorizedShot`
  with the weapon's effective pellet count.
- **Tuning payload:** `pellet_count` + `spread_degrees` replicate to clients;
  epoch bump; fixture re-bless.
- **Dev-mod reference:** `reference_shotgun` becomes a real pellet weapon with
  per-pellet damage retuned.

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
- **Divergence, named:** the emitter precedent seeds its cone RNG
  non-deterministically by design. This spec's sampling runs inside
  `simulate_tick`, which the determinism gate replays — so seeding here is
  deterministic from sim state, deliberately opposite the precedent's policy
  while reusing its math.

**Placement.** Spread lives on the weapon descriptor behind `effective()` — the
stat seam the roadmap reserves for accuracy axes — not on the camera/aim layer and
not as runtime-writable weapon state. The aim direction stays the pure screen-center
`aim_ray()`; pellet sampling perturbs per-pellet ray directions *around* that axis
at resolution time. **The Weapon Feel seam:** future dynamic accuracy (bloom,
recoil, ADS) composes by either moving the axis before sampling or scaling the
effective spread via the modifier layer — pellet sampling reads whatever axis and
spread it is handed and needs no rework.

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
  `EmitterBridgeState`, and its seeding policy is the opposite of what the
  determinism gate needs. Extract the pure math instead.

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
| 2 | `spreadDegrees: 0` with `pelletCount: 8` | zero cone | All 8 rays exactly the aim axis (sampler returns the axis unperturbed at zero spread); 8 records/impacts on the same target legal |
| 3 | Multi-tick client frame | 2+ fire ticks resolve in one frame | First tick samples and casts all N pellets; each additional tick sends an empty declaration retiring its shot — never a second sample/cast |
| 4 | Declaration larger than pellet count | client declares > effective count | Host applies at most `pellet_count` records; invalid records still consume clamp budget (shipped behavior, now exercised at authored counts) |
| 5 | All pellets miss entities | N world/no hits | Client declares an empty record list (valid, retires shot); local path spawns world-impact FX per pellet, applies no damage |
| 6 | Two identical sim runs, multi-pellet weapon | same inputs, same ticks | Identical per-pellet directions, impacts, deaths — determinism gate green with `pelletCount > 1` driven |
| 7 | Consecutive shells from one weapon | tick advances between shells | Different pellet directions per shell (seed varies with fire tick) |
| 8 | Hot reload edits `pelletCount`/`spreadDegrees` | refresh mid-session | Live component reseeds both stats; cooldown/magazine/state preserved; an already-open `AuthorizedShot` keeps the count it was minted with |
| 9 | All-pellets-headshot declaration | N records, all zone-tagged | Each record scales by its own zone multiplier — full multiplication accepted (PvE decision) |
| 10 | Pellet target lacks a NetworkId client-side | record dropped before send | Fewer records declared; remaining records validate normally |

## Acceptance criteria

- [ ] A weapon authored `pelletCount: 8`, `spreadDegrees: 4` fired once in
  single-player: consumes ammo cost once, produces up to 8 impacts inside the
  cone, applies per-pellet damage with per-pellet zone multipliers, and fires the
  impact-policy consumer once per pellet impact, effects settling between pellets.
- [ ] The same weapon in co-op: the firing client casts 8 rays at the rendered
  pose and declares 0..8 records; the host mints the shot with pellet count 8,
  clamps and validates per record, and applies per-record damage — including the
  ordering-pin rows 3, 4, 5, and 10.
- [ ] Every shipped weapon authored without the new fields behaves identically to
  today on all paths (ordering pin 1); the weapon script events `dry_fire` /
  `activate` / `impact` keep their at-most-once-per-shell cardinality.
- [ ] Descriptor validation rejects `pelletCount: 0` and `> 32`, and
  `spreadDegrees` negative, `> 45`, or non-finite, each with a load error naming
  the field; both VM parsers reject identically.
- [ ] Zero spread means exact axis: ordering pin 2 asserted at the sampler and at
  a fired-shot level.
- [ ] The determinism harness drives a `pelletCount > 1`, `spreadDegrees > 0`
  weapon through `simulate_tick` and two identical runs match (pins 6, 7).
- [ ] A connected client receives `pellet_count` and `spread_degrees` in the
  tuning payload and predicts N-ray fire from them; the payload epoch is bumped
  and the committed fixture re-blessed.
- [ ] Hot-reloading a weapon descriptor's `pelletCount` or `spreadDegrees`
  reseeds the live component per ordering pin 8.
- [ ] The generated `.d.ts` and `.d.luau` typedefs document both fields with
  their ranges and defaults; the committed-typedef tests pass.
- [ ] `reference_shotgun` fires 8 pellets per shell with per-pellet damage
  retuned; the combat demo walkthrough notes the pellet behavior.

## Tasks

### Task 1: Descriptor stats + effective() + SDK surface

Add `pellet_count: u32` (serde `pelletCount`, `#[serde(default)]` → 1) and
`spread_degrees: f32` (serde `spreadDegrees`, default 0.0) to `WeaponDescriptor`
(`crates/foundation/src/data_descriptors/types/combat.rs`), validated in
`WeaponDescriptor::validate`: `pelletCount` in `1..=32` via a
`pub const MAX_PELLET_COUNT: u32 = 32` in the same module, `spreadDegrees` finite
and in `0.0..=45.0` (cone half-angle, degrees); errors are
`DescriptorError::InvalidShape` naming the wire field, matching the existing
`fireRateMs` error style. Thread both through `WeaponComponent` and
`EffectiveStats` in `crates/entities/src/components/weapon.rs` (passthrough, like
every existing stat) and add both to the authored-tuning set
`refresh_from_descriptor` reseeds — live fire/reload state stays preserved as
today. Both VM parsers (`scripting-core/src/data_descriptors/js/entity.rs`,
`lua/entity.rs`) are serde-driven and need no reader edit; extend the shared parse
tests in `scripting-core/src/data_descriptors/tests/entity.rs` with authored,
defaulted, and each rejected-range case, parameterized over both VMs. Add a weapon-refresh test beside the existing
`refresh_plan.rs` weapon coverage asserting a descriptor edit to either stat
reseeds the live component while cooldown/magazine/state persist (ordering pin
8). Register both fields on the `WeaponDescriptor` type in
`crates/postretro/src/scripting/primitives/mod.rs` with doc strings stating
default, range, and half-angle semantics; regenerate typedefs
(`cargo run -p postretro --bin gen-script-types`) so `sdk/types/postretro.d.ts`,
`.d.luau`, and the typedef test fixtures update together.

### Task 2: Shared cone sampler + per-shot seed

Create a small pure module (suggest `crates/postretro/src/weapon/spread.rs`):
`sample_cone_direction(axis: Vec3, half_angle_rad: f32, u1: f32, u2: f32) -> Vec3`
— solid-angle-uniform inverse CDF (`cos θ = 1 − u·(1 − cos α)`, uniform azimuth,
orthonormal basis), returning a normalized vector, and returning `axis` exactly
when `half_angle_rad <= f32::EPSILON` (the zero-spread exactness pin; also required
because the entity ray cast debug-asserts unit-length directions). Port the math
from the emitter's private sampler in
`crates/postretro/src/scripting/systems/emitter_bridge.rs` and refactor the
emitter to delegate to the shared function through its own RNG — emitter seeding
policy (deliberately non-reproducible) is untouched. Beside it, a per-shot stream:
`PelletRng` wrapping the `SplitMix64` mixer from
`crates/postretro/src/trigger_pools.rs` (reuse or re-export; do not add a rand
dependency), seeded from `(fire tick, firing weapon id bits)` so directions are a
pure function of sim-visible state — identical across replayed runs, different
across shells. Unit tests: zero-spread exactness, unit-length output, seed
determinism (same seed → same sequence; tick+1 → different), and a distribution
check in the style of the emitter's `spawn_directions_distribute_within_cone`.

### Task 3: Pellet-general shot resolution (both paths)

In `crates/postretro/src/weapon/mod.rs`, generalize both resolution sites over
pellet count and spread, sourced from `effective()`. Change
`WeaponFireEvents.impact: Option<WeaponImpact>` to `impacts: Vec<WeaponImpact>`
and update `event_names()` to push `"impact"` at most once per fire when the list
is non-empty (script-event cardinality unchanged); update every `events.impact`
reader — the sim weapon stage borrow site and tests. `fire_hitscan` samples N
directions (`sample_cone_direction` over a `PelletRng` seeded per Task 2's recipe
from inputs threaded by its callers — `tick_resolved_component` gains the seed
inputs from `run_local_weapon_command`) and resolves each pellet independently
through the existing `resolve_nearest_hit`, producing one `WeaponImpact` per
pellet that hit anything (world or entity), each carrying its own point, normal,
target, and zone. `resolve_client_hitscan` samples the same way (seeded from
`client_tick` + weapon identity via `resolve_client_fire`) and returns one
`LocalHitRecord` per pellet whose nearest hit is an entity — the existing
entity-only record rule, per pellet. A `pelletCount: 1`, `spreadDegrees: 0`
weapon must resolve byte-identically to today through both functions. The
client's once-per-frame cast structure in `main.rs` is untouched — N pellets
resolve inside the single existing `resolve_client_fire` call, and the
empty-declaration retirement for additional ticks stays as is.

### Task 4: Local-path per-pellet consumption + authorized-shot mint

In `crates/postretro/src/sim/weapon_stage/commands.rs`: `run_local_weapon_command`
loops Task 3's `impacts` in order, applying the existing per-impact sequence to
each pellet — `spawn_impact_effect_at`, `apply_weapon_impact_damage`, then
`on_impact` — so the impact-policy consumer runs after each pellet exactly as the
host ingest already does for remote pellets (its "policy effects settle between
pellets" regression is the contract; a local analogue test asserts the same
observable). In the same file, `run_remote_weapon_commands` mints `AuthorizedShot`
with `pellet_count: stats.pellet_count as usize` from the weapon's `effective()`
instead of the literal 1 — the ingest clamp, zero-count rejection, and per-record
validation in `netcode/mod.rs` are consumed unchanged. Extend
`sim/determinism_tests.rs`: drive a multi-pellet, nonzero-spread weapon through
`simulate_tick` and assert two identical runs record identical
`authorized_shots` (now carrying the real count), weapon event names, and deaths
(ordering pins 6, 7).

### Task 5: Tuning payload + client prediction stats

Replicate the two stats to clients so predicted fire casts the real pellet fan.
`WieldableTuningPayload` (`crates/postretro/src/netcode/tuning_payload.rs`) gains
`pellet_count: u32` and `spread_degrees: f32`; bump `TUNING_PAYLOAD_EPOCH` 3 → 4;
re-bless `netcode/tests/fixtures/tuning_payload.expected.json` via
`POSTRETRO_BLESS_COMPATIBILITY_FIXTURES=1` (semantic change — the epoch bump is
the honest path the fixture's own failure message prescribes). Populate it in
`tuning_payload_for_pawn` (`netcode/mod.rs`) from the slot's live
`WeaponComponent`, and write both fields onto the client's local component in
`apply_net_wieldable_tuning` and `materialize_net_local_wieldable_at_slot`
(`scripting/builtins/net_descriptor.rs`) and the remote-materialize path
(`netcode/remote_materialize.rs`) beside `range`/`cooldown_ms`. `damage` stays
unreplicated — the host owns damage; the client needs count and cone, not
amounts. Add an end-to-end host↔client test in the existing netcode harness: a
`pelletCount: 8` weapon's declaration round-trips with up to 8 accepted records
applying per-record damage, plus ordering-pin rows 4 and 10 at authored counts.

### Task 6: Dev-mod reference + walkthrough

Retune `content/dev/scripts/reference-shotgun.ts` into a real pellet weapon:
`pelletCount: 8`, `spreadDegrees: 4`, `damage` dropped to a per-pellet value
(suggest 3.0 — shell total 24 versus today's 12; a dev-fixture tuning choice,
comment the per-pellet semantics inline). Leave `reference_pistol` and both
wieldable fixtures unauthored (they exercise ordering pin 1 by existing). Update
the combat demo README walkthrough: pellet behavior, the per-pellet damage
meaning, and the spread cone being a descriptor stat.

## Sequencing

**Phase 1 (concurrent):** Task 1 (descriptor surface), Task 2 (sampler) — disjoint
files, no dependency.
**Phase 2 (sequential):** Task 3 — thin slice through resolution; consumes Task 1's
stats and Task 2's sampler, falsifies the fire-events reshape and seeding
assumptions before the consumption fan-out.
**Phase 3 (concurrent):** Task 4 (weapon-stage consumption + mint), Task 5
(netcode payload + client stats) — both consume Task 3; disjoint files
(`sim/weapon_stage/` vs `netcode/` + `scripting/builtins/`).
**Phase 4 (sequential):** Task 6 — consumes all of it.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| pellet count | `WeaponDescriptor.pellet_count: u32`; `EffectiveStats.pellet_count`; `AuthorizedShot.pellet_count: usize` (existing) | descriptor key `pelletCount`; tuning payload JSON `pellet_count` (snake_case, matching `cooldown_ms`) | `pelletCount?: number` (default 1) | same | n/a |
| spread cone | `WeaponDescriptor.spread_degrees: f32`; `EffectiveStats.spread_degrees` | descriptor key `spreadDegrees`; tuning payload `spread_degrees` | `spreadDegrees?: number` (default 0, half-angle) | same | n/a |
| pellet cap | `MAX_PELLET_COUNT: u32 = 32` (foundation, beside the validator) | enforced at descriptor validation only — no wire count field exists | documented in typedef doc string | same | n/a |

No new wire messages, no wire-version bump: `HitDeclaration` already carries 0..N
records with no count field, and the tuning payload versions itself through its
own epoch.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| One authorized fire accepts ≤ `pellet_count` records; invalid records consume budget; zero count rejects hits | shipped ingest (`ingest_hit_declaration`) | Task 4's mint feeds it real counts — the clamp/validation shape must not be edited | AC 2, pins 4, 10 |
| Impact-policy consumer runs after each pellet, effects settling between pellets, on **both** paths | shipped ingest (remote); Task 4 (local loop) | any batching of the local apply loop re-opens the stale-policy-state bug the shipped regression pinned | AC 1, 2 |
| Unauthored weapons behave byte-identically (count 1, spread 0 ⇒ exact-axis single ray) | Task 1 (defaults), Task 2 (zero-spread exactness), Task 3 (single-sample path) | any sampler that perturbs at zero spread, or default drift in the descriptor | AC 3, 5, pins 1, 2 |
| Pellet directions are a pure function of sim-visible state | Task 2 (seed recipe), Task 3 (seed threading) | any wall-clock, global-counter, or emitter-style seeding reaching `simulate_tick` | AC 6, pins 6, 7 |
| Client casts pellets once per frame; additional fire ticks retire via empty declarations | shipped client fire loop | Task 3 must resolve N inside the existing single cast; Task 5 must not add a second cast site | AC 2, pin 3 |
| `damage` means per-pellet on every path; shell total is damage × count | shipped per-record/per-impact application; owner decision | any future divide-by-count "normalization" at an apply site | AC 1, 2, 10 |
| Weapon script events (`dry_fire`/`activate`/`impact`) fire at most once per shell | Task 3 (`event_names` over the list) | per-pellet event emission would N× audio and script dispatch | AC 3 |

## Script syntax examples

```ts
// Proposed design — reference_shotgun with the two new stats.
// damage is PER PELLET: this shell totals 8 × 3 = 24 on a full connect.
export const referenceShotgun: WeaponEntityDescriptor = {
  canonicalName: "reference_shotgun",
  components: {
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
  },
};
```

## Open questions

- **Local-path world-impact FX volume.** Up to 32 pellets × the existing
  9-particle burst per world impact is bounded but untuned; if playtest shows it
  noisy, thinning is a presentation tweak inside `spawn_impact_effect_at`'s
  caller, not a contract change. Default: ship unthinned.
- **Zone-multiplier stacking at high counts.** Pin 9 accepts full per-pellet
  multiplication (PvE). If a future balance pass wants a per-shell multiplier
  cap, it belongs in the Damage & Defenses milestone, not here.
