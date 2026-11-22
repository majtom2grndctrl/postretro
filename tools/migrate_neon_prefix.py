#!/usr/bin/env python3
"""Migrate a mod off the retired `neon_` texture prefix.

The engine once gave every `neon_` texture a glow strength of 4.0. Strength is
now content: the mod's `materials.toml` sets `emissive` per texture, and a
texture with an `_e` map and no entry glows at 1.0. This script keeps every
`neon_` look:

  * renames `textures/<collection>/neon_x.png` and every slot sibling
    (`neon_x_e.png`, `_s`, `_n`, `_h`, `_p`, `_t`) to `x...`;
  * rewrites `.map` face references to the renamed textures;
  * writes `emissive = 4.0` for each renamed texture that has an `_e` map into
    `<mod root>/materials.toml`, keyed `"collection/x"`.

Usage:  python3 tools/migrate_neon_prefix.py <mod root> [--apply]

Without `--apply` it is a dry run: it prints the plan and writes nothing.
Standard library only (Python 3.11+, for `tomllib`).

Rules
-----
* Textures resolve as prl-build resolves them: PNGs directly inside a
  collection directory under `textures/`, named `collection/stem`, matched
  case-insensitively with `\\` read as `/` and an optional leading
  `textures/`. A reference whose collection does not hold the texture falls
  back to its bare stem when that stem is unique across collections, as the
  compiler's alias does; such a reference is rewritten to the qualified name.
* A `materials.toml` entry already naming the old texture is carried: its key
  is renamed and its values kept, since it already sets the strength the
  texture shows today. Only a texture with no entry gets `emissive = 4.0`.
  The file is edited as text, so every comment and other entry survives.
* A texture with no `_e` map gets no entry: an entry there glows nothing and
  makes prl-build warn.
* A `neon_` reference in a map with no texture file behind it is reported and
  left unchanged; there is nothing to rename.

It refuses, writing nothing, when:
  * a renamed file's target already exists, or two files rename to one name;
  * the new name's material prefix (its first `_` token, case-folded) is a
    known material prefix, so the rename would change its material
    (`neon_metal_x` -> `metal_x`), or is still `neon`;
  * the new key normalizes equal to an existing `materials.toml` key, or two
    existing entries name the same old texture;
  * a map references the new stem bare and the rename would make that bare
    name ambiguous;
  * `materials.toml` does not parse, or an entry to carry cannot be located in
    its text.
"""

import argparse
import os
import re
import sys
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

OLD_PREFIX = "neon_"
EMISSIVE_STRENGTH = "4.0"
MATERIALS_TOML = "materials.toml"

# Every recognized material prefix, lowercase: the engine's `MATERIAL_PREFIXES`
# (crates/render-data/src/material.rs). This script ships in SDK bundles, which
# carry no engine source, so the list is copied here; a render-data test keeps
# the two in step.
KNOWN_MATERIAL_PREFIXES = ("metal", "concrete", "grate", "glass", "wood")

# Slot siblings a texture's maps are spelled with (the diffuse is the bare
# stem): specular, normal, emissive, height, smoothness, tint.
SLOT_SUFFIXES = ("_s", "_n", "_e", "_h", "_p", "_t")

# A `.map` face line: three plane points, then the texture name.
FACE_LINE = re.compile(
    r"^(?P<head>\s*\([^()]*\)\s*\([^()]*\)\s*\([^()]*\)\s*)"
    r"(?P<tex>\"[^\"]*\"|\S+)(?P<tail>.*)$",
    re.DOTALL,
)

# A TOML key at the start of a line: a basic string, a literal string or a bare
# key, optionally opened by a table header's `[`.
TOML_KEY_AT_LINE_START = re.compile(
    r"^(?P<lead>\s*(?:\[\s*)?)"
    r"(?P<key>\"(?:[^\"\\]|\\.)*\"|'[^']*'|[A-Za-z0-9_-]+)"
    r"(?=\s*(?:\]|=|\.))"
)


class Refusal(Exception):
    """The migration cannot run as planned; nothing was written."""


