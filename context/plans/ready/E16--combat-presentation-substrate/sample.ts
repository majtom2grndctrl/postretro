// ─────────────────────────────────────────────────────────────────────────────
// SPECULATIVE VISION — not a contract, does not compile against today's SDK.
//
// Discussion sketch (v2) for the combat presentation substrate author surface
// (roadmap: E16 › Combat Feedback & Economy › combat presentation substrate).
// Purpose: pin the SHAPE of the scripting API before spec editing. This revision
// folds in the decisions reached in the design thread; the few genuinely open
// questions are collected at the bottom.
//
// The item, verbatim: "a passive, capped presentation layer for combat-adjacent
// UI: floating damage numbers, current damaged-enemy health/shield bars, and
// pickup prompts. Reuses UI theme/text/draw-list machinery without modal stack,
// focus, hit testing, or input dispatch; game logic publishes readonly
// presentation facts, and renderer-local display state handles projection,
// lifetimes, and animation. Authored templates/styles stay flexible, but runtime
// instances stay pooled and bounded."
//
// ── The governing principle ──────────────────────────────────────────────────
// The engine owns that the fact HAPPENS (impact is collision; touch is overlap —
// the engine can't not own them) and owns the rendering primitives (Text/Bar/
// Image/draw-list — renderer owns the GPU). The AUTHOR owns what the fact MEANS,
// including "it means: show THIS, HERE, to THIS player." The engine renders; it
// never decides a hit is worth displaying. This is combat-events.md §2 ("facts,
// not policy") carried one hop into presentation.
//
// ── Two archetypes, three verbs ──────────────────────────────────────────────
// Every combat-presentation instance is one of two shapes:
//
//   SPAWN   — transient, pooled, self-expiring; born on an event EDGE, from
//             inside a policy. Damage numbers, "picked up ×8" toasts.
//   OVERLAY — persistent-while-a-fact-is-true; the engine drives show/hide off a
//             published fact and owns the world anchor. Enemy status bars,
//             pickup prompts.
//
//   | verb                        | role                              | archetype |
//   |-----------------------------|-----------------------------------|-----------|
//   | definePresentationTemplate  | the reusable LOOK (simple/tiered)  | both      |
//   | present(template, {...})    | spawn a transient from a POLICY    | spawn     |
//   | defineOverlay({ over, ... }) | stand up a FACT-DRIVEN overlay     | overlay   |
//
// Look is factored from binding on purpose (mirrors shipped `widgets`/`tree` vs
// `defineUiTree`): one template can be spawned via present() AND bound via
// defineOverlay. The use case lives in the OVERLAY SOURCE (`over:`), never in the
// verb name — a future source (objective markers, interactables) is a new source
// descriptor, not a new define*.
//
// ── Conventions decided in-thread ────────────────────────────────────────────
//   • Range is a SPATIAL property → it lives on the ENTITY (its touchable/spatial
//     block), never on a presentation template. One source of truth per range.
//   • Timing/animation is a PRESENTATION property → it lives on the TEMPLATE.
//   • All `*Ms` fields are INTEGER milliseconds. No floating-point ms.
//   • A template's id comes from its `const` binding (scripts-build naming sugar,
//     `descriptor-identity-and-naming-sugar`) — no `name:` string.
//   • One template concept. Some props are spawn-only (lifetimeMs, spawnScatter),
//     some overlay-only (tiers, appearDelayMs). A nonsensical pairing is a lint,
//     not a hard error — simple, non-harmful author mistakes are cheap to fix.
//   • Reuses ONLY Text/Bar/Image/theme/styleRanges/Switch/visibleWhen. No modal
//     stack, focus, hit-testing, or input dispatch — this layer is passive.
//
// ── Co-op transport & prediction (decided) ───────────────────────────────────
// The dividing line every friends-co-op shooter uses: replicate durable STATE,
// present transient EVENTS locally. No PvP + no live service LICENSES this — the
// techniques a competitive game needs (rollback, lag comp, anti-cheat) exist to
// arbitrate adversaries and protect an economy; delete both and they're moot, not
// merely cheaper. These are settled fundamentals (they follow from latency +
// trust model), not a bet on any engine trend.
//   • STATE (HP/ammo/XP totals) → replicated, authoritative, reconciled — the
//     shipped per-owner slot path. This is what the HUD's XP *number* rides.
//   • EVENT (damage numbers, toasts, the floating "+50") → transient, disposable.
//     The shooter's OWN numbers spawn CLIENT-LOCALLY off its own client-
//     authoritative hit (weapon tuning is already replicated) — no wire, no
//     reconcile. A locally-optimistic number is corrected only by the replicated
//     HP bar, which nobody reads a vanishing number against.
//   • Host-only-originated feedback (DoT, environmental, a teammate's numbers you
//     opt to see) → an UNRELIABLE, addressed, fire-and-forget presentation event,
//     host → owning client. Never the reliable slot path; a dropped one is
//     invisible. So per-player-currency is the transport for durable per-player
//     STATE, NOT for the floating popup — do not build damage numbers as slots.
//   • Predict-immediate; NEVER reconcile a cosmetic (it expires before a
//     correction would land). Client-side prediction of the owner's own numbers
//     is the DEFERRED feel-refinement; a host-addressed unreliable event is the
//     lean first cut, upgraded only if a playtest feels the round-trip.
//
// Scope note (2026-08-14 call): the status bar renders health today AND a shield
// track today — because a shield is just an author-declared value the source reads
// (per-entity state for enemies, `@state.shield`, shipped), not an engine
// component. Vanilla / elemental / custom shields all render through one bar. Only
// the damage-ABSORPTION mechanic (shield soaks damage before health, recharge,
// resistances) is future work (Damage & Defenses); this spec neither builds nor
// needs it. See Q7.
// ─────────────────────────────────────────────────────────────────────────────

