// Co-op vault parity through the loopback harness: a predicted vault onto a static ledge, replayed from every acked baseline, with no correction.
// See: context/lib/networking.md · context/lib/movement.md §4

#![cfg(test)]

use glam::Vec3;
use postretro_foundation::{
    MovementStateKind, PlayerMovementComponent, VaultEntry, VaultParams, VaultTrigger,
};

use super::predict_reconcile_harness_test_fixtures::{
    LoopbackHarness, MOVING_PLATFORM_ID, NO_CORRECTION_M, forward_command, idle_command, light_link,
};
use crate::collision::CollisionWorld;
use crate::prediction::DASH_CORRECTION_MAX_M;
use crate::sim::SimCommand;
use postretro_foundation::GroundRef;

/// The riser faces +Z at this z; `forward_command` runs the pawn along -Z.
pub(crate) const RISER_Z: f32 = -6.0;
pub(crate) const TOP_Y: f32 = 1.2;

pub(crate) fn vault(from: VaultEntry) -> VaultParams {
    VaultParams {
        max_height: 1.6,
        swim_max_height: 0.9,
        duration_ms: 320.0,
        from,
        allow_fire: false,
    }
}

/// Floor at y = 0 for z > `RISER_Z`, a riser facing +Z, and a top at `TOP_Y`
/// beyond it.
pub(crate) fn ledge_world() -> CollisionWorld {
    let (x0, x1) = (-500.0, 500.0);
    let points = vec![
        // Floor.
        Vec3::new(x0, 0.0, RISER_Z),
        Vec3::new(x1, 0.0, RISER_Z),
        Vec3::new(x1, 0.0, 500.0),
        Vec3::new(x0, 0.0, 500.0),
        // Riser.
        Vec3::new(x0, 0.0, RISER_Z),
        Vec3::new(x1, 0.0, RISER_Z),
        Vec3::new(x1, TOP_Y, RISER_Z),
        Vec3::new(x0, TOP_Y, RISER_Z),
        // Top.
        Vec3::new(x0, TOP_Y, -500.0),
        Vec3::new(x1, TOP_Y, -500.0),
        Vec3::new(x1, TOP_Y, RISER_Z),
        Vec3::new(x0, TOP_Y, RISER_Z),
    ];
    let triangles = vec![
        [0, 2, 1],
        [0, 3, 2],
        [4, 5, 6],
        [4, 6, 7],
        [8, 10, 9],
        [8, 11, 10],
    ];
    CollisionWorld::from_triangles_for_test(points, triangles)
}

struct VaultRun {
    corrections: Vec<f32>,
    windows: Vec<(Option<u32>, Option<u32>)>,
    /// `(client tick, predicted state)` per step.
    states: Vec<(u32, MovementStateKind)>,
    host_vaulted: bool,
    final_position: Vec3,
    final_ground: GroundRef,
}

fn run(from: VaultEntry, steps: u32, command_at: impl Fn(Vec3) -> SimCommand) -> VaultRun {
    let mut h = LoopbackHarness::new(light_link()).with_player_vault(vault(from));
    h.world = ledge_world();
    run_on(h, steps, command_at)
}

fn run_on(mut h: LoopbackHarness, steps: u32, command_at: impl Fn(Vec3) -> SimCommand) -> VaultRun {
    h.step_until_armed(&idle_command());
    let mut run = VaultRun {
        corrections: Vec::new(),
        windows: Vec::new(),
        states: Vec::new(),
        host_vaulted: false,
        final_position: Vec3::ZERO,
        final_ground: GroundRef::Airborne,
    };
    for _ in 0..steps {
        let position = h.client_position().expect("the client is armed");
        h.step(&command_at(position));
        run.corrections
            .extend(h.latest_local_corrections.iter().copied());
        run.windows
            .extend(h.latest_reconcile_windows.iter().copied());
        run.host_vaulted |= h
            .host_registry
            .get_component::<PlayerMovementComponent>(h.host_pawn)
            .is_ok_and(|movement| movement.movement_state.kind() == MovementStateKind::Vault);
        let pawn = h.client_pawn.expect("armed");
        let state = h
            .client_registry
            .get_component::<PlayerMovementComponent>(pawn)
            .expect("client pawn movement")
            .movement_state
            .kind();
        run.states
            .push((h.last_sent_client_tick.expect("sent this step"), state));
    }
    run.final_position = h.client_position().expect("armed");
    run.final_ground = h.client_ground();
    run
}

