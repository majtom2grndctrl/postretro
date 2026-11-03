# Addendum: revive marker

Addendum brief · compact · Epic 18 (extends `index.md` in this folder) · reads: `context/lib/ui.md` §1 (presentation layer), §4 (Glyphs) · `context/lib/networking.md` §Presentation events vs. replicated state · `context/lib/scripting.md` §12 (Assist) · read at `3943fdc`

## Problem

The owner playtested E18. Revive mechanics work, but nothing shows where a downed teammate is, so a player cannot find someone to revive. When this lands, every machine draws a world-anchored marker over each assistable teammate. The marker floats just above the pawn and shows through walls. While the local player is close enough that holding `assist` would revive that teammate, the marker also reads "Hold [glyph] to revive".

## Decisions

Owner-approved. Do not reopen these.

- **D1. Who gets a marker.** Every remote pawn whose `assistable` bool is set gets one, at any distance. The prompt line shows only while the local player is in revive range. Out of range, the marker is a plain "downed" marker.
- **D2. Through walls.** The presentation layer's CPU projection has no depth test, so the marker shows through walls by default (`ui.md` §1). No occlusion work is needed.
- **D3. "Hold" is hard-coded.** The dev mod's template writes the word itself. `input-contextual-priority` owns activator-aware wording. Its AC12 is a different thing: one screen-space HUD prompt bound to `input.interaction`, which says what a fresh press would do. This marker is a world-anchored mark on each teammate. Both read the same candidate query (D8), so they cannot disagree. When that spec lands, its rank can gate this marker's prompt line too. The two features complement each other, and neither replaces the other.
- **D4. Never your own pawn, and no new wire data.** Each machine derives the marker from data it already holds. A connected client uses retained remote assist facts (life state and the `assistable` bool), interpolated remote positions, and the host's pair table. A listen host or single player uses the registry, the pawns' `PlayerMovementComponent.assistable`, and `AssistState::pair_values`. `WIRE_VERSION` and `SNAPSHOT_VERSION` do not change. The host publishes no roster for this overlay, unlike damaged enemies.
- **D5. `Glyph` is allowed in presentation templates.** Templates may use it like `Text`, `Bar`, `Image`, `VStack` and `HStack`, with the same rules as a HUD glyph: passive, follows rebinding and device family, and draws nothing when the command is unbound.
- **D6. This extends the modder SDK.** The new overlay source and template glyphs are documented in `docs/scripting-reference.md`, and `ui.md` and `networking.md` are updated. Both runtimes (TS and Luau) ship the SDK changes. The dev-mod marker is TS-only, like E18's HUD rows.

Derived from source. The owner can veto these.

- **D7. Layer placement.** The engine decides which teammates are marked and whether each is in range. The mod decides how the marker looks: its template, wording and lift. The marker is cosmetic and local to each machine. Overlays are already left out of the mod compatibility digest (`digest_ignores_overlay_template_cap_linger_and_disable`), so adding or changing the marker never blocks a join.
- **D8. "In range" means a hold would revive this teammate.** The test reuses `sim::assist::assist_candidate` over the same candidates the client prediction step reads (`client_assist/glue.rs` `gather_candidates`). Range is the pair's `PairValues::in_range`, measured between pawn origins. The test is never written a second time. The local pawn must have a pair entry for the target and a state that permits `use`. A local pawn that is itself down sees only plain markers. If two teammates are in range, only the nearest gets the prompt, because a hold revives only that one. This avoids a prompt that lies.
- **D9. One producer fact.** The marker template binds one stable fact, `inRange` (Bool). Nothing else is stamped: no distance and no progress, since the HUD bars already show progress.
- **D10. Anchor.** The marker reads the template's `worldAnchor` like any overlay, with one change: its fallback is the pawn's presented origin plus `offsetY`, never the hitbox top. A pawn's origin is its capsule centre, which drops when posture is `crouch`. The dev player's hitbox spans the standing capsule whatever the posture (`content/dev/scripts/player.ts`), so a hitbox-top anchor would float half a metre above a downed pawn. A posed socket still wins when the model has one.
- **D11. Markers and damaged-enemy bars have separate pool keys.** Keeping, clearing or capping one overlay kind never removes the other kind's instances. Level unload and staged reload clear both.
- **D12. SDK shape (was Q1).** `presentationOverlays` accepts one descriptor or an array, at most one per kind. No second manifest field.
- **D13. Source name (was Q2).** The source builder is `assistableTeammates()`. Its `kind` string is `"assistableTeammates"`. Overlays are host-local manifest data and never cross the wire.
- **D14. Prompt scope (was Q3).** The prompt shows only on the teammate a hold would revive (see D8), not on every teammate in range.

