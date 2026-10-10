"""Guard: every damageable enemy in the dev maps carries the `enemy` tag.

The dev mod's `enemy-death` impact policy (content/dev/scripts/combat-lifecycle.ts)
matches on that tag. It supplies both the floating damage number and the death
clip plus corpse removal, so an untagged enemy shows no numbers and freezes
mid-pose at zero HP. A spawner passes only its `spawned_tags` to its spawns,
never its own `_tags`.
"""

import re
import unittest
from pathlib import Path

MAPS = Path(__file__).parents[2] / "content" / "dev" / "maps"

# Descriptors carrying both `health` and `behavior`. Extend this when the dev
# mod registers a new enemy.
ENEMY_CLASSNAMES = {
    "reference_enemy",
    "limitator",
    "crossfire_raider",
    "crossfire_sentinel",
    "faction_sentiment_cabal",
    "faction_sentiment_resistance",
    "positional_sound_grunt",
    "pose_fixture_enemy",
}
ENEMY_TAG = "enemy"
KVP = re.compile(r'^\s*"([^"]*)"\s+"([^"]*)"\s*$')


def entity_headers(text):
    """Yield each top-level entity's key/value pairs, skipping brushes and comments."""
    depth = 0
    header = None
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("//"):
            continue
        if stripped == "{":
            depth += 1
            if depth == 1:
                header = {}
            continue
        if stripped == "}":
            depth -= 1
            if depth == 0:
                yield header
            continue
        if depth == 1:
            match = KVP.match(line)
            if match:
                header[match.group(1)] = match.group(2)


class DevMapEnemyTagTests(unittest.TestCase):
    def test_every_enemy_placement_and_spawner_carries_the_enemy_tag(self):
        maps = sorted(MAPS.glob("*.map"))
        self.assertTrue(maps, f"no maps found under {MAPS}")
        missing = []
        checked = 0
        for path in maps:
            for header in entity_headers(path.read_text(errors="replace")):
                classname = header.get("classname")
                if classname in ENEMY_CLASSNAMES:
                    key = "_tags"
                elif classname == "entity_spawner":
                    key = "spawned_tags"
                else:
                    continue
                checked += 1
                if ENEMY_TAG not in header.get(key, "").split():
                    origin = header.get("origin", "?")
                    missing.append(f"{path.name}: {classname} at {origin} lacks {key} `enemy`")
        self.assertGreater(checked, 0)
        self.assertEqual(missing, [])


if __name__ == "__main__":
    unittest.main()