import { defineImpactEvent } from "postretro";
import {
  Bar,
  HStack,
  Image,
  Text,
  VStack,
  defineTheme,
  getDesignTokens,
  Switch,
  // ── proposed new exports under "postretro/ui" ──
  definePresentationTemplate, // authors the LOOK of one pooled instance (simple or tiered)
  defineOverlay,              // binds a template to a published fact source (overlay archetype)
  present,                    // impact-effect: spawn a transient from a policy (spawn archetype)
  damagedEnemies,            // overlay source descriptor: recently-damaged enemy stats
  pickupTargets,             // overlay source descriptor: prompt-eligible touchables, local player
  fact,                       // per-instance data leaf the engine stamps (value/text/bool/entity/tier)
} from "postretro/ui";

// Reuse the shipped theme surface unchanged (content/dev/scripts/hud.ts is the model).
const combatTheme = defineTheme({
  color: {
    dmg: { normal: [0.95, 0.95, 0.98, 1.0], crit: [0.98, 0.78, 0.16, 1.0], overkill: [0.86, 0.06, 0.12, 1.0] },
    enemy: { fill: [0.86, 0.16, 0.16, 1.0], shield: [0.30, 0.60, 0.95, 1.0], background: [0.05, 0.03, 0.03, 0.8] },
    prompt: { text: [0.82, 0.95, 0.98, 1.0], key: [0.98, 0.78, 0.16, 1.0] },
  },
  font: { mono: "JetBrains Mono" },
});
const { color, font } = getDesignTokens(combatTheme);

// ═════════════════════════════════════════════════════════════════════════════
// A. FLOATING DAMAGE NUMBERS  — SPAWN archetype
// ═════════════════════════════════════════════════════════════════════════════
// Facts available TODAY on an impact (sdk/lib/data_script.ts Impact/TargetHandle):
//   impact.amount               — post-mitigation damage (NumberRef)
//   impact.target.healthAfter   — floors at 0 for storage; negative reads as overkill
//   impact.target.healthBefore  — for the killed edge
//   impact.source               — the attacker; the per-attacker routing token
// Not yet shipped (combat-events.md §4, later milestones): wasCrit (crit math),
// zone as an impact fact, element (damage types). The template stays additive to
// them; today it colors by magnitude alone.

