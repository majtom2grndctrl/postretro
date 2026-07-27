# Resource Grant Chokepoint (E16)

## Goal

Engine-owned resources can only ever decrease. `apply_damage_with_context` subtracts health; nothing adds to health or ammo. Build the blessed inverse: one validated chokepoint that *adds* to health and ammo, reachable from two entry points — an impact-policy effect crediting the damage **source**, and a tag/activators-targeted reaction primitive. Ships the reference dev-mod policy a mod replaces wholesale: grant-ammo-on-kill plus a trigger-fed ammo pickup, so a playtest that empties its reserve is no longer a dead end.

**This is also where an impact effect first addresses an entity other than the one that was hit.** Every command effect today applies to the dispatch's target; grant is attacker-directed, so the planned command must carry which token it addresses and resolve the source. That widening is the least reversible thing in this spec and the stated prerequisite of `E16--per-player-currency` — it is a first-class goal here, not a side effect of Task 2.

## Prerequisites

- **`E16--impact-policy-substrate`** (shipped) — the impact dispatch source, the closed impact-effect set this adds two members to, the `@impact.source` command-target token (published, no methods today), the evaluator's evaluate-then-apply model, and the consequential/presentation arm split.
- **`E16--impact-death-lifecycle`** (shipped) — the kill latch, pending kill credit, and the `set_health_absolute` resurrect re-arm this spec's health grant flows through.
- **`E16--ammo-resource`** (shipped) — `AmmoReserve` and its `available`/`take`/`credit` interface, and the `player.ammo` / `player.ammoReserve` owner-private HUD projection that makes a grant observable.

## Scope

### In scope

- **One grant chokepoint** owning validation, delegation, and outcome reporting for both entry points, so a pickup cannot bypass a rule a kill-grant respects. The `[0, max]` clamp stays owned by `set_health_absolute`, which the health grant delegates to.
- **Source-addressed effect application** — a planned command carries the token it addresses, and the apply path resolves the source rather than assuming the target.
- **Health grant** — additive, routed through the existing absolute-write chokepoint so it inherits both the health-range clamp and the resurrect re-arm.
- **Ammo grant** — additive credit to a named reserve pool on the recipient, saturating.
- **Impact-effect arms** — `grantHealth` / `grantAmmo` on the `SourceHandle`, the sixth and seventh members of the closed impact-effect set (`despawn` / `setHealth` / `setState` / `playAnim` / `slot.add`), targeting `@impact.source` only. `Effect` itself is an opaque brand, not a union: adding a member is three type edits — two arms on the SDK's `ImpactEffectWire`, two on `BoundEffect`, and two on `ImpactEffect`, since `setState` and `slot.add` lower to `PlannedEffect::Write` rather than to `ImpactEffect`.
- **Reaction primitives** — `grantHealth` / `grantAmmo`, tag-targeted and activators-targeted, siblings of `applyDamage`.
- **Reference dev-mod policy** — grant-ammo-on-kill in the dev mod's mod-global lifecycle, plus a trigger-fed ammo pickup in the combat demo map.

### Out of scope

- **Per-player mod currency (XP, score, credits).** Grant covers engine-owned resources only. Mod currency is a store write, and a *per-player* one needs per-owner slot cardinality keyed by a player identity that outlives the level — `E16--per-player-currency`, which sequences after this spec and is parked pending the session work that owns that identity. Team-shared currency already works today via `slot.add`.
- **Damage numbers.** The roadmap item bundles them, but the closed effect set has no path to presentation and the `combat presentation substrate` roadmap item owns that display layer. The same roadmap item also lists armor as part of this chokepoint; both deferrals belong in one amendment.
- **Loot drops.** Spawning a pickup entity on death is a spawn effect, not a grant. Belongs with the `pickup` roadmap item (Weapon Systems). This chokepoint is what such an item calls into when walked over.
- **Armor.** No armor component exists anywhere in the engine; it belongs to the Damage & Defenses milestone. The impact-effect set stays open to an armor grant — a third grant verb, the set's eighth member.
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

