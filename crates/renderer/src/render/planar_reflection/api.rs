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

    /// This frame's mirror billboards and particles, one `(collection, packed
    /// bytes)` region per collection, collected over the mirror's cells and
    /// cull frustum; empty with no mirror view. They upload into the mirror's
    /// own sprite store, apart from the billboard pass's, and draw facing the
    /// mirror camera on a frame the planar pass records.
    pub fn set_planar_particle_draws<'a>(
        &mut self,
        collections: impl IntoIterator<Item = (&'a str, &'a [u8])>,
    ) {
        let Self {
            device,
            queue,
            full,
            ..
        } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        full.planar_reflection.sprites.upload(
            device,
            queue,
            full.smoke_pass.instance_bind_group_layout(),
            collections,
        );
    }

    /// The planar preset the next frame draws by: `None` below very high,
    /// and with dev-tools while the panel's planar-off switch holds, so the
    /// app selects no plane and SSR covers that water.
    pub fn planar_reflection_preset(&self) -> Option<PlanarPreset> {
        let full = self.full.as_ref()?;
        #[cfg(feature = "dev-tools")]
        if full.planar_reflection.dev.off {
            return None;
        }
        full.planar_reflection.preset(self.reflection_quality)
    }

    /// The app's verdict on whether the installed level holds a candidate
    /// plane (`PlanarCandidates::from_volumes` over the volumes whose fluid
    /// reflects): the planar layer and mirror targets exist only while it
    /// holds, at a planar tier. Call at every level install, before the
    /// fluid install, and at every fluid refresh; a refresh that removes the
    /// last candidate releases them. Without a verdict (capture, renderer
    /// tests) the same rule over every volume decides. Compiles nothing;
    /// never per frame.
    pub fn set_planar_candidate(&mut self, has_candidate: bool) {
        let Some(full) = self.full.as_mut() else {
            return;
        };
        full.fluid_surface.set_planar_app_candidate(has_candidate);
        self.sync_planar_layer();
    }

    /// Match the planar layer and the mirror targets to the kept tier and the
    /// installed level: allocated at the preset's scale of the scene extent
    /// while a level with a candidate plane is installed at a planar tier,
    /// released otherwise. The extent they already have builds nothing.
    ///
    /// A scene extent recorded but not yet committed defers a build to the
    /// next commit, so a tier change or install on a resize frame allocates
    /// once, at the extent that commit settles on. A release never waits.
    pub(in crate::render) fn sync_planar_layer(&mut self) {
        self.apply_planar_layer(false);
    }

    /// Run a layer build an earlier sync deferred to this extent commit. A
    /// commit that moved the scene already rebuilt the layer and cleared the
    /// deferral; this covers the commit that moved nothing, or only the
    /// surface, after the recorded size returned to the committed scene.
    /// Every commit calls it; with nothing deferred it builds nothing.
    pub(in crate::render) fn resume_deferred_planar_build(&mut self) {
        if self
            .full
            .as_ref()
            .is_some_and(|full| full.planar_reflection.build_deferred)
        {
            self.sync_planar_layer();
        }
    }

    /// Follow recreated scene targets: the planar layer, its group 5 and the
    /// mirror targets rebuild over the new scene depth at the new extent.
    pub(in crate::render) fn rebuild_planar_layer(&mut self) {
        self.apply_planar_layer(true);
    }

    fn apply_planar_layer(&mut self, rebuild: bool) {
        #[cfg(any(test, feature = "dev-tools", feature = "capture"))]
        self.follow_planar_tier();
        let quality = self.reflection_quality;
        let commit_pending = self.extent_state.scene_change_pending();
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
        let defer = !rebuild && wanted.is_some() && commit_pending;
        full.planar_reflection.build_deferred = defer;
        if defer {
            return;
        }
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
            #[cfg(test)]
            {
                pass.target_builds += 1;
            }
        }
        pass.uploaded_inputs = None;
        pass.uploaded_camera = None;
    }

    /// Force the resolve's displayed offset to zero (or restore it), so a
    /// proof can read a frame untilted at the same pose and clock. Reaches
    /// the inputs uniform on the next planar frame, through its
    /// change-driven upload.
    #[cfg(test)]
    pub(crate) fn set_planar_zero_offset(&mut self, zero: bool) {
        self.full_mut().planar_reflection.zero_offset = zero;
    }

    /// Frames that wrote the planar layer since full init.
    #[cfg(test)]
    pub(crate) fn planar_reflection_records(&self) -> u32 {
        self.full().planar_reflection.records
    }

    /// The mirror depth texture; `None` without the mirror targets.
    #[cfg(test)]
    pub(crate) fn planar_mirror_depth_texture(&self) -> Option<wgpu::Texture> {
        self.full()
            .planar_reflection
            .targets
            .as_ref()
            .map(|targets| targets.depth.texture().clone())
    }

    /// Mirror target builds since full init.
    #[cfg(test)]
    pub(crate) fn planar_mirror_target_builds(&self) -> u32 {
        self.full().planar_reflection.target_builds
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
