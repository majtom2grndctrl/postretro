//! A thin harness over the streamed compose planner for tests outside
//! `sh_streaming` (the mover channel's planner rows). It plans frames and
//! commits them as a successful encode would, exposing only planned rows.

use postretro_render_cpu::frame_uniforms::LightTermMask;

use super::compose_plan::{
    ComposeControlSnapshot, ComposeFramePlan, ComposePlannerFrame, PassMembership, RowMembership,
    StreamedComposePlanner,
};
use crate::render::animated_direct_sh_compose::AnimatedDirectShDebugOverride;
use crate::render::direct_sh_compose::DirectShDebugOverride;
use crate::render::mover_channel::MoverChannelRows;

/// One planned frame's direct-pass rows, in encode order.
#[derive(Debug, Default, PartialEq, Eq)]
pub(in crate::render) struct PlannedDirectRows {
    pub(in crate::render) static_rows: Vec<u32>,
    pub(in crate::render) animated_rows: Vec<u32>,
}

/// Membership changes making `rows` resident in both direct passes, with no
/// id-41 or id-45 contribution.
fn resident_rows(rows: &[u32]) -> Vec<(u32, RowMembership)> {
    let resident = PassMembership {
        resident: true,
        contributing: false,
        upstream: false,
    };
    rows.iter()
        .map(|&row| {
            (
                row,
                RowMembership {
                    indirect: PassMembership::default(),
                    static_direct: resident,
                    animated_direct: resident,
                },
            )
        })
        .collect()
}

pub(in crate::render) struct PlannerHarness {
    planner: StreamedComposePlanner,
    pending_membership: Vec<(u32, RowMembership)>,
    last: Option<ComposeFramePlan>,
}

impl PlannerHarness {
    pub(in crate::render) fn new(rows: usize) -> Self {
        Self {
            planner: StreamedComposePlanner::with_row_capacity(rows),
            pending_membership: Vec::new(),
            last: None,
        }
    }

    /// The next planned frame observes `rows` resident in both direct passes.
    pub(in crate::render) fn observe_resident(&mut self, rows: &[u32]) {
        self.pending_membership.extend(resident_rows(rows));
    }

    /// Plan one frame with no weight, curve or control change.
    pub(in crate::render) fn frame(
        &mut self,
        records_compose: bool,
        gated: &[u32],
        mover_changes: &[u32],
        mover_rows: &MoverChannelRows,
    ) -> PlannedDirectRows {
        self.frame_with(
            records_compose,
            gated,
            mover_changes,
            mover_rows,
            &[],
            false,
            LightTermMask::ALL,
            DirectShDebugOverride::default(),
            AnimatedDirectShDebugOverride::default(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::render) fn frame_with(
        &mut self,
        records_compose: bool,
        gated: &[u32],
        mover_changes: &[u32],
        mover_rows: &MoverChannelRows,
        static_weights: &[f32],
        animated_active: bool,
        light_term_mask: LightTermMask,
        promotion_override: DirectShDebugOverride,
        animated_override: AnimatedDirectShDebugOverride,
    ) -> PlannedDirectRows {
        let membership = std::mem::take(&mut self.pending_membership);
        let mut plan = ComposeFramePlan::default();
        self.planner.plan_frame_into(
            ComposePlannerFrame {
                records_compose,
                force_full_resident: false,
                gated_rows: gated,
                membership_changes: &membership,
                indirect_active: false,
                animated_direct_active: animated_active,
                effective_static_weights: static_weights,
                effective_animated_weights: &[],
                controls: ComposeControlSnapshot {
                    light_term_mask,
                    promotion_override,
                    animated_override,
                },
                mover_changes,
                mover_rows,
            },
            &mut plan,
        );
        let rows = PlannedDirectRows {
            static_rows: plan.static_direct.rows().to_vec(),
            animated_rows: plan.animated_direct.rows().to_vec(),
        };
        self.last = records_compose.then_some(plan);
        rows
    }

    /// Commit the last recorded frame's plans, as a successful encode does.
    pub(in crate::render) fn commit_all(&mut self) {
        if let Some(plan) = self.last.take() {
            self.planner.commit_pass(&plan.indirect);
            self.planner.commit_pass(&plan.static_direct);
            self.planner.commit_pass(&plan.animated_direct);
        }
    }
}