**Placement.** The chokepoint lives in the entities crate beside the nouns it writes (`set_health_absolute`, `AmmoReserve::credit`), not inside the impact evaluator. This is not a dependency-direction argument — both callers live in the `postretro` binary crate, so a binary-side module would compile fine. It is precedent and symmetry: the damage chokepoint this inverts already lives in the entities crate with the components, and `scripting.md` §13's IR-bearing-descriptor rule already splits this way — the entities-crate side holds the raw, scope-free data and logic, while the bound, scope-specialized program lives on binary-side runtime state. Grant logic beside the components, grant wiring beside the evaluator and the reaction registry, mirrors the damage path exactly.

**Alternatives rejected.**
- *One `grant` arm carrying a typed closed resource selector* — `grant(ammo("bullets"), 8)` over a `{ Health, Ammo { type } }` union, rather than one verb per resource. **This is the real fork, and it is the shape decision that is expensive to reverse.** It has a strong precedent one layer down: the weapon descriptor's `resource` field is already a serde-tagged union keyed by kind, with heat and cell as planned siblings, and making "what you grant" and "what a weapon consumes" one vocabulary is a property a later carry cap would want. Rejected on three grounds. First, the growth premise is weaker than it looks: heat and cell are per-weapon-*instance* resources with their own per-tick dissipate/regen, not pawn-held pools a reward credits, so armor is the only genuine third grantable resource — one added arm, not three. Second, every sibling in the closed effect union is a verb (`despawn`, `setHealth`, `playAnim`); a selector reintroduces a "which noun" parameter into a vocabulary that deliberately has none, and the union's whole point is that a policy cannot name an effect the evaluator does not implement. Third, health carries no pool key and ammo requires one, so a shared selector holds a field meaningless for one variant — a runtime validity rule where the type system could carry it. Reversal cost is bounded and known: the project is pre-stable and the SDK surface is generated, so if heat, cell, and armor all turn out to be grantable, collapsing the verbs into a selector is mechanical with call sites updated in the same pass.
- *A string-addressed `grant("player.ammo", 20)`* — the `combat-events.md` §5 sketch. Distinct from the typed selector above and weaker than it: the string cannot carry the ammo pool key, and the sketch names `player.ammo` — the read-only owner-private projection of the weapon *magazine*, not the reserve a grant credits, and not a writable slot at all. The string API teaches a false model of where resources live.
- *Grant as a store write with an engine-side readonly bypass.* Cheapest to build, and the write path already exists. Rejected: it collapses the engine-owned/mod-owned boundary the whole economy design rests on, and the HUD slots are projections of components — writing the slot would desync the component that feeds it.
- *Impact-effect arm only, no reaction primitive.* A scope trim rather than a rival shape — a later spec could add the reaction arm with no rework. Rejected because it ships the reward path while leaving pickups and heal stations unbuildable, and because the health case needs it most: the reaction registry today holds exactly one health primitive, `applyDamage`, so a medkit volume is unbuildable at any level of authoring effort. Fan-out is the other half — the IR has no iteration, so granting several players in one fire is only expressible through tag targeting.
- *Absolute-write reaction primitives (`setHealth` / `setAmmo`) instead of additive grants*, with policies expressing rewards as absolute writes over IR arithmetic. Rejected, and the reason argues for the chosen shape: a tag-targeted fan-out has no per-target read, so "heal everyone in this volume by 25" cannot be written as an absolute value. Additive is the correct primitive once the recipient is a set.

## Decisions

