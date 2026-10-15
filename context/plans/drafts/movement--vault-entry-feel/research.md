# movement--vault-entry-feel — research

Read at 41f21c8a.

## Shape rivals

| Shape | For | Against |
|---|---|---|
| Expression-valued fields (chosen) | Dash precedent (`movement.md` §2); shapes timing and splits enter from exit; the field widening `movement--state-view-pose` already names | Base and multiplier merge into one expression (`hard.times(-6)`) |
| Charge-style per-channel `scale` sidecar | Reads as base times multiplier; mirrors `activation.shot({ scale })` | Charge needs a sidecar because a shot's base lives on the weapon; here base and scale share a record. Magnitude only; one scale for both kicks; duplicates fields once `pose.vault` widens |
| One `intensity` multiplier | Smallest | A second vocabulary; cannot keep roll gentle while pitch hits hard |
| Literal poses keyed by entry context (`ground`/`air`/`swim`, mirroring vault `from`), blended still→fast by entry speed | No render-side expression scope; no tracker for context | "A long fall into a ledge heaviest" needs an airborne axis; each new axis grows an improvised blend space expressions already cover. Off the mechanism `movement.md` §2 calls canonical |

First draft took the sidecar; `/validate-plan` returned Reshape on these grounds and the owner chose field expressions.

## Vocabulary rivals

| Vocabulary | Verdict |
|---|---|
| `vaultEntry.*` per-state namespace | Renames concepts dash already reads (`speed`, `verticalSpeed`, `grounded`); four of its inputs apply to any transition |
| Live store slot (`getGameState().player.speed`) | Wrong value once the vault starts: base velocity is the climb path's. Crosses the closed-scope rule; no consumer |
| `motion.*` sharing dash read names, moment pinned per field (chosen) | One meaning per name; field sites bind the subset they supply, as dash's docs table already shows |
| Inputs addressed through `components.movement` | Impossible as authored: a literal cannot reference its own block. Scope stays enforced at bind |

## Input sources

| Input | Source | Exactness |
|---|---|---|
| `speed` | `MovementState::Vault.entry_speed`, set by `vault_entry` from base + knockback + drift | Exact |
| `climbHeight` | −`start_offset.y`; `VaultFrame` is upright (`UPRIGHT_MIN_UP_Y`) | Exact |
| `verticalSpeed`, `grounded`, `swimming`, `airborneMs` | View-feel tracker over `ViewFeelDriver` facts while not vaulting | Lags by the ticks the vault's frame ran |

Rejected sources: the vault payload (wire bump for presentation); `MovementEvents` (exact for vertical speed and context, but no tick-side airborne timer exists, and a joined vault has no edge, so the tracker is needed anyway). `coyote_timer_ms` counts swimming as airborne (`substrate::advance_forgiveness`), so it does not supply `airborneMs`.

## Fluent lowering

`expression_refs.ts` keeps a fluent ref's IR node in a module-private WeakMap, so the Rust parser cannot read a ref. Lowering must run SDK-side. `defineEntity` already walks the descriptor (`lowerLoadoutReferences`, TS and Luau), which makes it the one place to lower `components.movement` refs without a builder.
