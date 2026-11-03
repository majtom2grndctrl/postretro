// Runtime-side manifest types that embed render::ui descriptor data.
// See: context/lib/scripting.md §13 (Crate Architecture)

use postretro_foundation::{
    BUILTIN_PRESENTATION_TEMPLATE_ID_PREFIX, IrNode, PresentationEasing,
    is_builtin_presentation_template_id,
};
use serde::{Deserialize, Serialize};

use crate::ui::descriptor::{
    AnchoredTree, BarMax, BindSource, ColorValue, Predicate, ScalarValue, SpacingValue, TextTween,
    Widget,
};
use crate::ui::style_ranges::StyleRanges;

use super::{
    CrossingDescriptor, ImpactEventDescriptor, NamedReaction, PlayerEventDescriptor,
    TeleportEventDescriptor, TriggerPoolDescriptor, VolumeTriggerEventDescriptor,
};

/// A script-registered UI tree: a named [`AnchoredTree`] plus its stack
/// presentation attributes. Drained from `ModManifest.uiTrees` (mod scope) and
/// `setupLevel()` (level scope) returns.
#[derive(Debug, Clone, PartialEq)]
pub struct RegisteredUiTree {
    /// Registry name the render path resolves the tree by.
    pub name: String,
    /// The placement envelope + widget tree, parsed via the G1a bridge.
    pub tree: AnchoredTree,
    /// `alwaysOn` registration attribute: a tree that stays resolvable even when
    /// it is not on top of the modal stack. Defaults to `false` when absent.
    pub always_on: bool,
    /// `hideBelow` registration attribute: when this tree is pushed, lower
    /// pushed trees stay retained but are omitted from the composed draw list.
    /// Defaults to `false` when absent.
    pub hide_below: bool,
}

/// Renderer-consumed definition for one passive, world-anchored transient.
/// Scripts retain this as manifest data; the app resolves its timing parameters
/// when an impact plans a spawn, and the renderer resolves its widget subtree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PresentationTemplate {
    /// Binding-derived stable template handle. It is not author-facing mutable
    /// data: TypeScript's compiler supplies it from a direct `const` binding.
    pub id: String,
    pub root: Widget,
    pub lifetime_ms: u32,
    pub motion: PresentationTemplateMotion,
    pub fade: PresentationTemplateFade,
    pub spawn_scatter: PresentationTemplateSpawnScatter,
    /// Overlay-only anchor metadata. Spawn presentation anchors at the
    /// impact's contact point, else the target's transform, so omitting this
    /// remains valid for a number/toast template.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world_anchor: Option<PresentationWorldAnchor>,
}

impl PresentationTemplate {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty() {
            return Err("presentation template `id` must be nonempty".to_string());
        }
        if is_builtin_presentation_template_id(&self.id) {
            return Err(format!(
                "presentation template `id` must not use reserved engine namespace `{BUILTIN_PRESENTATION_TEMPLATE_ID_PREFIX}`"
            ));
        }
        if !self.motion.rise.is_finite() {
            return Err("presentation template `motion.rise` must be finite".to_string());
        }
        if !self.spawn_scatter.radius.is_finite() || self.spawn_scatter.radius < 0.0 {
            return Err(
                "presentation template `spawnScatter.radius` must be finite and non-negative"
                    .to_string(),
            );
        }
        if self.fade.start_ms > self.lifetime_ms {
            return Err(
                "presentation template `fade.startMs` must not exceed `lifetimeMs`".to_string(),
            );
        }
        if let Some(anchor) = &self.world_anchor {
            anchor.validate()?;
        }
        validate_presentation_widget_root(&self.root)?;
        Ok(())
    }
}

