// Planar renderer API: the frame's mirror view and the planar layer's lifetime hooks.
// See: context/lib/rendering_pipeline.md §7.3a

use super::*;

impl Renderer {
    /// This frame's mirror view of the selected plane, or `None` with no
    /// plane selected, and the mirror's visible cells. Call before
    /// `update_per_frame_uniforms`. The frame's tier decides whether it
    /// draws: below very high, or without a planar layer, it records nothing
    /// and every reflecting draw reads SSR at the tier.
    pub fn set_planar_reflection_view(
        &mut self,
        camera: Option<MirrorCamera>,
        visible: &VisibleCells,
    ) {
        let pass = &mut self.full_mut().planar_reflection;
        pass.camera = camera;
        match (&mut pass.visible, visible) {
            (VisibleCells::Culled(out), VisibleCells::Culled(cells)) => out.clone_from(cells),
            (out, VisibleCells::Culled(cells)) => *out = VisibleCells::Culled(cells.clone()),
            (out, VisibleCells::DrawAll) => *out = VisibleCells::DrawAll,
        }
    }

    /// The planar preset the next frame draws by: `None` below very high.
    pub fn planar_reflection_preset(&self) -> Option<PlanarPreset> {
        self.full
            .as_ref()
            .and_then(|full| full.planar_reflection.preset(self.reflection_quality))
    }

    /// Match the planar layer and the mirror targets to the kept tier and the
    /// installed level: allocated at the preset's scale of the scene extent
    /// while a level with a candidate plane is installed at a planar tier,
    /// released otherwise. The extent they already have builds nothing.
    pub(in crate::render) fn sync_planar_layer(&mut self) {
        self.apply_planar_layer(false);
    }

    /// Follow recreated scene targets: the planar layer, its group 5 and the
    /// mirror targets rebuild over the new scene depth at the new extent.
    pub(in crate::render) fn rebuild_planar_layer(&mut self) {
        self.apply_planar_layer(true);
    }

    fn apply_planar_layer(&mut self, rebuild: bool) {
        let quality = self.reflection_quality;
        let Self { device, full, .. } = self;
        let Some(full) = full.as_mut() else {
            return;
        };
        let scene = full.depth_view.texture().size();
        let wanted = full
            .planar_reflection
            .preset(quality)
            .filter(|_| full.fluid_surface.has_planar_candidate())
            .map(|preset| preset.target_extent((scene.width, scene.height)));
        full.fluid_surface.set_planar_layer(device, wanted, rebuild);
        let pass = &mut full.planar_reflection;
        if !rebuild && pass.targets.as_ref().map(|targets| targets.extent) == wanted {
            return;
        }
        pass.targets = None;
        if let Some(extent) = wanted {
            pass.targets = Some(PlanarTargets::new(
                device,
                &pass.pipelines,
                &pass.inputs_buffer,
                &full.depth_view,
                extent,
            ));
        }
        pass.uploaded_inputs = None;
        pass.uploaded_camera = None;
    }

    /// Replace the tier's preset with `preset` until cleared, for the D, R
    /// and scale proofs; the layer follows a scale change at once.
    #[cfg(test)]
    pub(crate) fn set_planar_preset_override(&mut self, preset: Option<PlanarPreset>) {
        self.full_mut().planar_reflection.preset_override = preset;
        self.sync_planar_layer();
    }

    /// Frames that wrote the planar layer since full init.
    #[cfg(test)]
    pub(crate) fn planar_reflection_records(&self) -> u32 {
        self.full().planar_reflection.records
    }

    /// The mirror targets' extent; `None` without them.
    #[cfg(test)]
    pub(crate) fn planar_mirror_extent(&self) -> Option<(u32, u32)> {
        self.full()
            .planar_reflection
            .targets
            .as_ref()
            .map(|targets| targets.extent)
    }
}
