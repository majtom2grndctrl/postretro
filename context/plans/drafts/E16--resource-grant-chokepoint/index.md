# Resource Grant Chokepoint (E16)

## Goal

Engine-owned resources can only ever decrease. `apply_damage_with_context` subtracts health; nothing adds to health or ammo. Build the blessed inverse: one validated chokepoint that *adds* to health and ammo, reachable from two entry points — an impact-policy effect crediting the damage **source**, and a tag/activators-targeted reaction primitive. Ships the reference dev-mod policy a mod replaces wholesale: grant-ammo-on-kill plus a trigger-fed ammo pickup, so a playtest that empties its reserve is no longer a dead end.

## Prerequisites

- **`E16--impact-policy-substrate`** (shipped) — the impact dispatch source, the closed `Effect` union this adds an arm to, the `@impact.source` command-target token (published, no methods today), the evaluator's evaluate-then-apply model, and the consequential/presentation arm split.
- **`E16--impact-death-lifecycle`** (shipped) — the kill latch, pending kill credit, and the `set_health_absolute` resurrect re-arm this spec's health grant flows through.
- **`E16--ammo-resource`** (shipped) — `AmmoReserve` and its `available`/`take`/`credit` interface, and the `player.ammo` / `player.ammoReserve` owner-private HUD projection that makes a grant observable.

## Scope

### In scope

- **One grant chokepoint** owning validation, clamping, and application for both entry points, so a pickup cannot bypass a rule a kill-grant respects.
- **Health grant** — additive, clamped to the health range, routed through the existing absolute-write chokepoint so it inherits the resurrect re-arm.
- **Ammo grant** — additive credit to a named reserve pool on the recipient, saturating.
- **Impact-effect arms** — `grantHealth` / `grantAmmo` on the `SourceHandle`, the sixth and seventh arms of the closed `Effect` union, targeting `@impact.source` only.
- **Reaction primitives** — `grantHealth` / `grantAmmo`, tag-targeted and activators-targeted, siblings of `applyDamage`.
- **Reference dev-mod policy** — grant-ammo-on-kill in the dev mod's mod-global lifecycle, plus a trigger-fed ammo pickup in the combat demo map.

### Out of scope

- **Per-player mod currency (XP, score, credits).** Grant covers engine-owned resources only. Mod currency is a store write, and a *per-player* one needs the cross-entity per-entity-state write path — `E16--per-player-currency`, which sequences after this spec. Team-shared currency already works today via `slot.add`.
- **Damage numbers.** The roadmap item bundles them, but the closed effect set has no path to presentation and the `combat presentation substrate` roadmap item owns that display layer. Recommend amending the roadmap item.
- **Loot drops.** Spawning a pickup entity on death is a spawn effect, not a grant. Belongs with the `pickup` roadmap item (Weapon Systems). This chokepoint is what such an item calls into when walked over.
- **Armor.** No armor component exists anywhere in the engine; it belongs to the Damage & Defenses milestone. The `Effect` union stays open to a third arm.
- **Per-type ammo carry caps.** Decided, not deferred by omission — see Decisions.
- **A declared-ammo-type registry.** The type stays a free-form charset-validated identifier, as the ammo spec pinned.
- **Client-side prediction of grants.** Grants are host-authoritative and reach the owner through the shipped owner-private slot projection on the next snapshot.

## Direction

**Problem.** Engine resources are write-once-downward. The only ammo in the world is what a weapon descriptor seeds at spawn, so a playtest that empties its reserve cannot recover, and no reward policy can pay out in anything the engine owns. The cause is a missing chokepoint, not a missing policy: `combat-events.md` §7 named the resource-grant path as "the one genuinely new engine mechanism" the economy needs, and three shipped specs each deferred to it by name.

