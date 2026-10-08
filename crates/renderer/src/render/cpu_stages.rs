// The renderer's CPU stage set: SH drain, per-pass recording, submit, debug UI.
// See: context/lib/rendering_pipeline.md §12

use postretro_stage_timing::{StageKind, StageSet};

/// CPU spent by the renderer while recording a frame. Roots sit under the
/// binary's render stage; `render_record` holds one substage per pass. Surface
/// acquire is not a stage here: the binary counts it as wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderStage {
    /// Loader→renderer SH residency admission.
    ShDrain,
    /// Every scene, overlay, UI and resolve pass the frame records.
    Record,
    /// Mesh-frame planning and the selected-static promotion gate.
    MeshPlan,
    /// Dynamic light slot assignment and shadow debug.
    LightSlots,
    /// Streamed SH compose preparation and pre-scene compute (cull,
    /// animated lightmap, SH and direct SH compose).
    PreScene,
    /// Streamed SH compose planning for this frame's sample regions.
    ShComposePrep,
    /// Candidate gather and BVH cull dispatch, diagnostics included.
    Cull,
    /// CPU leaf walks that only feed cull diagnostics (submitted-leaf counts,
    /// the dev-tools tree-walk estimate), not the GPU cull itself.
    CullDiagnostics,
    AnimatedLightmapCompose,
    /// Indirect SH compose dispatch.
    ShCompose,
    /// Direct SH and billboard-scatter compose dispatch.
    DirectShCompose,
    /// Skinned-mesh pose sampling and palette/instance upload.
    MeshUpload,
    /// CPU inside `sample_instance`, summed over every resampled instance.
    MeshPoseSampling,
    /// Resampled instances this frame.
    MeshPoseSamples,
    /// Spot and cube shadow depth.
    ShadowDepth,
    /// CPU BVH walks that find each shadow region's world reach.
    ShadowReach,
    /// Depth pre-pass and SDF shadow.
    DepthSdf,
    Forward,
    KinematicBrush,
    SkinnedMesh,
    Smoke,
    Fog,
    Bloom,
    /// Wireframe overlay and debug lines.
    Overlay,
    Viewmodel,
    Ui,
    /// Scene-color resolve into the swapchain and the timing-query resolve.
    Resolve,
    /// Queue submit and readback bookkeeping.
    Submit,
    /// The egui overlay's separate submit (`dev-tools`).
    DebugUi,
}

impl StageSet for RenderStage {
    const ALL: &'static [Self] = &[
        Self::ShDrain,
        Self::Record,
        Self::MeshPlan,
        Self::LightSlots,
        Self::PreScene,
        Self::ShComposePrep,
        Self::Cull,
        Self::CullDiagnostics,
        Self::AnimatedLightmapCompose,
        Self::ShCompose,
        Self::DirectShCompose,
        Self::MeshUpload,
        Self::MeshPoseSampling,
        Self::MeshPoseSamples,
        Self::ShadowDepth,
        Self::ShadowReach,
        Self::DepthSdf,
        Self::Forward,
        Self::KinematicBrush,
        Self::SkinnedMesh,
        Self::Smoke,
        Self::Fog,
        Self::Bloom,
        Self::Overlay,
        Self::Viewmodel,
        Self::Ui,
        Self::Resolve,
        Self::Submit,
        Self::DebugUi,
    ];

    fn index(self) -> usize {
        self as usize
    }

    fn label(self) -> &'static str {
        match self {
            Self::ShDrain => "render_sh_drain",
            Self::Record => "render_record",
            Self::MeshPlan => "rec_mesh_plan",
            Self::LightSlots => "rec_light_slots",
            Self::PreScene => "rec_pre_scene",
            Self::ShComposePrep => "rec_sh_compose_prep",
            Self::Cull => "rec_cull",
            Self::CullDiagnostics => "rec_cull_diagnostics",
            Self::AnimatedLightmapCompose => "rec_animated_lm",
            Self::ShCompose => "rec_sh_compose",
            Self::DirectShCompose => "rec_direct_sh",
            Self::MeshUpload => "rec_mesh_upload",
            Self::MeshPoseSampling => "mesh_pose_sampling",
            Self::MeshPoseSamples => "mesh_pose_samples",
            Self::ShadowDepth => "rec_shadow_depth",
            Self::ShadowReach => "rec_shadow_reach",
            Self::DepthSdf => "rec_depth_sdf",
            Self::Forward => "rec_forward",
            Self::KinematicBrush => "rec_kinematic_brush",
            Self::SkinnedMesh => "rec_skinned_mesh",
            Self::Smoke => "rec_smoke",
            Self::Fog => "rec_fog",
            Self::Bloom => "rec_bloom",
            Self::Overlay => "rec_overlay",
            Self::Viewmodel => "rec_viewmodel",
            Self::Ui => "rec_ui",
            Self::Resolve => "rec_resolve",
            Self::Submit => "render_submit",
            Self::DebugUi => "render_debug_ui",
        }
    }

    fn parent(self) -> Option<Self> {
        match self {
            Self::ShDrain | Self::Record | Self::Submit | Self::DebugUi => None,
            Self::MeshPoseSampling => Some(Self::MeshUpload),
            Self::MeshPoseSamples => Some(Self::MeshPoseSampling),
            Self::ShComposePrep
            | Self::Cull
            | Self::AnimatedLightmapCompose
            | Self::ShCompose
            | Self::DirectShCompose => Some(Self::PreScene),
            Self::CullDiagnostics => Some(Self::Cull),
            Self::ShadowReach => Some(Self::ShadowDepth),
            _ => Some(Self::Record),
        }
    }

    fn kind(self) -> StageKind {
        match self {
            Self::MeshPoseSamples => StageKind::Count,
            _ => StageKind::Time,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mesh_pose_sampling_nests_under_render_recording() {
        assert_eq!(
            RenderStage::MeshPoseSampling.parent(),
            Some(RenderStage::MeshUpload)
        );
        assert_eq!(RenderStage::MeshUpload.parent(), Some(RenderStage::Record));
        assert_eq!(RenderStage::Record.parent(), None);
    }

    #[test]
    fn mesh_pose_sampling_no_longer_reads_the_gpu_timing_gate() {
        let source = include_str!("mesh_pass.rs");
        assert!(!source.contains("POSTRETRO_GPU_TIMING"));
        assert!(source.contains("RenderStage::MeshPoseSampling"));
    }

    #[test]
    fn every_stage_index_matches_its_position() {
        for (position, &stage) in RenderStage::ALL.iter().enumerate() {
            assert_eq!(stage.index(), position, "{stage:?}");
        }
    }
}
