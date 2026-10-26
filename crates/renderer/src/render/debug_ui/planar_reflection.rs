// Diagnostics-panel planar reflection tuning: live overrides of the active tier's preset.
// See: context/lib/rendering_pipeline.md §7.3a

use super::Renderer;

/// The "Planar reflection" section. The scale, D and R controls override the
/// active tier's engine-constant preset until the next tier change, which
/// resets them to that tier's preset; they re-read the renderer every draw, so
/// they always show what the next frame draws by. "Planar off" routes the
/// whole selected plane to SSR (plane identity, never per pixel); "Crop"
/// switches the mirror walk's crop to the selected plane. Never player-facing.
pub(super) fn draw(ui: &mut egui::Ui, renderer: &mut Renderer) {
    egui::CollapsingHeader::new("Planar reflection").show(ui, |ui| {
        let mut off = renderer.planar_dev_off();
        if ui
            .checkbox(&mut off, "Planar off (the selected plane reads SSR)")
            .changed()
        {
            renderer.set_planar_dev_off(off);
        }
        let mut crop = renderer.planar_dev_crop();
        if ui
            .checkbox(&mut crop, "Crop the mirror walk to the plane")
            .changed()
        {
            renderer.set_planar_dev_crop(crop);
        }
        let Some(mut preset) = renderer.planar_tuned_preset() else {
            ui.label("Planar runs at reflection quality very high and ultra only.");
            return;
        };
        let before = preset;
        ui.horizontal(|ui| {
            ui.label("Resolution");
            for (scale, label) in [(0.25, "¼"), (0.5, "½"), (1.0, "1")] {
                ui.selectable_value(&mut preset.scale, scale, label);
            }
        });
        distance(ui, "Mirror reach D", &mut preset.d);
        ui.add(egui::Slider::new(&mut preset.band_d, 0.1..=16.0).text("D fade metres"));
        distance(ui, "Water reach R", &mut preset.r);
        ui.add(egui::Slider::new(&mut preset.band_r, 0.1..=16.0).text("R fade metres"));
        if ui.button("Reset to the tier's preset").clicked() {
            renderer.set_planar_preset_override(None);
        } else if preset != before {
            renderer.set_planar_preset_override(Some(preset));
        }
    });
}

/// Where a distance limit starts when switched on: mid-slider and past a
/// room's depth, so turning a limit on never blanks the near reflection.
const SWITCHED_ON_LIMIT_METRES: f32 = 30.0;

/// A distance limit's on/off checkbox and its metres slider.
fn distance(ui: &mut egui::Ui, label: &str, limit: &mut Option<f32>) {
    let mut enabled = limit.is_some();
    ui.checkbox(&mut enabled, label);
    if enabled {
        let value = limit.get_or_insert(SWITCHED_ON_LIMIT_METRES);
        ui.add(egui::Slider::new(value, 1.0..=100.0).suffix(" m"));
    } else {
        *limit = None;
    }
}
