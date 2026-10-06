"""Focused source-map tests for the stress-warren generator."""

import contextlib
import importlib.util
import io
import math
import tempfile
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
    def test_warren_animation_budget_emits_both_animation_sources(self):
        lights = []
        budget = [GENERATOR.ANIMATED_LIGHT_CAP]
        scripted = 0
        for room in range(2):
            _, room_scripted = GENERATOR.emit_room_lights(
                lights, room * 2048, room * 2048 + 1024, 0, 1024, 0, 512, [],
                GENERATOR.random.Random(room + 1), "static", 4, 1, 1.0, 1.0,
                len(lights), budget,
            )
            scripted += room_scripted

        animated = [
            light for light in lights
            if any(
                "brightness_curve" in line or GENERATOR.SCRIPT_LIGHT_TAG in line
                for line in light
            )
        ]
        self.assertEqual(len(animated), GENERATOR.ANIMATED_LIGHT_CAP)
        self.assertGreater(scripted, 0)
        self.assertTrue(any(
            any("brightness_curve" in line for line in light) for light in animated
        ))
        self.assertTrue(any(
            any(GENERATOR.SCRIPT_LIGHT_TAG in line for line in light) for light in animated
        ))

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


VOLUME_CLASSES = ('"classname" "fluid_volume"', '"classname" "gravity_volume"')


def generate_env(seed=1, volumes=0, emitters=4, enemies=6, grid=(3, 3, 2)):
    """A small env-volume map: (brushes, spawn, entities)."""
    nx, ny, nz = grid
    result = GENERATOR.generate(
        nx, ny, nz, seed, 0.15, 0.5, "none", 1, 0, 1.0, 0.5, 4, True,
        0, enemies, 0, 0, "touch", 0, 0, 0.0,
        n_env_volumes=volumes, n_emitters=emitters,
        emitter_rate=50.0, emitter_lifetime=5.0,
    )
    return result[0], result[1], result[3]


def is_volume(entity):
    return entity[1] in VOLUME_CLASSES


def kvp(entity, key):
    prefix = f'"{key}" "'
    for line in entity:
        if line.startswith(prefix):
            return line[len(prefix):-1]
    return None


def brush_planes(entity):
    """Each brush of a brush entity as a list of three-point planes."""
    brushes, current = [], None
    for line in entity[1:-1]:
        if line == "{":
            current = []
        elif line == "}":
            brushes.append(current)
            current = None
        elif current is not None:
            parts = line.replace("(", " ").replace(")", " ").split()
            nums = [float(v) for v in parts[:9]]
            current.append((tuple(nums[0:3]), tuple(nums[3:6]), tuple(nums[6:9])))
    return brushes