/// Retained UI trees have no producer-stamped fact bundle. Rejecting fact
/// sources at the descriptor boundary prevents an ordinary tree from quietly
/// resolving every such read as absent. Also runs the shared `compare` checks
/// (not alongside `equals`, finite f32 threshold, Number engine slot); engine
/// JSON trees call it too, since they bypass the script bridges.
pub fn validate_retained_widget_sources(widget: &Widget) -> Result<(), String> {
    validate_widget_sources(widget, "UI tree `root`", false)
}

/// Validate the passive template vocabulary and every f32-backed draw input
/// before a VM value is lowered through JSON. This keeps non-finite bridge
/// values from turning into an unhelpful serialization failure.
pub(crate) fn validate_presentation_widget_root(widget: &Widget) -> Result<(), String> {
    validate_presentation_widget(widget, "presentation template `root`")
}

fn validate_widget_sources(widget: &Widget, path: &str, allow_facts: bool) -> Result<(), String> {
    let source = |source: &BindSource, field: &str| {
        let (kind, name) = match source {
            BindSource::Slot { slot } => ("slot", slot),
            BindSource::Local { local } => ("local", local),
            BindSource::Fact { fact } => ("fact", fact),
        };
        if name.is_empty() {
            return Err(format!("{path}.{field}.{kind} must be nonempty"));
        }
        if !allow_facts && matches!(source, BindSource::Fact { .. }) {
            return Err(format!(
                "{path}.{field} uses a presentation fact outside a presentation template"
            ));
        }
        Ok(())
    };
    let one_predicate = |predicate: &Predicate, field: &str| {
        source(&predicate.source, field)?;
        match &predicate.compare {
            Some(compare) => crate::ui::descriptor::validate_predicate_compare(
                predicate.source.slot(),
                predicate.equals.is_some(),
                compare,
                &format!("{path}.{field}"),
            ),
            None => Ok(()),
        }
    };
    let predicate = |predicate: &Option<Predicate>, field: &str| match predicate {
        Some(predicate) => one_predicate(predicate, field),
        None => Ok(()),
    };

    match widget {
        Widget::Text(text) => {
            if let Some(bind) = &text.bind {
                source(&bind.source, "bind")?;
                if bind.decimal_places.is_some_and(|places| places > 6) {
                    return Err(format!("{path}.bind.decimalPlaces must be between 0 and 6"));
                }
            }
            predicate(&text.visible_when, "visibleWhen")
        }
        Widget::Panel(panel) => {
            if let Some(bind) = &panel.bind {
                source(&bind.source, "bind")?;
            }
            predicate(&panel.visible_when, "visibleWhen")
        }
        Widget::Image(image) => predicate(&image.visible_when, "visibleWhen"),
        Widget::VStack(container) | Widget::HStack(container) => {
            predicate(&container.visible_when, "visibleWhen")?;
            for (index, child) in container.children.iter().enumerate() {
                validate_widget_sources(child, &format!("{path}.children[{index}]"), allow_facts)?;
            }
            Ok(())
        }
        Widget::Grid(grid) => {
            predicate(&grid.visible_when, "visibleWhen")?;
            for (index, child) in grid.children.iter().enumerate() {
                validate_widget_sources(child, &format!("{path}.children[{index}]"), allow_facts)?;
            }
            Ok(())
        }
        Widget::Spacer(spacer) => predicate(&spacer.visible_when, "visibleWhen"),
        Widget::Glyph(glyph) => predicate(&glyph.visible_when, "visibleWhen"),
        Widget::Button(button) => {
            predicate(&button.selected, "selected")?;
            predicate(&button.checked, "checked")?;
            predicate(&button.bind, "bind")?;
            for (index, case) in button.value_text.iter().enumerate() {
                for (when_index, condition) in case.when.iter().enumerate() {
                    one_predicate(condition, &format!("valueText[{index}].when[{when_index}]"))?;
                }
            }
            predicate(&button.visible_when, "visibleWhen")
        }
        Widget::Slider(slider) => {
            source(&slider.bind.source, "bind")?;
            predicate(&slider.visible_when, "visibleWhen")
        }
        Widget::Bar(bar) => {
            source(&bar.bind.source, "bind")?;
            predicate(&bar.visible_when, "visibleWhen")
        }
        Widget::Ring(ring) => {
            for (field, scalar) in [("radius", &ring.radius), ("thickness", &ring.thickness)] {
                if let ScalarValue::Bound(bound) = scalar {
                    source(&bound.source, field)?;
                }
            }
            for (field, scalar) in [
                ("startAngle", ring.start_angle.as_ref()),
                ("sweep", ring.sweep.as_ref()),
            ] {
                if let Some(ScalarValue::Bound(bound)) = scalar {
                    source(&bound.source, field)?;
                }
            }
            predicate(&ring.visible_when, "visibleWhen")
        }
        Widget::Announce(announce) => predicate(&announce.visible_when, "visibleWhen"),
    }
}

