// The client assist step's App seams: its level-scoped reset in
// `unload_level` and the per-role display projection.
use glam::Vec3;
use postretro_entities::EntityId;

use super::lifecycle_test_fixtures::{connect_test_app_as_client, test_app};
use crate::client_assist::{ClientDisplay, UiProjection, ui_projection};
use crate::sim::assist::{AssistCandidate, AssistHelper, PawnLife, Row6Record};
use crate::sim::assist_pairs::PairValues;

fn released() -> UiProjection {
    UiProjection::Client {
        prediction: 0,
        state: ClientDisplay::Released,
        increment: 0,
        alpha: 0.0,
    }
}

/// O35 at the App: `unload_level` clears the client step beside
/// `reset_level_scoped_client_state`, so no hold reaches the next level.
#[test]
fn client_assist_o35_unload_level_clears_the_client_step() {
    let mut app = test_app();
    connect_test_app_as_client(&mut app);
    let helper = AssistHelper {
        id: EntityId::from_raw(1),
        position: Vec3::ZERO,
        permits_use: true,
        life: PawnLife {
            life_state: 0,
            entry: None,
        },
    };
    let target = AssistCandidate {
        id: EntityId::from_raw(2),
        position: Vec3::X,
        life: PawnLife {
            life_state: 1,
            entry: None,
        },
        assistable: true,
        pair: Some(PairValues {
            radius: 1.5,
            required_ticks: 10,
        }),
    };
    for tick in 0..8 {
        app.client_assist
            .predict_tick(tick, true, helper, &[target]);
    }
    assert_ne!(app.client_assist.client_projection(0.0), released());
    app.unload_level();
    assert_eq!(app.client_assist.client_projection(0.0), released());
}

/// The projection's role: single player (no endpoint) reads the host
/// accumulator's row-6 record; a connected client reads its own step.
#[test]
fn client_assist_projection_role_follows_the_endpoint() {
    let mut app = test_app();
    let session = app.session.as_ref().unwrap();
    assert_eq!(
        ui_projection(&app.client_assist, session, 0.3),
        UiProjection::Host {
            row6: Row6Record::default(),
            alpha: 0.3
        }
    );
    connect_test_app_as_client(&mut app);
    let session = app.session.as_ref().unwrap();
    assert!(matches!(
        ui_projection(&app.client_assist, session, 0.3),
        UiProjection::Client {
            state: ClientDisplay::Released,
            ..
        }
    ));
}
