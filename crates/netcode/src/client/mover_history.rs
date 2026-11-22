// Client mover history: authoritative and predicted mover samples from each
// snapshot apply, read by reconcile replay and rider presentation.
// See: context/lib/networking.md §Snapshot apply ordering

use super::*;

pub(super) const MOVER_HISTORY_LIMIT: usize = 128;

#[derive(Debug, Clone)]
pub(crate) struct MoverHistorySample {
    pub(crate) server_tick: u32,
    pub(crate) pose: MoverPose,
    pub(crate) phase: KinematicMoverComponent,
    /// The pose a snapshot delivered, as opposed to the client's prediction at
    /// the fast-forward tick recorded beside it. Rider presentation reads only
    /// these; replay reads both.
    pub(crate) authoritative: bool,
}

#[derive(Debug, Default)]
pub(crate) struct MoverHistoryBuffer {
    pub(super) samples: HashMap<u32, VecDeque<MoverHistorySample>>,
    /// Bumped by every change to `samples`; keys the replay memo's working phases.
    pub(super) generation: u64,
    pub(super) replay: replay_poses::ReplayPoseMemo,
}

impl MoverHistoryBuffer {
    pub(super) fn clear(&mut self) {
        self.samples.clear();
        self.generation = self.generation.wrapping_add(1);
        self.replay = replay_poses::ReplayPoseMemo::default();
    }

    pub(crate) fn record(&mut self, mover_id: u32, sample: MoverHistorySample) {
        self.generation = self.generation.wrapping_add(1);
        let samples = self.samples.entry(mover_id).or_default();
        if samples
            .back()
            .is_some_and(|existing| existing.server_tick == sample.server_tick)
        {
            samples.pop_back();
        }
        samples.push_back(sample);
        while samples.len() > MOVER_HISTORY_LIMIT {
            samples.pop_front();
        }
    }

    /// Whether any sample of `mover_id` is held.
    pub(crate) fn has_samples(&self, mover_id: u32) -> bool {
        self.samples
            .get(&mover_id)
            .is_some_and(|samples| !samples.is_empty())
    }

    /// The newest authoritative pose at or before `server_tick`. A mover the
    /// host did not resend has not changed, so an older sample still holds. `None`
    /// when no retained authoritative sample reaches back that far, never the
    /// oldest sample in its place (rider rebase treats it as missing history).
    /// Samples are not tick-sorted (a predicted sample interleaves each apply),
    /// so this scans the mover's bounded history; it never allocates.
    pub(crate) fn authoritative_transform_at_or_before(
        &self,
        mover_id: u32,
        server_tick: u32,
    ) -> Option<Transform> {
        self.samples
            .get(&mover_id)?
            .iter()
            .filter(|sample| sample.authoritative && tick_le(sample.server_tick, server_tick))
            .min_by_key(|sample| ticks_between(sample.server_tick, server_tick))
            .map(|sample| sample.pose.transform)
    }

    #[cfg(test)]
    pub(crate) fn sample_count(&self, mover_id: u32) -> usize {
        self.samples.get(&mover_id).map_or(0, VecDeque::len)
    }

    /// Reference extrapolation the replay memo must reproduce exactly; see
    /// `replay_poses`. Production replay reads `replay_pose` instead; this stays
    /// as the oracle the memo and snapshot tests compare against.
    #[cfg(test)]
    pub(crate) fn pose_at_tick(
        &self,
        mover_id: u32,
        server_tick: u32,
        tick_dt: f32,
    ) -> Option<MoverPose> {
        let samples = self.samples.get(&mover_id)?;
        if let Some(exact) = samples
            .iter()
            .rev()
            .find(|sample| sample.server_tick == server_tick)
        {
            return Some(exact.pose);
        }
        let seed = samples
            .iter()
            .rev()
            .find(|sample| tick_le(sample.server_tick, server_tick))
            .or_else(|| samples.front())?;
        let mut phase = seed.phase.clone();
        let mut transform = seed.pose.transform;
        let mut pose = seed.pose;
        let advance_ticks = ticks_between(seed.server_tick, server_tick);
        for _ in 0..advance_ticks {
            pose = advance_mover_phase_one_tick(&mut phase, &mut transform, tick_dt);
        }
        Some(pose)
    }
}