**Prior commitments.**
- *Closed-but-extensible effects.* `E16--impact-policy-substrate` Decisions pinned that a later spec needing a new effect "adds an arm to the set," naming "the economy epic's `grant`" as the case. This is that spec; it adds arms rather than widening the union to a hollow type.
- *Resources engine-owned, currencies mod-owned.* `combat-events.md` §2 — health/ammo are engine-owned and readonly to scripts; granting flows through a blessed chokepoint, never a store write. Honored.
- *`applyDamage`'s dual shape.* The damage path is both an in-tick chokepoint and a tag-targeted reaction primitive. Grant mirrors it exactly, including the negative/non-finite warn-and-no-op precedent.
- *The reserve's write side was parked here.* `E16--ammo-resource` Decisions: "'Takes up space' is a write-side concern enforced when ammo *enters* the reserve — the deferred grant/pickup chokepoint." This spec owns that seam and decides it (Decisions: carry cap).
- **Divergence.** `combat-events.md` §5 sketches `grant("player.health", 25)` — a resource-name string. Not adopted; see Alternatives. That research doc's `defineCombatHandler` / `onKill` surface is superseded wholesale by the impact substrate: "grant on kill" is an impact policy gated on the kill edge, not a kill event. The doc's §2 principles and §6 attribution model still hold.

**Placement.** The chokepoint lives in the entities crate beside the nouns it writes (`set_health_absolute`, `AmmoReserve::credit`), not inside the impact evaluator. Two unrelated callers — the evaluator in the binary and the reaction registry — must share one validation path, and the entities crate is the only layer both already depend on. Putting it in the evaluator would force the reaction primitive to either duplicate the rules or depend upward.

**Alternatives rejected.**
- *A single string-addressed `grant(resource, amount)`.* Reads uniformly and matches the research sketch, but cannot carry the ammo pool key — ammo is pooled by type — and it would name `player.ammoReserve` as a grant target when that name is a read-only owner-private *projection*, not a writable slot. The string API teaches a false model of where resources live.
- *Grant as a store write with an engine-side readonly bypass.* Cheapest to build, and the write path already exists. Rejected: it collapses the engine-owned/mod-owned boundary the whole economy design rests on, and the HUD slots are projections of components — writing the slot would desync the component that feeds it.
- *Impact-effect arm only, no reaction primitive.* Half the spec, and it would ship the reward path while leaving pickups unbuildable. It also forecloses multi-recipient grants entirely: the IR has no iteration, so fan-out to several players is only expressible through tag targeting.

## Decisions