// The LOOK — a simple (single-state) template. `fact.number("value")` is a
// per-instance leaf the engine stamps at spawn; styleRanges (shipped) maps the
// value to a color band with no per-spawn branching in script.
const damageNumber = definePresentationTemplate({
  root: Text({
    content: "0",
    font: font.mono,
    fontSize: 28.0,
    bind: fact.number("value", { format: "{}" }),
    color: color.dmg.normal,
    styleRanges: {
      bind: fact.number("value"),
      max: 100.0,
      entries: [
        { upTo: 25.0, color: color.dmg.normal },
        { upTo: 80.0, color: color.dmg.crit },
        { color: color.dmg.overkill },
      ],
    },
  }),
  // engine-owned lifetime + animation (renderer-local, not slot-driven)
  lifetimeMs: 900,
  motion: { rise: 0.6 /* world units */, easing: "easeOut" },
  fade: { startMs: 500 },
  spawnScatter: { radius: 0.15 }, // world-anchor jitter so stacked hits don't overlap
});

// DECIDED (Fork 1a): a damage number is a member of the CLOSED impact-effect set,
// spelled `present(...)` inside the shipped `defineImpactEvent` do:-list. Reuses
// the impact pipeline wholesale (per-impact, host-side, already addresses target
// + source) and auto-routes the render to `impact.source`'s client via the
// owner-private seam. Rejected: a standalone `defineDamageNumbers` binding — it
// re-subscribes the engine to the impact stream and re-derives routing the policy
// already carries, quietly re-prescribing meaning the author should own.
export const damageNumbers = defineImpactEvent(
  "damage-numbers",
  { tag: "enemy" },
  (impact) => {
    const killed = impact.target.healthBefore.gt(0).and(impact.target.healthAfter.le(0));
    return [
      {
        do: [
          present(damageNumber, {
            at: impact.target,    // world anchor = the thing that got hit
            to: impact.source,    // whose screen — per-attacker routing (co-op)
            value: impact.amount, // the per-instance fact the template binds
          }),
        ],
      },
      // a kill could show a distinct flourish — same verb, a different template
      { when: killed, do: [/* present(killFlourish, { at: impact.target, to: impact.source }) */] },
    ];
  },
);

// ═════════════════════════════════════════════════════════════════════════════
// B. DAMAGED-ENEMY STATUS BAR  — OVERLAY archetype  (health + author-defined shield)
// ═════════════════════════════════════════════════════════════════════════════
// Renderer-local per client off already-replicated enemy stats — NO per-owner
// routing. Pooled + capped: only the N most-recently-damaged enemies show a bar.

const enemyBar = definePresentationTemplate({
  root: VStack({ gap: 2.0 }, [
    Bar({
      // per-instance scalar the SOURCE stamps for each tracked enemy (Q2: facts are
      // source-stamped, not live bindings). Health is an engine stat the source reads.
      bind: fact.number("healthFraction", { tween: { durationMs: 160, easing: "easeOut" } }),
      max: 1.0,
      height: 5.0,
      fill: color.enemy.fill,
      background: color.enemy.background,
    }),
    // A shield is an AUTHOR-declared value (per-entity state `@state.shield`, shipped)
    // — not an engine component — so this renders TODAY. The source stamps
    // `shieldFraction` + `hasShield` only when the overlay is given a shield accessor;
    // omit it and the row hides. Vanilla / elemental / custom shields differ only in
    // which state the accessor points at.
    Bar({
      bind: fact.number("shieldFraction"),
      max: 1.0,
      height: 3.0,
      fill: color.enemy.shield,
      background: color.enemy.background,
      visibleWhen: fact.bool("hasShield"),
    }),
  ]),
  worldAnchor: { socket: "head", offsetY: 0.35 }, // above the model's head hit-zone
});

