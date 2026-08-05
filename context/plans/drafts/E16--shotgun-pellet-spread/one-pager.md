# Shotgun Pellet Spread — Pre-Spec One-Pager

> **Status:** planning brief, not a spec. Seeds a future `/draft-plan` session.
> **Owner question answered here:** is pellet spread part of the roadmap's
> Weapon Feel "spread / recoil / accuracy" bullet, or its own spec — and which
> goes first?

## Verdict: separate specs; pellet spread first

**They are different features that share one word.** Prior specs already carved
the boundary:

- **Pellet spread** (this brief) is a *resolution* property: one trigger pull
  resolves N hitscan rays in a fixed per-shot pattern. It belongs to the
  **Resolution Modes** milestone — `E16--weapon-state-machine/research.md` §5
  explicitly assigns "raising `pellet_count` above 1" there, and
  `E16--client-authoritative-combat` names "the pellet spec" as the owner of the
  weapon-side count field.
- **Weapon Feel "spread / recoil / accuracy"** (`roadmap.md`, Epic 16 → Weapon
  Feel) is a *feel/stat* axis: per-weapon accuracy stats, sustained-fire bloom,
  recoil patterns, a `player.spread` HUD slot, and a crosshair spread-ring
  widget.

**Why pellet spread goes first — prerequisites:**

- Pellet spread's prerequisites are **all shipped**: client-authoritative combat
  (the wire, `HitRecord` list, pellet-count clamp, and the 0..N-record
  `LocalHitRecord` shape are already pellet-general), the weapon state machine +
  per-shell reload, the ammo resource, and the impact-policy substrate (one
  pellet = one impact fire is already the host-side contract, with a regression
  test).
- Weapon Feel spread has an **unfulfilled prerequisite**: its crosshair
  spread-ring widget consumes the Epic 13 radial/ring UI primitive, which is
  still on the deferred list (no ring/arc widget exists — `ui_quad.wgsl` draws
  axis-aligned quads only). It also reads better *after* pellet spread, which
  establishes the direction-perturbation seam bloom later composes with.

## What the spec covers

Give a hitscan weapon `pelletCount > 1` with a per-shot spread cone, end to end:
descriptor → `effective()` stats → client N-ray fire → host validation → per-
pellet impact policy → presentation.

**Seams to extend (all named by shipped specs):**

| Seam | Today | Change |
|------|-------|--------|
| Weapon descriptor | no pellet field | `pelletCount` + spread-cone stat(s) behind `effective()`; SDK types, validation, typedefs in the same pass (primitive-surface contract, `index.md` §2) |
| Client fire | one ray, one `LocalHitRecord` | sample N directions in the cone, cast N rays, emit 0..N records (shape already supports it) |
| Host `AuthorizedShot.pellet_count` | hardcoded `1` at every construction site (`netcode/lifecycle.rs:767,947`, `sim/weapon_stage.rs:617`, remote-commands path) | read the weapon's effective pellet count; ingest clamp (`netcode/mod.rs:2639`) generalizes unchanged |
| Impact policy | per-pellet dispatch already correct | no change; verify `@impact.*` facts per pellet |
| Wire format | `HitRecord` list + clamp already general | **no wire change** (a foreclosure `E16--client-authoritative-combat` banked deliberately) |

**Decisions the draft session must resolve:**

1. **Damage model** — per-pellet damage stat vs. dividing the existing `damage`
   by `pelletCount`; per-pellet zone multipliers (each record already carries a
   zone).
2. **Pattern sampling** — uniform cone vs. authored fixed pattern; units and
   validation ranges; precedent: the emitter cone-sampling math
   (`plans/done/scripting-foundation/plan-3-emitter-entity.md`).
3. **RNG placement** — directions are client-sampled (sound under
   client-authoritative HIT: the host validates each record's world-LOS, never
   the pattern); pick a seeding story compatible with
   `sim/determinism_tests.rs`, which already threads `pellet_count`.
4. **Ammo semantics** — confirm one trigger pull consumes one magazine unit
   regardless of pellet count.
5. **The Weapon Feel seam** — name the point where a future accuracy/bloom
   modifier perturbs the aim direction *before* the pellet pattern is applied
   around it, so the Weapon Feel spec plugs in without reopening this one.
6. **Presentation cap** — N impact-FX/tracers per shot vs. capped; coordinate
   with the combat presentation substrate item (not a blocker).

**Out of scope (stays with Weapon Feel):** recoil, sustained-fire bloom,
accuracy stats, `player.spread` slot, the spread-ring widget, ADS modifiers.

**Testable outcome sketch:** a dev-mod shotgun with `pelletCount: 8` fires one
shell, consumes one magazine unit, lands up to 8 client-declared /
host-clamped-and-validated hits in one tick, fires 8 impact-policy dispatches
(partial-blast elemental policy works), and behaves identically in
single-player, listen-host, and remote-client co-op.

**Process:** run `/draft-plan` for the spec proper, then `/review-draft-spec` →
`/review-implementability` before promotion.