### Scripting surface

```ts
import { Glyph, HStack, Text, VStack, assistableTeammates, defineOverlay,
  definePresentationTemplate, fact } from "postretro/ui";

export const reviveMarker = definePresentationTemplate({
  root: VStack({ gap: 2 }, [
    Text({ content: "DOWNED", fontSize: 14, color: [1, 0.3, 0.3, 1] }),
    HStack({ gap: 4, visibleWhen: fact.bool("inRange") }, [
      Text({ content: "Hold", fontSize: 14, color: [1, 1, 1, 1] }),
      Glyph({ command: "assist" }),
      Text({ content: "to revive", fontSize: 14, color: [1, 1, 1, 1] }),
    ]),
  ]),
  lifetimeMs: 1, motion: { rise: 0, easing: "linear" }, fade: { startMs: 1 },
  spawnScatter: { radius: 0 },
  worldAnchor: { offsetY: 0.6 }, // `socket` is optional; the dev player model has no `head` socket
});

export const reviveMarkerOverlay = defineOverlay({
  over: assistableTeammates(), template: reviveMarker, maxVisible: 8,
});
// defineMod({ ..., presentationOverlays: [damagedEnemyOverlay, reviveMarkerOverlay] })
```

The array form and the name `assistableTeammates` are settled (D12, D13).

## Acceptance

Row ids are R-prefixed so they do not collide with `index.md`'s ACs.

### Automated
- [ ] **R1** (unit, both runtimes): `presentationOverlays` accepts one descriptor, as today, or an array of them. An array that repeats an `over.kind` keeps the first descriptor and warns, naming the kind. TS and Luau lower the array to the same JSON. Existing single-descriptor manifests load unchanged.
- [ ] **R2** (unit, both runtimes): `assistableTeammates()` lowers to `{ kind: "assistableTeammates" }`. Unknown keys are rejected. `defineOverlay` accepts the new source, and its existing checks (`worldAnchor`, `maxVisible` 1–64) still apply.
- [ ] **R3** (unit, SDK validators and the Rust template validator): `Glyph({ command })` is accepted at a template root and nested inside stacks. An empty `command` is rejected. Every other kind that templates reject today is still rejected, with the same message, except that the TS and Luau messages' list of allowed kinds now names `glyph`.
- [ ] **R4** (unit, app): for the same command and device family, a glyph inside a template draws exactly what a HUD glyph draws: art, label or nothing. It changes after a rebind or a device-family switch with no reload, and keeps its `visibleWhen`.
- [ ] **R5** (unit, client producer): with retained facts present, a marker exists for every mapped remote pawn whose `assistable` is set, at any distance. No marker exists for an unassistable pawn, the local pawn, or an unmapped pawn. A marker is removed on the frame its facts flip to unassistable. `maxVisible` caps markers: the teammate a hold would revive always keeps a slot; the rest fill in `players()` order.
- [ ] **R6** (unit, client producer): `inRange` is true only for the pawn that `assist_candidate` picks for the local pawn. It is false when:
  - the local pawn's state forbids `use`;
  - the pair entry has not arrived;
  - the radius is ≤ 0;
  - the distance is past the radius;
  - another downed teammate is nearer (on equal distance, the earlier pawn in `players()` order wins).
