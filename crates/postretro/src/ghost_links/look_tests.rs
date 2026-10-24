// Per-link style resolution at install and on committed reload: declared, floor and
// undeclared names, the once-per-link warning, armed and disarmed links alike.
// See: context/lib/entity_model.md §7d · context/lib/scripting.md §2 (Ghost style declarations)

use glam::Vec3;
use log::Level;
use postretro_entities::EntityRegistry;
use postretro_foundation::{
    ARM_GHOST_LINK, DISARM_GHOST_LINK, GhostStyle, GhostStyleDescriptor, GhostStyleRegistry,
    SET_GHOST_LINK_COLOR_FILTER,
};
use postretro_level_format::environment_volumes::EnvironmentBrush;
use postretro_level_format::ghost_links::{GhostLinkRecord, GhostMembers};
use postretro_level_loader::{CellData, CellLocatorChild, LevelWorld};
use postretro_test_log_capture::LogCapture;
use serde_json::json;

use super::super::app_test_fixtures::{LinkedApp, box_link};
use super::super::membership::ramp_step;
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

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-5
}

// A committed staged manifest reload, through the real commit route, re-resolves
// installed links: an armed link whose member is mid ramp-in and a disarmed
// link whose member is mid fade-out both present the reloaded style from the
// next frame, while their membership ramps run on unbroken. The field
// `setColorFilter` set keeps its value and its ramp runs to its target.
#[test]
fn a_committed_staged_reload_re_resolves_links_mid_ramp_armed_or_not_and_keeps_filter_fields() {
    const VOLUME: (Vec3, Vec3) = (Vec3::new(0.0, 0.0, -5.0), Vec3::new(10.0, 4.0, 5.0));
    let echo = style(0.45, [0.6, 0.8, 1.0], true);
    let link = |source_entity, armed, target: Vec3| {
        box_link(
            source_entity,
            VOLUME,
            (Vec3::ZERO, 0.0),
            (target, 0.0),
            armed,
            GhostMembers::all(),
            "echo",
        )
    };
    let world = world(vec![
        link(4, false, Vec3::new(100.0, 0.0, 0.0)),
        link(5, true, Vec3::new(0.0, 0.0, 100.0)),
    ]);
    let mut linked = LinkedApp::new(&world, styles(&[("echo", echo)]), &["wraith"]);
    let wraith = linked.entity("wraith", Vec3::new(2.0, 1.0, 0.0));
    linked.tick_and_frame();
    assert!(linked.ghost(0, wraith).is_none(), "link 0 starts disarmed");
    assert_eq!(
        linked.ghost(1, wraith).map(|ghost| ghost.membership),
        Some(1.0),
        "link 1 is armed on spawn"
    );

    let (link_0, link_1) = (linked.link(0), linked.link(1));
    linked.step(link_0, ARM_GHOST_LINK, json!({}));
    linked.step(
        link_0,
        SET_GHOST_LINK_COLOR_FILTER,
        json!({ "tint": [0.0, 0.0, 0.0], "periodMs": 1000 }),
    );
    linked.step(link_1, DISARM_GHOST_LINK, json!({}));
    for _ in 0..4 {
        linked.tick_and_frame();
    }
    let rising = linked
        .ghost(0, wraith)
        .expect("the armed link's member ramps in");
    let fading = linked
        .ghost(1, wraith)
        .expect("the disarmed link's member fades out");
    assert!(!rising.frozen && rising.membership > 0.0 && rising.membership < 1.0);
    assert!(fading.frozen && fading.membership > 0.0 && fading.membership < 1.0);
    let filtered_tint = linked.app.ghost_links.link_look(0).tint;
    assert_ne!(filtered_tint, echo.tint, "the tint ramp is under way");

    // The reloaded manifest keeps the wraith opted in and retunes `echo`.
    let mod_root = tempfile::tempdir().expect("temporary mod root");
    std::fs::write(
        mod_root.path().join("start-script.js"),
        "globalThis.__postretroModManifest = { name: 'Ghosts', id: 'ghosts', version: '1', \
         entities: [{ canonicalName: 'wraith', ghost: true }], \
         ghostStyles: [{ name: 'echo', tint: [1, 0.4, 0.3], opacity: 0.8, saturation: 0.7, \
                         audio: false, impactAudio: false }] };",
    )
    .expect("reloaded manifest written");
    crate::startup::lifecycle::lifecycle_environment_install_tests::commit_staged_manifest(
        &mut linked.app,
        mod_root.path(),
    );
    let retuned = *linked
        .ctx
        .data_registry
        .borrow()
        .ghost_styles
        .get("echo")
        .expect("the reload declares `echo`");
    assert!(
        close(retuned.opacity, 0.8)
            && close(retuned.saturation, 0.7)
            && retuned.tint != echo.tint
            && !retuned.audio,
        "the commit retuned `echo`: {retuned:?}"
    );

    let reloaded = linked.app.ghost_links.link_look(0);
    assert_eq!(
        reloaded.tint, filtered_tint,
        "the filtered tint keeps its ramp's value"
    );
    assert_eq!(
        (reloaded.opacity, reloaded.saturation),
        (retuned.opacity, retuned.saturation),
        "unset fields follow the reloaded style"
    );
    assert_eq!(
        linked.app.ghost_links.link_look(1),
        look_of(retuned),
        "the disarmed link re-resolves"
    );
    assert_eq!(
        linked.app.ghost_links.link_style(1),
        retuned,
        "flags come from the reloaded style"
    );

    linked.tick_and_frame();
    let step = ramp_step(linked.app.ghost_links.presented_elapsed());
    let risen = linked.ghost(0, wraith).expect("still ramping in");
    assert!(
        close(risen.membership, rising.membership + step),
        "the ramp-in runs on from {} to {}",
        rising.membership,
        risen.membership
    );
    assert_eq!(
        (risen.look.opacity, risen.look.saturation),
        (retuned.opacity, retuned.saturation),
        "the rising member presents the reloaded style"
    );
    assert!(
        risen.look.tint[2] < filtered_tint[2] && risen.look.tint[2] > 0.0,
        "the tint ramp runs on toward black: {:?}",
        risen.look.tint
    );
    let faded = linked.ghost(1, wraith).expect("still fading out");
    assert!(faded.frozen);
    assert!(
        close(faded.membership, fading.membership - step),
        "the fade runs on from {} to {}",
        fading.membership,
        faded.membership
    );
    assert_eq!(
        faded.look,
        look_of(retuned),
        "the fading member presents the reloaded style"
    );

    for _ in 0..60 {
        linked.tick_and_frame();
    }
    let settled = linked.ghost(0, wraith).expect("the armed link shows it");
    assert_eq!(settled.membership, 1.0);
    assert_eq!(
        settled.look.tint, [0.0; 3],
        "the filter ramp reaches its target"
    );
    assert!(linked.ghost(1, wraith).is_none(), "the fade ran out");
}