fn validate_presentation_widget(widget: &Widget, path: &str) -> Result<(), String> {
    validate_widget_sources(widget, path, true)?;
    match widget {
        Widget::Text(text) => {
            validate_positive_f32(text.font_size, &format!("{path}.fontSize"))?;
            validate_color(&text.color, &format!("{path}.color"))?;
            if let Some(bind) = &text.bind {
                validate_text_tween(bind.tween.as_ref(), &format!("{path}.bind.tween"))?;
            }
            validate_style_ranges(text.style_ranges.as_ref(), &format!("{path}.styleRanges"))
        }
        Widget::Bar(bar) => {
            bar.validate()
                .map_err(|reason| format!("{path}: {reason}"))?;
            validate_text_tween(bar.bind.tween.as_ref(), &format!("{path}.bind.tween"))?;
            match &bar.max {
                BarMax::Literal(value) => validate_positive_f32(*value, &format!("{path}.max"))?,
                BarMax::State(reference) if reference.slot.is_empty() => {
                    return Err(format!("{path}.max.slot must be nonempty"));
                }
                BarMax::State(_) => {}
            }
            validate_color(&bar.fill, &format!("{path}.fill"))?;
            validate_color(&bar.background, &format!("{path}.background"))?;
            validate_style_ranges(bar.style_ranges.as_ref(), &format!("{path}.styleRanges"))
        }
        Widget::Ring(_) => Err(format!(
            "{path}.kind `ring` is not supported in passive presentation templates"
        )),
        Widget::Image(image) => {
            if image.asset.is_empty() {
                return Err(format!("{path}.asset must be nonempty"));
            }
            Ok(())
        }
        Widget::VStack(container) | Widget::HStack(container) => {
            validate_spacing(&container.gap, &format!("{path}.gap"))?;
            validate_spacing(&container.padding, &format!("{path}.padding"))?;
            if let Some(fill) = &container.fill {
                validate_color(fill, &format!("{path}.fill"))?;
            }
            if let Some(border) = &container.border {
                if border.texture.is_empty() {
                    return Err(format!("{path}.border.texture must be nonempty"));
                }
                for (index, value) in border.slice.iter().copied().enumerate() {
                    validate_non_negative_f32(value, &format!("{path}.border.slice[{index}]"))?;
                }
                validate_color(&border.tint, &format!("{path}.border.tint"))?;
            }
            for (index, child) in container.children.iter().enumerate() {
                validate_presentation_widget(child, &format!("{path}.children[{index}]"))?;
            }
            Ok(())
        }
        Widget::Panel(_) => Err(format!(
            "{path}.kind `panel` is not supported in passive presentation templates"
        )),
        Widget::Grid(_) => Err(format!(
            "{path}.kind `grid` is not supported in passive presentation templates"
        )),
        Widget::Spacer(_) => Err(format!(
            "{path}.kind `spacer` is not supported in passive presentation templates"
        )),
        Widget::Glyph(glyph) => {
            if glyph.command.is_empty() {
                return Err(format!("{path}.command must be nonempty"));
            }
            Ok(())
        }
        Widget::Button(_) => Err(format!(
            "{path}.kind `button` is interactive and is not supported in passive presentation templates"
        )),
        Widget::Slider(_) => Err(format!(
            "{path}.kind `slider` is interactive and is not supported in passive presentation templates"
        )),
        Widget::Announce(_) => Err(format!(
            "{path}.kind `announce` is not supported in passive presentation templates"
        )),
    }
}