- [ ] **R7** (unit, host and SP): R5 and R6 also hold on a listen host, using registry facts and `AssistState::pair_values`. Single player produces no markers.
- [ ] **R8** (unit): when both overlay kinds are live, damaged-enemy roster retain, clear and cap never remove a marker, and the marker pass never removes a damaged-enemy bar. Level unload and staged reload clear both kinds.
- [ ] **R9** (unit): with no posed socket, the anchor is the presented position plus `offsetY`. On a connected client that is the interpolated pose at `render_server_tick`. The hitbox top is never used.
- [ ] **R10** (regression): no wire version bump. Adding or removing the marker overlay leaves the mod digest unchanged (extend the existing digest test).
- [ ] **R11** (fixture): the dev mod registers the Scripting surface example. The mod-load and typecheck tests load it in both overlay slots alongside `damagedEnemyOverlay`.
- [ ] **R13** (unit, host and client): with only the marker overlay declared, markers survive every frame. The damaged-enemy paths that run with no damaged-enemy config (host roster, client anchors) clear nothing of the marker kind, and the pool's per-frame linger expiry never removes a marker.
- [ ] **R14** (unit, listen host): for a connected client's downed pawn, the marker's anchor follows the delayed pose the host draws for that pawn, not its authoritative pose. `inRange` is measured on the authoritative positions the host's assist pass reads, so it flips on the same tick a hold would start or stop choosing that pawn.
- [ ] **R15** (unit, app): a template glyph draws what a HUD glyph draws on the first frame after each event that installs templates or changes glyph art, with no binding or device change in between: a staged reload that edits only the marker template, and glyph art that finishes loading after the templates were installed.
- [ ] **R12** (review and grep): docs cover `presentationOverlays` in both forms, `assistableTeammates`, the `inRange` fact and template glyphs. Every command shown runs from an SDK bundle. No non-test source comment cites a spec ID.

### Manual
- [ ] **M1**: add this line to `manual-proof.md`. In a two-seat run at 150 ms RTT, on both the host and the client seat:
  - a downed teammate shows a marker at any range and through walls;
  - "Hold [E] to revive" appears within 1.5 m and goes away past it;
  - after gamepad input, the glyph switches to the west button;
  - the marker goes away when the teammate is revived or bleeds out;
  - a downed player never sees a marker on themselves;
  - with both players down, each sees the other's marker with no prompt.

## Path

Non-binding.

- **Overlay surface (rival).** `PresentationOverlaySource` is already a kind-tagged enum. Internally the manifest already carries `Vec<PresentationOverlay>`, and `replace_presentation_overlays` documents "disjoint key namespace" as the planned extension. Growing the existing field to accept one descriptor or an array, with at most one per kind, follows that design. The rival is a second manifest field per kind. It avoids changing the accepted shape, but every future kind would need its own field, drain and typedef.
- **Pool keys (riskiest; build this first).** `PresentationPool.overlays` is keyed by `EntityId`. Change the key to (overlay kind, entity) before anything else in T2. Every whole-map operation becomes kind-scoped:
  - `retain_overlays` and `clear_overlays`;
  - linger expiry (`presentation_pool.rs:109`);
  - the caps in `refresh_overlay` and `upsert_authoritative_overlay` (`:270`, `:315`);
  - the tracked-id iterator (`impact_policy.rs:942`).

  Create markers as authoritative overlays, or linger expiry deletes them every frame. Prove R8 and R13 before building on top.
