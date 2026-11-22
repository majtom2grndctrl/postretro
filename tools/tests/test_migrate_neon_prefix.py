"""Tests for migrate_neon_prefix.py on throwaway mod trees.

Run with `python3 -m unittest discover -s tools/tests -p test_migrate_neon_prefix.py`.
"""

import contextlib
import importlib.util
import io
import tempfile
import tomllib
import unittest
from pathlib import Path


def load_module():
    script = Path(__file__).parents[1] / "migrate_neon_prefix.py"
    spec = importlib.util.spec_from_file_location("migrate_neon_prefix_test", script)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


MIGRATE = load_module()

FACE = "( 0 0 0 ) ( 0 1 0 ) ( 1 0 0 ) {tex} 0 0 0 1 1\n"


def map_text(*textures):
    faces = "".join(FACE.format(tex=tex) for tex in textures)
    return '{\n"classname" "worldspawn"\n{\n' + faces + "}\n}\n"


class ModTree:
    """A mod root in a temporary directory."""

    def __init__(self):
        self._dir = tempfile.TemporaryDirectory()
        self.root = Path(self._dir.name)
        (self.root / "textures").mkdir()
        (self.root / "maps").mkdir()

    def close(self):
        self._dir.cleanup()

    def texture(self, collection, *stems):
        directory = self.root / "textures" / collection
        directory.mkdir(parents=True, exist_ok=True)
        for stem in stems:
            (directory / f"{stem}.png").write_bytes(f"png:{collection}/{stem}".encode())

    def map(self, name, *textures):
        (self.root / "maps" / name).write_text(map_text(*textures))

    def toml(self, text):
        (self.root / "materials.toml").write_text(text)

    def snapshot(self):
        return {
            str(path.relative_to(self.root)): path.read_bytes()
            for path in sorted(self.root.rglob("*"))
            if path.is_file()
        }

    def run(self, *args):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = MIGRATE.main([str(self.root), *args])
        return code, out.getvalue(), err.getvalue()

    def read(self, relative):
        return (self.root / relative).read_text()


