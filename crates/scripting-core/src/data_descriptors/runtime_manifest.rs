// Runtime-side manifest types that embed render::ui descriptor data.
// See: context/lib/scripting.md §13 (Crate Architecture)

use postretro_foundation::{IrNode, PresentationEasing};
use serde::{Deserialize, Serialize};

use crate::ui::descriptor::{
    AnchoredTree, BarMax, BindSource, ColorValue, Predicate, SpacingValue, TextTween, Widget,
};
use crate::ui::style_ranges::StyleRanges;

use super::{
    CrossingDescriptor, ImpactEventDescriptor, NamedReaction, TriggerEventDescriptor,
    TriggerPoolDescriptor,
};

/// A script-registered UI tree: a named [`AnchoredTree`] plus the `alwaysOn`
/// registration attribute. Drained from `ModManifest.uiTrees` (mod scope) and
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
    /// Overlay-only anchor metadata. Spawn presentation continues to use the
    /// impact target's transform directly, so omitting this remains valid for
    /// a number/toast template.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world_anchor: Option<PresentationWorldAnchor>,
}

impl PresentationTemplate {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty() {
            return Err("presentation template `id` must be nonempty".to_string());
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
/// resolving every such read as absent.
pub(crate) fn validate_retained_widget_sources(widget: &Widget) -> Result<(), String> {
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
    let predicate = |predicate: &Option<Predicate>, field: &str| match predicate {
        Some(predicate) => source(&predicate.source, field),
        None => Ok(()),
    };

    match widget {
        Widget::Text(text) => {
            if let Some(bind) = &text.bind {
                source(&bind.source, "bind")?;
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
        Widget::Button(button) => {
            predicate(&button.selected, "selected")?;
            predicate(&button.checked, "checked")?;
            predicate(&button.bind, "bind")?;
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
    if let Some(from) = tween.from {
        if !from.is_finite() {
            return Err(format!("{field}.from must be a finite f32"));
        }
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
        if let Some(up_to) = entry.up_to {
            if !up_to.is_finite() {
                return Err(format!("{entry_path}.upTo must be a finite f32"));
            }
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
/// named socket (or a matching hit-zone tag) from CPU-side hit-zone data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PresentationWorldAnchor {
    pub socket: String,
    pub offset_y: f32,
}

impl PresentationWorldAnchor {
    fn validate(&self) -> Result<(), String> {
        if self.socket.is_empty() {
            return Err("presentation template `worldAnchor.socket` must be nonempty".to_string());
        }
        if !self.offset_y.is_finite() {
            return Err("presentation template `worldAnchor.offsetY` must be finite".to_string());
        }
        Ok(())
    }
}

/// The fact-driven passive overlay declaration from
/// `ModManifest.presentationOverlays`. The manifest field accepts one of these,
/// not an array. It is host-local authoring data, never a transport payload.
pub const MAX_PRESENTATION_OVERLAY_VISIBLE: usize = 64;

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
    /// Trigger-volume enter/exit watchers declared via the `triggerEvents`
    /// field. Composes with mod-global `ModManifest.triggerEvents` entries
    /// matched by the `levels` tag selector; per-level and cleared on unload.
    pub trigger_events: Vec<TriggerEventDescriptor>,
    /// Trigger-volume pool declarations. Their `levels` selector is retained
    /// for the shared descriptor contract, but level-local pools always apply
    /// to the level that declared them.
    pub trigger_pools: Vec<TriggerPoolDescriptor>,
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
}