// The BINDING — generic. The use case is the SOURCE, and its visibility rule
// (linger, hide-at-full) is source config, not template config.
export const enemyStatusBars = defineOverlay({
  over: damagedEnemies({
    lingerMs: 2500,     // a bar stays up this long after the last hit
    hideAtFull: true,   // full-health enemies stay clean (retro feel)
    // include: "aimedAt" — alt/union source rule: also show the enemy under the reticle
    // Health is an engine stat the source reads directly. A shield is AUTHOR state,
    // so POINT the bar at whichever per-entity-state (or slot) holds it — omit for no
    // shield track. Elemental/custom shields just point the accessor at a different
    // state. Uses shipped per-entity `state(name)` + IR `dividedBy`.
    shield: (e) => e.state("shield").dividedBy(e.state("maxShield")), // stamped as shieldFraction
  }),
  template: enemyBar,
  maxVisible: 8,        // the pool cap — extra damaged enemies show no bar
});

// ═════════════════════════════════════════════════════════════════════════════
// C. PICKUP OVERLAY  — OVERLAY archetype, TIERED
// ═════════════════════════════════════════════════════════════════════════════
// The touch system (E16--wieldable-pickup-drop) already computes the overlap and
// holds "prompt-eligible pairs"; it deferred PUBLISHING them to an author surface
// — this substrate is that surface. Local-player-only; no per-owner routing.
//
// Range lives on the ENTITY, declared once. The acquire radius already ships
// (`touchable: { mode, radius }`); a two-tier presentation adds an optional
// larger `awarenessRadius` that DEFAULTS to `radius`, so today's single-tier
// declarations are unchanged:
//
//     touchable: { mode: "press", radius: 1.0, awarenessRadius: 6.0 }
//
// The engine publishes a per-(player, item) proximity TIER, and the template
// names tiers — no distance literal ever enters the UI:
//     out       — no overlay
//     aware     — inside awarenessRadius, outside acquire radius → minified marker
//     eligible  — inside acquire radius, awaiting a press → the full prompt
// `auto` mode never reaches `eligible` (the enter edge acquires), so it shows
// only the `aware` marker then vanishes; its "picked up" feedback, if wanted, is
// a SPAWN (present()), not a tier. `press` walks aware → eligible.
//
// A TIER ARM is itself a mini-template: content PLUS its own optional anchor /
// appearDelay / fade — so per-tier anchoring (marker on the item, prompt on the
// reticle) and the mid-combat appear delay both fall out with no special fields.
// What each tier CONTAINS is the author's call: GW2 shows a bare "press F",
// Borderlands shows a full stats card — same Switch, different content.
const pickupOverlay = definePresentationTemplate({
  tiers: fact.tier("proximity"),
  aware: {
    content: Image({ src: fact.text("icon"), size: 24.0 }),
    worldAnchor: { onEntity: true, offsetY: 0.4 }, // marker sits on the item
    // no delay — instant readability at distance
  },
  eligible: {
    content: HStack({ gap: 8.0, align: "center" }, [
      Text({ content: "[E]", font: font.mono, color: color.prompt.key,
             visibleWhen: fact.bool("needsKey") /* mode === "press" */ }),
      Text({ font: font.mono, color: color.prompt.text,
             bind: fact.text("label", { format: "Pick up {}" }) }),
    ]),
    screenAnchor: { anchor: "center", offset: [0.0, 64.0] }, // reticle-locked prompt
    appearDelayMs: 250, // must stay eligible this long before showing — less interruptive mid-combat
  },
});

export const pickupPrompt = defineOverlay({
  over: pickupTargets(),
  template: pickupOverlay,
  // per-instance facts the source exposes for each eligible touchable
  icon: (item) => item.iconAsset,
  label: (item) => item.displayName,
  needsKey: (item) => item.mode.eq("press"),
  keybindFor: "use", // ties the "[E]" chip to the same action `press` fires on
  maxVisible: 1,      // nearest eligible touchable wins
});