class MigrationTest(unittest.TestCase):
    def setUp(self):
        self.mod = ModTree()
        self.addCleanup(self.mod.close)

    def standard_mod(self):
        self.mod.texture("neon", "neon_glow", "neon_glow_e", "neon_glow_s", "neon_glow_n",
                         "neon_glow_h", "neon_glow_p", "neon_glow_t", "neon_plain", "other_wall")
        self.mod.map("a.map", "neon/neon_glow", "neon/other_wall", "NEON/Neon_Glow")
        self.mod.map("b.map", "textures\\neon\\neon_glow", "neon_plain", '"neon/neon_glow"')

    def test_dry_run_lists_the_plan_and_writes_nothing(self):
        self.standard_mod()
        before = self.mod.snapshot()
        code, out, _ = self.mod.run()
        self.assertEqual(code, 0)
        self.assertEqual(self.mod.snapshot(), before)
        self.assertIn("rename neon/neon_glow -> neon/glow", out)
        self.assertIn("textures/neon/neon_glow_e.png -> textures/neon/glow_e.png", out)
        self.assertIn("map maps/a.map: 1 face neon/neon_glow -> neon/glow", out)
        self.assertIn('materials.toml: add ["neon/glow"] emissive = 4.0', out)
        self.assertIn("no entry for neon/plain (no `_e` map)", out)
        self.assertIn("Dry run: nothing written", out)

    def test_apply_renames_every_slot_sibling_rewrites_references_and_writes_entries(self):
        self.standard_mod()
        code, _, err = self.mod.run("--apply")
        self.assertEqual(code, 0, err)
        names = sorted(p.name for p in (self.mod.root / "textures" / "neon").iterdir())
        self.assertEqual(
            names,
            ["glow.png", "glow_e.png", "glow_h.png", "glow_n.png", "glow_p.png", "glow_s.png",
             "glow_t.png", "other_wall.png", "plain.png"],
        )
        # Content moved with the name.
        self.assertEqual(
            (self.mod.root / "textures/neon/glow_e.png").read_bytes(), b"png:neon/neon_glow_e"
        )
        self.assertEqual(
            self.mod.read("maps/a.map"), map_text("neon/glow", "neon/other_wall", "NEON/Glow")
        )
        # Spelling kept; a bare alias becomes the qualified name; quotes kept.
        self.assertEqual(
            self.mod.read("maps/b.map"),
            map_text("textures\\neon\\glow", "neon/plain", '"neon/glow"'),
        )
        parsed = tomllib.loads(self.mod.read("materials.toml"))
        self.assertEqual(parsed, {"neon/glow": {"emissive": 4.0}})

    def test_an_existing_file_keeps_every_other_entry_and_comment(self):
        self.mod.texture("neon", "neon_sign", "neon_sign_e")
        original = (
            "# Dev mod strengths.\n"
            '"signs/bar" = { emissive = 0 }\n'
            "\n"
            '["metal/vent"]  # a vent\n'
            "emissive = 2.5\n"
        )
        self.mod.toml(original)
        code, _, err = self.mod.run("--apply")
        self.assertEqual(code, 0, err)
        text = self.mod.read("materials.toml")
        self.assertTrue(text.startswith(original), text)
        self.assertEqual(
            tomllib.loads(text),
            {"metal/vent": {"emissive": 2.5}, "signs/bar": {"emissive": 0},
             "neon/sign": {"emissive": 4.0}},
        )

    def test_an_entry_for_the_old_name_is_carried_not_duplicated(self):
        self.mod.texture("neon", "neon_a", "neon_a_e", "neon_b", "neon_b_e")
        self.mod.toml(
            "# kept\n"
            '"Textures/Neon/NEON_b" = { emissive = 0.5 }\n'
            "\n"
            '["neon/neon_a"]\n'
            "emissive = 7\n"
        )
        code, out, err = self.mod.run("--apply")
        self.assertEqual(code, 0, err)
        self.assertIn('carry ["neon/neon_a"] -> ["neon/a"]', out)
        text = self.mod.read("materials.toml")
        self.assertEqual(
            text,
            "# kept\n"
            '"neon/b" = { emissive = 0.5 }\n'
            "\n"
            '["neon/a"]\n'
            "emissive = 7\n",
        )

    def test_a_table_field_spelled_like_an_old_key_is_not_renamed(self):
        self.mod.texture("neon", "neon_a", "neon_a_e")
        original = '["other"]\nneon_a = 1\n'
        self.mod.toml('"neon_a" = { emissive = 2 }\n' + original)
        code, _, err = self.mod.run("--apply")
        self.assertEqual(code, 0, err)
        self.assertEqual(
            self.mod.read("materials.toml"), '"neon/a" = { emissive = 2 }\n' + original
        )

    def assert_refused(self, expected_message):
        before = self.mod.snapshot()
        code, _, err = self.mod.run("--apply")
        self.assertEqual(code, 1)
        self.assertIn("refused, nothing written", err)
        self.assertIn(expected_message, err)
        self.assertEqual(self.mod.snapshot(), before, "a refusal writes nothing")

    def test_an_existing_entry_for_the_new_key_refuses_as_a_collision(self):
        self.mod.texture("neon", "neon_sign", "neon_sign_e", "neon_ok", "neon_ok_e")
        self.mod.map("a.map", "neon/neon_ok", "neon/neon_sign")
        self.mod.toml('["textures/NEON/Sign"]\nemissive = 1\n')
        self.assert_refused("already names neon/sign")

    def test_a_target_file_collision_refuses_with_no_partial_write(self):
        # `neon_ok` alone would rename cleanly; the collision stops everything.
        self.mod.texture("neon", "neon_ok", "neon_ok_e", "neon_wall", "neon_wall_e", "Wall_e")
        self.mod.map("a.map", "neon/neon_ok", "neon/neon_wall")
        self.assert_refused("target wall_e.png already exists")

    def test_a_known_material_prefix_after_the_rename_refuses(self):
        for stem in ("neon_metal_x", "neon_Metal_x", "neon_glass"):
            with self.subTest(stem=stem):
                mod = ModTree()
                self.addCleanup(mod.close)
                self.mod = mod
                mod.texture("signs", stem, f"{stem}_e")
                self.assert_refused("material; rename it by hand")

    def test_an_unknown_prefix_after_the_rename_renames(self):
        self.mod.texture("signs", "neon_metalwork_x", "neon_panel", "neon_panel_e")
        code, out, err = self.mod.run("--apply")
        self.assertEqual(code, 0, err)
        self.assertIn("rename signs/neon_metalwork_x -> signs/metalwork_x", out)
        self.assertIn("rename signs/neon_panel -> signs/panel", out)
        self.assertTrue((self.mod.root / "textures/signs/metalwork_x.png").exists())
        self.assertTrue((self.mod.root / "textures/signs/panel_e.png").exists())

    def test_a_reference_with_no_texture_file_is_reported_and_left(self):
        self.mod.texture("neon", "neon_real", "neon_real_e")
        self.mod.map("a.map", "neon/neon_missing", "neon/neon_real")
        code, out, err = self.mod.run("--apply")
        self.assertEqual(code, 0, err)
        self.assertIn("references neon/neon_missing, which has no texture file", out)
        self.assertEqual(self.mod.read("maps/a.map"), map_text("neon/neon_missing", "neon/real"))

    def test_a_bare_reference_the_rename_would_make_ambiguous_refuses(self):
        self.mod.texture("neon", "neon_tile", "neon_tile_e")
        self.mod.texture("floors", "tile")
        self.mod.map("a.map", "tile")
        self.assert_refused("references `tile` bare")

    def test_an_unparsable_materials_toml_refuses(self):
        self.mod.texture("neon", "neon_sign", "neon_sign_e")
        self.mod.toml("[broken\n")
        self.assert_refused("does not parse")

    def test_a_sibling_only_bundle_renames_together_and_gets_an_entry(self):
        self.mod.texture("neon", "neon_x_e", "neon_x_n")
        self.mod.map("a.map", "neon/neon_x")
        code, out, err = self.mod.run("--apply")
        self.assertEqual(code, 0, err)
        self.assertNotIn("has no texture file", out)
        names = sorted(p.name for p in (self.mod.root / "textures" / "neon").iterdir())
        self.assertEqual(names, ["x_e.png", "x_n.png"])
        self.assertEqual(self.mod.read("maps/a.map"), map_text("neon/x"))
        self.assertEqual(
            tomllib.loads(self.mod.read("materials.toml")), {"neon/x": {"emissive": 4.0}}
        )

    def test_a_sibling_of_a_sibling_refuses_cleanly(self):
        self.mod.texture("neon", "neon_x", "neon_x_p", "neon_x_p_e")
        self.assert_refused("neon_x_p_e.png")

    def test_an_orphan_sibling_at_the_target_refuses(self):
        self.mod.texture("neon", "neon_x", "neon_x_e", "x_n")
        self.assert_refused("x_n.png already exists in the target bundle neon/x")

    def test_a_crlf_materials_toml_gets_crlf_appended(self):
        self.mod.texture("neon", "neon_sign", "neon_sign_e")
        (self.mod.root / "materials.toml").write_bytes(b'["metal/vent"]\r\nemissive = 2.5\r\n')
        code, _, err = self.mod.run("--apply")
        self.assertEqual(code, 0, err)
        data = (self.mod.root / "materials.toml").read_bytes()
        self.assertNotIn(b"\n", data.replace(b"\r\n", b""))
        self.assertEqual(
            tomllib.loads(data.decode()),
            {"metal/vent": {"emissive": 2.5}, "neon/sign": {"emissive": 4.0}},
        )


if __name__ == "__main__":
    unittest.main()
