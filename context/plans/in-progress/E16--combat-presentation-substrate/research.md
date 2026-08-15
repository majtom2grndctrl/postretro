# Combat Presentation Substrate — Research

Grounding for `index.md`. Five parallel source-grounding passes, 2026-08-15. Identifiers/paths are current-as-of; they belong here (ephemeral), not in the durable library. `index.md` references behavior; this file carries the seam map.

## Design decisions the grounding forced

1. **Enemy health is NOT replicated.** The replicated entity-component set is closed to four kinds — `ComponentPayload::{Transform, PlayerMovementState, MeshAnimationState, KinematicMoverState}` (`crates/net/src/wire.rs:395`). `HealthComponent` (kind 10) and `EntityStateComponent` (kind 17) are host-only. The only replicated health is the local player's own HP via the owner-private `player.health` slot (`engine_state_catalog.rs:349`). So a co-op client cannot read a tracked *enemy's* health/shield locally. **Correction to the sample:** enemy bars are not "off already-replicated enemy health." The fix: the host *pushes* enemy health/shield presentation facts to the damaging client over the same unreliable channel as damage numbers — present events, don't replicate state.

2. **`present()` joins an existing lane.** The impact-effect apply already splits consequential vs. presentation (`evaluate_dispatch`, `impact_policy.rs:274`); `apply_planned` is called twice — `false` then `true` (`impact_policy.rs:287`). `PlayAnimation` is the only current presentation-lane effect. `present()` is a new presentation-lane member, intercepted in `apply_planned` where `ScriptCtx` + the `&ImpactDispatch` are in scope (the `SetOwnerSlot` interception precedent, `impact_policy.rs:340`) — NOT in `apply_effect`, which has only `&mut EntityRegistry`.

3. **The draw-list layer is decoupled from the retained tree.** Reusable without taffy/modal/focus/input. `UiComposition::from_layer_draws` consumes `&[UiDrawData]`; `push_focus_ring` is precedent for a non-tree renderer producer.

4. **Impact policies are host-only** (client `continue`s past `simulate_tick`). `present()` fires host-side; the shooter's own number reaches its client either by host-addressed event (first cut) or a client-local spawn off `resolution.hits` (deferred refinement).

## Seam map — impact / `present()` effect

- `BoundEffect` enum — `crates/postretro/src/impact_policy.rs:51`. `ImpactEffect` — `crates/postretro/src/impact_effects.rs:41`. Intermediate `PlannedEffect { Write, Command }` with `CommandRecipient::{Target, Source}` — `impact_policy.rs:81,86`.
- Lifecycle: `bind_effect` (`impact_policy.rs:441`, per-effect at registration, enforces `require_impact_token`) → `plan_effect` (`impact_policy.rs:604`, evaluates IR operands to concrete values per-fire, assigns recipient) → `apply_planned` (`impact_policy.rs:291`, has `self.ctx: ScriptCtx` + `&ImpactDispatch` + `&mut EntityRegistry`) → `apply_effect` (`impact_effects.rs:72`, registry-only).
- Lane sort: `evaluate_dispatch` (`impact_policy.rs:273-283`) sorts into `self.consequential` / `self.presentation`; applied `apply_planned(false)` then `apply_planned(true)` (`impact_policy.rs:287`).
- Chokepoint: `apply_damage_with_context` (`crates/entities/src/components/health.rs:424`) pushes `ImpactDispatch { target, source: Option<EntityId>, amount, health_before, health_after (unfloored), max_health, producer }` (`health.rs:125`, `push_impact_dispatch` at `:460`). Evaluation drains only `producer == DamageProducer::InTick` (`impact_policy.rs:162`). Tokens `@impact.target`/`@impact.source` = `health.rs:108,111` — command targets, never IR leaves.
- SDK: `ImpactEffectWire` union (`sdk/lib/data_script.ts:239`), `TargetHandle`/`SourceHandle`/`Impact` (`:258,270,286`), `IMPACT_TARGET`/`IMPACT_SOURCE` singletons + `impactEffect`/`sourceImpactEffect` lowering (`:372,386,409`), `defineImpactEvent` (`:519`). Parity: `data_script.luau`, `sdk/types/postretro.d.luau`, typedef goldens `crates/postretro/src/scripting/typedef/tests/`.
- **Oversized:** `impact_policy.rs` 2336 (~700 impl), `data_script.ts` 924. `impact_effects.rs` 712.

## Seam map — UI render / draw-list / projection