- **No-config clears.** With no damaged-enemy config, the host roster path and the client anchor path both call `pool.clear_overlays()` (`overlays.rs:246`, `:371`). Those clears must touch only the damaged-enemy kind. R13 covers this.
- **Listen-host pose timing.** Measure in-range before `host_present_client_pawns` (`frame_loop/mod.rs:1634`), against the authoritative pose, as the assist pass does. Stamp the marker anchor after it, from the presented (delayed) pose, including the socket lookup. R14 covers this.
- **Template glyph re-resolution.** Resolve glyphs at both sites where raw templates reach the renderer: `staged_manifest_lifecycle.rs:287` and `splash_lifecycle.rs:628`. Add a token that changes when glyph art finishes loading, because `GlyphArtState` loads lazily once the renderer is ready, after templates install. Re-resolve when only the template changes on a reload, which does not bump the binding generation (`binding_state.rs:104`). R15 covers this.
- **Client config.** `client_overlay_config()` reads `.first()`, and `replace_presentation_overlays` rejects more than one overlay. Both need per-kind lookup. Binding an `assistableTeammates` overlay needs no IR.
- **Producer home.** Put the producer in a new `client_assist/markers.rs`, not in `impact_policy.rs` (4172 lines) or `main.rs`. Like `glue::ui_projection`, it dispatches on endpoint: the client reads `ClientReplication`, and the host and SP read the registry and `session.assist_state`. Call it once per frame after the overlay updates (`frame_loop` after `update_client_overlay_anchors`, and after the host or SP overlay branch). Reuse `gather_candidates` and `local_helper` from `glue.rs`, and widen their visibility rather than copying them. On the host, build `AssistCandidate`s the way `assist_pass` does.
- **Template glyphs.** The renderer stores raw templates (`UiPass::replace_presentation_templates`), and `resolve_snapshot_glyphs` only walks the snapshot's trees. Resolve template roots app-side with the same `resolve_in`. Re-send them through `set_presentation_templates` only when the glyph inputs change: the binding table, device family, the art-loaded token, or the templates themselves. A re-send clears the instance layouts, which is acceptable at that rate.

**Suggested split:**
- **T1 [Opus]:** SDK and scripting-core contract. Covers the array form in the JS and Luau drains, `assistableTeammates` in both runtimes, `Glyph` in all three validators, the typedef template and regenerated `sdk/types`, and the per-kind runtime config. Delivers R1–R3 and R10.
- **T2a [Opus]:** the pool key split to (overlay kind, entity) and kind-scoped retain, clear, linger expiry, caps and tracked-id iterator, with markers created as authoritative. Delivers R8 and R13's pool-only rows.
- **T2b [Opus]:** the marker producer for both roles, the anchor fallback, the no-config clears, listen-host pose timing, and app-side template glyph resolution with its re-resolve token. Delivers R4–R7, R9, R13 (no-config clears), R14 and R15.
- **T3 [Sonnet]:** the dev-mod template (in `combat-presentation.ts` or a new `revive-marker.ts`), wiring in `start-script.ts`, the docs (a short presentation-overlay section in `docs/scripting-reference.md`; `ui.md` §1 and §4; one sentence in `networking.md` that markers never cross the wire), and the M1 line. Delivers R11, R12 and M1.

T1 and T2a can run at the same time. T2b and T3 start after T1, because they need the `AssistableTeammates` variant and per-kind config.

**Builder notes:**
- A resolved glyph label is fixed at font size 16, next to the template's 14 pt text.
- The dev player model has no `head` socket, so M1 exercises the fallback anchor.

## Research pins

