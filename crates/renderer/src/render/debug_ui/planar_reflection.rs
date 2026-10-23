use super::Renderer;

pub(super) fn draw(ui: &mut egui::Ui, renderer: &mut Renderer) {
    egui::CollapsingHeader::new("Planar water spike").show(ui, |ui| {
        let mut settings = renderer.planar_reflection_settings();
        let before = settings;
        ui.checkbox(&mut settings.on, "Planar reflections");
        ui.horizontal(|ui| {
            ui.label("Resolution");
            for (scale, label) in [(0.25, "¼"), (0.5, "½"), (1.0, "1")] {
                ui.selectable_value(&mut settings.scale, scale, label);
            }
        });
        distance(ui, "Mirror draw distance D", &mut settings.d);
        ui.add(egui::Slider::new(&mut settings.band_d, 0.1..=16.0).text("D fade metres"));
        distance(ui, "Water reach R", &mut settings.r);
        ui.add(egui::Slider::new(&mut settings.band_r, 0.1..=16.0).text("R fade metres"));
        if settings != before {
            renderer.set_planar_reflection_settings(settings);
        }
    });
}

fn distance(ui: &mut egui::Ui, label: &str, limit: &mut Option<f32>) {
    let mut enabled = limit.is_some();
    ui.checkbox(&mut enabled, label);
    if enabled {
        let value = limit.get_or_insert(30.0);
        ui.add(egui::Slider::new(value, 1.0..=100.0).suffix(" m"));
    } else {
        *limit = None;
    }
}