impl MoverPoseSource for MoverHistoryBuffer {
    fn pose(&self, mover_id: u32) -> Option<MoverPose> {
        self.samples
            .get(&mover_id)
            .and_then(|samples| samples.back())
            .map(|sample| sample.pose)
    }
}

#[derive(Debug, Clone)]
pub(super) struct MoverApplyPlan {
    pub(super) mover_id: u32,
    pub(super) phase: KinematicMoverComponent,
    pub(super) transform: Transform,
    pub(super) pose: MoverPose,
    pub(super) history_samples: Vec<MoverHistorySample>,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn prepare_mover_apply(
    registry: &EntityRegistry,
    id: EntityId,
    network_id: NetworkId,
    bound_mover_id: Option<u32>,
    wire: WireKinematicMoverState,
    server_tick: u32,
    mover_target_tick: u32,
    anchor_transform: Transform,
    tick_dt: f32,
) -> Option<MoverApplyPlan> {
    let mut phase = bindings::validate_kinematic_mover_state_binding(
        registry,
        id,
        network_id,
        bound_mover_id,
        &wire,
    )?;
    seed_kinematic_mover_phase(&mut phase, &wire)?;

    let phase_pose = mover_pose_for_current_phase(anchor_transform, &phase, tick_dt);
    let mut server_pose = phase_pose;
    server_pose.transform = anchor_transform;
    server_pose.tick_delta = if tick_dt.is_finite() && tick_dt > 0.0 {
        server_pose.linear_velocity * tick_dt
    } else {
        Vec3::ZERO
    };
    let anchor_drift = (phase_pose.transform.position - anchor_transform.position).length();
    if anchor_drift > 0.01 {
        log::warn!(
            "[Net] mover {network_id:?}/{} Transform anchor differs from phase pose by {anchor_drift:.3} m",
            wire.mover_id
        );
    }
    let mut history_samples = vec![MoverHistorySample {
        server_tick,
        pose: server_pose,
        phase: phase.clone(),
        authoritative: true,
    }];

    let mut predicted_phase = phase.clone();
    let mut predicted_transform = server_pose.transform;
    let mut predicted_pose = server_pose;
    let advance_ticks = ticks_between(server_tick, mover_target_tick);
    for _ in 0..advance_ticks {
        predicted_pose =
            advance_mover_phase_one_tick(&mut predicted_phase, &mut predicted_transform, tick_dt);
    }
    if advance_ticks > 0 {
        history_samples.push(MoverHistorySample {
            server_tick: server_tick.wrapping_add(advance_ticks),
            pose: predicted_pose,
            phase: predicted_phase.clone(),
            authoritative: false,
        });
    }

    Some(MoverApplyPlan {
        mover_id: wire.mover_id,
        phase: predicted_phase,
        transform: predicted_pose.transform,
        pose: predicted_pose,
        history_samples,
    })
}

pub(super) fn seed_kinematic_mover_phase(
    mover: &mut KinematicMoverComponent,
    wire: &WireKinematicMoverState,
) -> Option<()> {
    if !matches!(wire.direction, -1 | 1) {
        return None;
    }
    mover.segment_index = wire.segment_index;
    mover.direction_sign = wire.direction;
    mover.mode = match wire.mode {
        0 => KinematicMoverMode::Once,
        1 => KinematicMoverMode::PingPong,
        _ => return None,
    };
    mover.segment_elapsed_ms = wire.segment_elapsed_ms;
    mover.wait_remaining_ms = wire.wait_remaining_ms;
    mover.started = wire.started;
    mover.completed = wire.completed;
    mover.blocked = wire.blocked;
    mover.current_linear_velocity = Vec3::from_array(wire.velocity);
    mover.target_segment = wire.target_segment;
    mover.spin_angle_rad = wire.spin_angle_rad;
    mover.spin_angle_before_tick_rad = wire.spin_angle_before_tick_rad;
    mover.was_active_this_tick = wire.was_active_this_tick;
    mover.spin_rate_rad_s = wire.spin_rate_rad_s;
    mover.spin_target_rate_rad_s = wire.spin_target_rate_rad_s;
    Some(())
}

pub(super) fn tick_le(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) <= 0
}

pub(super) fn ticks_between(from: u32, to: u32) -> u32 {
    if tick_le(to, from) {
        0
    } else {
        to.wrapping_sub(from)
    }
}