// ═════════════════════════════════════════════════════════════════════════════
// DECISIONS & OPEN ITEMS
// ═════════════════════════════════════════════════════════════════════════════
// Honest audit: nearly everything that read as an "open question" was a decision
// we hadn't made yet, not a genuine unknown. Made below. After resolving shields,
// NO open design forks remain — only build-time confirmations (V1) that belong to
// the spec's research phase.
//
// ── DECIDED ──────────────────────────────────────────────────────────────────
// Q1  Spawn spelling → present() in a policy (Fork 1a).
// Q3  Co-op transport → see "Co-op transport & prediction (decided)" up top.
//     State replicated; events local/unreliable; per-player-currency is the
//     transport for durable state, NOT for the popup.
// Q4  Prediction → predict-immediate, never reconcile a cosmetic; client-side
//     prediction of the owner's own numbers deferred, host-addressed unreliable
//     event is the first cut.
// Q6  Binding name → generic defineOverlay + source descriptors; template kept.
// Q8  Pickup range → on the touchable (`awarenessRadius` defaults to `radius`);
//     presentation names tiers, never distances.
// Q9  Shared-template prop mismatch → one template + a lint.
// Q2  Fact vocabulary → {number, text, bool, tier}. Resolved the architecture:
//     a `fact.*` leaf is always a value the PRODUCER stamped per instance,
//     uniformly — a spawn's producer is the present() args; an overlay's producer
//     is the SOURCE, which computes per-entity facts (healthFraction, icon, tier)
//     and stamps scalars. There is NO live per-instance binding into the global
//     slot table; `fact.entity` is a source-stamped scalar, not new binding
//     machinery, and it tweens like any stamped value (the shipped Bar tween
//     precedent). What each source exposes is part of its descriptor contract
//     (`pickupTargets` → icon/label/mode/tier; `damagedEnemies` → health/shield
//     fraction). Residual is a build-time confirm, not a design fork (see V1).
// Q5  Pool exhaustion → evict-oldest by default (simple, bounded, predictable).
//     Per-target COALESCE (merge stacked hits into a running total, Borderlands-
//     style) is an opt-in for damage numbers, deferred until asked — not first-cut.
// Q10 Continuous distance → deferred, demand-gated. Discrete tier ships; a
//     normalized `distance` fact for opacity/scale ramps lands if authors want it.
// Q7  Shield bar → RESOLVED, and it no longer touches unbuilt ground. A shield is
//     an AUTHOR-declared value the source reads — per-entity state for enemies
//     (`@state.shield`, the shipped impact-policy keystone), a per-owner/engine slot
//     for players. The bar reads a scalar; vanilla / elemental / custom shields all
//     render through it now. No engine shield component, so the earlier forward-
//     compat risk is gone.
//     Forward note (Damage & Defenses, NOT this spec): the combat MECHANIC — shield
//     soaks damage before health — has one irreducibly engine-owned seam, because
//     the chokepoint subtracts damage from health BEFORE any policy runs (confirmed:
//     the impact dispatch fires *after* the health decrement, index.md Task 2). So a
//     pure-policy shield can only refund health, not pre-absorb; the engine must own
//     a general "nominated absorption layer, drained before health" primitive.
//     Recharge, max, element, and resistances stay author state + policy — the
//     slot-based vision holds all the way down, leaner than a bespoke shield component.
//
// ── NO GENUINE DESIGN UNKNOWNS REMAIN — only build-time confirms ──────────────
// V1  Build-time seam confirmations for the spec's research phase — implementation
//     checks, not design forks: (a) an unreliable, client-addressed presentation-
//     event channel on the transport (renet supports unreliable channels — confirm
//     the emit seam and the client-address path); (b) the per-instance fact-table
//     feed from Q2 (renderer fed a per-instance fact bundle each frame). Both are
//     "read the code and confirm," and belong in research, not here.