fn validate_positive_f32(value: f32, field: &str) -> Result<(), String> {
    if !value.is_finite() || value <= 0.0 {
        return Err(format!("{field} must be a finite f32 greater than zero"));
    }
    Ok(())
}

fn validate_non_negative_f32(value: f32, field: &str) -> Result<(), String> {
    if !value.is_finite() || value < 0.0 {
        return Err(format!("{field} must be a finite non-negative f32"));
    }
    Ok(())
}

fn validate_spacing(value: &SpacingValue, field: &str) -> Result<(), String> {
    match value {
        SpacingValue::Literal(value) => validate_non_negative_f32(*value, field),
        SpacingValue::Token(token) if token.is_empty() => {
            Err(format!("{field} token must be nonempty"))
        }
        SpacingValue::Token(_) => Ok(()),
    }
}

fn validate_color(value: &ColorValue, field: &str) -> Result<(), String> {
    match value {
        ColorValue::Literal(color) if color.iter().any(|value| !value.is_finite()) => {
            Err(format!("{field} components must be finite f32 values"))
        }
        ColorValue::Token(token) if token.is_empty() => {
            Err(format!("{field} token must be nonempty"))
        }
        ColorValue::Literal(_) | ColorValue::Token(_) => Ok(()),
    }
}

fn validate_text_tween(tween: Option<&TextTween>, field: &str) -> Result<(), String> {
    let Some(tween) = tween else {
        return Ok(());
    };
    validate_non_negative_f32(tween.duration_ms, &format!("{field}.durationMs"))?;
    if let Some(from) = tween.from
        && !from.is_finite()
    {
        return Err(format!("{field}.from must be a finite f32"));
    }
    Ok(())
}

