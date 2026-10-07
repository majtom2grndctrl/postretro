// Glyphs: the mod's art loads into the UI image registry, and each frame's
// snapshot resolves every `glyph` widget into the image or text it draws.
// See: context/lib/ui.md §4 · context/lib/input.md §2

use std::collections::HashSet;
use std::path::{Component, Path};

use postretro_ui::descriptor::{
    ColorValue, ImageWidget, SpacerWidget, TextWidget, Widget,
};

use crate::input::{
    Command, DeviceFamily, EffectiveTable, GlyphView, glyph_key, resolve_glyph,
};
use crate::*;

/// The glyph art the renderer holds: which directories loaded, and the image
/// keys they produced.
#[derive(Debug, Default)]
pub(crate) struct GlyphArtState {
    loaded_dirs: Option<Vec<String>>,
    keys: HashSet<String>,
}

/// The directories the author's `input.glyphs` names, in family order.
fn declared_dirs(glyphs: &crate::input::GlyphDirs) -> Vec<String> {
    [
        &glyphs.keyboard_mouse,
        &glyphs.xbox,
        &glyphs.playstation,
        &glyphs.nintendo,
    ]
    .into_iter()
    .flatten()
    .cloned()
    .collect()
}

/// A directory inside the mod root: relative, with no `..`.
fn is_mod_relative(dir: &str) -> bool {
    Path::new(dir)
        .components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

/// Decode every PNG in `<mod_root>/<dir>`, as `(key, rgba, width, height)`.
fn read_glyph_dir(mod_root: &Path, dir: &str) -> Vec<(String, Vec<u8>, u32, u32)> {
    if !is_mod_relative(dir) {
        log::warn!("[UI] input.glyphs directory `{dir}` must stay inside the mod; skipping it");
        return Vec::new();
    }
    let path = mod_root.join(dir);
    let Ok(entries) = std::fs::read_dir(&path) else {
        log::warn!(
            "[UI] input.glyphs directory `{dir}` is not readable at {}; glyphs draw input names",
            path.display()
        );
        return Vec::new();
    };
    let mut images = Vec::new();
    for entry in entries.flatten() {
        let file = entry.path();
        if file.extension().and_then(|ext| ext.to_str()) != Some("png") {
            continue;
        }
        let Some(stem) = file.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        match image::open(&file) {
            Ok(decoded) => {
                let rgba = decoded.to_rgba8();
                let (width, height) = rgba.dimensions();
                images.push((glyph_key(dir, stem), rgba.into_raw(), width, height));
            }
            Err(err) => log::warn!("[UI] glyph art {} did not decode: {err}", file.display()),
        }
    }
    images
}

impl App {
    /// Load the mod's glyph art once the renderer can take it, and again when
    /// the declared directories change (a staged reload). Cheap otherwise.
    pub(crate) fn sync_glyph_art(&mut self) {
        let (Some(session), Some(renderer)) = (self.session.as_mut(), self.renderer.as_mut())
        else {
            return;
        };
        if !renderer.is_full_ready() {
            return;
        }
        let dirs = declared_dirs(&session.bindings.author().glyphs);
        if session.glyph_art.loaded_dirs.as_ref() == Some(&dirs) {
            return;
        }
        let mut keys = HashSet::new();
        for dir in &dirs {
            for (key, rgba, width, height) in read_glyph_dir(&self.content_root, dir) {
                renderer.register_ui_image(&key, rgba, width, height);
                keys.insert(key);
            }
        }
        if !dirs.is_empty() {
            log::info!("[UI] loaded {} glyph image(s) from {} directories", keys.len(), dirs.len());
        }
        session.glyph_art = GlyphArtState {
            loaded_dirs: Some(dirs),
            keys,
        };
    }
}

/// What one glyph widget draws this frame. An unknown command ID draws
/// nothing.
fn resolved_widget(
    glyph: &postretro_ui::descriptor::GlyphWidget,
    table: &EffectiveTable,
    glyphs: &crate::input::GlyphDirs,
    family: DeviceFamily,
    art: &HashSet<String>,
) -> Widget {
    let view = Command::from_id(&glyph.command)
        .and_then(|command| resolve_glyph(table, glyphs, family, command, |key| art.contains(key)));
    match view {
        Some(GlyphView::Art(asset)) => Widget::Image(ImageWidget {
            asset,
            id: glyph.id.clone(),
            focus_neighbors: Default::default(),
            label: None,
            decorative: true,
            visible_when: glyph.visible_when.clone(),
            role: None,
        }),
        Some(GlyphView::Label(content)) => Widget::Text(TextWidget {
            content,
            font_size: 16.0,
            color: ColorValue::Token("ok".to_string()),
            id: glyph.id.clone(),
            focus_neighbors: Default::default(),
            font: Some("mono".to_string()),
            bind: None,
            style_ranges: None,
            visible_when: glyph.visible_when.clone(),
            role: None,
        }),
        None => Widget::Spacer(SpacerWidget {
            flex_grow: 0.0,
            id: glyph.id.clone(),
            visible_when: glyph.visible_when.clone(),
            role: None,
        }),
    }
}

fn resolve_in(
    widget: &mut Widget,
    table: &EffectiveTable,
    glyphs: &crate::input::GlyphDirs,
    family: DeviceFamily,
    art: &HashSet<String>,
) {
    match widget {
        Widget::Glyph(glyph) => *widget = resolved_widget(glyph, table, glyphs, family, art),
        Widget::VStack(container) | Widget::HStack(container) => {
            for child in &mut container.children {
                resolve_in(child, table, glyphs, family, art);
            }
        }
        Widget::Grid(grid) => {
            for child in &mut grid.children {
                resolve_in(child, table, glyphs, family, art);
            }
        }
        _ => {}
    }
}

/// Replace every `glyph` widget in the snapshot's trees with what it draws on
/// the current family. A changed glyph changes the descriptor, so the retained
/// layer rebuilds on the frame after a rebinding or a family switch.
pub(crate) fn resolve_snapshot_glyphs(
    snapshot: &mut postretro_ui::UiReadSnapshot,
    session: &crate::session::Session,
) {
    let table = session.bindings.table();
    let glyphs = &session.bindings.author().glyphs;
    let family = session.device_family.current();
    for entry in &mut snapshot.trees {
        resolve_in(
            &mut entry.descriptor.root,
            table,
            glyphs,
            family,
            &session.glyph_art.keys,
        );
    }
}

#[cfg(test)]
#[path = "glyph_art_tests.rs"]
mod tests;
