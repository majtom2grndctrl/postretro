# movement--vault-entry-feel — research

Read at 41f21c8a.

## Product fit

PostRetro is a Doom/Quake-lineage shooter in which movement is a combat tool, with Ultrakill and Neon White as the ceiling (`movement.md` §5). Its movement surface is a closed, composable vocabulary for modders (`movement.md` §3), and reduce motion is an accessibility commitment (`E23--accessibility`).

| Recommendation | After reflection | Why |
|---|---|---|
| Expression fields over a scale sidecar | Stronger | Same mechanism as dash, engine-owned evaluator, closed vocabulary: FPS-flexible without framework creep |
| One `motion` vocabulary | Stronger | The ceiling games chain moves; one name with one meaning across dash, vault and later slide and wall-run is what lets authors compose them |
| Catch speed, not hang time | Stronger | The camera should answer how hard the pawn hit the ledge; an apex catch that hangs long and arrives slowly is common with double jumps and launch pads |
| Not a live store slot | Unchanged | Closed scopes and no store reads keep movement-adjacent evaluation cheap and side-effect free |
| Silent clamp, as dash | Unchanged, weakly | A modder pushing values high sees exaggeration, not an error, which matches the owner's wide-range intent; a dev-only clamp notice is a later option |
| Vault fields only | Unchanged | Slide and dash kicks are the next consumers; each pins its own evaluation moment |
| Pose brief first, this one after | **Phased (owner chose)** | The kick half has no pose dependency and carries most of the hard-catch feel. Holding the vocabulary behind an unrelated brief delays the foundation and its first consumers for no design gain |
| Reduced motion multiplies the evaluated output | Stronger | Entry-scaled intensity raises peak motion for hard catches; the comfort floor must stay a single multiplier over all of it |

## Shape rivals

| Shape | For | Against |
|---|---|---|
| Expression-valued fields (chosen) | Dash precedent (`movement.md` §2); separates enter from exit; the field widening `movement--state-view-pose` already names | Base and multiplier merge into one expression (`hard.times(-6)`) |
| Charge-style per-channel `scale` sidecar | Reads as base times multiplier; mirrors `activation.shot({ scale })` | Charge needs a sidecar because a shot's base lives on the weapon; here base and scale share a record. Magnitude only; one scale for both kicks; duplicates fields once `pose.vault` widens |
| One `intensity` multiplier | Smallest | A second vocabulary; cannot keep roll gentle while pitch hits hard |
| Literal poses keyed by entry context (`ground`/`air`/`swim`), blended still→fast by entry speed | No expression scope | Catch speed has a horizontal and a downward axis; each axis grows an improvised blend space expressions already cover |
| Overlap: sum the release and the new pose, each with its own values | Each vault keeps its own values outright | Breaks literal bit-identity; a literal pose can reach nearly twice its authored angle; the single row weight in `movement--state-view-pose` cannot carry two value sets |
| Overlap: newest values take over at the shipped larger weight | Simplest | Steps on any re-vault whose values differ, sign flips included |
| Overlap: shipped larger weight, values blended by weight (chosen) | Continuous; literal output unchanged; one weight, so it folds into `pose.vault`; a rewind needs no rule | Values of two vaults mix briefly |
| `easeIn`/`easeOut` as expressions | A fast catch eases in sooner | Coupled clamp; `movement--state-view-pose` times rows by rates, so the fold would drop them. Kick `tension` already carries entry-dependent snap |

## Vocabulary rivals

| Vocabulary | Verdict |
|---|---|
| `vaultEntry.*` per-state namespace | Renames concepts dash already reads (`speed`, `verticalSpeed`, `grounded`) |
| Live store slot (`getGameState().player.speed`) | Wrong value once the vault starts: base velocity is the climb path's. Crosses the closed-scope rule; no consumer |
| `motion.*` sharing dash read names, moment pinned per field (chosen) | One meaning per name; each field site binds the subset it supplies, as dash's docs table already shows |
| Hang time (`airborneMs`) as the intensity input | Rejected: a tall jump or a launch caught at its apex hangs long but arrives slowly, and a render tracker would be the only inexact input. Catch speed (`speed`, `verticalSpeed`) measures what the camera answers |
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
| `verticalSpeed`, `grounded`, `swimming` | Entry record, from total motion (base, knockback, drift) before entry peels the layers and the vault intent overwrites velocity | Exact; same meaning as dash's `verticalSpeed` |

