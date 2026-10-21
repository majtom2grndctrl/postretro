//! Quake translation of the `_reflection_capture` key into the canonical
//! per-brush reflection-probe capture override.
//! See: context/lib/build_pipeline.md §Source-format neutrality · resource_management.md §5.2

use std::collections::HashMap;

use shambler::brush::BrushId;

use crate::format::quake_map;
use crate::map_data::{BrushVolume, ReflectionCapture};

pub(super) const REFLECTION_CAPTURE_KEY: &str = "_reflection_capture";

/// Every value the key accepts, in the FGD's order.
pub(super) const REFLECTION_CAPTURE_VALUES: [ReflectionCapture; 3] = [
    ReflectionCapture::Auto,
    ReflectionCapture::Always,
    ReflectionCapture::Never,
];

/// The key's spelling of each value. Exhaustive, so a new variant cannot
/// ship without one.
pub(super) fn reflection_capture_spelling(value: ReflectionCapture) -> &'static str {
    match value {
        ReflectionCapture::Auto => "auto",
        ReflectionCapture::Always => "always",
        ReflectionCapture::Never => "never",
    }
}

/// Authored override values keyed by source brush id. Keyed by id rather
/// than position so a degenerate brush the hull builder drops shifts no value
/// onto the next brush.
#[derive(Debug, Default)]
pub(super) struct ReflectionCaptureOverrides {
    by_brush_id: HashMap<BrushId, ReflectionCapture>,
}

impl ReflectionCaptureOverrides {
    /// A `func_group`: an authored brush entity carries the key for every
    /// brush; a TrenchBroom group or layer does not, since TrenchBroom drops
    /// the key on save, so an authored key value is ignored with a warning.
    pub(super) fn record_func_group(
        &mut self,
        props: &HashMap<String, String>,
        entity: &str,
        brush_ids: &[BrushId],
    ) {
        if !quake_map::is_trenchbroom_container(props.get("_tb_type").map(String::as_str)) {
            self.record_brush_entity(props, entity, brush_ids);
            return;
        }
        if let Some(raw) = authored_value(props) {
            log::warn!(
                "[Compiler] {entity} is a TrenchBroom group or layer, which keeps no custom keys \
                 on save; ignoring `{REFLECTION_CAPTURE_KEY}` `{raw}`. To override reflection \
                 capture, select the brushes and use Create Brush Entity > func_group"
            );
        }
    }

    /// An authored brush entity (`switch`, or `func_group` without a
    /// non-blank `_tb_type`): every brush takes the entity's value. Warns once per
    /// entity on a bad value, which acts as `auto`.
    pub(super) fn record_brush_entity(
        &mut self,
        props: &HashMap<String, String>,
        entity: &str,
        brush_ids: &[BrushId],
    ) {
        let Some(raw) = authored_value(props) else {
            return;
        };
        let value = REFLECTION_CAPTURE_VALUES
            .into_iter()
            .find(|&value| reflection_capture_spelling(value) == raw);
        let value = match value {
            Some(value) => value,
            None => {
                log::warn!(
                    "[Compiler] {entity} `{REFLECTION_CAPTURE_KEY}` value `{raw}` is not `auto`, \
                     `always` or `never`; treating it as `auto`"
                );
                ReflectionCapture::Auto
            }
        };
        if value == ReflectionCapture::Auto {
            return;
        }
        for brush_id in brush_ids {
            self.by_brush_id.insert(*brush_id, value);
        }
    }

    /// The override of each hull that survived parsing, index-aligned with
    /// them. Unlisted brushes, worldspawn's included, are `Auto`.
    pub(super) fn for_retained_hulls(
        &self,
        retained_hulls: &[(BrushId, BrushVolume)],
    ) -> Vec<ReflectionCapture> {
        retained_hulls
            .iter()
            .map(|(brush_id, _)| self.by_brush_id.get(brush_id).copied().unwrap_or_default())
            .collect()
    }
}

/// The trimmed value, or `None` when absent or blank: a field cleared in
/// TrenchBroom arrives as `""` and means unset.
fn authored_value(props: &HashMap<String, String>) -> Option<&str> {
    props
        .get(REFLECTION_CAPTURE_KEY)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
}