fn first_tick_in(run: &VaultRun, kind: MovementStateKind) -> u32 {
    run.states
        .iter()
        .find(|(_, state)| *state == kind)
        .map(|(tick, _)| *tick)
        .unwrap_or_else(|| panic!("the client never entered {kind:?}"))
}

fn state_at(run: &VaultRun, tick: u32) -> Option<MovementStateKind> {
    run.states
        .iter()
        .find(|(recorded, _)| *recorded == tick)
        .map(|(_, state)| *state)
}

/// Reconcile windows `(acked, last_sent]` that replay `tick` from an acked
/// baseline the client recorded in `baseline`.
fn windows_spanning(run: &VaultRun, tick: u32, baseline: MovementStateKind) -> usize {
    run.windows
        .iter()
        .filter(|window| match **window {
            (Some(acked), Some(last_sent)) => {
                acked < tick && tick <= last_sent && state_at(run, acked) == Some(baseline)
            }
            _ => false,
        })
        .count()
}

fn assert_vaulted_without_correction(run: &VaultRun) {
    assert!(run.host_vaulted, "the host pawn vaulted too");
    assert!(
        run.final_position.z < RISER_Z && run.final_position.y > TOP_Y,
        "the client ends on the top, at {:?}",
        run.final_position
    );
    let entry = first_tick_in(run, MovementStateKind::Vault);
    // A replay from a pre-vault baseline re-derives the entry from position; one
    // from a mid-vault baseline replays the restored vault payload.
    assert!(
        windows_spanning(run, entry, MovementStateKind::Normal) > 0,
        "a reconcile must replay the entry tick {entry} from a pre-vault baseline; windows {:?}",
        run.windows
    );
    assert!(
        windows_spanning(run, entry + 3, MovementStateKind::Vault) > 0,
        "a reconcile must replay from a mid-vault baseline; windows {:?}",
        run.windows
    );
    assert!(
        run.corrections.len() >= 20,
        "reconciles must run throughout; saw {}",
        run.corrections.len()
    );
    let worst = run.corrections.iter().copied().fold(0.0_f32, f32::max);
    assert!(
        worst <= NO_CORRECTION_M,
        "every correction must be at most {NO_CORRECTION_M} m; worst {worst}"
    );
}

#[test]
fn a_running_vault_onto_a_static_ledge_predicts_with_no_correction() {
    let from = VaultEntry {
        ground: Some(VaultTrigger::Auto),
        ..VaultEntry::default()
    };
    let run = run(from, 90, |_| forward_command(false));
    assert_vaulted_without_correction(&run);
}

#[test]
fn a_jumping_vault_onto_a_static_ledge_predicts_with_no_correction() {
    // Default `from`: only an airborne pawn vaults, so the run jumps short of
    // the riser and the probe catches it in the air.
    let run = run(VaultEntry::default(), 90, |position| {
        let mut command = forward_command(false);
        command.movement.jump_pressed = position.z < RISER_Z + 2.5;
        command
    });
    assert_vaulted_without_correction(&run);
}

// The mover moves on both peers by replicated phase; a client's mover pose may
// lead or lag the host's, so the vault onto it is held to the vault tolerance
// rather than to zero.
#[test]
fn a_vault_onto_a_moving_mover_stays_within_the_vault_tolerance() {
    let from = VaultEntry {
        ground: Some(VaultTrigger::Auto),
        ..VaultEntry::default()
    };
    let h = LoopbackHarness::with_moving_ledge(light_link(), RISER_Z, TOP_Y)
        .with_player_vault(vault(from));
    // Stop once over the top so the pawn stays on the mover's 6 m deck.
    let run = run_on(h, 90, |position| {
        if position.z < RISER_Z - 1.0 {
            idle_command()
        } else {
            forward_command(false)
        }
    });
    assert!(run.host_vaulted, "the host pawn vaulted onto the mover");
    assert!(
        run.states
            .iter()
            .any(|(_, state)| *state == MovementStateKind::Vault),
        "the client vaulted onto the mover"
    );
    assert_eq!(run.final_ground, GroundRef::Mover(MOVING_PLATFORM_ID));
    let worst = run.corrections.iter().copied().fold(0.0_f32, f32::max);
    assert!(
        worst <= DASH_CORRECTION_MAX_M,
        "every correction must be within the vault tolerance {DASH_CORRECTION_MAX_M} m; worst {worst}"
    );
}