## Fluent lowering

`expression_refs.ts` keeps a fluent ref's IR node in a module-private WeakMap, and the Luau twin in a module-private table, so the Rust parser cannot read a ref. A ref outside lowering converts to an object with no `op` and fails the IR node's tag, naming the field. `defineEntity` today reads one fixed path, `components.inventory.loadout`; the `components.movement` walk is new code that can use `loweredValue` from `expression_refs` (TS and Luau). Dash fields are the only expression fields in `components.movement` today.

## Pins

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | Entry edge on the first frame of a vault, including the first vault after load | evaluate, then apply the enter kick, then the pose | the kick's first peak carries this vault's values |
| P2 | Pawn falling into a ledge | entry facts are taken before the vault intent writes the climb velocity | `verticalSpeed` is the fall, not the climb |
| P3 | Entry edge, first-shown frame and a pose rewind on one frame | the entry edge evaluates; the others find an evaluation | one evaluation for the vault |
| P4 | Vault A's exit edge and vault B's entry edge in one frame | A's exit kick applies with A's evaluation, then B evaluates, then B's enter kick | A's exit kick carries A's values; B's kick and pose carry B's |
| P5 | Reconcile ends vault A with no edge; a predicted tick then starts vault B | A releases with A's evaluation; B's entry edge evaluates | no exit kick for A; B evaluates once and kicks |
| P6 | Reconcile shifts progress with no entry edge, under or past the rewind threshold | the vault keeps its evaluation | presented values do not change mid-climb |
| P7 | View-feel reset mid-vault | a reset clears evaluations | a vault still shown after the reset evaluates once from neutral facts (on single-player and host a hot reload ends the vault, so only a connected client sees it again after reconcile) |
| P10 | A vault begins at view-feel scale 0; scale restored mid-climb | evaluation ignores scale | the restored pose shows the entry values |
| P11 | Vault kick expressions, no `viewFeel.vault` block | evaluation needs no pose state | each vault kicks with its own values |
| P12 | Vault B starts while vault A's pose release holds weight | presented weight is the larger of A's release and B's own; values blend by the two weights | no step beyond B's own ease-in; literal-only output equals today's |
| P13 | A vault's entry and exit edges land in one frame; no frame shows it | entry evaluates, enter kick, exit kick | both kicks carry the vault's values; no pose shows |
| P14 | Descriptor refresh on the frame a vault's exit edge applies | the refresh clears evaluations before the edge | the exit kick evaluates from neutral facts |
| P15 | Vault B's entry edge while A's exit kick is still decaying | the shared vault spring retunes to B's evaluated tension | A's tail decays at B's rate |
| P16 | Pose expressions, no `impulse` block | the frame's edges are consumed regardless | each vault evaluates from its own entry, not neutral facts |
| P18 | Offline, host and client-predicted vaults | every path that delivers the entry edge delivers its record | no path falls back to neutral facts for a vault it entered |
| P19 | Descriptor refresh on the frame a vault begins | the refresh clears view-feel state, then the frame's edges apply | the vault evaluates from its real entry facts under the new descriptor |
| P20 | Vault blocked on an early tick, then a new vault at once | the new entry edge starts a new vault, whatever the progress drop | the new vault presents its own values |
| P21 | Two vaults begin within one frame's ticks | each entry edge pairs with its own tick's record, in tick order | each vault's kicks carry its own values |
| P22 | Three vaults in quick succession | a new release starts from the presented pose | no step when an older release is cut off |
| P23 | Knockback or drift moving the pawn as it vaults | entry facts read before entry empties the layers | `verticalSpeed` and `speed` include them, as dash reads them |
| P24 | Hot reload mid-vault on single-player or host | the reload ends the vault with no edge | the pose releases with the vault's evaluation; no neutral join |
| P25 | Entry frame with the previous vault's kick tail still settling | the tail advances at its tension up to the new edge's age; the new edge retunes the spring | no step in the kick |
| P26 | Neutral exit kick after a refresh on the exit frame | no payload remains at the exit edge | `speed` and `climbHeight` read 0 |