def normalize_texture_name(name):
    """The material stage's normalization: lowercase, `\\` as `/`, no `textures/`."""
    lowered = name.lower().replace("\\", "/")
    return lowered[len("textures/"):] if lowered.startswith("textures/") else lowered


def material_prefix(bare_name):
    """The engine's prefix: the bare name's first `_` token, case-folded."""
    return bare_name.split("_", 1)[0].lower()


@dataclass
class Texture:
    """One `neon_` texture and its slot siblings, in one collection."""

    collection: str  # directory name, as on disk
    old_stem: str
    files: list = field(default_factory=list)  # (old Path, new Path)
    has_emissive: bool = False

    @property
    def new_stem(self):
        return self.old_stem[len(OLD_PREFIX):]

    @property
    def old_key(self):
        return normalize_texture_name(f"{self.collection}/{self.old_stem}")

    @property
    def new_key(self):
        return f"{self.collection}/{self.new_stem}"


@dataclass
class Plan:
    textures: list
    map_rewrites: dict  # Path -> (new text, [(old ref, new ref, count)])
    unresolved: list  # (map Path, reference)
    carried: list  # (old key text, new key, line)
    added: list  # Texture
    no_entry: list  # Texture
    toml_text: str | None
    notes: list


def collection_dirs(textures_root):
    if not textures_root.is_dir():
        return []
    return sorted(path for path in textures_root.iterdir() if path.is_dir())


def png_files(directory):
    return sorted(
        path for path in directory.iterdir() if path.is_file() and path.suffix.lower() == ".png"
    )


def scan_textures(textures_root):
    """Every `neon_` texture with its siblings, and every collection's stems."""
    textures = []
    stems_by_collection = {}
    for directory in collection_dirs(textures_root):
        pngs = png_files(directory)
        stems = {path.stem.lower() for path in pngs}
        stems_by_collection[directory.name] = stems
        neon = [path for path in pngs if path.stem.lower().startswith(OLD_PREFIX)]
        neon_stems = {path.stem.lower() for path in neon}

        def bundle_base(lowered):
            """The bundle base `lowered` belongs to, as prl-build groups bundles.

            prl-build treats a stem as a bundle base when it or any known
            sibling (`SLOT_SUFFIXES`) exists, so a sibling-only bundle
            (`neon_x_e` with no `neon_x`) is one bundle.
            """
            for suffix in SLOT_SUFFIXES:
                base = lowered[: -len(suffix)]
                if not lowered.endswith(suffix) or not base.startswith(OLD_PREFIX):
                    continue
                return base
            return lowered

        bases = {}
        for path in neon:
            lowered = path.stem.lower()
            base = bundle_base(lowered)
            if base != lowered and base in neon_stems and bundle_base(base) != base:
                raise Refusal(
                    f"{path}: its base `{base}` is itself a sibling of `{bundle_base(base)}`; "
                    f"rename these files by hand"
                )
            texture = bases.get(base)
            if texture is None:
                texture = bases[base] = Texture(directory.name, path.stem[: len(base)])
            new_name = path.name[len(OLD_PREFIX):]
            texture.files.append((path, path.with_name(new_name)))
            if lowered == base + "_e":
                texture.has_emissive = True
        # A sibling-only bundle's base is a texture name too (alias counts).
        stems.update(bases)
        textures.extend(bases[key] for key in sorted(bases))
    return textures, stems_by_collection


