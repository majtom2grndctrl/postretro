// Per-link style resolution at install and on committed reload: declared, floor and
// undeclared names, the once-per-link warning, armed and disarmed links alike.
// See: context/lib/entity_model.md §7d · context/lib/scripting.md §2 (Ghost style declarations)

use glam::Vec3;
use log::Level;
use postretro_entities::EntityRegistry;
use postretro_foundation::{GhostStyle, GhostStyleDescriptor, GhostStyleRegistry};
use postretro_level_format::environment_volumes::EnvironmentBrush;
use postretro_level_format::ghost_links::{GhostLinkRecord, GhostMembers};
use postretro_level_loader::{CellData, CellLocatorChild, LevelWorld};
use postretro_test_log_capture::LogCapture;

use super::*;
use crate::ghost_links::GhostLinks;

const UNDECLARED: &str = "is not declared; using the floor style";

fn style(opacity: f32, tint: [f32; 3], audio: bool) -> GhostStyle {
    GhostStyle {
        tint,
        opacity,
        saturation: 0.2,
        audio,
        impact_audio: false,
    }
}

fn styles(entries: &[(&str, GhostStyle)]) -> GhostStyleRegistry {
    GhostStyleRegistry::from_declarations(
        "test",
        entries.iter().map(|(name, style)| {
            Ok(GhostStyleDescriptor {
                name: (*name).to_string(),
                style: *style,
            })
        }),
    )
}

fn record(style: &str, enabled_on_spawn: bool) -> GhostLinkRecord {
    GhostLinkRecord {
        source_entity: 0,
        enabled_on_spawn,
        members: GhostMembers::all(),
        style: style.to_string(),
        brushes: vec![EnvironmentBrush {
            aabb_min: [0.0; 3],
            aabb_max: [1.0; 3],
            planes: Vec::new(),
        }],
        landmark_origin: [0.0; 3],
        landmark_yaw: 0.0,
        target_origin: [10.0, 0.0, 0.0],
        target_yaw: 0.0,
        ghost_tags: Vec::new(),
    }
}

fn world(links: Vec<GhostLinkRecord>) -> LevelWorld {
    let mut world = LevelWorld::new_visibility_only(
        vec![CellData {
            bounds_min: Vec3::splat(-100.0),
            bounds_max: Vec3::splat(100.0),
            face_start: 0,
            face_count: 0,
            portal_ref_start: 0,
            portal_ref_count: 0,
            is_solid: false,
            is_exterior: false,
            is_drawable: false,
        }],
        Vec::new(),
        CellLocatorChild::Cell(0),
        Vec::new(),
        Vec::new(),
        false,
    )
    .unwrap();
    world.ghost_links.links = links;
    world
}

fn look_of(style: GhostStyle) -> GhostLook {
    GhostLook {
        tint: style.tint,
        opacity: style.opacity,
        saturation: style.saturation,
    }
}

#[test]
fn install_presents_declared_style_floor_for_none_and_warns_once_per_undeclared_link() {
    let logs = LogCapture::start();
    let echo = style(0.45, [0.6, 0.8, 1.0], true);
    let records = [
        record("", true),
        record("echo", true),
        record("missing", true),
        record("absent", false),
    ];

    let looks = LinkLooks::bind(&records, &styles(&[("echo", echo)]));

    assert_eq!(
        looks.style(0),
        GhostStyle::FLOOR,
        "no style name: the floor"
    );
    assert_eq!(
        looks.resolve(0, &Default::default(), 0.0),
        look_of(GhostStyle::FLOOR)
    );
    assert_eq!(
        looks.style(1),
        echo,
        "a declared style presents its values and flags"
    );
    assert_eq!(looks.resolve(1, &Default::default(), 0.0), look_of(echo));
    assert_eq!(looks.style(2), GhostStyle::FLOOR, "undeclared: the floor");
    assert_eq!(looks.style(3), GhostStyle::FLOOR);
    logs.assert_logged_once(
        Level::Warn,
        "[Ghost] ghost_link style `missing` is not declared",
    );
    logs.assert_logged_once(
        Level::Warn,
        "[Ghost] ghost_link style `absent` is not declared",
    );
    assert_eq!(
        logs.records()
            .iter()
            .filter(|record| record.message.contains(UNDECLARED))
            .count(),
        2,
        "the empty name and the declared name warn nothing"
    );
}

#[test]
fn committed_reload_re_resolves_every_link_naming_a_changed_style_armed_or_not() {
    let logs = LogCapture::start();
    let world = world(vec![
        record("echo", true),
        record("echo", false),
        record("veil", true),
    ]);
    let registry = EntityRegistry::new();
    let mut links = GhostLinks::install(&world, &registry);
    let echo = style(0.45, [0.6, 0.8, 1.0], true);
    let veil = style(0.3, [1.0, 1.0, 1.0], false);
    links.bind_styles(&world, &styles(&[("echo", echo), ("veil", veil)]));
    assert_eq!(links.link_look(0), look_of(echo));
    assert_eq!(links.link_look(1), look_of(echo));

    // The reload retunes `echo`, keeps `veil` and drops nothing.
    let echo_retuned = style(0.8, [1.0, 0.4, 0.3], false);
    links.rebind_styles(&styles(&[("veil", veil), ("echo", echo_retuned)]));

    assert_eq!(
        links.link_look(0),
        look_of(echo_retuned),
        "armed link re-resolved"
    );
    assert_eq!(
        links.link_look(1),
        look_of(echo_retuned),
        "disarmed link re-resolved"
    );
    assert_eq!(
        links.link_style(1),
        echo_retuned,
        "flags come from the new style"
    );
    assert_eq!(links.link_look(2), look_of(veil));
    logs.assert_not_logged(Level::Warn, UNDECLARED);
}

#[test]
fn reload_dropping_a_style_warns_once_per_link_and_a_still_undeclared_link_stays_quiet() {
    let logs = LogCapture::start();
    let echo = style(0.45, [0.6, 0.8, 1.0], true);
    let records = [record("echo", true), record("missing", true)];
    let mut looks = LinkLooks::bind(&records, &styles(&[("echo", echo)]));
    logs.assert_logged_once(Level::Warn, "ghost_link style `missing` is not declared");

    looks.rebind(&styles(&[]));
    looks.rebind(&styles(&[]));

    assert_eq!(
        looks.style(0),
        GhostStyle::FLOOR,
        "a dropped style falls to the floor"
    );
    logs.assert_logged_once(Level::Warn, "ghost_link style `echo` is not declared");
    logs.assert_logged_once(Level::Warn, "ghost_link style `missing` is not declared");

    // Declaring it again restores the style, and a later drop warns again.
    looks.rebind(&styles(&[("echo", echo)]));
    assert_eq!(looks.style(0), echo);
    looks.rebind(&styles(&[]));
    assert_eq!(
        logs.records()
            .iter()
            .filter(|record| record.message.contains("ghost_link style `echo`"))
            .count(),
        2
    );
}
