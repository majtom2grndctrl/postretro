// Presentation templates' `glyph` widgets: resolved app-side by the same code
// HUD trees use, and re-sent to the renderer only when what they draw could
// change (bindings, device family, loaded art, or the templates themselves).
// See: context/lib/ui.md §1 (presentation layer), §4

use postretro_scripting_core::data_descriptors::PresentationTemplate;
use postretro_ui::descriptor::Widget;

use super::resolve_in;
use crate::App;
use crate::input::DeviceFamily;

/// Everything a resolved template set depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GlyphInputs {
    bindings: u64,
    family: DeviceFamily,
    art: u64,
    templates: u64,
}

/// The committed manifest's raw presentation templates, kept so their glyphs
/// can be resolved again without a reload.
#[derive(Debug, Default)]
pub(crate) struct TemplateGlyphs {
    raw: Vec<PresentationTemplate>,
    /// Counts installs; 0 means nothing was installed through this state.
    generation: u64,
    /// Any template holds a glyph. Without one, only an install re-sends.
    has_glyphs: bool,
    resolved_for: Option<GlyphInputs>,
}

impl TemplateGlyphs {
    fn install(&mut self, templates: Vec<PresentationTemplate>) {
        self.has_glyphs = templates
            .iter()
            .any(|template| contains_glyph(&template.root));
        self.raw = templates;
        self.generation += 1;
        self.resolved_for = None;
    }

    /// Send the installed set again once a renderer can take it: the one that
    /// held it is gone, or the art keys it resolved against were re-registered.
    pub(super) fn resend(&mut self) {
        self.resolved_for = None;
    }
}

fn contains_glyph(widget: &Widget) -> bool {
    match widget {
        Widget::Glyph(_) => true,
        Widget::VStack(container) | Widget::HStack(container) => {
            container.children.iter().any(contains_glyph)
        }
        Widget::Grid(grid) => grid.children.iter().any(contains_glyph),
        _ => false,
    }
}

/// The installed templates with every glyph resolved for the current inputs,
/// when they were never sent or something they depend on changed since; else
/// `None`. Records the inputs, so the caller must send what it returns.
pub(crate) fn stale_resolved_templates(
    session: &mut crate::session::Session,
) -> Option<Vec<PresentationTemplate>> {
    let art = &session.glyph_art;
    let templates = &art.templates;
    if templates.generation == 0 {
        return None;
    }
    let inputs = GlyphInputs {
        bindings: session.bindings.generation(),
        family: session.device_family.current(),
        art: art.art_generation,
        templates: templates.generation,
    };
    let stale = templates
        .resolved_for
        .is_none_or(|resolved| templates.has_glyphs && resolved != inputs);
    if !stale {
        return None;
    }
    let table = session.bindings.table();
    let glyphs = &session.bindings.author().glyphs;
    let mut resolved = templates.raw.clone();
    for template in &mut resolved {
        resolve_in(&mut template.root, table, glyphs, inputs.family, art);
    }
    session.glyph_art.templates.resolved_for = Some(inputs);
    Some(resolved)
}

impl App {
    /// Install a committed manifest's presentation templates: keep the raw
    /// set for later glyph re-resolution and send the resolved set to the
    /// renderer.
    pub(crate) fn install_presentation_templates(&mut self, templates: Vec<PresentationTemplate>) {
        let Some(session) = self.session.as_mut() else {
            if let Some(renderer) = self.renderer.as_mut() {
                renderer.set_presentation_templates(templates);
            }
            return;
        };
        session.glyph_art.templates.install(templates);
        self.refresh_template_glyphs();
    }

    /// Re-send the presentation templates when a glyph in them would now draw
    /// something else. A re-send rebuilds live instances' layouts.
    pub(crate) fn refresh_template_glyphs(&mut self) {
        let (Some(session), Some(renderer)) = (self.session.as_mut(), self.renderer.as_mut())
        else {
            return;
        };
        if !renderer.is_full_ready() {
            return;
        }
        if let Some(resolved) = stale_resolved_templates(session) {
            renderer.set_presentation_templates(resolved);
        }
    }
}

#[cfg(test)]
#[path = "template_glyphs_tests.rs"]
mod tests;