| Claim | Pin |
|---|---|
| SDK template allow-list | `sdk/lib/ui/presentation.ts:243`, `sdk/lib/ui/presentation.luau:184` |
| `defineOverlay` accepts only `damagedEnemies` | `sdk/lib/ui/presentation.ts:394`, `:402`; `presentation.luau:287` |
| Rust overlay types (`over` is a kind-tagged enum) | `crates/scripting-core/src/data_descriptors/runtime_manifest.rs:417`, `:442` |
| Rust template validator rejects `glyph` | `runtime_manifest.rs:278` (`validate_presentation_widget`) |
| Manifest drain accepts one descriptor only | `data_descriptors/js/manifest.rs:836`, `lua/manifest.rs:688` |
| Typedef text: "accepts one descriptor" | `crates/sim/src/scripting/primitives/manifest.rs:291` |
| Runtime rejects more than one overlay; planned disjoint key namespace | `crates/sim/src/impact_policy.rs:355` (`replace_presentation_overlays`), `:339` (`client_overlay_config`) |
| Pool keyed by `EntityId`; whole-map retain and clear | `crates/sim/src/sim/presentation_pool.rs:40`, `:301`, `:399`, `:411` |
| Host roster projection and publish | `crates/netcode/src/presentation/overlays.rs:232`; called at `crates/postretro/src/frame_loop/mod.rs:1591` |
| Client roster projection | `overlays.rs:356`; `main.rs:5092`; `frame_loop/mod.rs:1206` |
| Anchor order: socket, then hitbox top, then mesh bound | `crates/netcode/src/presentation.rs:160` (`client_overlay_anchor`) |
| Dev bar anchor | `content/dev/scripts/combat-presentation.ts:59` (`socket: "head", offsetY: 0.35`) |
| Pawn origin is capsule centre; hitbox spans standing capsule; `down` is crouch | `content/dev/scripts/player.ts:21`, `:33` |
| Retained remote facts, positions, pair table | `crates/netcode/src/client/assist_retention.rs:125`, `:133`, `:152`, `:173` |
| Client candidate gathering, local helper | `crates/postretro/src/client_assist/glue.rs:15`, `:36`; role dispatch `:144` |
| In-range predicate | `crates/sim/src/sim/assist.rs:178` (`assist_candidate`); `assist_pairs.rs:60` (`PairValues::in_range`) |
| Host pair values | `assist.rs:343` (`AssistState::pair_values`); `PlayerMovementComponent.assistable` at `crates/foundation/src/movement/player_movement.rs:439` |
| Glyph resolution covers trees only | `crates/postretro/src/app/glyph_art.rs:239`, `:265`; renderer template store `crates/renderer/src/render/ui/retained_layers.rs:47` |
| Fact-bool `visibleWhen` already works in templates | `crates/ui/src/tree/presentation_layout.rs` test `presentation_bool_fact_drives_visibility_without_local_state` |
| Overlays are left out of the compat digest | `crates/postretro/src/mod_digest.rs:1107` |
| Dev wiring | `content/dev/start-script.ts:179`; HUD bars `content/dev/scripts/hud.ts:332`, `:365` |
| Whole-map overlay operations beyond retain and clear: linger expiry, the caps in `refresh_overlay` and `upsert_authoritative_overlay`, and the tracked-id iterator the single-player damage path samples | `presentation_pool.rs:109`, `:270`, `:315`, `:433`; `impact_policy.rs:942` |
| With no damaged-enemy config, host and client clear the whole pool every frame | `overlays.rs:244`–`:246` (host), `:369`–`:371` (client) |
| Host overlay branch runs before the delayed client-pawn pose is written (Transform is authoritative there) | `frame_loop/mod.rs:1591` vs `:1634` (`host_present_client_pawns`); `networking.md` §Host-side remote-pawn presentation |
| Socket lookup reads the registry Transform too | `crates/sim/src/scripting_systems/hit_zones.rs:337` (`posed_socket_world`) |
| Dev player model has no `head` socket (only `hand_l`, `hand_r`), so the dev marker always takes the D10 fallback | `content/dev/models/exo_red/model.gltf` node extras; model named at `player.ts:59` |
| Raw templates reach the renderer at two install sites | `crates/postretro/src/startup/staged_manifest_lifecycle.rs:287`, `startup/splash_lifecycle.rs:628` |
| Glyph inputs: binding table has a generation; glyph art has none (keyed by dirs and staged-reload generation, loaded lazily once the renderer is full-ready) | `crates/postretro/src/input/binding_state.rs:104`; `glyph_art.rs:21`, `:146` |
| Host pair values cover only helpers permitting `use` and assistable targets, from the last fixed-tick pass | `crates/sim/src/sim/assist_pass.rs:68`–`:83` |
| `scripting-reference.md` has no presentation-template section today; `Glyph` documented at about line 4199 | `docs/scripting-reference.md` |

Constraints: no spec IDs in non-test source comments. Every command in `docs/` must run from an SDK bundle.

## Open questions

Nothing is owner-blocking. Q1–Q3 are closed as D12–D14.

- **Q4.** How should the posed-socket anchor behave for remote pawns on a client? On a snapshot-apply frame the socket reads the registry pose, while the fallback reads the interpolated pose. — builder note, **delegated**