- **One chokepoint, two entry points.** Both entry points call the same pair of functions. Validation, delegation, and logging live there once; neither caller re-implements a rule.
- **The chokepoint owns every warn.** Invalid-amount and missing-component warnings are emitted once, inside the grant functions, rate-limited per (recipient, primitive) on the same helper `applyDamage`'s per-target warn uses. The outcome enum is returned for control flow only; neither entry point logs on top of it.
- **Health grant routes through the existing absolute-write chokepoint** — read current, add, write via `set_health_absolute`, which already clamps to `[0, max]` and, on a positive stored result, clears `death_handled`, the pending kill credit, and the live contributor ledger. **This is deliberate, not incidental:** healing a downed entity above zero must re-arm kill detection and discard the credit from the down it recovered from, which is exactly what an authored `setHealth` resurrect already does. One health-write chokepoint, not two. A grant that leaves the entity at zero (amount `0`, or a dead entity at zero max) re-arms nothing.
- **Ammo grant credits the named pool, saturating.** The reserve is `u32`; a granted amount is truncated toward zero. Saturation at the integer ceiling is the existing `credit` behavior and is preserved. An amount that truncates to `0` returns applied and credits nothing, without a warn; an amount above `u32::MAX` saturates to `u32::MAX` before `credit`.
- **No per-type carry cap in v1.** A cap needs an authoring surface for the per-type limit, and none of the three plausible homes exists yet (the weapon descriptor's type block is per-weapon not per-pawn; there is no declared-ammo-type registry; the inventory that will own the reserve is unbuilt). Granting is unbounded up to saturation. The *seam* is what this spec banks: every path by which ammo enters a reserve goes through one function, so a later cap is a single-function change with no call-site churn.
- **Amounts: IR expression on the impact arm, plain number on the reaction arm.** The impact arm takes a `NumberValue`, so "grant a quarter of the overkill" is expressible. The reaction arm takes a finite `f32` — **this is forced, not stylistic**: `E18--dispatch-scope-params` pins that the only IR-valued descriptor arg positions are the `setState` value and `accumulate`, and a runtime value anywhere else is rejected rather than evaluated. Both arms validate identically at the chokepoint.
- **Negative and non-finite amounts warn and no-op.** Mirrors `applyDamage`'s precedent. The rationale is health-shaped and should not be over-read: for health, a negative grant would be a damage path bypassing the contributor ledger, the impact dispatch, and the kill latch — every attribution guarantee in the epic. Ammo has no ledger, no dispatch, and no latch, so the honest rule there is simply that **subtraction gets its own verb**. A later spend/drain/disarm effect is deferred, not foreclosed, and it should not read as violating an invariant this spec wrote. On the impact arm a non-finite amount cannot reach the chokepoint: non-finite IR arithmetic resolves to zero upstream and authored literals must be finite, so that path applies a zero-amount no-op. Non-finite rejection is exercised at the chokepoint and on the reaction arm, where a non-finite literal is rejected by descriptor validation.
- **The impact arm targets `@impact.source` and nothing else.** Grant is attacker-directed by definition, and a policy naming `@impact.target` fails to bind with a named diagnostic, exactly as a mistargeted `setState` does today. **The cost is asymmetric and worth stating plainly.** For health nothing is lost: healing the entity that was hit is already expressible as `setHealth(healthAfter.plus(n))`, an absolute write with additive arithmetic. For ammo there is no analogue — no absolute ammo write exists anywhere in the engine — so a co-op support weapon that resupplies a teammate you shoot is inexpressible after this spec. Accepted for v1 because co-op resupply is not a demonstrated need and the fix is cheap and additive (permit the target token on the ammo arm), but it is a real foreclosure, not a free one.
- **A missing or wrong-shaped recipient is a skip, not a bind failure.** The dispatch's source is optional — a script `applyDamage` reaction carries none, and a weapon fire whose wielder does not resolve carries none — and a policy cannot know at bind time which fires will have one. Enemy melee is a positive case: it sets `DamageContext.attacker`, so a grant on an enemy-melee impact credits the attacking enemy. So: no source → skip the effect; a source the registry no longer resolves (despawned, id recycled) → skip; a source lacking the component the grant writes (an enemy has health but no reserve) → skip with a rate-limited warn. Every other effect in the same fire still applies. This mirrors `applyDamage`'s per-target skip-and-warn.
- **No ammo-type validation.** The type is a free-form identifier with no declared registry to check against, and granting a pool no currently-equipped weapon uses is legitimate (stocking shells before the shotgun exists). A typo therefore creates a dead pool silently. Accepted; the future "declare-your-categoricals → codegen" spec the ammo spec named is what closes it.
- **The impact arm inherits the producer gate; the reaction arm does not.** App-drain impacts (DoT, environmental) run no policy in v1, so a grant on the impact arm never fires for them. A trigger-fired grant always runs. Both stated in the authoring docs so neither is discovered by surprise.
- **Grant is consequential, not presentation.** It dispatches in the consequential arm of the per-fire loop, before `playAnim`.
- **Concurrency.** A grant writes a host-authoritative component, so it carries no read-modify-write race. This differs from `slot.add`, which lowers to a self-referential IR add on a store slot with documented last-writer-wins. Two reward paths, two concurrency stories — worth knowing when a policy mixes them.

## Acceptance criteria

- [ ] An impact policy gated on the kill edge that grants ammo raises the killer's reserve for the named pool; with that pool equal to the equipped weapon's ammo type, the `player.ammoReserve` HUD readout reflects it on the next snapshot.
- [ ] A trigger-fired grant raises the reserve of every activator that entered the volume, and a tag-targeted grant raises it for every tagged entity.
- [ ] A grant of a negative or non-finite amount changes nothing and logs a warning — through both entry points.
- [ ] A health grant that would exceed maximum health stores maximum health, not more.
- [ ] A health grant lifting a latched zero-HP entity above zero re-arms kill detection: the entity can be killed again and reports exactly one further kill. A grant that leaves it at zero preserves the latch and its pending credit.
- [ ] An impact whose source is absent (a script `applyDamage` reaction, an unresolved weapon wielder) applies no grant and does not panic; other effects in the same fire still apply.
- [ ] A grant whose recipient lacks the written component is skipped with a warning; sibling effects in the same fire still apply.
- [ ] A policy that aims a grant at the impact target fails to bind with a diagnostic naming the event, and every other registered policy still loads and runs.
- [ ] App-drain-sourced impacts run no grant, while a reaction-fired grant on the same frame applies normally.
- [ ] Repeated grants past the reserve's integer ceiling saturate rather than wrap.
- [ ] Walkthrough on the combat demo map: killing enemies raises the reserve, and walking into the pickup volume raises it again — a player who empties the reserve can recover without restarting.
- [ ] The impact-effect grant arms are exposed on `SourceHandle` and not on the target handle; the reaction builders are exposed as `data_script` free functions; the dev-mod fixture type-checks against the shipped SDK.
- [ ] A tag-targeted `grantHealth` raises health on every tagged entity carrying health in one fire, and a trigger-fired `grantHealth` on `on.activators` heals every activator.
- [ ] A policy group containing both a grant and a `playAnim` applies the grant before the animation switch within the same fire.
- [ ] An impact-arm grant whose amount is an IR expression over impact facts credits the evaluated value; a `RuntimeValue` in a reaction-arm amount is rejected by descriptor validation with a diagnostic rather than evaluated.
- [ ] `docs/scripting-reference.md` documents both grant entry points and the producer-gate asymmetry, and `context/lib/scripting.md` §10.5 no longer claims the reaction registry has no healing path.

## Tasks

### Task 1: Grant chokepoint

Add a grant module to the entities crate, beside the components it writes, owning two functions: an additive health grant and an ammo-pool credit. Each takes the registry, the recipient id, the amount, and (for ammo) the pool key, and returns an outcome enum distinguishing applied / skipped-no-component / skipped-invalid-amount so callers log consistently without re-deriving the reason. Both reject non-finite and negative amounts before touching state, logging the same warn shape `applyDamage` uses (`crates/postretro/src/health/reactions.rs` is the wording precedent). The health grant reads the current value, adds, and writes through `set_health_absolute` (`crates/entities/src/components/health.rs`) — do not clamp or store directly, because that function owns both the `[0, max]` clamp and the positive-result re-arm of `death_handled` / pending kill credit / contributor ledger, and bypassing it would silently drop the re-arm. The ammo grant truncates the amount toward zero to an unsigned count and calls `AmmoReserve::credit` (`crates/entities/src/components/ammo_reserve.rs`), which already saturates; a recipient with no reserve component returns the skipped outcome rather than creating one, so a grant cannot conjure a reserve onto an entity that owns no weapon. No carry cap: this function is the sole entry point for ammo into a reserve, which is what makes a later cap a one-function change. Unit-test the clamp, the re-arm hand-off, saturation, the negative/non-finite rejection, and both skip outcomes.

### Task 2: Impact-effect grant arms

Add the two grant arms to the impact-policy path end to end. **SDK:** add `grantHealth(amount: NumberValue)` and `grantAmmo(type: string, amount: NumberValue)` to `SourceHandle` — today an empty branded interface, the seam this spec was left — both returning the opaque `Effect`. `sdk/types/postretro.d.ts` is generated and must not be hand-edited: declare the methods in the typedef templates (`crates/scripting-core/src/typedef/templates/sdk_lib.d.ts` and `sdk_lib.luau`), implement them on the frozen `IMPACT.source` object in `sdk/lib/data_script.ts` and its Luau twin `sdk/lib/data_script.luau`, then regenerate `sdk/types/postretro.d.ts` / `.d.luau` via `cargo run -p postretro --bin gen-script-types` and update the committed drift fixtures under `crates/postretro/src/scripting/typedef/tests/fixtures/`; document on each that the recipient is the damager, that a fire with no damager skips the effect, and that app-drain impacts run no policy in v1. Lower both to effect descriptors carrying `"target": "@impact.source"`, matching the existing `target` string channel `bind_effect` already parses. The SDK's shared `impactEffect()` helper hardcodes `target: "@impact.target"` in both `sdk/lib/data_script.ts` and `sdk/lib/data_script.luau`, and the internal `ImpactEffectWire` union pins that literal per arm — add a source-targeted lowering helper and two `target: "@impact.source"` arms to the wire union rather than reusing `impactEffect` as-is. **Bind:** in `crates/postretro/src/impact_policy.rs`, add the two primitives to `bind_effect`; they require `@impact.source` where the four target-bearing shipped arms (`despawn`, `playAnim`, `setHealth`, `setState`) require `@impact.target` and `slot.add` requires the token be absent, so generalize `require_impact_target` into a checker taking the expected token and emitting the same diagnostic shape, rather than adding a second bespoke guard. The checker rejects an absent or mismatched `target` on the grant arms; `slot.add` keeps its no-target path. Bind the amount operand through `bind_read` and reject a non-number root exactly as `setHealth` does. The amount lowers through the same number-node path `setState` and `setHealth` use. **Apply:** the planned-command path currently applies every command to one id — `apply_planned` takes `dispatch.target` and hands it to `apply_effect`. Widen the planned command to carry which token it addresses — a `CommandRecipient { Target, Source }` discriminant populated at bind from the parsed `target` string, defaulted to `Target` for the four target-bearing shipped arms and not carried by `slot.add`. Have `apply_planned` resolve the source id from the dispatch (`ImpactDispatch.source`, an `Option<EntityId>`, already carried) and skip the effect when it is absent or no longer resolves in the registry. Route both new commands to Task 1's functions. Grant is consequential, so classify it with the write/setHealth/despawn arm, not the presentation arm.

### Task 3: Grant reaction primitives

Register `grantHealth` and `grantAmmo` as reaction primitives alongside `applyDamage`, in `crates/postretro/src/health/grant_reactions.rs`, a sibling of `reactions.rs` (258 lines) in the same module directory, so the grant registrar is separable from the damage one as either grows. Each deserializes typed args (an amount; for ammo, also the pool key), applies the empty-target-set debug no-op and the negative/non-finite dispatch-wide warn no-op that `dispatch` already models, then calls Task 1's functions per target, skipping and warning per target on a missing component so one bad tag does not abort the rest. Export the SDK builders in `sdk/lib/data_script.ts` and its Luau twin `sdk/lib/data_script.luau`, and declare them in `crates/scripting-core/src/typedef/templates/sdk_lib.d.ts` / `sdk_lib.luau`, mirroring `damage(target: ActivatorsTarget | string, amount: number): PrimitiveReactionDescriptor` — the same dual accepting either the `@activators` token or a tag string. The two forms emit different wire keys: the tag form carries `tag`, the token form carries `target: "@activators"`. A trigger-event reaction can therefore grant to whoever entered the volume, and a plain reaction can grant to a tagged set. This entry point is not producer-gated and is the only way to grant to several recipients in one fire, since the IR has no iteration. Update `docs/scripting-reference.md` with a grant section covering both entry points and the producer-gate asymmetry, and amend `context/lib/scripting.md` §10.5, whose "no healing path" sentence this spec falsifies.

### Task 4: Reference dev-mod policy and pickup

Ship the reference economy a mod replaces wholesale, in the dev mod. Add a grant-ammo-on-kill policy to `content/dev/scripts/combat-lifecycle.ts` beside the existing enemy-death base/override pair, gated on the kill edge (`healthBefore.gt(0).and(healthAfter.le(0))` — a level gate would re-pay on every corpse hit) and granting the reference pistol's pool — `"bullets.light"`, declared in `content/dev/scripts/reference-pistol.ts` — to `impact.source`; register it in the dev manifest's `events` list in `content/dev/start-script.ts`. Add the trigger-fed pickup: a `trigger_volume` brush tagged `ammo_pickup` in `content/dev/maps/combat-demo.map` (which has none today — `closet-reveal.map` and `spawner-test.map` are the authoring precedents), wired to a trigger-event reaction granting 24 `"bullets.light"` to `on.activators`. Register that reaction and its `onTriggerEvent` entry through `setupLevel()`'s returned `reactions` and `triggerEvents` in `content/dev/scripts/combat-demo-reaction.ts`, the level data script `combat-demo.map`'s worldspawn `data_script` KVP already names. The volume is a repeating dispenser in v1 — no self-disarm — since consumable one-shot pickups belong to the `pickup` roadmap item. Keep the two paths visibly distinct in the walkthrough so the demo shows both entry points: kills pay out, and the volume pays out. Update the map's README with the walkthrough, and note in the dev-mod comments that both policies are reference content — the engine has no concept of a reward.

## Sequencing

**Phase 1 (sequential):** Task 1 — the chokepoint both entry points call.
**Phase 2 (concurrent):** Task 2, Task 3 — independent entry points over Task 1. Their Rust files are disjoint (`impact_policy.rs` / `impact_effects.rs` vs. `health/`), but they share `sdk/lib/data_script.{ts,luau}`, the typedef templates, and the regenerated `postretro.d.{ts,luau}` plus drift fixtures — land the SDK edits in one pass or sequence Task 3 after Task 2.
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
| health grant (impact arm) | grant chokepoint fn | `{ primitive: "grantHealth", target: "@impact.source", args: { amount: IR node or number literal } }` | `impact.source.grantHealth(amount)` | `impact.source:grantHealth(amount)` |
| ammo grant (impact arm) | grant chokepoint fn | `{ primitive: "grantAmmo", target: "@impact.source", args: { type: string, amount: IR node or number literal } }` | `impact.source.grantAmmo(type, amount)` | `impact.source:grantAmmo(type, amount)` |
| health grant (reaction, tag form) | reaction primitive | `{ primitive: "grantHealth", tag, args: { amount } }` | `grantHealth(tag, amount)` | same |
| health grant (reaction, activators form) | reaction primitive | `{ primitive: "grantHealth", target: "@activators", args: { amount } }` | `grantHealth(on.activators, amount)` | same |
| ammo grant (reaction, tag form) | reaction primitive | `{ primitive: "grantAmmo", tag, args: { type, amount } }` | `grantAmmo(tag, type, amount)` | same |
| ammo grant (reaction, activators form) | reaction primitive | `{ primitive: "grantAmmo", target: "@activators", args: { type, amount } }` | `grantAmmo(on.activators, type, amount)` | same |
| source handle | command-target token `@impact.source` | existing token channel | `SourceHandle` (gains methods) | (table) |
| ammo pool key | reserve map key | free-form `[A-Za-z0-9_.:-]` string | `type: string` | same |

## Script syntax examples

```ts
// Proposed design — the engine has no concept of a reward; this is reference content.
const ammoOnKill = defineImpactEvent("dev:ammo-on-kill", { tag: "enemy" }, (impact) => {
  const killed = impact.target.healthBefore.gt(0).and(impact.target.healthAfter.le(0));
  return [
    // The kill EDGE, not a level gate: a level gate re-pays on every corpse hit.
    { when: killed, do: [impact.source.grantAmmo("bullets.light", 8)] },
  ];
});

// A glory-kill-style heal reads the impact's own facts to size the reward.
const healOnOverkill = defineImpactEvent("dev:overkill-heal", { tag: "enemy" }, (impact) => {
  const overkill = impact.target.healthAfter.times(-1);
  return [{ when: impact.target.healthAfter.le(-40), do: [impact.source.grantHealth(overkill)] }];
});

// The pickup: a trigger volume grants to whoever walked in. Not producer-gated.
// A primitive builder already returns a PrimitiveReactionDescriptor — no sequence wrapper.
const ammoPickup = defineReaction("dev.ammoPickup", (on: TriggerEventParams) =>
  grantAmmo(on.activators, "bullets.light", 24),
);
// returned from setupLevel():
//   { reactions: [ammoPickup],
//     triggerEvents: [onTriggerEvent({ tag: "ammo_pickup" }, "enter", [ammoPickup])] }
```

## Open questions

- **Roadmap amendment (owner call).** The roadmap item bundles "a reference XP/score/damage-number policy." This spec ships the ammo/health half; XP moves to `E16--per-player-currency` and damage numbers to the `combat presentation substrate` item. The roadmap line wants rewording to match.
- **Research doc status note (owner call).** `context/research/combat-events.md` §5's `defineCombatHandler` / `getCombatEvent` / `onKill` / `grant("player.health", 25)` surface is superseded by the impact substrate and by this spec. Its §2 principles and §6 attribution model still hold. A header note would stop future specs inheriting the stale API sketch.
- **Verb-per-resource vs. a typed resource selector (owner call, and the one shape decision that is costly to reverse).** Argued in Alternatives and decided for verb-per-resource on the grounds that armor is the only genuine third grantable resource — heat and cell are per-weapon-instance pools with their own per-tick update. If the owner expects heat or cell to become pawn-held and grantable, the selector is the better shape and should be chosen before the SDK surface publishes.
- **Target-directed ammo.** Source-only targeting leaves co-op resupply-by-shooting inexpressible, with no `setAmmo` escape hatch (Decisions). Cheap and additive to permit later; flagged in case co-op support weapons are nearer than assumed.
- **Attribution depth.** The ledger's pre-reduced scalars (`damageBy(source)`, `topContributorShare`) are not IR-readable — the impact scope publishes four number facts. "Grant to the killer" works through `source`; "split the reward among contributors" is not expressible. No leaf is exposed here; the demand-gated expressiveness fork in `combat-events.md` §6 is where that lands if a mod needs it.