def check_textures(textures, stems_by_collection):
    """Refuse a rename that collides or changes a material."""
    problems = []
    targets = {}
    for texture in textures:
        stem = texture.new_stem
        if not stem:
            problems.append(f"{texture.collection}/{texture.old_stem}: nothing follows `neon_`")
            continue
        prefix = material_prefix(stem)
        if prefix in KNOWN_MATERIAL_PREFIXES:
            problems.append(
                f"{texture.collection}/{texture.old_stem}: `{stem}` would derive the `{prefix}` "
                f"material; rename it by hand"
            )
        elif prefix == OLD_PREFIX[:-1]:
            problems.append(
                f"{texture.collection}/{texture.old_stem}: `{stem}` still starts with `neon_`; "
                f"rename it by hand"
            )
        for old, new in texture.files:
            existing = stems_by_collection[texture.collection]
            if new.stem.lower() in existing or new.exists():
                problems.append(f"{old}: target {new.name} already exists")
            key = str(new).lower()
            if key in targets:
                problems.append(f"{old} and {targets[key]} both rename to {new.name}")
            targets[key] = old
        renamed = {new.stem.lower() for _, new in texture.files}
        existing = stems_by_collection[texture.collection]
        for suffix in ("",) + SLOT_SUFFIXES:
            orphan = stem.lower() + suffix
            if orphan in existing and orphan not in renamed:
                problems.append(
                    f"{texture.collection}/{orphan}.png already exists in the target bundle "
                    f"{texture.new_key}; the rename would adopt it"
                )
    return problems


def resolve_reference(name, qualified, bare):
    """The renamed file a map reference resolves to, as prl-build resolves it.

    Returns ((collection, old stem, new stem), via_alias), or None.
    """
    normalized = normalize_texture_name(name)
    if normalized in qualified:
        return qualified[normalized], False
    stem = normalized.rsplit("/", 1)[-1]
    if stem in bare:
        return bare[stem], True
    return None


def plan_maps(maps_root, textures, stems_by_collection):
    """Rewrite each face reference to a renamed file; report the rest."""
    qualified = {}
    for texture in textures:
        for old, new in texture.files:
            qualified[normalize_texture_name(f"{texture.collection}/{old.stem}")] = (
                texture.collection,
                old.stem,
                new.stem,
            )
        qualified[texture.old_key] = (texture.collection, texture.old_stem, texture.new_stem)
    stem_count = {}
    for stems in stems_by_collection.values():
        for stem in stems:
            stem_count[stem] = stem_count.get(stem, 0) + 1
    # The compiler's bare-stem alias holds only for a stem unique across
    # collections.
    bare = {
        value[1].lower(): value
        for value in qualified.values()
        if stem_count.get(value[1].lower(), 0) == 1
    }
    new_stems = {value[2].lower() for value in qualified.values()}

    rewrites = {}
    unresolved = []
    bare_new_refs = []
    map_paths = sorted(maps_root.rglob("*.map")) if maps_root.is_dir() else []
    for map_path in map_paths:
        with open(map_path, encoding="utf-8", errors="surrogateescape", newline="") as handle:
            text = handle.read()
        lines = text.split("\n")
        changes = {}
        changed = False
        for index, line in enumerate(lines):
            match = FACE_LINE.match(line)
            if not match:
                continue
            token = match["tex"]
            quoted = token.startswith('"')
            name = token[1:-1] if quoted else token
            normalized = normalize_texture_name(name)
            bare_segment = normalized.rsplit("/", 1)[-1]
            if "/" not in normalized and bare_segment in new_stems:
                bare_new_refs.append((map_path, name))
            resolved = resolve_reference(name, qualified, bare)
            if resolved is None:
                if bare_segment.startswith(OLD_PREFIX):
                    unresolved.append((map_path, name))
                continue
            (collection, old_stem, new_stem), via_alias = resolved
            if via_alias:
                new_name = f"{collection}/{new_stem}"
            else:
                cut = max(name.rfind("/"), name.rfind("\\")) + 1
                new_name = name[:cut] + name[cut + len(OLD_PREFIX):]
            new_token = f'"{new_name}"' if quoted else new_name
            lines[index] = match["head"] + new_token + match["tail"]
            changes[(name, new_name)] = changes.get((name, new_name), 0) + 1
            changed = True
        if changed:
            rewrites[map_path] = (
                "\n".join(lines),
                [(old, new, count) for (old, new), count in sorted(changes.items())],
            )

    problems = []
    for map_path, name in bare_new_refs:
        stem = normalize_texture_name(name)
        if stem_count.get(stem, 0) >= 1:
            problems.append(
                f"{map_path}: references `{name}` bare; renaming a texture to that stem would "
                f"make it ambiguous"
            )
    return rewrites, unresolved, problems


