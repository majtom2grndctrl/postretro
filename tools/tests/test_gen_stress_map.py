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


if __name__ == "__main__":
    unittest.main()
