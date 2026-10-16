# movement--vault-entry-feel — research

Read at 41f21c8a.

## Shape rivals

| Shape | For | Against |
|---|---|---|
| Expression-valued fields (chosen) | Dash precedent (`movement.md` §2); separates enter from exit; the field widening `movement--state-view-pose` already names | Base and multiplier merge into one expression (`hard.times(-6)`) |
| Charge-style per-channel `scale` sidecar | Reads as base times multiplier; mirrors `activation.shot({ scale })` | Charge needs a sidecar because a shot's base lives on the weapon; here base and scale share a record. Magnitude only; one scale for both kicks; duplicates fields once `pose.vault` widens |
| One `intensity` multiplier | Smallest | A second vocabulary; cannot keep roll gentle while pitch hits hard |
| Literal poses keyed by entry context (`ground`/`air`/`swim`), blended still→fast by entry speed | No expression scope | "A long fall into a ledge heaviest" needs an airborne axis; each new axis grows an improvised blend space expressions already cover |
| `easeIn`/`easeOut` as expressions | A fast catch eases in sooner | Coupled clamp; `movement--state-view-pose` times rows by rates, so the fold would drop them. Kick `tension` already carries entry-dependent snap |

## Vocabulary rivals

| Vocabulary | Verdict |
|---|---|
| `vaultEntry.*` per-state namespace | Renames concepts dash already reads (`speed`, `verticalSpeed`, `grounded`) |
| Live store slot (`getGameState().player.speed`) | Wrong value once the vault starts: base velocity is the climb path's. Crosses the closed-scope rule; no consumer |
| `motion.*` sharing dash read names, moment pinned per field (chosen) | One meaning per name; each field site binds the subset it supplies, as dash's docs table already shows |
| Inputs addressed through `components.movement` | Impossible as authored: a literal cannot reference its own block. Scope stays enforced at bind |

## Evaluation moment and input sources

| Option | Verdict |
|---|---|
| First frame that shows the vault, render-side tracker for all context (rejected) | Misses a vault no frame shows; back-to-back vaults read the previous vault's frozen context or its exit-settle velocity; the entry frame's own facts carry climb velocity; kicks without a pose block have no detector, since the pose state resets when the block is absent |
| Vault payload fields | Wire bump for presentation |
| Entry edge with a local entry record from the movement tick (chosen) | Exact, ordered with the edges, independent of presentation; reconcile replay emits none, which the neutral-fact rule covers |

| Input | Source | Exactness |
|---|---|---|
| `speed` | Entry record: horizontal speed of base + knockback + drift, as the payload's entry speed | Exact |
| `climbHeight` | Entry record: vertical part of the start offset | Exact on static ledges and yaw-only movers; the upright gate admits about 0.8° of tilt, off by up to about 1.4% of the horizontal offset |
| `verticalSpeed`, `grounded`, `swimming` | Entry record, taken before the vault intent overwrites velocity | Exact |
| `airborneMs` | Render tracker over simulated frame time; resets while grounded, swimming or vaulting | Lags by the ticks of the entry frame. `coyote_timer_ms` is no source: it counts time in water as airborne (`substrate::advance_forgiveness`) |

## Fluent lowering

`expression_refs.ts` keeps a fluent ref's IR node in a module-private WeakMap, and the Luau twin in a module-private table, so the Rust parser cannot read a ref. A ref outside lowering converts to an object with no `op` and fails the IR node's tag, naming the field. `defineEntity` today reads one fixed path, `components.inventory.loadout`; the `components.movement` walk is new code that can reuse its `loweredValue` helper.

## Pins

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | Entry edge on the first frame of a vault, including the first vault after load | evaluate, then apply the enter kick, then the pose | the kick's first peak carries this vault's values |
| P2 | Pawn falling into a ledge | entry facts are taken before the vault intent writes the climb velocity | `verticalSpeed` is the fall, not the climb |
| P3 | Entry edge, first-shown frame and a pose rewind on one frame | the entry edge evaluates; the others find an evaluation | one evaluation for the vault |
| P4 | Vault A's exit edge and vault B's entry edge in one frame | A's exit kick applies with A's evaluation, then B evaluates, then B's enter kick | A's exit kick carries A's values; B's kick and pose carry B's |
| P5 | Reconcile ends vault A with no edge; a predicted tick then starts vault B | A releases with A's evaluation; B's entry edge evaluates | no exit kick for A; B evaluates once and kicks |
| P6 | Reconcile shifts progress with no entry edge, under or past the rewind threshold | the vault keeps its evaluation | presented values do not change mid-climb |
| P7 | Descriptor refresh or hot reload mid-vault; mid-fall; pawn change or level install | the refresh clears evaluations, not the airborne count; a pawn change or level install restarts the count | the shown vault evaluates once from neutral facts; airtime keeps counting across a refresh |
| P8 | Airborne across a stall frame or a level hold | the airborne count advances by simulated tick time | a stall adds at most the frame's tick cap; a hold with no ticks adds nothing |
| P9 | A vault's entry frame has zero frame time | evaluation runs on the edge, not on elapsed time | it evaluates and kicks that frame |
| P10 | A vault begins at view-feel scale 0; scale restored mid-climb | evaluation ignores scale | the restored pose shows the entry values |
| P11 | Vault kick expressions, no `viewFeel.vault` block | evaluation needs no pose state | each vault kicks with its own values |
| P12 | Vault B starts while vault A's pose release holds weight | A's release keeps A's values; B's pose eases in with B's; the two sum, each within its clamp | no step beyond B's own ease-in |
| P13 | A vault's entry and exit edges land in one frame; no frame shows it | entry evaluates, enter kick, exit kick | both kicks carry the vault's values; no pose shows |
| P14 | Descriptor refresh on the frame a vault's exit edge applies | the refresh clears evaluations before the edge | the exit kick evaluates from neutral facts |
| P15 | Vault B's entry edge while A's exit kick is still decaying | the shared vault spring retunes to B's evaluated tension | A's tail decays at B's rate |