- Pass: `crates/renderer/src/render/ui/mod.rs` (1244) + `text.rs`. CPU-free half in `crates/ui/` (`postretro_ui`). `UiPass` (`mod.rs:188`), `encode` (`mod.rs:859`), fold point = `renderer_render_frame.rs:883-941`, `encode` call `:943`. Runs after scene, before screen-effects resolve (`:962`).
- Draw list (pure CPU, reusable): `UiInstance` (`crates/ui/src/output.rs:15`, `panel`/`image` ctors), `UiDrawList` (`output.rs:88`), `UiText` (`output.rs:129`), `UiDrawData` (`crates/ui/src/tree/draw.rs:209`) via `push_quad`/`push_image`/`push_text` (`draw.rs:229`). `UiComposition::from_layer_draws(&[UiDrawData])` (`mod.rs:296`), `from_batches` (`mod.rs:404`). Non-tree producer precedent: `push_focus_ring` (`mod.rs:1109`).
- Projection (net-new for UI): billboards project GPU-only. CPU helper pattern `agent_overlay_world_to_screen` (`crates/postretro/src/agent_diagnostics.rs:93`, dev-tools/egui-typed — lift, don't reuse). Camera: renderer `last_view_proj: Mat4` (`renderer_types.rs:705`, `pub(super)`, no getter), app `RenderCamera.view_projection` (`camera.rs:33`, `view_projection()` `:156`). UI uniform is viewport-only (`UiUniform`, `output.rs:118`).
- Widgets: `Widget` enum, 11 kinds (`crates/scripting-core/src/ui/descriptor/widgets.rs:28`) incl. `Bar` (`BarWidget`, passive bound display). `styleRanges` (`style_ranges.rs`, on Text/Panel), `visibleWhen` (`Predicate`, on Text/Panel), value `tween` (`TextTween`/`PanelTween`, runtime `TweenState<T>` in `crates/ui/src/tree/style.rs`). `Switch` = SDK sugar over `visibleWhen` (`sdk/lib/ui/state.ts:189`). Layout: `layout_gameplay_tree` (`mod.rs:728`, retained). Theme: `UiTheme::color/font/spacing` (`crates/scripting-core/src/ui/theme.rs:22`), consumed outside the tree walk already (focus ring). Images: `UiImageRegistry` (`mod.rs:72`).
- **Oversized:** `render/ui/mod.rs` 1244, `descriptor/mod.rs` 1103, `sdk/lib/ui/widgets.ts` 1016. `draw.rs` 554, `widgets.rs` 694.

## Seam map — state / per-instance facts

- Binding resolution: retained UI resolves through a per-frame `HashMap<String, SlotValue>` snapshot — `UiRenderOutput.slot_values` (`crates/ui/src/output.rs:214`), resolved in `resolve_bindings(slot_values, cell_values, …)` (`crates/ui/src/tree/bindings.rs:25`). `cell_values` is the per-instance `localState` source (G2) — the precedent for per-instance keying. Producer: `PlayerHudStatePublisher::write_hud_slot` (`crates/postretro/src/scripting/systems/ui_proxy.rs:111`).
- Per-entity state: `EntityStateComponent { values: HashMap<String,f32> }` (`crates/entities/src/components/entity_state.rs:14`), `get(name)->f32` (0.0 if unset). **Direct Rust read exists:** `registry.get_component::<EntityStateComponent>(id).get(name)` (used by `seed_target_from_registry`, `scopes.rs:370`). IR leaf `@state.<name>` via `EntityScope` (`scopes.rs:493`), **host-only** (`scopes.rs:306`). Write `registry.entity_state_mut(id).set(...)`.
- Health: `HealthComponent { max, current, … }` (`crates/entities/src/components/health.rs:314`). Read `registry.get_component::<HealthComponent>(id).current/.max`.
- Iterate tracked entities host-side: `EntityRegistry::iter_with_kind(kind)` (`registry.rs:888`), `query_by_component_and_tag` (`registry.rs:857`). Both facts computable host-side per frame with existing accessors.
- **Per-instance fact snapshot is net-new:** the global `slot_values` map has no per-instance dimension; per-instance facts key like `cell_values` (localState), supplied by the producer per instance.
- **Oversized:** `slot_table.rs` 1026, `scopes.rs` 1317, `health.rs` 1137 (majority tests).

## Seam map — netcode transport

- Channels: `Channel::{Control=reliable, Snapshot=Unreliable, Input=reliable}` (`crates/net/src/transport.rs:27,43`). **Unreliable transport already exists (Snapshot).**
- Host→one-client addressed: `ServerMessage::{TimeSync, ShotVerdicts}` on Input (`crates/net/src/wire.rs:1112`), sent `NetServer::send_input(client_id, payload)` (`transport.rs:699`); `ServerControlMessage` on Control (`control.rs:262`), `send_control(client_id, …)` (`transport.rs:688`). **`ShotVerdicts` is the owner-private host→one-client precedent** — send path `send_shot_verdict` (`netcode/mod.rs:1749`).
- Client-local hit signal: `run_client_fire_path_post_loop` (`main.rs:6100`), `resolve_client_fire`→`ClientFireResolution { hits: Vec<LocalHitRecord { target, point, zone }> }` (`weapon/mod.rs:435,58`); hits populated at `main.rs:6217` before the declaration send (`:6247`).
- Wire version: `WIRE_VERSION=18` (`handshake.rs:18`), bitcode positional + append-only (no unknown-variant skip). New `ServerMessage` variant → bump + handshake-gated refusal of old peers.
- **Oversized:** `net/wire.rs` 2728, `transport.rs` 2090, `netcode/mod.rs` 4781, `main.rs` >8000.

## Deferred / follow-on (pickup prompts — not this spec)

Touch pass is host-only (`sim/touch.rs`, clients skip `simulate_tick`); `prompts: Vec<(PlayerId, EntityId)>` (`touch.rs:85`) has no consumer. Pickup prompts need a net-new client-side local overlap pass + `awarenessRadius` on `TouchableDescriptor` (`combat.rs:207`) + net-new `label`/`icon` descriptor fields (no display name/icon exists on wieldables). Dwell asymmetry confirmed (press has a standing window; auto acquires on enter). `touch.rs` 2501 lines.
