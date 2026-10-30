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

    def test_hallway_spots_sit_in_open_air_centred_across_their_band(self):
        # Regression: spots were spaced along both axes of a band at once, so
        # they walked diagonally off its centre line, and junction spots sat
        # inside the diagonal guide fin. The compiler drops a light buried in
        # solid, so every such spot was lost.
        band = GENERATOR.CORRIDOR_BAND
        pitch = GENERATOR.LATTICE_PITCH_XY
        starts = [0, pitch, 2 * pitch]
        centres = {start + GENERATOR.PITCH_XY + band // 2 for start in starts}
        fins = 0
        for seed in range(3):
            brushes, lights = [], []
            GENERATOR.emit_reserved_corridor_grid(
                brushes, starts, starts, 64, "test", seed,
                hallway_lights=lights)
            solids = [brush_text_planes(brush) for brush in brushes]
            # Only the oriented guide fins carry fractional coordinates.
            fins += sum("." in brush for brush in brushes)
            for light in lights:
                point = origin_of(light)
                for planes in solids:
                    self.assertFalse(
                        point_inside_planes(point, planes),
                        f"seed {seed}: spot {point} is inside a corridor brush",
                    )
                x, y, _ = point
                at_junction = (any(abs(x - c) <= band // 2 for c in centres)
                               and any(abs(y - c) <= band // 2 for c in centres))
                if not at_junction:
                    self.assertTrue(
                        x in centres or y in centres,
                        f"seed {seed}: spot {point} is off its band's centre line",
                    )
        self.assertGreater(fins, 0, "some seed must exercise a finned junction")

    def test_room_lights_inside_a_closet_move_to_its_front(self):
        inside_wall = GENERATOR.light_entity(
            "static", (-236, 400, 552), (255, 255, 255), 1024, 180, True, None)
        outside = GENERATOR.light_entity(
            "static", (0, 100, 552), (255, 255, 255), 1024, 180, True, None)
        GENERATOR.move_lights_out_of_closet([inside_wall, outside], 0, 256, 64, 576)
        front = 256 - (GENERATOR.CLOSET_TRIGGER_REACH - GENERATOR.LIGHT_MARGIN)
        self.assertEqual(origin_of(inside_wall), (-236.0, float(front), 552.0))
        self.assertEqual(origin_of(outside), (0.0, 100.0, 552.0))

    def test_closet_light_move_accepts_non_integer_origins(self):
        fractional = GENERATOR.light_entity(
            "static", (-236.5, 400.0, 552.25), (255, 255, 255), 1024, 180, True, None)
        GENERATOR.move_lights_out_of_closet([fractional], 0, 256, 64, 576)
        front = 256 - (GENERATOR.CLOSET_TRIGGER_REACH - GENERATOR.LIGHT_MARGIN)
        self.assertEqual(origin_of(fractional), (-236.5, float(front), 552.25))

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
        self.assertIn('"spawned_tags" "enemy"', spawner)

    def test_placed_enemy_carries_the_enemy_tag(self):
        enemy = GENERATOR.enemy_entity((0, 0, 16), 90)
        self.assertIn('"classname" "reference_enemy"', enemy)
        self.assertIn('"_tags" "enemy"', enemy)

    def test_every_emitted_enemy_and_spawner_carries_the_enemy_tag(self):
        entities = GENERATOR.generate(
            3, 3, 1, 1, 0.15, 0.5, "none", 1, 0, 0.2, 0.5, 1, True,
            0, 4, 0, 0, "touch", 0, 1, 0.0,
        )[3]
        enemies = [e for e in entities if '"classname" "reference_enemy"' in e]
        spawners = [e for e in entities if '"classname" "entity_spawner"' in e]
        self.assertTrue(enemies)
        self.assertTrue(spawners)
        for enemy in enemies:
            self.assertIn('"_tags" "enemy"', enemy)
        for spawner in spawners:
            self.assertIn('"spawned_tags" "enemy"', spawner)

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
        # The default-priority volumes sit in the appended tail too.
        tail = full_text[len(base_text):].split("// entity")
        self.assertTrue(any("_volume" in e and '"priority"' not in e for e in tail))

    def test_a_handful_of_volumes_omit_priority_and_only_explicit_zeros(self):
        entities, _ = generate_zoned()
        volumes = [e for e in entities if is_volume(e)]
        bare = [e for e in volumes if kvp(e, "priority") is None]
        zeros = sum(1 for e in volumes if kvp(e, "priority") == "0") + len(bare)
        every = GENERATOR.stress_environment.DEFAULT_PRIORITY_EVERY
        self.assertEqual(len(bare), math.ceil(zeros / every))
        self.assertGreaterEqual(len(bare), 3)
        self.assertLessEqual(len(bare), 20)
        # Deterministic: the same volumes lose the KVP on every run.
        again = [e for e in generate_zoned()[0] if is_volume(e) and kvp(e, "priority") is None]
        self.assertEqual(bare, again)

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
            priority = kvp(entity, "priority")
            priorities.append(0 if priority is None else int(priority))
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


def generate_zoned(volumes=1000, seed=1, animated_frac=1.0):
    """The env-volumes preset's zoned map: (entities, zone_index)."""
    result = GENERATOR.generate(
        8, 6, 3, seed, 0.3, 1.0, "static", 1, 0, 1.0, 0.5, 4, True,
        1, 64, 8, 6, "use", 2, 3, animated_frac,
        n_env_volumes=volumes, n_emitters=40, emitter_rate=50.0,
        emitter_lifetime=5.0, zoned=True,
    )
    return result[3], result[12]


def is_animated(entity):
    return any("brightness_curve" in line or GENERATOR.SCRIPT_LIGHT_TAG in line
               for line in entity)


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
        return [rect for name, _, rect, _ in self.zones if name == zone]

    def test_every_zone_is_indexed(self):
        names = {name for name, _, _, _ in self.zones}
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
        animated = [e for e in self.entities if is_animated(e)]
        self.assertLessEqual(len(animated), GENERATOR.ANIMATED_LIGHT_CAP)
        room = self.rooms("animated")[0]
        self.assertEqual(sum(inside(origin_of(e), room) for e in animated), len(animated))

    def test_zero_volume_zoned_map_differs_only_by_volumes(self):
        base, _ = generate_zoned(volumes=0)
        self.assertEqual(base, [e for e in self.entities if not is_volume(e)])

    def test_arena_spawns_are_spaced_and_off_the_drop_gap(self):
        arena = self.rooms("arena")[0]
        placed = [origin_of(e) for e in self.entities
                  if (e[1] == f'"classname" "{GENERATOR.ENEMY_CLASS}"'
                      or any(e[1] == f'"classname" "{c}"' for c in GENERATOR.WEAPON_CLASSES))
                  and inside(origin_of(e), arena)]
        self.assertGreaterEqual(len(placed), GENERATOR.stress_zones.ARENA_ENEMIES + 4)
        for n, a in enumerate(placed):
            for b in placed[n + 1:]:
                self.assertGreaterEqual(math.dist(a[:2], b[:2]), GENERATOR.SPAWN_SPACING)
        _, _, x0, x1, y0, y1 = arena
        walk = GENERATOR.ARENA_MEZZ_WALK - GENERATOR.WALL_T // 2   # gap inset from x0i
        for x, y, _ in placed:
            self.assertFalse(x0 + walk <= x <= x1 - walk and y0 + walk <= y <= y1 - walk)


class WarrenPlacementTests(unittest.TestCase):
    def test_warren_spawns_keep_capsule_spacing_across_seeds(self):
        for seed in (0, 1, 7, 42):
            result = GENERATOR.generate(
                6, 5, 3, seed, 0.3, 0.6, "static", 1, 1, 1.0, 0.5, 4, True,
                1, 12, 6, 6, "use", 2, 3, 1.0,
            )
            placed = [origin_of(e) for e in result[3]
                      if e[1] == f'"classname" "{GENERATOR.ENEMY_CLASS}"'
                      or any(e[1] == f'"classname" "{c}"' for c in GENERATOR.WEAPON_CLASSES)]
            self.assertEqual(len(placed), 18)
            for n, a in enumerate(placed):
                for b in placed[n + 1:]:
                    if a[2] == b[2]:
                        self.assertGreaterEqual(math.dist(a[:2], b[:2]),
                                                GENERATOR.SPAWN_SPACING, (seed, a, b))


class ZoneGuaranteeTests(unittest.TestCase):
    SEEDS = (1, 2, 7, 42, 1000)

    def test_doors_and_crates_zones_get_their_features_on_every_seed(self):
        for seed in self.SEEDS:
            entities, zones = generate_zoned(volumes=0, seed=seed)
            by_zone = {name: (rect, missing) for name, _, rect, missing in zones}
            for zone in ("doors", "crates"):
                self.assertEqual(by_zone[zone][1], [], (seed, zone))
            doors_room = by_zone["doors"][0]
            switches = [e for e in entities if e[1] == '"classname" "switch"']
            self.assertTrue(any(inside(brush_points(e)[0], doors_room) for e in switches),
                            seed)
            crates_room = by_zone["crates"][0]
            spots = {e[1] for e in entities
                     if origin_of(e) and inside(origin_of(e), crates_room)}
            self.assertIn('"classname" "light_spot"', spots, seed)
            self.assertIn('"classname" "light_dynamic_spot"', spots, seed)

    def test_an_unplaceable_door_is_reported_not_claimed(self):
        # With a lift taking a top-layer cell, a 3x3x2 grid's doors room has
        # only dedicated neighbours, so no doorway can carry a door; the zone
        # index says so.
        result = GENERATOR.generate(
            3, 3, 2, 1, 0.15, 1.0, "none", 1, 0, 1.0, 0.5, 4, True,
            0, 0, 0, 6, "use", 1, 0, 0.0, zoned=True,
        )
        missing = {name: m for name, _, _, m in result[12]}
        self.assertEqual(missing["doors"], ["door", "switch"])
        out, err = io.StringIO(), io.StringIO()
        with tempfile.TemporaryDirectory() as tmp, contextlib.redirect_stdout(out), \
                contextlib.redirect_stderr(err):
            GENERATOR.main(["--zones", "--grid", "3", "3", "2", "--doors", "6",
                            "--lifts", "1", "--shaft-prob", "1.0",
                            "-o", str(Path(tmp) / "z.map")])
        self.assertIn("warning: zone doors", err.getvalue())
        self.assertNotIn(GENERATOR.stress_zones.ZONE_LABELS["doors"], out.getvalue())

    def test_zones_with_lifts_and_no_lights_apply_zone_lighting(self):
        result = GENERATOR.generate(
            3, 3, 2, 1, 0.15, 1.0, "none", 1, 0, 1.0, 0.5, 4, True,
            0, 0, 0, 0, "touch", 1, 0, 0.0, zoned=True,
        )
        entities, zones = result[3], result[12]
        self.assertEqual(result[9], 1)
        lift_rects = [rect for name, _, rect, _ in zones if name == "lift"]
        self.assertEqual(len(lift_rects), 2)
        water = next(rect for name, _, rect, _ in zones if name == "water")
        self.assertTrue(any(e[1] == '"classname" "light_spot"' and inside(origin_of(e), water)
                            for e in entities))

    def test_animated_frac_zero_emits_no_zone_animated_lights(self):
        entities, _ = generate_zoned(volumes=0, animated_frac=0.0)
        self.assertFalse(any(is_animated(e) for e in entities))
        animated_on, _ = generate_zoned(volumes=0)
        self.assertGreaterEqual(sum(is_animated(e) for e in animated_on),
                                GENERATOR.stress_zones.UNDERWATER_LIGHTS)


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

    def test_negative_and_non_finite_emitter_args_are_rejected(self):
        for flag, value in (("--emitter-rate", "-10"), ("--emitter-rate", "nan"),
                            ("--emitter-rate", "inf"), ("--emitter-lifetime", "-1"),
                            ("--emitter-lifetime", "nan"), ("--emitter-lifetime", "inf")):
            with tempfile.TemporaryDirectory() as tmp, \
                    contextlib.redirect_stderr(io.StringIO()), \
                    self.assertRaises(SystemExit):
                GENERATOR.main(["--emitters", "1", flag, value,
                                "-o", str(Path(tmp) / "e.map")])


def sub(a, b):
    return (a[0] - b[0], a[1] - b[1], a[2] - b[2])


def cross(a, b):
    return (a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0])


def dot(a, b):
    return a[0] * b[0] + a[1] * b[1] + a[2] * b[2]


def brush_text_planes(brush):
    """The three-point planes of one emitted brush string."""
    planes = []
    for line in brush.splitlines():
        if line.startswith("("):
            parts = line.replace("(", " ").replace(")", " ").split()
            nums = [float(v) for v in parts[:9]]
            planes.append((tuple(nums[0:3]), tuple(nums[3:6]), tuple(nums[6:9])))
    return planes


def point_inside_planes(point, planes):
    """True when `point` is strictly inside the convex brush `planes`."""
    corners = [p for plane in planes for p in plane]
    centre = tuple(sum(c[k] for c in corners) / len(corners) for k in range(3))
    for a, b, c in planes:
        normal = cross(sub(b, a), sub(c, a))
        if dot(normal, sub(centre, a)) > 0:
            normal = tuple(-v for v in normal)
        if dot(normal, sub(point, a)) >= 0:
            return False
    return True



# --- Street map (`--preset street`, tools/stress_street.py) -----------------

STREET = GENERATOR.stress_street
STREET_MAP = Path(__file__).parents[2] / "content" / "dev" / "maps" / "stress-street.map"
GAP_M = 0.001   # the classifier's 1 mm touching epsilon


def generate_street():
    return STREET.generate(GENERATOR.box_brush, GENERATOR.SCRIPT_LIGHT_TAG)


def boxes_of(street, role):
    return [r.box for r in street.roles if r.role == role]


def box_gap(a, b):
    """Largest per-axis separation between two boxes; <= 0 means overlap or contact."""
    return max(max(a[k] - b[k + 3], b[k] - a[k + 3]) for k in range(3))


def boxes_overlap(a, b):
    return box_gap(a, b) < -GAP_M


def rect_box(rect, z0, z1):
    return (rect[0], rect[1], z0, rect[2], rect[3], z1)


def extents(box):
    return sorted(box[k + 3] - box[k] for k in range(3))


def kvps(entity):
    out = {}
    for line in entity:
        if line.startswith('"'):
            key, _, value = line[1:-1].partition('" "')
            out[key] = value
    return out


def touching_components(boxes):
    """Connected components of boxes under the 1 mm touching rule."""
    seen, comps = set(), []
    for start in range(len(boxes)):
        if start in seen:
            continue
        stack, comp = [start], []
        seen.add(start)
        while stack:
            n = stack.pop()
            comp.append(n)
            for m in range(len(boxes)):
                if m not in seen and box_gap(boxes[n], boxes[m]) <= GAP_M:
                    seen.add(m)
                    stack.append(m)
        comps.append(comp)
    return comps


class StreetMapTests(unittest.TestCase):
    """Pins every feature the occluder-first-partition street-map Decision lists."""

    @classmethod
    def setUpClass(cls):
        cls.street = generate_street()

    def test_street_snakes_round_a_left_and_a_right_corner(self):
        self.assertEqual(STREET.route_turns(), [1, -1])
        axes = [s["axis"] for s in STREET.SEGMENTS]
        self.assertEqual(axes, [0, 1, 0])

    def test_promenade_runs_the_street_floor_under_the_elevated_street(self):
        st = self.street
        lower = boxes_of(st, "lower_deck")
        # The promenade is the canyon floor from the plaza to the hull's far wall.
        canyon = STREET.band(0.0, STREET.CANYON_W)
        self.assertEqual(canyon[0][0], STREET.PLAZA[2])
        self.assertEqual(canyon[-1][2], STREET.HULL_X)
        soffit = min(b[2] for b in lower)
        self.assertGreaterEqual(soffit, 5.0)
        # Nothing but curbs and deck columns stands in the promenade under the
        # deck; lamp posts are group entities, not world brushes.
        volume = [rect_box(r, 0.0, soffit) for r in canyon]
        for role in st.roles:
            if role.role in ("curb", "column"):
                continue
            for v in volume:
                self.assertFalse(boxes_overlap(role.box, v), (role.role, role.name, role.box))
        # The elevated street covers the promenade along every segment.
        for deck, floor in zip(lower, canyon):
            self.assertGreater(deck[2], 0.0)
            self.assertTrue(boxes_overlap(deck, rect_box(floor, deck[2], deck[5])))
        # The curb and columns are the small brushes the classifier must exempt.
        for curb in boxes_of(st, "curb"):
            self.assertLess(extents(curb)[1], STREET.FLOOR_M)
        for column in boxes_of(st, "column"):
            self.assertLess(extents(column)[1], STREET.FLOOR_M)
            self.assertEqual(column[2], 0.0)
            self.assertEqual(column[5], soffit)

    def test_upper_deck_carries_opposite_traffic_above_the_elevated_street(self):
        st = self.street
        lower, upper = boxes_of(st, "lower_deck"), boxes_of(st, "upper_deck")
        (ld, (_, lower_top)), (ud, (upper_soffit, _)) = STREET.LOWER_DECK, STREET.UPPER_DECK
        self.assertEqual([b[:2] + b[3:5] for b in lower],
                         [r[:2] + r[2:] for r in STREET.band(*ld)])
        self.assertEqual([b[:2] + b[3:5] for b in upper],
                         [r[:2] + r[2:] for r in STREET.band(*ud)])
        # One deck hangs off each facade, so the two directions run apart.
        self.assertEqual(ld[1], STREET.CANYON_W)
        self.assertEqual(ud[0], 0.0)
        self.assertTrue(all(b[5] == lower_top for b in lower))
        self.assertGreaterEqual(upper_soffit - lower_top, 4.0)   # headroom on the deck
        # Ramps join plaza, elevated street and upper deck.
        ramps = {r.name: r.box for r in st.roles if r.role == "ramp"}
        self.assertEqual(ramps["plaza"][2], 0.0)
        self.assertEqual(ramps["plaza"][5], lower_top)
        self.assertEqual(ramps["deck"][2], lower_top)
        self.assertEqual(ramps["deck"][5], upper[0][5])

    def test_both_decks_snake_round_both_corners(self):
        for role in ("lower_deck", "upper_deck"):
            decks = boxes_of(self.street, role)
            self.assertEqual(len(decks), len(STREET.SEGMENTS))
            for deck, seg in zip(decks, STREET.SEGMENTS):
                long_axis = 0 if deck[3] - deck[0] > deck[4] - deck[1] else 1
                self.assertEqual(long_axis, seg["axis"], role)
                # Every deck slab passes the absolute length on its long axis.
                self.assertGreaterEqual(extents(deck)[2], STREET.ABSOLUTE_M)
            for a, b in zip(decks, decks[1:]):
                self.assertLessEqual(box_gap(a, b), 0.0, f"{role} breaks at a corner")
                self.assertFalse(boxes_overlap(a, b))

    def test_every_side_street_is_closed_by_a_barrier_and_a_deck_guard(self):
        st = self.street
        frontages, gaps = STREET.frontages()
        self.assertGreaterEqual(len(gaps), 3)
        self.assertEqual(len(st.side_streets), len(gaps))
        barriers, guards = boxes_of(st, "barrier"), boxes_of(st, "deck_guard")
        self.assertEqual(len(barriers), len(gaps))
        self.assertEqual(len(guards), len(gaps))
        lower_top = STREET.LOWER_DECK[1][1]
        for g in st.side_streets:
            rect = g["rect"]
            width = (rect[2] - rect[0]) if STREET.SEGMENTS[g["seg"]]["axis"] == 0 else (rect[3] - rect[1])
            # The ground barrier spans the side street wall to wall, stands on
            # its floor and is taller than a player can reach.
            ground = [b for b in barriers if boxes_overlap(b, rect_box(rect, 0.0, 1.0))]
            self.assertEqual(len(ground), 1)
            b = ground[0]
            self.assertAlmostEqual(extents(b)[2], width)
            self.assertEqual(b[2], 0.0)
            self.assertGreater(b[5] - b[2], STREET.PLAYER_REACH_M)
            # The guard closes the elevated street's edge across the gap.
            gap_rect = STREET.span_rect(g["seg"], g["lo"], g["hi"], 0.0, STREET.SIDE_STREET_D[1])
            deck_guard = [b for b in guards if boxes_overlap(b, rect_box(gap_rect, 0.0, 20.0))]
            self.assertEqual(len(deck_guard), 1)
            b = deck_guard[0]
            self.assertAlmostEqual(extents(b)[2], width)
            self.assertEqual(b[2], lower_top)
            self.assertGreater(b[5] - b[2], STREET.PLAYER_REACH_M)
            # Past the barrier the street dead-ends against solid: a block or the hull.
            blocks = boxes_of(st, "block") + boxes_of(st, "hull")
            self.assertTrue(any(box_gap(blk, rect_box(rect, 0.0, 1.0)) <= GAP_M
                                and not boxes_overlap(blk, rect_box(rect, 0.0, 1.0))
                                for blk in blocks))
        # Side B is buildings and side streets only: they tile its facade band.
        for i in range(len(STREET.SEGMENTS)):
            spans = sorted([(f["lo"], f["hi"]) for f in frontages
                            if f["side"] == "B" and f["seg"] == i]
                           + [(g["lo"], g["hi"]) for g in gaps if g["seg"] == i])
            lo, hi = STREET.band_along(i, STREET.CANYON_W, STREET.CANYON_W + STREET.FACADE_T)
            self.assertEqual(spans[0][0], lo)
            self.assertEqual(spans[-1][1], hi)
            self.assertTrue(all(a[1] == b[0] for a, b in zip(spans, spans[1:])))

    def test_abutting_row_is_continuous_round_both_corners(self):
        frontages, gaps = STREET.frontages()
        row = [f for f in frontages if f["side"] == "A"]
        self.assertEqual({g["side"] for g in gaps}, {"B"})
        for i in range(len(STREET.SEGMENTS)):
            spans = [(f["lo"], f["hi"]) for f in row if f["seg"] == i]
            lo, hi = STREET.band_along(i, -STREET.FACADE_T, 0.0)
            self.assertEqual(spans[0][0], lo)
            self.assertEqual(spans[-1][1], hi)
            self.assertTrue(all(a[1] == b[0] for a, b in zip(spans, spans[1:])))
        # The row's strips touch from the plaza to the hull, through both
        # corners, so the whole row is one cluster.
        strips = [r.box for r in self.street.roles
                  if r.role == "facade_strip" and r.name.startswith("A")]
        self.assertEqual(len(touching_components(strips)), 1)
        self.assertGreaterEqual(len({f["name"] for f in row}), 6)

    def test_facades_are_narrow_strips_round_windows(self):
        st = self.street
        frontages = {(f["name"], f["seg"], f["lo"]): f for f in st.frontages}
        self.assertTrue(frontages)
        for role in ("facade_strip", "free_strip"):
            for box in boxes_of(st, role):
                # Exempt alone: under the floor on one of the two broad axes.
                self.assertLess(extents(box)[1], STREET.FLOOR_M, role)
        coverages = []
        for f in st.frontages:
            self.assertGreater(f["height"], STREET.SEE_OVER_M)
            # Windows: the strips leave openings, and they cover the face
            # without overlapping.
            self.assertLess(f["coverage"], 0.75)
            self.assertGreater(f["coverage"], 0.4)
            coverages.append(f["coverage"])
            mine = [r.box for r in st.roles if r.role == "facade_strip"
                    and r.name == f["name"]]
            for a in range(len(mine)):
                for b in range(a + 1, len(mine)):
                    self.assertFalse(boxes_overlap(mine[a], mine[b]))
        # Coverage brackets the 50 % cluster constant, so the sweep shows both.
        self.assertTrue(any(c < STREET.COVERAGE for c in coverages))
        self.assertTrue(any(c > STREET.COVERAGE for c in coverages))
        # Each building's facade is one touching cluster, and the windows see
        # into a room behind it (the room gets a roof) unless it is a corner piece.
        for name in {f["name"] for f in st.frontages}:
            mine = [r.box for r in st.roles if r.role == "facade_strip" and r.name == name]
            self.assertEqual(len(touching_components(mine)), 1, name)
        roofs = {r.name for r in st.roles if r.role == "roof"}
        for f in st.frontages:
            self.assertEqual(f["name"] in roofs, f["room"])
        # Frontages straddle the absolute length.
        lengths = [f["hi"] - f["lo"] for f in st.frontages]
        self.assertTrue(any(v < STREET.ABSOLUTE_M for v in lengths))
        self.assertTrue(any(v > STREET.ABSOLUTE_M for v in lengths))

    def test_a_free_standing_building_stands_in_the_open_plaza(self):
        st = self.street
        strips = boxes_of(st, "free_strip")
        roof = boxes_of(st, "free_roof")
        self.assertEqual(len(roof), 1)
        (x0, y0, x1, y1), storeys, _ = STREET.FREE_BUILDING
        px0, py0, px1, py1 = STREET.PLAZA
        self.assertTrue(px0 < x0 < x1 < px1 and py0 < y0 < y1 < py1)
        # Four strip facades, one cluster with each corner closed.
        self.assertEqual(len(touching_components(strips)), 1)
        self.assertEqual(min(b[0] for b in strips), x0)
        self.assertEqual(max(b[3] for b in strips), x1)
        self.assertEqual(min(b[1] for b in strips), y0)
        self.assertEqual(max(b[4] for b in strips), y1)
        self.assertEqual(roof[0][5], STREET.building_height(storeys))
        # Open on every side: no other solid comes within 8 m of it.
        own = rect_box((x0, y0, x1, y1), 0.0, STREET.SKY_Z)
        for r in st.roles:
            if r.role in ("free_strip", "free_roof") or r.name in ("ground", "sky"):
                continue
            self.assertGreaterEqual(box_gap(own, r.box), 8.0, (r.role, r.name))
        # One pair of faces passes the absolute length; the other does not.
        self.assertGreater(y1 - y0, STREET.ABSOLUTE_M)
        self.assertLess(x1 - x0, STREET.ABSOLUTE_M)

    def test_lamp_posts_are_linked_groups_each_holding_a_light(self):
        st = self.street
        groups = [{**kvps(e), "_lines": e} for e in st.entities
                  if kvps(e).get("classname") == "func_group"]
        lights = [kvps(e) for e in st.entities if kvps(e).get("classname") == "light"]
        self.assertEqual(len(groups), len(STREET.LAMPS))
        self.assertGreaterEqual(len(groups), 20)
        ids = [g["_tb_id"] for g in groups]
        self.assertEqual(len(set(ids)), len(ids))
        base = [box for _, box in STREET.LAMP_BASE]
        for g in groups:
            self.assertEqual(g["_tb_type"], "_tb_group")
            self.assertEqual(g["_tb_linked_group_id"], STREET.LAMP_LINKED_GROUP)
            self.assertRegex(g["_tb_linked_group_id"], r"^\{[0-9a-f-]{36}\}$")
            matrix = tuple(int(v) for v in g["_tb_transformation"].split())
            self.assertEqual(len(matrix), 16)
            # Each instance is the base lamp under its own transformation.
            brushes = "\n".join(g["_lines"])
            for box in base:
                tb = STREET.transform_box(matrix, box)
                expect = GENERATOR.box_brush(*tb, STREET.LAMP_TEX, STREET.LAMP_TEX,
                                             STREET.LAMP_TEX).rstrip("\n")
                self.assertIn(expect, brushes)
            self.assertEqual(brushes.count("// brush"), len(base))
            held = [L for L in lights if L.get("_tb_group") == g["_tb_id"]]
            self.assertEqual(len(held), 1)
            origin = tuple(int(v) for v in held[0]["origin"].split())
            self.assertEqual(origin, STREET.apply_transform(matrix, STREET.LAMP_LIGHT))
        # The base lamp: touching pole, arm and head; the light under the head.
        self.assertEqual(len(touching_components([tuple(v * STREET.UNIT_M for v in b)
                                                  for b in base])), 1)
        head = dict(STREET.LAMP_BASE)["head"]
        self.assertLess(STREET.LAMP_LIGHT[2], head[2])
        self.assertTrue(head[0] <= STREET.LAMP_LIGHT[0] <= head[3])
        # Lamps stand on the promenade, both decks and the plaza.
        floors = {t[2] for _, t in STREET.lamp_placements()}
        self.assertEqual(floors, {0, STREET.u(6.0), STREET.u(11.0)})

    def test_map_is_sealed_by_its_hull_with_a_player_start(self):
        st = self.street
        hull = {r.name: r.box for r in st.roles if r.role == "hull"}
        self.assertEqual(set(hull), {"ground", "sky", "west", "east", "south", "north"})
        inner = (0.0, 0.0, 0.0, STREET.HULL_X, STREET.HULL_Y, STREET.SKY_Z)
        # The six slabs close the interior: each touches it on one face.
        for box in hull.values():
            self.assertLessEqual(box_gap(box, inner), 0.0)
            self.assertFalse(boxes_overlap(box, inner))
        eps = 1e-9
        for r in st.roles:
            if r.role == "hull":
                continue
            self.assertTrue(all(r.box[k] >= inner[k] - eps and r.box[k + 3] <= inner[k + 3] + eps
                                for k in range(3)), (r.role, r.name, r.box))
        # Every point entity sits in open air inside the hull, none buried.
        solids = [brush_text_planes(b) for b in st.brushes]
        for _, _, boxes, _ in st.lamps:
            solids += [brush_text_planes(GENERATOR.box_brush(*b, "t", "t", "t")) for b in boxes]
        points = [tuple(float(v) for v in kvps(e)["origin"].split())
                  for e in st.entities if "origin" in kvps(e)]
        points.append(tuple(float(v) for v in st.spawn))
        self.assertGreaterEqual(len(points), len(STREET.LAMPS) + 2)
        for p in points:
            self.assertTrue(all(0 < p[k] < STREET.u(inner[k + 3]) for k in range(3)), p)
            self.assertFalse(any(point_inside_planes(p, planes) for planes in solids), p)
        # The player starts in the plaza on its floor.
        sx, sy, sz = st.spawn
        px0, py0, px1, py1 = STREET.PLAZA
        self.assertTrue(STREET.u(px0) < sx < STREET.u(px1) and STREET.u(py0) < sy < STREET.u(py1))
        self.assertEqual(sz, 32)

    def test_committed_map_and_sidecar_match_the_generator(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / STREET_MAP.name
            with contextlib.redirect_stdout(io.StringIO()):
                GENERATOR.main(["--preset", "street", "-o", str(out)])
            text = out.read_text()
            sidecar = out.with_name("stress-street.generated.ts").read_text()
        self.assertIn('"data_script" "stress-street.generated.ts"', text)
        # The sidecar pulses the map's tagged lights.
        self.assertIn(GENERATOR.SCRIPT_LIGHT_TAG, sidecar)
        self.assertEqual(text.count(f'"_tags" "{GENERATOR.SCRIPT_LIGHT_TAG}"'),
                         len(STREET.NEON))
        self.assertEqual(text, STREET_MAP.read_text(),
                         "regenerate: python3 tools/gen_stress_map.py --preset street "
                         "-o content/dev/maps/stress-street.map")
        self.assertEqual(sidecar, STREET_MAP.with_name("stress-street.generated.ts").read_text())

    def test_street_generation_is_deterministic(self):
        again = generate_street()
        self.assertEqual(again.brushes, self.street.brushes)
        self.assertEqual(again.entities, self.street.entities)


if __name__ == "__main__":
    unittest.main()