class EnvironmentVolumeTests(unittest.TestCase):
    def test_volume_count_is_honoured(self):
        for count in (1, 37, 250):
            _, _, entities = generate_env(volumes=count)
            self.assertEqual(sum(1 for e in entities if is_volume(e)), count)

    def test_zero_volumes_is_the_volume_map_minus_its_volume_entities(self):
        brushes0, spawn0, entities0 = generate_env(volumes=0)
        brushes1, spawn1, entities1 = generate_env(volumes=300)
        self.assertEqual(brushes0, brushes1)
        self.assertEqual(spawn0, spawn1)
        self.assertEqual(entities0, [e for e in entities1 if not is_volume(e)])
        # Volumes come last, so the baseline file is a strict prefix of the
        # volume file: entity numbering of everything else is unchanged.
        first = next(n for n, e in enumerate(entities1) if is_volume(e))
        self.assertFalse(any(not is_volume(e) for e in entities1[first:]))

    def test_preset_baseline_file_is_a_prefix_of_the_volume_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            full = Path(tmp) / "full.map"
            base = Path(tmp) / "base.map"
            sidecar = Path(tmp) / "full.generated.ts"
            with contextlib.redirect_stdout(io.StringIO()):
                GENERATOR.main(["--preset", "env-volumes", "--env-volumes", "300",
                                "-o", str(full)])
                GENERATOR.main(["--preset", "env-volumes", "--env-volumes", "0",
                                "--data-script", str(sidecar), "-o", str(base)])
            full_text, base_text = full.read_text(), base.read_text()
        self.assertTrue(full_text.startswith(base_text))
        self.assertEqual(full_text.count('"classname" "fluid_volume"')
                         + full_text.count('"classname" "gravity_volume"'), 300)
        self.assertNotIn("fluid_volume", base_text)
        self.assertNotIn("gravity_volume", base_text)

    def test_generation_is_deterministic_under_the_seed(self):
        self.assertEqual(generate_env(seed=7, volumes=200),
                         generate_env(seed=7, volumes=200))
        a = [e for e in generate_env(seed=7, volumes=200)[2] if is_volume(e)]
        b = [e for e in generate_env(seed=8, volumes=200)[2] if is_volume(e)]
        self.assertNotEqual(a, b)

    def test_volume_kvps_are_present_and_well_formed(self):
        _, _, entities = generate_env(volumes=400)
        volumes = [e for e in entities if is_volume(e)]
        fluids, gravities, priorities = set(), set(), []
        for entity in volumes:
            priorities.append(int(kvp(entity, "priority")))
            if entity[1] == VOLUME_CLASSES[0]:
                self.assertIn(kvp(entity, "fluid"), GENERATOR.stress_environment.FLUIDS)
                self.assertIsNone(kvp(entity, "gravity"))
                fluids.add(kvp(entity, "fluid"))
            else:
                self.assertIsNone(kvp(entity, "fluid"))
                gravities.add(kvp(entity, "gravity"))
        self.assertEqual(fluids, set(GENERATOR.stress_environment.FLUIDS))
        self.assertIn("0 0 0", gravities)
        # Mixed priorities, with ties.
        self.assertGreater(len(set(priorities)), 1)
        self.assertLess(len(set(priorities)), len(priorities))

    def test_gravity_is_straight_down_or_zero(self):
        _, _, entities = generate_env(volumes=400)
        for entity in entities:
            if entity[1] != VOLUME_CLASSES[1]:
                continue
            x, y, z = (float(v) for v in kvp(entity, "gravity").split())
            self.assertEqual((x, y), (0.0, 0.0))
            self.assertLessEqual(z, 0.0)

    def test_every_volume_brush_is_a_convex_hull_with_outward_planes(self):
        _, _, entities = generate_env(volumes=400)
        multi_brush = non_axis_aligned = 0
        for entity in (e for e in entities if is_volume(e)):
            brushes = brush_planes(entity)
            self.assertGreaterEqual(len(brushes), 1)
            multi_brush += len(brushes) > 1
            for planes in brushes:
                self.assertGreaterEqual(len(planes), 5)
                points = [p for plane in planes for p in plane]
                inside = tuple(sum(p[i] for p in points) / len(points) for i in range(3))
                for p1, p2, p3 in planes:
                    n = cross(sub(p3, p1), sub(p2, p1))
                    self.assertGreater(math.sqrt(dot(n, n)), 0.0)
                    self.assertLess(dot(n, sub(inside, p1)), 0.0)
                    unit = [abs(c) / math.sqrt(dot(n, n)) for c in n]
                    non_axis_aligned += max(unit) < 0.999
        self.assertGreater(multi_brush, 0)
        self.assertGreater(non_axis_aligned, 0)

    def test_some_volumes_span_a_doorway(self):
        doors = {(0, "x", 1, 0): 500}
        X, Y = [0, GENERATOR.LATTICE_PITCH_XY], [0]
        X_END = [x + GENERATOR.PITCH_XY for x in X]
        boxes = GENERATOR.doorway_boxes(doors, X, X_END, Y, [GENERATOR.PITCH_XY],
                                        [0, GENERATOR.PITCH_Z])
        (x0, _, x1, _, _, _), = boxes
        # Through both room walls and the corridor band between them.
        self.assertLess(x0, X_END[0] - GENERATOR.WALL_T // 2)
        self.assertGreater(x1, X[1] + GENERATOR.WALL_T // 2)


def generate_zoned(volumes=1000):
    """The env-volumes preset's zoned map: (entities, zone_index)."""
    result = GENERATOR.generate(
        8, 6, 3, 1, 0.3, 1.0, "static", 1, 0, 1.0, 0.5, 4, True,
        1, 64, 8, 6, "use", 2, 3, 1.0,
        n_env_volumes=volumes, n_emitters=40, emitter_rate=50.0,
        emitter_lifetime=5.0, zoned=True,
    )
    return result[3], result[12]


def origin_of(entity):
    value = kvp(entity, "origin")
    return tuple(float(v) for v in value.split()) if value else None


def inside(point, rect):
    zf, zc, x0, x1, y0, y1 = rect
    x, y, z = point
    return x0 <= x <= x1 and y0 <= y <= y1 and zf - 64 <= z <= zc


def brush_points(entity):
    return [p for planes in brush_planes(entity) for plane in planes for p in plane]


class ZonedLayoutTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.entities, cls.zones = generate_zoned()

    def rooms(self, zone):
        return [rect for name, _, rect in self.zones if name == zone]

    def test_every_zone_is_indexed(self):
        names = {name for name, _, _ in self.zones}
        self.assertEqual(names, set(GENERATOR.stress_zones.POLICY))
        self.assertEqual(len(self.rooms("water")), 2)

    def test_water_only_rooms_hold_nothing_live(self):
        live = ('"classname" "reference_enemy"', '"classname" "billboard_emitter"',
                '"classname" "kinematic_mover"', '"classname" "entity_spawner"',
                '"classname" "trigger_volume"', '"classname" "light_dynamic"',
                '"classname" "light_dynamic_spot"', '"classname" "fog_volume"')
        for rect in self.rooms("water"):
            volumes = 0
            for entity in self.entities:
                if is_volume(entity):
                    pts = brush_points(entity)
                    volumes += all(inside(p, rect) for p in pts)
                    continue
                point = origin_of(entity) or (brush_points(entity) or [None])[0]
                if point is not None and inside(point, rect):
                    self.assertNotIn(entity[1], live)
            self.assertGreater(volumes, 10)

    def test_every_lift_carries_a_dynamic_light(self):
        lifts = [kvp(e, "name") for e in self.entities
                 if e[1] == '"classname" "kinematic_mover"'
                 and (kvp(e, "name") or "").startswith("warren_lift_")]
        carried = {kvp(e, "carrier") for e in self.entities if kvp(e, "carrier")}
        self.assertEqual(len(lifts), 2)
        self.assertTrue(set(lifts) <= carried)

    def test_mixed_zone_holds_most_volumes_agents_and_particles(self):
        volumes = [e for e in self.entities if is_volume(e)]
        self.assertEqual(len(volumes), 1000)
        enemies = [e for e in self.entities if e[1] == '"classname" "reference_enemy"']
        arena = self.rooms("arena")[0]
        in_arena = sum(inside(origin_of(e), arena) for e in enemies)
        self.assertEqual(len(enemies), 64)
        self.assertEqual(in_arena, GENERATOR.stress_zones.ARENA_ENEMIES)
        # Arena enemies are spread out, not stacked on one spawn point.
        arena_spots = {kvp(e, "origin") for e in enemies if inside(origin_of(e), arena)}
        self.assertEqual(len(arena_spots), GENERATOR.stress_zones.ARENA_ENEMIES)
        weapons = [e for e in self.entities
                   if any(e[1] == f'"classname" "{c}"' for c in GENERATOR.WEAPON_CLASSES)]
        self.assertGreaterEqual(sum(inside(origin_of(e), arena) for e in weapons), 4)
        emitters = [e for e in self.entities if e[1] == '"classname" "billboard_emitter"']
        particle_room = self.rooms("particles")[0]
        self.assertEqual(sum(inside(origin_of(e), particle_room) for e in emitters),
                         GENERATOR.stress_zones.PARTICLE_ZONE_EMITTERS)

    def test_combination_features_are_placed(self):
        classes = {e[1] for e in self.entities}
        for name in ("fog_volume", "fog_lamp", "fog_tube", "switch", "prop_mesh",
                     "trigger_volume", "entity_spawner", "light_dynamic_spot"):
            self.assertIn(f'"classname" "{name}"', classes)
        carousel = [e for e in self.entities if kvp(e, "spin_axis")]
        self.assertEqual(len(carousel), 1)
        animated = [e for e in self.entities
                    if any("brightness_curve" in l or GENERATOR.SCRIPT_LIGHT_TAG in l
                           for l in e)]
        self.assertLessEqual(len(animated), GENERATOR.ANIMATED_LIGHT_CAP)
        room = self.rooms("animated")[0]
        self.assertEqual(sum(inside(origin_of(e), room) for e in animated), len(animated))

    def test_zero_volume_zoned_map_differs_only_by_volumes(self):
        base, _ = generate_zoned(volumes=0)
        self.assertEqual(base, [e for e in self.entities if not is_volume(e)])


class ParticleEmitterTests(unittest.TestCase):
    def test_emitters_hold_the_requested_steady_state_under_the_cap(self):
        _, _, entities = generate_env(volumes=0, emitters=9)
        emitters = [e for e in entities if e[1] == '"classname" "billboard_emitter"']
        self.assertEqual(len(emitters), 9)
        for emitter in emitters:
            live = float(kvp(emitter, "rate")) * float(kvp(emitter, "lifetime"))
            self.assertLessEqual(live, GENERATOR.stress_environment.MAX_SPRITES_PER_EMITTER)
        buoyancy = {float(kvp(e, "buoyancy")) for e in emitters}
        self.assertTrue(any(b > 0 for b in buoyancy) and any(b < 0 for b in buoyancy))

    def test_rate_over_the_per_emitter_cap_is_rejected(self):
        with self.assertRaises(ValueError):
            GENERATOR.stress_environment.emitter_entities(
                1, 1, [(0, 512, 0, 1024, 0, 1024)], 1000.0, 5.0)


def sub(a, b):
    return (a[0] - b[0], a[1] - b[1], a[2] - b[2])


def cross(a, b):
    return (a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0])


def dot(a, b):
    return a[0] * b[0] + a[1] * b[1] + a[2] * b[2]


if __name__ == "__main__":
    unittest.main()