- **One chokepoint, two entry points.** Both entry points call the same pair of functions. Validation, clamping, and logging live there once; neither caller re-implements a rule.
- **Health grant routes through the existing absolute-write chokepoint** — read current, add, write via `set_health_absolute`, which already clamps to `[0, max]` and, on a positive stored result, clears `death_handled`, the pending kill credit, and the live contributor ledger. **This is deliberate, not incidental:** healing a downed entity above zero must re-arm kill detection and discard the credit from the down it recovered from, which is exactly what an authored `setHealth` resurrect already does. One health-write chokepoint, not two. A grant that leaves the entity at zero (amount `0`, or a dead entity at zero max) re-arms nothing.
- **Ammo grant credits the named pool, saturating.** The reserve is `u32`; a granted amount is truncated toward zero. Saturation at the integer ceiling is the existing `credit` behavior and is preserved.
- **No per-type carry cap in v1.** A cap needs an authoring surface for the per-type limit, and none of the three plausible homes exists yet (the weapon descriptor's type block is per-weapon not per-pawn; there is no declared-ammo-type registry; the inventory that will own the reserve is unbuilt). Granting is unbounded up to saturation. The *seam* is what this spec banks: every path by which ammo enters a reserve goes through one function, so a later cap is a single-function change with no call-site churn.
- **Amounts: IR expression on the impact arm, plain number on the reaction arm.** The impact arm takes a `NumberValue`, so "grant a quarter of the overkill" is expressible; the reaction arm takes a finite `f32` in its args, matching `ApplyDamageArgs`. Both validate identically at the chokepoint.
- **Negative and non-finite amounts warn and no-op.** Mirrors `applyDamage`'s precedent. Load-bearing beyond hygiene: a negative grant would be a damage path that bypasses the contributor ledger, the impact dispatch, and the kill latch — every attribution guarantee in the epic. Grant only ever adds.
- **The impact arm targets `@impact.source` and nothing else.** Grant is attacker-directed by definition. A policy naming `@impact.target` for a grant fails to bind with a named diagnostic, exactly as a mistargeted `setState` does today. Healing the entity that was hit is already expressible as `setHealth(healthAfter.plus(n))` — an absolute write with additive arithmetic — so no authoring power is lost, and one name never means two opposite things.
- **A missing or wrong-shaped recipient is a skip, not a bind failure.** The dispatch's source is optional (enemy melee and app-drain damage carry none) and a policy cannot know at bind time which fires will have one. So: no source → skip the effect; a source the registry no longer resolves (despawned, id recycled) → skip; a source lacking the component the grant writes (an enemy has health but no reserve) → skip with a rate-limited warn. Every other effect in the same fire still applies. This mirrors `applyDamage`'s per-target skip-and-warn.
- **No ammo-type validation.** The type is a free-form identifier with no declared registry to check against, and granting a pool no currently-equipped weapon uses is legitimate (stocking shells before the shotgun exists). A typo therefore creates a dead pool silently. Accepted; the future "declare-your-categoricals → codegen" spec the ammo spec named is what closes it.
- **The impact arm inherits the producer gate; the reaction arm does not.** App-drain impacts (DoT, environmental) run no policy in v1, so a grant on the impact arm never fires for them. A trigger-fired grant always runs. Both stated in the authoring docs so neither is discovered by surprise.
- **Grant is consequential, not presentation.** It dispatches in the consequential arm of the per-fire loop, before `playAnim`.
- **Concurrency.** A grant writes a host-authoritative component, so it carries no read-modify-write race. This differs from `slot.add`, which lowers to a self-referential IR add on a store slot with documented last-writer-wins. Two reward paths, two concurrency stories — worth knowing when a policy mixes them.

## Acceptance criteria

- [ ] An impact policy gated on the kill edge that grants ammo raises the killer's reserve for the named pool; the HUD reserve readout reflects it.
- [ ] A trigger-fired grant raises the reserve of every activator that entered the volume, and a tag-targeted grant raises it for every tagged entity.
- [ ] A grant of a negative or non-finite amount changes nothing and logs a warning — through both entry points.
- [ ] A health grant that would exceed maximum health stores maximum health, not more.
- [ ] A health grant lifting a latched zero-HP entity above zero re-arms kill detection: the entity can be killed again and reports exactly one further kill. A grant that leaves it at zero preserves the latch and its pending credit.
- [ ] An impact whose source is absent (enemy melee, app-drain) applies no grant and does not panic; other effects in the same fire still apply.
- [ ] A grant whose recipient lacks the written component is skipped with a warning; sibling effects in the same fire still apply.
- [ ] A policy that aims a grant at the impact target fails to bind with a diagnostic naming the event, and every other registered policy still loads and runs.
- [ ] App-drain-sourced impacts run no grant, while a reaction-fired grant on the same frame applies normally.
- [ ] Repeated grants past the reserve's integer ceiling saturate rather than wrap.
- [ ] Walkthrough on the combat demo map: killing enemies raises the reserve, and walking into the pickup volume raises it again — a player who empties the reserve can recover without restarting.
- [ ] The SDK exposes the grant arms on the source handle only, and the dev-mod fixture type-checks against the shipped SDK.

## Tasks

### Task 1: Grant chokepoint

Add a grant module to the entities crate, beside the components it writes, owning two functions: an additive health grant and an ammo-pool credit. Each takes the registry, the recipient id, the amount, and (for ammo) the pool key, and returns an outcome enum distinguishing applied / skipped-no-component / skipped-invalid-amount so callers log consistently without re-deriving the reason. Both reject non-finite and negative amounts before touching state, logging the same warn shape `applyDamage` uses (`crates/postretro/src/health/reactions.rs` is the wording precedent). The health grant reads the current value, adds, and writes through `set_health_absolute` (`crates/entities/src/components/health.rs`) — do not clamp or store directly, because that function owns both the `[0, max]` clamp and the positive-result re-arm of `death_handled` / pending kill credit / contributor ledger, and bypassing it would silently drop the re-arm. The ammo grant truncates the amount toward zero to an unsigned count and calls `AmmoReserve::credit` (`crates/entities/src/components/ammo_reserve.rs`), which already saturates; a recipient with no reserve component returns the skipped outcome rather than creating one, so a grant cannot conjure a reserve onto an entity that owns no weapon. No carry cap: this function is the sole entry point for ammo into a reserve, which is what makes a later cap a one-function change. Unit-test the clamp, the re-arm hand-off, saturation, the negative/non-finite rejection, and both skip outcomes.

### Task 2: Impact-effect grant arms

Add the two grant arms to the impact-policy path end to end. **SDK:** fill in `SourceHandle` in `sdk/types/postretro.d.ts` — today an empty branded interface, the seam this spec was left — with `grantHealth(amount: NumberValue)` and `grantAmmo(type: string, amount: NumberValue)`, both returning the opaque `Effect`, and mirror them in the Luau typedef; document on each that the recipient is the damager, that a fire with no damager skips the effect, and that app-drain impacts run no policy in v1. Lower both to effect descriptors carrying `"target": "@impact.source"`, matching the existing `target` string channel `bind_effect` already parses. **Bind:** in `crates/postretro/src/impact_policy.rs`, add the two primitives to `bind_effect`; they require `@impact.source` where the five shipped arms require `@impact.target`, so generalize `require_impact_target` into a checker taking the expected token and emitting the same diagnostic shape, rather than adding a second bespoke guard. Bind the amount operand through `bind_read` and reject a non-number root exactly as `setHealth` does. **Apply:** the planned-command path currently applies every command to one id — `apply_planned` takes `dispatch.target` and hands it to `apply_effect`. Widen the planned command to carry which token it addresses, and have `apply_planned` resolve the source id from the dispatch (`ImpactDispatch.source`, an `Option<EntityId>`, already carried) and skip the effect when it is absent or no longer resolves in the registry. Route both new commands to Task 1's functions. Grant is consequential, so classify it with the write/setHealth/despawn arm, not the presentation arm.

### Task 3: Grant reaction primitives

Register `grantHealth` and `grantAmmo` as reaction primitives alongside `applyDamage`, in the health reactions module or a sibling grant module in the same directory (`crates/postretro/src/health/`, 258 lines — room to host both, split only if the file grows past the guidance). Each deserializes typed args (an amount; for ammo, also the pool key), applies the empty-target-set debug no-op and the negative/non-finite dispatch-wide warn no-op that `dispatch` already models, then calls Task 1's functions per target, skipping and warning per target on a missing component so one bad tag does not abort the rest. Export the SDK builders in `sdk/lib/data_script.ts` mirroring `damage(target: ActivatorsTarget | string, amount)` — the same dual accepting either the `@activators` token or a tag string — so a trigger-event reaction can grant to whoever entered the volume and a plain reaction can grant to a tagged set. This entry point is not producer-gated and is the only way to grant to several recipients in one fire, since the IR has no iteration.

### Task 4: Reference dev-mod policy and pickup

Ship the reference economy a mod replaces wholesale, in the dev mod. Add a grant-ammo-on-kill policy to `content/dev/scripts/combat-lifecycle.ts` beside the existing enemy-death base/override pair, gated on the kill edge (`healthBefore.gt(0).and(healthAfter.le(0))` — a level gate would re-pay on every corpse hit) and granting the reference pistol's pool to `impact.source`; register it in the dev manifest's `events` list in `content/dev/start-script.ts`. Add the trigger-fed pickup: a `trigger_volume` brush in `content/dev/maps/combat-demo.map` (which has none today — `closet-reveal.map` and `spawner-test.map` are the authoring precedents) wired to a trigger-event reaction that grants ammo to `on.activators`. Keep the two paths visibly distinct in the walkthrough so the demo shows both entry points: kills pay out, and the volume pays out. Update the map's README with the walkthrough, and note in the dev-mod comments that both policies are reference content — the engine has no concept of a reward.

## Sequencing

**Phase 1 (sequential):** Task 1 — the chokepoint both entry points call.
**Phase 2 (concurrent):** Task 2, Task 3 — independent entry points over Task 1; they touch disjoint files.
**Phase 3 (sequential):** Task 4 — consumes both entry points.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Grant only ever adds — never a damage path | Task 1 (negative/non-finite rejection before any write) | Both entry points must route through Task 1; a caller that writes a component directly bypasses it | AC 3 |
| One health-write chokepoint owns clamp + resurrect re-arm | Task 1 (routes through the absolute-write function) | A health grant that clamps or stores inline would drop the re-arm silently | AC 4, 5 |
| A grant never fabricates state on the recipient | Task 1 (missing component → skip outcome) | Task 2 and Task 3 must log and continue, not abort the fire or the dispatch | AC 7 |
| Grant on the impact arm addresses the source only | Task 2 (token check at bind) | A future arm reusing the generalized checker must state its own expected token | AC 8 |
| A skipped grant never aborts sibling effects | Task 2 (per-effect skip), Task 3 (per-target skip) | Shared with the fire's evaluate-then-apply loop | AC 6, 7 |
| Ammo enters a reserve through exactly one function | Task 1 | Task 3 and any future pickup/loot spec must call it rather than credit directly — the property a later carry cap depends on | AC 10 |

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| health grant (impact arm) | grant chokepoint fn | effect `primitive: "grantHealth"`, `target: "@impact.source"` | `impact.source.grantHealth(amount)` | `impact.source:grantHealth(amount)` |
| ammo grant (impact arm) | grant chokepoint fn | effect `primitive: "grantAmmo"`, `target: "@impact.source"`, args `{ type, amount }` | `impact.source.grantAmmo(type, amount)` | `impact.source:grantAmmo(type, amount)` |
| health grant (reaction) | reaction primitive | `"grantHealth"`, `target?: "@activators"` or tag, args `{ amount }` | `grantHealth(target, amount)` | same |
| ammo grant (reaction) | reaction primitive | `"grantAmmo"`, `target?: "@activators"` or tag, args `{ type, amount }` | `grantAmmo(target, type, amount)` | same |
| source handle | command-target token `@impact.source` | existing token channel | `SourceHandle` (gains methods) | (table) |
| ammo pool key | reserve map key | free-form `[A-Za-z0-9_.:-]` string | `type: string` | same |

## Script syntax examples

```ts
// Proposed design — the engine has no concept of a reward; this is reference content.
const ammoOnKill = defineImpactEvent("dev:ammo-on-kill", { tag: "enemy" }, (impact) => {
  const killed = impact.target.healthBefore.gt(0).and(impact.target.healthAfter.le(0));
  return [
    // The kill EDGE, not a level gate: a level gate re-pays on every corpse hit.
    { when: killed, do: [impact.source.grantAmmo("bullets", 8)] },
  ];
});

// A glory-kill-style heal reads the impact's own facts to size the reward.
const healOnOverkill = defineImpactEvent("dev:overkill-heal", { tag: "enemy" }, (impact) => {
  const overkill = impact.target.healthAfter.times(-1);
  return [{ when: impact.target.healthAfter.le(-40), do: [impact.source.grantHealth(overkill)] }];
});

// The pickup: a trigger volume grants to whoever walked in. Not producer-gated.
onTriggerEvent({ tag: "ammo_pickup" }, "enter", [
  defineReaction((on: TriggerEventParams) => seq([grantAmmo(on.activators, "bullets", 24)])),
]);
```

## Open questions

- **Roadmap amendment (owner call).** The roadmap item bundles "a reference XP/score/damage-number policy." This spec ships the ammo/health half; XP moves to `E16--per-player-currency` and damage numbers to the `combat presentation substrate` item. The roadmap line wants rewording to match.
- **Research doc status note (owner call).** `context/research/combat-events.md` §5's `defineCombatHandler` / `getCombatEvent` / `onKill` / `grant("player.health", 25)` surface is superseded by the impact substrate and by this spec. Its §2 principles and §6 attribution model still hold. A header note would stop future specs inheriting the stale API sketch.
- **Attribution depth.** The ledger's pre-reduced scalars (`damageBy(source)`, `topContributorShare`) are not IR-readable — the impact scope publishes four number facts. "Grant to the killer" works through `source`; "split the reward among contributors" is not expressible. No leaf is exposed here; the demand-gated expressiveness fork in `combat-events.md` §6 is where that lands if a mod needs it.
