//! A `sweep` that reaches an area light, across co-op (area-lights acceptance
//! row 43, research.md O14): the tagged refusal writes no target, so host and
//! client keep every light's animation, while the same sweep over spots alone
//! applies and replicates. The handler half is `postretro-lighting`
//! `script_primitives` tests.
//! See: context/lib/scripting.md §12 (Entity addressing) · networking.md §Presentation state
use super::parity::{host_write, joined};
use super::*;
use postretro_entities::components::light::LightAnimation;
use postretro_level_loader::LevelWorld;
use postretro_test_log_capture::LogCapture;

fn spot(tags: &[&str]) -> MapLight {
    let mut light = map_light();
    light.light_type = LightType::Spot;
    light.cone_angle_inner = 0.2;
    light.cone_angle_outer = 0.5;
    light.cone_direction = [0.0, -1.0, 0.0];
    light.tags = tags.iter().map(|tag| (*tag).to_owned()).collect();
    light
}

/// A baked area light holding compose slot 0, as the loader installs one.
fn area(tags: &[&str]) -> MapLight {
    let mut light = map_light();
    light.light_type = LightType::Area;
    light.is_dynamic = false;
    light.animated_slot = Some(0);
    light.tags = tags.iter().map(|tag| (*tag).to_owned()).collect();
    light
}

/// Spot S1, area light A and spot S2, in that entity order, all tagged
/// `sweep`; the spots also carry `spots`.
fn sweep_world() -> LevelWorld {
    let mut world = fixture_world();
    world.lights = vec![
        spot(&["sweep", "spots"]),
        area(&["sweep"]),
        spot(&["sweep", "spots"]),
    ];
    world
}

fn sweep(tag: &str) -> NamedReaction {
    primitive(
        "parity",
        "setLightAnimation",
        Some(tag),
        json!({
            "periodMs": 1000.0,
            "direction": [{ "x": 0.0, "y": -1.0, "z": 0.0 }, { "x": 1.0, "y": -1.0, "z": 0.0 }],
        }),
    )
}

fn light_entity(app: &App, index: usize) -> EntityId {
    app.session
        .as_ref()
        .unwrap()
        .light_bridge
        .entity_for_map_index(index)
        .expect("fixture light")
}

/// Every fixture light's animation on `app`, in map order.
fn animations(app: &App) -> Vec<Option<LightAnimation>> {
    let ctx = script_ctx(app);
    (0..3)
        .map(|index| {
            ctx.registry
                .borrow()
                .get_component::<LightComponent>(light_entity(app, index))
                .unwrap()
                .animation
                .clone()
        })
        .collect()
}

#[test]
fn a_tagged_sweep_over_spot_area_spot_changes_no_light_on_host_or_client() {
    let mut h = joined(sweep_world());
    let host_before = animations(&h.host);
    let client_before = animations(&h.peers[0].app);
    let area_id = light_entity(&h.host, 1);

    let logs = LogCapture::start();
    host_write(&mut h, vec![sweep("sweep")]);
    logs.assert_logged(log::Level::Warn, &format!("target {area_id}"));
    logs.assert_logged(log::Level::Warn, "area light");
    assert_eq!(animations(&h.host), host_before, "the host changed a light");
    assert!(
        (0..3).all(|index| {
            !script_ctx(&h.host)
                .presentation_overrides
                .borrow()
                .records
                .contains_key(&(light_entity(&h.host, index), ComponentKind::Light))
        }),
        "a refused sweep captures nothing to replicate"
    );
    h.advance(24);
    assert_eq!(animations(&h.host), host_before);
    assert_eq!(
        animations(&h.peers[0].app),
        client_before,
        "the client changed a light"
    );

    // The same sweep over the two spots alone applies to both, on the host
    // and, through replication, on the client; the area light keeps its own.
    host_write(&mut h, vec![sweep("spots")]);
    h.advance(24);
    for (machine, app) in [("host", &h.host), ("client", &h.peers[0].app)] {
        let now = animations(app);
        for index in [0, 2] {
            assert!(
                now[index]
                    .as_ref()
                    .is_some_and(|animation| animation.direction.is_some()),
                "{machine}: spot {index} sweeps: {:?}",
                now[index]
            );
        }
        let before = if machine == "host" {
            &host_before
        } else {
            &client_before
        };
        assert_eq!(now[1], before[1], "{machine}: the area light is unchanged");
    }
}