def toml_key_literal_value(literal):
    """The key a TOML key literal spells."""
    if literal.startswith(('"', "'")):
        return next(iter(tomllib.loads(f"{literal} = 0")))
    return literal


def quote_key(key):
    return '"' + key.replace("\\", "\\\\").replace('"', '\\"') + '"'


def plan_materials_toml(toml_path, textures, qualified_alias_stems):
    """Carry entries for old names, add 4.0 for the rest, refuse collisions."""
    problems = []
    text = None
    keys = {}
    if toml_path.exists():
        with open(toml_path, encoding="utf-8", newline="") as handle:
            text = handle.read()
        try:
            keys = tomllib.loads(text)
        except tomllib.TOMLDecodeError as error:
            raise Refusal(f"{toml_path}: does not parse ({error}); fix it first") from error

    by_normalized = {}
    for key in keys:
        by_normalized.setdefault(normalize_texture_name(key), []).append(key)

    carried = []
    added = []
    no_entry = []
    renames = {}  # old key text -> new key
    carried_keys = set()
    for texture in textures:
        old_names = {texture.old_key}
        if texture.old_stem.lower() in qualified_alias_stems:
            old_names.add(texture.old_stem.lower())
        old_keys = [key for name in sorted(old_names) for key in by_normalized.get(name, [])]
        if len(old_keys) > 1:
            problems.append(
                f"{toml_path}: entries {old_keys} all name {texture.collection}/"
                f"{texture.old_stem}; merge them first"
            )
            continue
        if old_keys:
            renames[old_keys[0]] = texture.new_key
            carried_keys.add(old_keys[0])
            carried.append((old_keys[0], texture.new_key))
        elif texture.has_emissive:
            added.append(texture)
        else:
            no_entry.append(texture)

    for texture in textures:
        new_normalized = normalize_texture_name(texture.new_key)
        clashing = [k for k in by_normalized.get(new_normalized, []) if k not in carried_keys]
        if clashing:
            problems.append(
                f"{toml_path}: entry {clashing[0]!r} already names {texture.new_key}; "
                f"renaming {texture.collection}/{texture.old_stem} would collide"
            )
    if problems:
        return None, carried, added, no_entry, problems

    new_text = text if text is not None else ""
    if renames:
        lines = new_text.split("\n")
        found = {key: 0 for key in renames}
        # A texture key is top-level: a table header anywhere, or a dotted or
        # inline key before the first header. After one, `key = ...` belongs
        # to that table.
        in_table = False
        for index, line in enumerate(lines):
            is_header = line.lstrip().startswith("[")
            match = TOML_KEY_AT_LINE_START.match(line)
            top_level = is_header or not in_table
            in_table = in_table or is_header
            if not match or not top_level:
                continue
            spelled = toml_key_literal_value(match["key"])
            if spelled in renames:
                found[spelled] += 1
                lines[index] = (
                    match["lead"] + quote_key(renames[spelled]) + line[match.end("key"):]
                )
        missing = [key for key, count in found.items() if count == 0]
        if missing:
            raise Refusal(
                f"{toml_path}: cannot find {missing} at the start of a line to rename; "
                f"edit them by hand"
            )
        new_text = "\n".join(lines)
    if added:
        block = ""
        if new_text and not new_text.endswith("\n"):
            block += "\n"
        if new_text:
            block += "\n"
        block += "# Kept from the retired `neon_` prefix's 4.0 by tools/migrate_neon_prefix.py.\n"
        block += "\n".join(
            f"[{quote_key(texture.new_key)}]\nemissive = {EMISSIVE_STRENGTH}\n" for texture in added
        )
        # Append in the file's own newline convention.
        if "\r\n" in new_text:
            block = block.replace("\n", "\r\n")
        new_text += block
    if new_text == (text or ""):
        new_text = None
    return new_text, carried, added, no_entry, problems


