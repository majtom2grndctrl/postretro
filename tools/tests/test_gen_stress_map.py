"""Focused source-map tests for the stress-warren generator."""

import importlib.util
import math
import unittest
from pathlib import Path


def load_generator_module():
    script = Path(__file__).parents[1] / "gen_stress_map.py"
    spec = importlib.util.spec_from_file_location("gen_stress_map_test", script)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


GENERATOR = load_generator_module()


class SpatialLayoutTests(unittest.TestCase):
    def test_room_interior_is_at_least_two_storeys_and_corridors_are_one(self):
        self.assertGreaterEqual(
            GENERATOR.PITCH_Z - GENERATOR.SLAB_T,
            2 * GENERATOR.STORY_H,
        )
        self.assertEqual(
            GENERATOR.LATTICE_PITCH_XY - GENERATOR.PITCH_XY,
            GENERATOR.CORRIDOR_BAND,
        )

    def test_reserved_corridor_grid_has_floor_and_roof_outside_room_footprints(self):
        brushes = []
        starts = [0, GENERATOR.LATTICE_PITCH_XY]
        GENERATOR.emit_reserved_corridor_grid(brushes, starts, starts, 64,
                                              "test", 1)
        # The single vertical band is split around the junction and combines
        # with two horizontal bands; each gets a floor and roof. A seed of 1
        # emits no diagonal fin for this one junction.
        self.assertEqual(len(brushes), 10)
        self.assertNotIn(f"( 0 0 64 )", "".join(brushes))

    def test_reserved_corridors_get_bake_only_spots_at_regular_spacing(self):
        brushes, lights = [], []
        starts = [0, GENERATOR.LATTICE_PITCH_XY]
        GENERATOR.emit_reserved_corridor_grid(brushes, starts, starts, 64,
                                              "test", 1, hallway_lights=lights)
        self.assertGreaterEqual(len(lights), 1)
        for light in lights:
            self.assertIn('"classname" "light_spot"', light)
            self.assertIn('"_bake_only" "0"', light)
            self.assertIn(
                f'"_falloff_range" "{GENERATOR.HALLWAY_SPOT_FALLOFF}"', light,
            )

    def test_ordinary_cells_are_not_merged_across_reserved_bands(self):
        rooms = GENERATOR.tile_layer(2, 2, None, set())
        self.assertEqual(len(set(rooms.values())), 4)


class LiftAuthoringTests(unittest.TestCase):
    def test_even_lift_is_a_multi_brush_car_with_a_carried_dynamic_light(self):
        entities = GENERATOR.lift_entities(0, 0, 0, 64, GENERATOR.PITCH_Z, "lift")
        mover, _, _, light = entities

        self.assertIn('"classname" "kinematic_mover"', mover)
        self.assertIn('"name" "warren_lift_0"', mover)
        self.assertEqual(mover.count("{") - 1, 5)
        self.assertIn('"classname" "light_dynamic"', light)
        self.assertIn('"carrier" "warren_lift_0"', light)

    def test_odd_lift_has_the_same_car_but_no_cabin_light(self):
        entities = GENERATOR.lift_entities(1, 0, 0, 64, GENERATOR.PITCH_Z, "lift")
        self.assertEqual(len(entities), 3)
        self.assertEqual(entities[0].count("{") - 1, 5)


class DoorAndClosetAuthoringTests(unittest.TestCase):
    def test_use_door_emits_an_action_button_trigger(self):
        entities = GENERATOR.door_entities(0, "x", 0, 0, 64, "door", "use")
        trigger = entities[-1]
        self.assertIn('"activation" "use"', trigger)
        self.assertIn('"target_tag" "door"', trigger)

    def test_monster_closet_has_one_shot_door_trigger_and_scoped_spawner(self):
        brushes, entities = GENERATOR.monster_closet_entities(2, 0, 256, 64, 576)
        self.assertEqual(len(brushes), 6)
        mover, _, _, trigger, spawner = entities
        self.assertIn('"classname" "kinematic_mover"', mover)
        self.assertIn('"on_fire" "warren.closet.2.spawn"', trigger)
        self.assertIn('"fire_mode" "once"', trigger)
        self.assertIn('"_tags" "warren_closet_spawner_2"', spawner)

    def test_closets_do_not_require_other_gameplay_content(self):
        result = GENERATOR.generate(
            3, 3, 1, 1, 0.15, 0.5, "none", 1, 0, 0.2, 0.5, 1, True,
            0, 0, 0, 0, "touch", 0, 1, 0.0,
        )
        self.assertEqual(result[10], 1)


class RoomLightingTests(unittest.TestCase):
    def test_static_room_contract_is_four_spots_and_one_dim_bake_only_point(self):
        lights = []
        added, scripted = GENERATOR.emit_room_lights(
            lights, 0, 1024, 0, 1024, 0, 512, [], GENERATOR.random.Random(1),
            "static", 4, 1, 1.0, 0.0, 0, [0],
        )

        self.assertEqual((added, scripted, len(lights)), (5, 0, 5))
        self.assertIn('"classname" "light"', lights[0])
        self.assertIn('"light" "80"', lights[0])
        self.assertIn('"_bake_only" "1"', lights[0])
        for spotlight in lights[1:]:
            self.assertIn('"classname" "light_spot"', spotlight)
            self.assertIn('"light" "220"', spotlight)
            self.assertIn('"_bake_only" "1"', spotlight)

    def test_dim_point_range_reaches_a_large_room_corner(self):
        lights = []
        zf, zc = 64, 1536
        x1i, y1i = 4096, 2048
        GENERATOR.emit_room_lights(
            lights, 0, x1i, 0, y1i, zf, zc, [], GENERATOR.random.Random(1),
            "static", 4, 1, 1.0, 0.0, 0, [0],
        )
        expected = math.ceil(math.sqrt(
            (x1i / 2) ** 2 + (y1i / 2) ** 2 + (zc - 24 - zf) ** 2
        ) + GENERATOR.LIGHT_MARGIN)
        self.assertIn(f'"_falloff_range" "{expected}"', lights[0])


if __name__ == "__main__":
    unittest.main()
