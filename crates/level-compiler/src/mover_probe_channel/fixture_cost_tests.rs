// The SH gate's direct-entry bound against the production direct bake on the
// dev maps with movers (task 7b): the bound prices every (light, mover, cell)
// the bake's own prefilter admits, so it covers the entries the bake holds.

use std::mem::size_of;

use super::cost::direct_entry_bound;
use super::direct::{DirectEntry, MoverDirectBake, ProbeCells};
use super::fixture_scene::Scene;
use crate::fixture_pipeline::load_fixture;

/// Bytes the bake's entries hold, as the bound prices them.
fn held_entry_bytes(bake: &MoverDirectBake) -> u64 {
    bake.lights
        .iter()
        .flat_map(|light| &light.entries)
        .map(|entry| {
            (entry.visibility.len() + entry.tiles.len() * 2) as u64
                + size_of::<DirectEntry>() as u64
        })
        .sum()
}

/// Assert the bound covers the bake on each named dev map.
fn assert_bound_covers(maps: &[&str]) {
    for &name in maps {
        let scene = Scene::of(load_fixture(name));
        let lights = scene.static_channel_lights();
        let cells = ProbeCells::of_layout(&scene.layout);
        let held = held_entry_bytes(&scene.bake(&lights, &cells));
        let baked: Vec<_> = lights.iter().map(|light| &light.baked).collect();
        let bound = direct_entry_bound(&baked, &scene.poses(), &scene.layout);
        eprintln!("{name}: direct-entry bound {bound} B over {held} B held");
        assert!(bound >= held, "{name}: bound {bound} B under {held} B held");
    }
}

#[test]
fn the_direct_entry_bound_covers_the_bake_on_the_dev_maps_with_movers() {
    assert_bound_covers(&[
        "mover-closet",
        "mover-setpiece",
        "kinematic-platform",
        "campaign-test",
    ]);
}

// The stress maps, on demand: minutes of fixture load and direct bake.
#[test]
#[ignore = "loads and bakes the showcase and hallway stress maps; run on demand"]
fn the_direct_entry_bound_covers_the_bake_on_the_stress_maps() {
    assert_bound_covers(&["stress-warren-showcase", "stress-warren-hallway-inspection"]);
}