def build_plan(mod_root):
    textures_root = mod_root / "textures"
    textures, stems_by_collection = scan_textures(textures_root)
    problems = check_textures(textures, stems_by_collection)
    rewrites, unresolved, map_problems = plan_maps(mod_root / "maps", textures, stems_by_collection)
    problems.extend(map_problems)

    stem_count = {}
    for stems in stems_by_collection.values():
        for stem in stems:
            stem_count[stem] = stem_count.get(stem, 0) + 1
    alias_stems = {t.old_stem.lower() for t in textures if stem_count[t.old_stem.lower()] == 1}
    toml_text, carried, added, no_entry, toml_problems = plan_materials_toml(
        mod_root / MATERIALS_TOML, textures, alias_stems
    )
    problems.extend(toml_problems)
    if problems:
        raise Refusal("\n".join(problems))
    notes = []
    for texture in textures:
        if stem_count.get(texture.new_stem.lower(), 0) >= 1:
            notes.append(
                f"{texture.new_key}: the stem `{texture.new_stem}` also exists in another "
                f"collection, so its bare name becomes ambiguous (reference it qualified)"
            )
    return Plan(textures, rewrites, unresolved, carried, added, no_entry, toml_text, notes)


def describe(plan, mod_root):
    out = []

    # The plan reads the same on every host: components joined by `/`.
    def rel(path):
        return Path(os.path.relpath(path, mod_root)).as_posix()

    if not plan.textures:
        out.append("No `neon_` textures found.")
    for texture in plan.textures:
        out.append(f"rename {texture.collection}/{texture.old_stem} -> {texture.new_key}")
        for old, new in texture.files:
            out.append(f"  {rel(old)} -> {rel(new)}")
    for map_path, (_, changes) in sorted(plan.map_rewrites.items()):
        for old, new, count in changes:
            faces = "face" if count == 1 else "faces"
            out.append(f"map {rel(map_path)}: {count} {faces} {old} -> {new}")
    for old_key, new_key in plan.carried:
        out.append(f"{MATERIALS_TOML}: carry [{quote_key(old_key)}] -> [{quote_key(new_key)}]")
    for texture in plan.added:
        out.append(
            f"{MATERIALS_TOML}: add [{quote_key(texture.new_key)}] emissive = {EMISSIVE_STRENGTH}"
        )
    for texture in plan.no_entry:
        out.append(f"{MATERIALS_TOML}: no entry for {texture.new_key} (no `_e` map)")
    for map_path, name in plan.unresolved:
        out.append(f"note: {rel(map_path)} references {name}, which has no texture file; left as is")
    for note in plan.notes:
        out.append(f"note: {note}")
    return out


def write_text(path, text):
    temporary = path.with_name(path.name + ".migrate-tmp")
    with open(temporary, "w", encoding="utf-8", errors="surrogateescape", newline="") as handle:
        handle.write(text)
    os.replace(temporary, path)


def apply_plan(plan, mod_root):
    for map_path, (text, _) in plan.map_rewrites.items():
        write_text(map_path, text)
    if plan.toml_text is not None:
        write_text(mod_root / MATERIALS_TOML, plan.toml_text)
    for texture in plan.textures:
        for old, new in texture.files:
            old.rename(new)


def main(argv=None):
    parser = argparse.ArgumentParser(
        description="Rename a mod's `neon_` textures and keep their glow in materials.toml."
    )
    parser.add_argument("mod_root", type=Path, help="Mod root holding textures/ and maps/")
    parser.add_argument("--apply", action="store_true", help="Perform the migration")
    args = parser.parse_args(argv)

    mod_root = args.mod_root
    if not (mod_root / "textures").is_dir():
        parser.error(f"{mod_root} has no textures/ directory")
    try:
        plan = build_plan(mod_root)
    except Refusal as refusal:
        print(f"refused, nothing written:\n{refusal}", file=sys.stderr)
        return 1
    for line in describe(plan, mod_root):
        print(line)
    if args.apply:
        apply_plan(plan, mod_root)
        print("Applied.")
    else:
        print("Dry run: nothing written. Re-run with --apply to perform it.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
