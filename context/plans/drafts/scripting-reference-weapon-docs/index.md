# Scripting Reference — Weapon Docs Gap

## Goal

`docs/scripting-reference.md` is the modder-facing API reference (M6: "covers all bound APIs"), but it has zero weapon coverage — the M10 weapon-primitives plan shipped the `components.weapon` block, `defaultWeapon` equip, and the `activate` / `impact` events without documenting them. Close the gap. Docs only; no engine change.

Discovered during the M10--entity-health-damage implementability review (2026-06). That plan documents its own surface (health block, `applyDamage`, `playerDied`, `player.health`); this one back-fills the weapon surface it builds on.

## Scope

### In scope

- Document in `docs/scripting-reference.md`: the `components.weapon` descriptor block (`damage`, `range`, `fireRateMs`, `fireMode`, `resolution` — confirm field list against `WeaponDescriptor` in `scripting/data_descriptors.rs` and the generated typedefs), the `defaultWeapon` equip field on `defineEntity`, and the `activate` / `impact` event names the fire system emits.
- Tone: `docs/` is human-facing — example-led prose for a human engineer, not the context-library's token-lean register. `content/dev/scripts/reference-pistol.ts` is the natural example to inline.

### Out of scope

- Any engine or SDK change.
- Documenting the health/damage surface (owned by `M10--entity-health-damage` Task 6).

## Acceptance criteria

- [ ] A modder can author and equip a working weapon from `docs/scripting-reference.md` alone — block fields with valid ranges, equip wiring, emitted event names — without reading engine source.