fn validate_style_ranges(ranges: Option<&StyleRanges>, field: &str) -> Result<(), String> {
    let Some(ranges) = ranges else {
        return Ok(());
    };
    validate_positive_f32(ranges.max, &format!("{field}.max"))?;
    for (index, entry) in ranges.entries.iter().enumerate() {
        let entry_path = format!("{field}.entries[{index}]");
        if let Some(up_to) = entry.up_to
            && !up_to.is_finite()
        {
            return Err(format!("{entry_path}.upTo must be a finite f32"));
        }
        if let Some(color) = &entry.color {
            validate_color(color, &format!("{entry_path}.color"))?;
        }
        if let Some(pulse) = entry.pulse {
            validate_positive_f32(pulse.period_ms, &format!("{entry_path}.pulse.periodMs"))?;
        }
        if let Some(flash) = entry.flash {
            validate_non_negative_f32(
                flash.duration_ms,
                &format!("{entry_path}.flash.durationMs"),
            )?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PresentationTemplateMotion {
    pub rise: f32,
    pub easing: PresentationEasing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PresentationTemplateFade {
    pub start_ms: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PresentationTemplateSpawnScatter {
    pub radius: f32,
}

/// Model-local anchor selected by an overlay template. The host resolves the
/// named socket (or a matching hit-zone tag) from CPU-side hit-zone data. With
/// no socket, or one the model lacks, the overlay source's own fallback anchor
/// applies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PresentationWorldAnchor {
    /// `None` when the template names no socket. An explicit empty name is
    /// rejected, as the SDK validators reject it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket: Option<String>,
    pub offset_y: f32,
}

impl PresentationWorldAnchor {
    fn validate(&self) -> Result<(), String> {
        if !self.offset_y.is_finite() {
            return Err("presentation template `worldAnchor.offsetY` must be finite".to_string());
        }
        if self.socket.as_deref() == Some("") {
            return Err("presentation template `worldAnchor.socket` must be nonempty".to_string());
        }
        Ok(())
    }
}

/// Upper bound on one overlay's `maxVisible`.
pub const MAX_PRESENTATION_OVERLAY_VISIBLE: usize = 64;

/// One fact-driven passive overlay declaration from
/// `ModManifest.presentationOverlays`. The manifest field accepts one of these
/// or an array of them, at most one per [`PresentationOverlayKind`]. It is
/// host-local authoring data, never a transport payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PresentationOverlay {
    pub over: PresentationOverlaySource,
    pub template: String,
    pub max_visible: usize,
}

impl PresentationOverlay {
    pub fn validate(&self) -> Result<(), String> {
        if self.template.is_empty() {
            return Err("presentation overlay `template` must be nonempty".to_string());
        }
        if self.max_visible == 0 {
            return Err("presentation overlay `maxVisible` must be at least 1".to_string());
        }
        if self.max_visible > MAX_PRESENTATION_OVERLAY_VISIBLE {
            return Err(format!(
                "presentation overlay `maxVisible` must be at most {MAX_PRESENTATION_OVERLAY_VISIBLE}"
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum PresentationOverlaySource {
    DamagedEnemies(DamagedEnemiesOverlay),
    AssistableTeammates(AssistableTeammatesOverlay),
}

impl PresentationOverlaySource {
    pub fn kind(&self) -> PresentationOverlayKind {
        match self {
            Self::DamagedEnemies(_) => PresentationOverlayKind::DamagedEnemies,
            Self::AssistableTeammates(_) => PresentationOverlayKind::AssistableTeammates,
        }
    }
}

/// The discriminant of [`PresentationOverlaySource`]. A manifest carries at
/// most one overlay per kind, and each kind owns its own presentation instances.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PresentationOverlayKind {
    DamagedEnemies,
    AssistableTeammates,
}

impl PresentationOverlayKind {
    /// The authored `over.kind` string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DamagedEnemies => "damagedEnemies",
            Self::AssistableTeammates => "assistableTeammates",
        }
    }
}

impl std::fmt::Display for PresentationOverlayKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Marker over every remote teammate whose life state is assistable. It has
/// no authored parameters: the engine decides which teammates are marked and
/// stamps the Bool fact [`ASSISTABLE_TEAMMATES_IN_RANGE_FACT`] on each one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssistableTeammatesOverlay {}

/// Bool fact stamped on each `assistableTeammates` overlay instance: true only
/// for the one teammate a held `assist` would revive right now.
pub const ASSISTABLE_TEAMMATES_IN_RANGE_FACT: &str = "inRange";

/// Keep the first overlay of each kind, in authored order, and warn once for
/// every later repeat, naming its kind.
pub fn first_presentation_overlay_per_kind(
    overlays: Vec<PresentationOverlay>,
    scope: &str,
) -> Vec<PresentationOverlay> {
    let mut seen = std::collections::BTreeSet::new();
    overlays
        .into_iter()
        .filter(|overlay| {
            let kind = overlay.over.kind();
            let first = seen.insert(kind);
            if !first {
                log::warn!(
                    "[Scripting] {scope}: `presentationOverlays` repeats kind `{kind}`; only the first `{kind}` overlay is kept"
                );
            }
            first
        })
        .collect()
}

/// The overlay of `kind` in a manifest's overlay list, if one was declared.
pub fn presentation_overlay_of_kind(
    overlays: &[PresentationOverlay],
    kind: PresentationOverlayKind,
) -> Option<&PresentationOverlay> {
    overlays.iter().find(|overlay| overlay.over.kind() == kind)
}

/// Event-driven enemy status overlay configuration. `shield` carries raw IR
/// expressions; the host binds them against one entity's `@state.*` namespace
/// and computes the safe fraction at the post-tick sample point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DamagedEnemiesOverlay {
    pub linger_ms: u32,
    pub hide_at_full: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shield: Option<DamagedEnemiesShield>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DamagedEnemiesShield {
    pub value: IrNode,
    pub max: IrNode,
}

/// The full bundle returned by a level's `setupLevel(ctx)` export.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LevelManifest {
    pub reactions: Vec<NamedReaction>,
    /// Level-local impact-policy declarations. Task 5 composes these after
    /// mod-global declarations and performs the author-id merge.
    pub events: Vec<ImpactEventDescriptor>,
    /// State-crossing watchers (M13 HUD dynamics). Parsed alongside `reactions`
    /// from the widened `{ reactions, events, crossings, triggerEvents, triggerPools }` setup-manifest return and
    /// drained into the per-level `DataRegistry`; cleared on level unload.
    pub crossings: Vec<CrossingDescriptor>,
    /// Level member trigger events (`t.on`) declared via the `triggerEvents`
    /// field, keyed by volume. Tag-keyed entries are rejected here — they
    /// belong in `ModManifest.triggerEvents`. Bound after the matching
    /// mod-global events on a shared edge; per-level and cleared on unload.
    pub trigger_events: Vec<VolumeTriggerEventDescriptor>,
    pub teleport_events: Vec<TeleportEventDescriptor>,
    /// Trigger-volume pool declarations. Their `levels` selector is retained
    /// for the shared descriptor contract, but level-local pools always apply
    /// to the level that declared them.
    pub trigger_pools: Vec<TriggerPoolDescriptor>,
    /// Level player events (`players().on`) from the `playerEvents` field. An
    /// entry carrying `levels` is skipped with a warning (its siblings install);
    /// it belongs in `ModManifest`.
    pub player_events: Vec<PlayerEventDescriptor>,
    /// Per-level UI trees declared via the `uiTrees` field. A malformed entry is
    /// logged and skipped rather than aborting level load (`ui.md` §1.1).
    pub ui_trees: Vec<RegisteredUiTree>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlay(max_visible: usize) -> PresentationOverlay {
        serde_json::from_value(serde_json::json!({
            "over": {
                "kind": "damagedEnemies",
                "lingerMs": 2500,
                "hideAtFull": true
            },
            "template": "enemyStatus",
            "maxVisible": max_visible
        }))
        .expect("test overlay descriptor is valid")
    }

    #[test]
    fn presentation_overlay_visible_budget_is_bounded() {
        assert!(overlay(1).validate().is_ok());
        assert!(overlay(MAX_PRESENTATION_OVERLAY_VISIBLE).validate().is_ok());
        assert_eq!(
            overlay(MAX_PRESENTATION_OVERLAY_VISIBLE + 1)
                .validate()
                .unwrap_err(),
            "presentation overlay `maxVisible` must be at most 64"
        );
    }

    fn template(root: serde_json::Value, world_anchor: serde_json::Value) -> PresentationTemplate {
        serde_json::from_value(serde_json::json!({
            "id": "marker",
            "root": root,
            "lifetimeMs": 1, "motion": { "rise": 0, "easing": "linear" },
            "fade": { "startMs": 1 }, "spawnScatter": { "radius": 0 },
            "worldAnchor": world_anchor,
        }))
        .expect("test template descriptor is valid")
    }

    fn anchor() -> serde_json::Value {
        serde_json::json!({ "socket": "head", "offsetY": 0 })
    }

    #[test]
    fn presentation_templates_admit_glyphs_at_the_root_and_nested() {
        let glyph = serde_json::json!({ "kind": "glyph", "command": "assist" });
        template(glyph.clone(), anchor()).validate().unwrap();
        template(
            serde_json::json!({ "kind": "vstack", "gap": 2, "padding": 0, "align": "start", "children": [
                { "kind": "hstack", "gap": 4, "padding": 0, "align": "start",
                  "visibleWhen": { "fact": "inRange" }, "children": [glyph] },
            ] }),
            anchor(),
        )
        .validate()
        .unwrap();

        assert_eq!(
            template(
                serde_json::json!({ "kind": "glyph", "command": "" }),
                anchor()
            )
            .validate()
            .unwrap_err(),
            "presentation template `root`.command must be nonempty"
        );
        assert_eq!(
            template(
                serde_json::json!({ "kind": "hstack", "gap": 0, "padding": 0, "align": "start",
                    "children": [{ "kind": "glyph", "command": "" }] }),
                anchor(),
            )
            .validate()
            .unwrap_err(),
            "presentation template `root`.children[0].command must be nonempty"
        );
    }

    #[test]
    fn presentation_templates_still_reject_every_other_unsupported_kind() {
        for (root, message) in [
            (
                serde_json::json!({ "kind": "spacer", "flexGrow": 1 }),
                "presentation template `root`.kind `spacer` is not supported in passive presentation templates",
            ),
            (
                serde_json::json!({ "kind": "button", "id": "b", "label": "x", "onPress": "go" }),
                "presentation template `root`.kind `button` is interactive and is not supported in passive presentation templates",
            ),
        ] {
            let template: Result<PresentationTemplate, _> =
                serde_json::from_value(serde_json::json!({
                    "id": "marker", "root": root, "lifetimeMs": 1,
                    "motion": { "rise": 0, "easing": "linear" }, "fade": { "startMs": 1 },
                    "spawnScatter": { "radius": 0 },
                }));
            match template {
                Ok(template) => assert_eq!(template.validate().unwrap_err(), message),
                Err(error) => panic!("test descriptor must deserialize: {error}"),
            }
        }
    }

    #[test]
    fn assistable_teammates_source_is_a_bare_kind_tag_and_rejects_unknown_keys() {
        let source: PresentationOverlaySource =
            serde_json::from_value(serde_json::json!({ "kind": "assistableTeammates" })).unwrap();
        assert_eq!(source.kind(), PresentationOverlayKind::AssistableTeammates);
        assert_eq!(
            serde_json::to_value(&source).unwrap(),
            serde_json::json!({ "kind": "assistableTeammates" })
        );
        assert!(
            serde_json::from_value::<PresentationOverlaySource>(
                serde_json::json!({ "kind": "assistableTeammates", "radius": 2 })
            )
            .is_err()
        );
    }

    #[test]
    fn world_anchor_socket_is_optional_and_omitted_when_absent() {
        let socketless = template(
            serde_json::json!({ "kind": "glyph", "command": "assist" }),
            serde_json::json!({ "offsetY": 0.6 }),
        );
        socketless.validate().unwrap();
        let anchor = socketless.world_anchor.as_ref().unwrap();
        assert_eq!(anchor.socket, None);
        assert_eq!(
            serde_json::to_value(anchor).unwrap(),
            serde_json::json!({ "offsetY": 0.6 })
        );
    }

    #[test]
    fn world_anchor_rejects_an_explicit_empty_socket() {
        let named = template(
            serde_json::json!({ "kind": "glyph", "command": "assist" }),
            serde_json::json!({ "socket": "head", "offsetY": 0.6 }),
        );
        named.validate().unwrap();
        assert_eq!(
            named.world_anchor.as_ref().unwrap().socket.as_deref(),
            Some("head")
        );
        let empty = template(
            serde_json::json!({ "kind": "glyph", "command": "assist" }),
            serde_json::json!({ "socket": "", "offsetY": 0.6 }),
        );
        assert_eq!(
            empty.validate().unwrap_err(),
            "presentation template `worldAnchor.socket` must be nonempty"
        );
    }
}
