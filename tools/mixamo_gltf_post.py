"""Post-export glTF pass for mixamo_to_gltf.py: joint tags, clip edits, library binding.
See: context/lib/resource_management.md §7"""

import json
import re
import struct
from pathlib import Path
from urllib.parse import unquote

from mixamo_clip_events import attach_events, cut_key, set_clip_events, \
    stamp_library_events, trim_events


# ---------------------------------------------------------------------------
# Mixamo bone -> engine extras mapping (bone names without "mixamorig:" prefix)
# ---------------------------------------------------------------------------

MIXAMO_PREFIX = "mixamorig:"
NUMBERED_PREFIX = re.compile(r"^mixamorig\d+:")

UPPER_BODY = {
    "Spine", "Spine1", "Spine2",
    "Neck", "Head", "HeadTop_End",
    "LeftShoulder", "LeftArm", "LeftForeArm", "LeftHand",
    "RightShoulder", "RightArm", "RightForeArm", "RightHand",
    "LeftHandThumb1", "LeftHandThumb2", "LeftHandThumb3", "LeftHandThumb4",
    "LeftHandIndex1", "LeftHandIndex2", "LeftHandIndex3", "LeftHandIndex4",
    "LeftHandMiddle1", "LeftHandMiddle2", "LeftHandMiddle3", "LeftHandMiddle4",
    "LeftHandRing1", "LeftHandRing2", "LeftHandRing3", "LeftHandRing4",
    "LeftHandPinky1", "LeftHandPinky2", "LeftHandPinky3", "LeftHandPinky4",
    "RightHandThumb1", "RightHandThumb2", "RightHandThumb3", "RightHandThumb4",
    "RightHandIndex1", "RightHandIndex2", "RightHandIndex3", "RightHandIndex4",
    "RightHandMiddle1", "RightHandMiddle2", "RightHandMiddle3", "RightHandMiddle4",
    "RightHandRing1", "RightHandRing2", "RightHandRing3", "RightHandRing4",
    "RightHandPinky1", "RightHandPinky2", "RightHandPinky3", "RightHandPinky4",
}

LOWER_BODY = {
    "Hips",
    "LeftUpLeg", "LeftLeg", "LeftFoot", "LeftToeBase", "LeftToe_End",
    "RightUpLeg", "RightLeg", "RightFoot", "RightToeBase", "RightToe_End",
}

AIM_SPINE_WEIGHTS = {
    "Spine": 0.4,
    "Spine1": 0.7,
    "Spine2": 1.0,
}

LEG_L = {"LeftUpLeg", "LeftLeg", "LeftFoot"}
LEG_R = {"RightUpLeg", "RightLeg", "RightFoot"}
FOOT_L = {"LeftFoot"}
FOOT_R = {"RightFoot"}

SOCKETS = {
    "RightHand": "hand_r",
    "LeftHand": "hand_l",
}

# Skeletal hit zones (bone -> `hitZone` tag). A descriptor's `zoneMultipliers`
# key matches a joint's zone tag by string, so without this tag the head/leg
# multipliers are silently dead. Radius is omitted; the engine applies its
# DEFAULT_ZONE_RADIUS (see scripting/systems/hit_zones.rs). Feet are left
# untagged (small, ground-level); the thigh+shin carry the "leg" zone.
HIT_ZONES = {
    "Head": "head",
    "LeftUpLeg": "leg",
    "LeftLeg": "leg",
    "RightUpLeg": "leg",
    "RightLeg": "leg",
}

# Extras keys tag_skeleton writes on a joint.
ENGINE_TAGS = ("poseMask", "aimBendWeight", "socket", "hitZone")

# These reference accessors or buffer views from inside `extensions`, where
# accessor_slots() and accessor_view_slots() do not look.
UNTRACKED_EXTENSIONS = ("EXT_mesh_gpu_instancing", "KHR_draco_mesh_compression")


def canonical_bone_name(name):
    return NUMBERED_PREFIX.sub(MIXAMO_PREFIX, name)


def read_gltf(gltf_path):
    with open(gltf_path, 'r', encoding='utf-8') as f:
        return json.load(f)


def write_gltf(gltf_path, gltf):
    # newline: text mode would otherwise write CRLF on Windows.
    with open(gltf_path, 'w', encoding='utf-8', newline='\n') as f:
        json.dump(gltf, f, indent='\t', ensure_ascii=False)


def normalize_bone_prefixes(gltf):
    """Rewrite numbered `mixamorig<N>:` node names to `mixamorig:`.

    Tagging and clip binding both match joints by canonical name, so this runs
    first and regardless of --no-tag. Returns the number of nodes renamed.
    """
    renamed = 0
    for node in gltf.get("nodes", []):
        name = node.get("name", "")
        canonical = canonical_bone_name(name)
        if canonical != name:
            node["name"] = canonical
            renamed += 1
    return renamed


def tag_skeleton(gltf):
    """Add engine extras to canonical Mixamo bones and set the skin root.

    Returns the number of bones tagged.
    """
    nodes = gltf.get("nodes", [])
    skins = gltf.get("skins", [])
    tagged = 0

    for node in nodes:
        name = node.get("name", "")
        if not name.startswith(MIXAMO_PREFIX):
            continue
        short = name[len(MIXAMO_PREFIX):]

        extras = node.get("extras") or {}

        masks = []
        if short in UPPER_BODY:
            masks.append("upperBody")
        if short in LOWER_BODY:
            masks.append("lowerBody")
        if short in AIM_SPINE_WEIGHTS:
            masks.append("aimSpine")
            extras["aimBendWeight"] = AIM_SPINE_WEIGHTS[short]
        if short in LEG_L:
            masks.append("legL")
        if short in LEG_R:
            masks.append("legR")
        if short in FOOT_L:
            masks.append("footL")
        if short in FOOT_R:
            masks.append("footR")

        if masks:
            extras["poseMask"] = masks if len(masks) > 1 else masks[0]

        if short in SOCKETS:
            extras["socket"] = SOCKETS[short]

        if short in HIT_ZONES:
            extras["hitZone"] = HIT_ZONES[short]

        if extras:
            node["extras"] = extras
            tagged += 1

    for i, node in enumerate(nodes):
        if node.get("name") == MIXAMO_PREFIX + "Hips":
            if skins:
                skins[0]["skeleton"] = i
            break

    return tagged


def has_engine_tags(gltf):
    return any(
        tag in (node.get("extras") or {})
        for node in gltf.get("nodes", [])
        for tag in ENGINE_TAGS
    )


def print_tag_summary(gltf, tagged):
    print(f"\nTagged {tagged} bones with engine extras")
    mask_counts = {}
    socket_count = 0
    hit_zone_counts = {}
    for node in gltf.get("nodes", []):
        extras = node.get("extras")
        if not extras:
            continue
        pm = extras.get("poseMask")
        if pm:
            for m in (pm if isinstance(pm, list) else [pm]):
                mask_counts[m] = mask_counts.get(m, 0) + 1
        if "socket" in extras:
            socket_count += 1
        hz = extras.get("hitZone")
        if hz:
            hit_zone_counts[hz] = hit_zone_counts.get(hz, 0) + 1
    for mask, count in sorted(mask_counts.items()):
        print(f"  poseMask '{mask}': {count} bone(s)")
    if socket_count:
        print(f"  sockets: {socket_count}")
    for zone, count in sorted(hit_zone_counts.items()):
        print(f"  hitZone '{zone}': {count} bone(s)")
    weights_str = ", ".join(f"{n}={w}" for n, w in sorted(AIM_SPINE_WEIGHTS.items()))
    print(f"  aimBendWeight: {weights_str}")
    skins = gltf.get("skins", [])
    if skins and "skeleton" in skins[0]:
        print(f"  skin skeleton root: node {skins[0]['skeleton']} (Hips)")


def accessor_slots(gltf):
    """Yield (container, key) for each accessor reference in the document."""
    for mesh in gltf.get("meshes", []):
        for primitive in mesh.get("primitives", []):
            for attributes in [primitive.get("attributes", {}), *primitive.get("targets", [])]:
                for semantic in attributes:
                    yield attributes, semantic
            if "indices" in primitive:
                yield primitive, "indices"
    for skin in gltf.get("skins", []):
        if "inverseBindMatrices" in skin:
            yield skin, "inverseBindMatrices"
    for animation in gltf.get("animations", []):
        for sampler in animation.get("samplers", []):
            yield sampler, "input"
            yield sampler, "output"


def accessor_view_slots(accessor):
    """Yield (container, key) for each buffer view reference in one accessor."""
    if "bufferView" in accessor:
        yield accessor, "bufferView"
    sparse = accessor.get("sparse")
    if sparse:
        yield sparse["indices"], "bufferView"
        yield sparse["values"], "bufferView"


def remove_accessors(gltf, bin_bytes, candidates):
    """Remove each candidate accessor nothing references any more.

    Mutates `gltf`, rewriting accessor and buffer view indices, and returns the
    compacted buffer. Sharing is resolved per buffer view: a view any surviving
    accessor or image reads is kept whole. Packing keeps file order and each
    view's offset modulo 4, so an accessor offset inside a view stays as aligned
    as it was; glTF requires that alignment of vertex attribute data.
    """
    untracked = set(UNTRACKED_EXTENSIONS) & set(gltf.get("extensionsUsed", []))
    if untracked:
        raise ValueError(f"cannot compact a document using {sorted(untracked)}: "
                         f"its accessor references are not tracked")
    buffers = gltf.get("buffers", [])
    if len(buffers) != 1:
        raise ValueError(f"expected one buffer, found {len(buffers)}")
    if len(bin_bytes) != buffers[0].get("byteLength"):
        raise ValueError(f"buffer is {len(bin_bytes)} bytes, but the document "
                         f"declares {buffers[0].get('byteLength')}")

    doomed_accessors = candidates - {c[k] for c, k in accessor_slots(gltf)}
    if not doomed_accessors:
        return bin_bytes

    accessor_remap = {}
    survivors = []
    freed_views = set()
    for index, accessor in enumerate(gltf["accessors"]):
        if index in doomed_accessors:
            freed_views.update(c[k] for c, k in accessor_view_slots(accessor))
        else:
            accessor_remap[index] = len(survivors)
            survivors.append(accessor)
    gltf["accessors"] = survivors
    for container, key in accessor_slots(gltf):
        container[key] = accessor_remap[container[key]]

    view_slots = [slot for accessor in survivors for slot in accessor_view_slots(accessor)]
    view_slots += [(image, "bufferView") for image in gltf.get("images", [])
                   if "bufferView" in image]
    doomed_views = freed_views - {c[k] for c, k in view_slots}

    view_remap = {}
    kept_views = []
    for index, view in enumerate(gltf.get("bufferViews", [])):
        if index not in doomed_views:
            view_remap[index] = len(kept_views)
            kept_views.append(view)
    gltf["bufferViews"] = kept_views
    for container, key in view_slots:
        container[key] = view_remap[container[key]]

    packed = bytearray()
    for view in sorted(kept_views, key=lambda v: v.get("byteOffset", 0)):
        start = view.get("byteOffset", 0)
        packed.extend(bytes((start - len(packed)) % 4))
        view["byteOffset"] = len(packed)
        packed.extend(bin_bytes[start:start + view["byteLength"]])
    buffers[0]["byteLength"] = len(packed)
    return bytes(packed)


def strip_clips(gltf, bin_bytes):
    """Remove every animation and all data only animations used.

    Mutates `gltf` and returns the compacted buffer.
    """
    clip_accessors = {
        accessor
        for animation in gltf.pop("animations", [])
        for sampler in animation.get("samplers", [])
        for accessor in (sampler["input"], sampler["output"])
    }
    return remove_accessors(gltf, bin_bytes, clip_accessors)


def strip_skins(gltf, bin_bytes):
    """Remove every skin and its inverse binds, keeping the joint nodes.

    The exporter writes a skin for an armature even with no mesh to deform. A
    library has no mesh to bind, so its inverse binds are dead weight.

    Mutates `gltf` and returns the compacted buffer.
    """
    bind_accessors = {
        skin["inverseBindMatrices"]
        for skin in gltf.pop("skins", [])
        if "inverseBindMatrices" in skin
    }
    for node in gltf.get("nodes", []):
        node.pop("skin", None)
    return remove_accessors(gltf, bin_bytes, bind_accessors)


def check_clip_library_path(clip_library):
    """Raise ValueError unless the engine would accept this reference.

    Mirrors the engine's relative-URI rule, so a bad value fails here rather
    than loading as a character with no clips. Backslashes fail too: they are
    separators on one platform and filename characters on another.
    """
    if not isinstance(clip_library, str) or not clip_library:
        raise ValueError("--clip-library must be a non-empty relative path")
    try:
        decoded = unquote(clip_library, errors="strict")
    except UnicodeDecodeError:
        raise ValueError(f"--clip-library must decode to UTF-8: '{clip_library}'") from None
    if ":" in decoded or "\\" in decoded or decoded.startswith("/"):
        raise ValueError(f"--clip-library must be a relative path with forward "
                         f"slashes and no scheme: '{clip_library}'")


def check_clip_library_target(output, clip_library):
    """Raise ValueError if the reference resolves to the output file itself.

    Export would replace the library with the character bound to it.
    """
    output = Path(output)
    if (output.parent / unquote(clip_library)).resolve() == output.resolve():
        raise ValueError(f"--clip-library '{clip_library}' resolves to --output itself")


def stamp_clip_library(gltf, clip_library):
    check_clip_library_path(clip_library)
    extras = gltf.setdefault("extras", {})
    if not isinstance(extras, dict):
        raise ValueError("document extras is not an object; cannot stamp clipLibrary")
    extras["clipLibrary"] = clip_library


def check_library(gltf):
    """Raise ValueError if an export is not the mesh-less, clip-bearing library."""
    if gltf.get("meshes"):
        raise ValueError("library export contains a mesh")
    if not gltf.get("animations"):
        raise ValueError("library export contains no clips")


def parse_trim(spec):
    """Parse `NEW=SOURCE:END_MS` or `NEW=SOURCE:START_MS-[END_MS]` into
    (new, source, start_ms, end_ms); an open end is None."""
    number = r"[0-9]+(?:\.[0-9]+)?"
    match = re.fullmatch(rf"([^=:]+)=([^=:]+):(?:({number})-({number})?|({number}))", spec)
    if not match:
        raise ValueError(f"--trim '{spec}': expected NEW=SOURCE:END_MS or "
                         "NEW=SOURCE:START_MS-[END_MS]")
    new, source = match.group(1), match.group(2)
    if match.group(5) is not None:
        start_ms, end_ms = 0.0, float(match.group(5))
    else:
        start_ms = float(match.group(3))
        end_ms = float(match.group(4)) if match.group(4) is not None else None
    if new == source:
        raise ValueError(f"--trim '{spec}': the new clip needs its own name")
    if end_ms is not None and end_ms <= start_ms:
        raise ValueError(f"--trim '{spec}': the end must come after the start")
    return new, source, start_ms, end_ms


ACCESSOR_WIDTH = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4}


def read_floats(gltf, bin_bytes, index):
    """Return one float accessor's elements as a list of tuples."""
    accessor = gltf["accessors"][index]
    if accessor.get("componentType") != 5126 or "sparse" in accessor:
        raise ValueError(f"accessor {index}: expected dense float data")
    width = ACCESSOR_WIDTH[accessor["type"]]
    view = gltf["bufferViews"][accessor["bufferView"]]
    start = view.get("byteOffset", 0) + accessor.get("byteOffset", 0)
    stride = view.get("byteStride", 4 * width)
    return [struct.unpack_from(f"<{width}f", bin_bytes, start + i * stride)
            for i in range(accessor["count"])]


def write_floats(gltf, bin_bytes, index, elements):
    """Overwrite one float accessor's elements in `bin_bytes` (a bytearray)."""
    accessor = gltf["accessors"][index]
    width = ACCESSOR_WIDTH[accessor["type"]]
    view = gltf["bufferViews"][accessor["bufferView"]]
    start = view.get("byteOffset", 0) + accessor.get("byteOffset", 0)
    stride = view.get("byteStride", 4 * width)
    for i, element in enumerate(elements):
        struct.pack_into(f"<{width}f", bin_bytes, start + i * stride, *element)
    if "min" in accessor or "max" in accessor:
        accessor["min"] = [min(e[c] for e in elements) for c in range(width)]
        accessor["max"] = [max(e[c] for e in elements) for c in range(width)]


def append_floats(gltf, bin_bytes, elements, accessor_type, bounds):
    """Append a tightly packed float accessor; return its index."""
    bin_bytes.extend(bytes(-len(bin_bytes) % 4))
    width = ACCESSOR_WIDTH[accessor_type]
    offset = len(bin_bytes)
    for element in elements:
        bin_bytes.extend(struct.pack(f"<{width}f", *element))
    gltf.setdefault("bufferViews", []).append(
        {"buffer": 0, "byteOffset": offset, "byteLength": len(bin_bytes) - offset})
    accessor = {"bufferView": len(gltf["bufferViews"]) - 1, "componentType": 5126,
                "count": len(elements), "type": accessor_type}
    if bounds:
        accessor["min"] = [min(e[c] for e in elements) for c in range(width)]
        accessor["max"] = [max(e[c] for e in elements) for c in range(width)]
    gltf.setdefault("accessors", []).append(accessor)
    gltf["buffers"][0]["byteLength"] = len(bin_bytes)
    return len(gltf["accessors"]) - 1


def clip_track_times(gltf, bin_bytes, animation):
    """Map each of a clip's time accessors to its key times in seconds."""
    return {s["input"]: [t for (t,) in read_floats(gltf, bin_bytes, s["input"])]
            for s in animation["samplers"]}


def document_track_times(gltf, bin_bytes):
    """Map each clip's name to its tracks' key times in seconds."""
    return {animation.get("name"): list(clip_track_times(gltf, bin_bytes, animation).values())
            for animation in gltf.get("animations", [])}


def find_clip(gltf, name):
    for animation in gltf.get("animations", []):
        if animation.get("name") == name:
            return animation
    raise ValueError(f"no clip named '{name}'")


def pin_root_in_place(gltf, bin_bytes, clip):
    """Hold the root's ground-plane translation (glTF X and Z) at its first key.

    `bin_bytes` is a bytearray, edited in place; vertical (Y) motion is kept.
    """
    animation = find_clip(gltf, clip)
    root = next((i for i, node in enumerate(gltf.get("nodes", []))
                 if node.get("name") == MIXAMO_PREFIX + "Hips"), None)
    if root is None:
        raise ValueError(f"--in-place {clip}: no '{MIXAMO_PREFIX}Hips' joint")
    outputs = {animation["samplers"][c["sampler"]]["output"]
               for c in animation.get("channels", [])
               if c["target"].get("node") == root and c["target"]["path"] == "translation"}
    if not outputs:
        raise ValueError(f"--in-place {clip}: the clip does not move its root")
    for output in outputs:
        sampler = next(s for s in animation["samplers"] if s["output"] == output)
        keys = read_floats(gltf, bin_bytes, output)
        if sampler.get("interpolation") == "CUBICSPLINE":
            raise ValueError(f"--in-place {clip}: cubic spline root tracks are not supported")
        x0, _, z0 = keys[0]
        write_floats(gltf, bin_bytes, output, [(x0, y, z0) for _, y, _ in keys])


def sample_track(times, values, at, interpolation, rotation, context):
    """Return one track's value at time `at`: its key there, the earlier key
    under STEP, or a blend of the keys either side (renormalized for a
    rotation). A track keyed only before `at` holds its last value."""
    for t, value in zip(times, values):
        if abs(t - at) <= 1e-4:
            return value
    before = [i for i, t in enumerate(times) if t < at]
    if not before:
        raise ValueError(f"{context}: a track starts after {at * 1000:.0f} ms")
    prev = before[-1]
    if prev + 1 == len(times) or interpolation == "STEP":
        return values[prev]
    nxt = prev + 1
    weight = (at - times[prev]) / (times[nxt] - times[prev])
    later = values[nxt]
    if rotation and sum(a * b for a, b in zip(values[prev], later)) < 0:
        # q and -q are the same rotation; blend toward the nearer sign.
        later = [-c for c in later]
    blend = [a + (b - a) * weight for a, b in zip(values[prev], later)]
    if rotation:
        length = sum(c * c for c in blend) ** 0.5
        blend = [c / length for c in blend]
    return tuple(blend)


def trim_clip(gltf, bin_bytes, new, source, end_ms, start_ms=0.0):
    """Append clip `new`: `source` from `start_ms` to `end_ms`, each rounded
    down to a key, shifted to begin at zero. An `end_ms` of None runs to the
    end of `source`.

    The start rounds down to the latest key of any track, the key a windup cut
    at the same time ends on. A track with no key there gets one sampled from
    its neighbours; a track keyed only before the start holds its last value.
    The copy begins on the windup's last pose when every track still moving at
    the cut has a key there, as Blender's resampled exports do: its exporter
    thins only still tracks, to their first and last keys. A track moving
    between keys at the cut would start the copy off the windup, which holds
    that track's last key.

    A copy that starts after zero moves its root (Hips) on the ground plane to
    start where `source` starts. The engine removes each clip's net ground
    drift so its first and last root keys sit over the entity, so a windup
    holds at the source's first root position. Unshifted, the strike would
    start wherever the windup's raw drift left the hips, and the body would pop
    there. `bin_bytes` is a bytearray that grows with the copy's keys.

    The copy keeps the source's events inside its cut, shifted with its keys;
    mixamo_clip_events.carried_events holds the rule.
    """
    if any(a.get("name") == new for a in gltf.get("animations", [])):
        raise ValueError(f"--trim {new}: a clip with that name already exists")
    if end_ms is not None and end_ms <= start_ms:
        raise ValueError(f"--trim {new}: the end must come after the start")
    animation = find_clip(gltf, source)
    for sampler in animation["samplers"]:
        if sampler.get("interpolation") == "CUBICSPLINE":
            raise ValueError(f"--trim {new}: cubic spline clips are not supported")
    tracks = clip_track_times(gltf, bin_bytes, animation)
    clip_end = max(times[-1] for times in tracks.values())
    end = clip_end + 1e-4 if end_ms is None else end_ms / 1000.0 + 1e-4
    if end_ms is not None and clip_end < end - 2e-4:
        raise ValueError(f"--trim {new}: {end_ms:g} ms is past the end of "
                         f"'{source}' ({clip_end * 1000:.0f} ms)")
    start = 0.0
    if start_ms:
        if start_ms / 1000.0 >= clip_end - 1e-4:
            raise ValueError(f"--trim {new}: {start_ms:g} ms is at or past the end "
                             f"of '{source}' ({clip_end * 1000:.0f} ms)")
        start = cut_key(tracks.values(), start_ms / 1000.0)
        if start is None:
            raise ValueError(f"--trim {new}: {start_ms:g} ms is before the first key")
        if not any(start + 1e-4 < t <= end for times in tracks.values() for t in times):
            raise ValueError(f"--trim {new}: the range has no key after its start "
                             f"({start * 1000:.0f} ms), so it would hold one pose")
    rotations = {c["sampler"] for c in animation["channels"]
                 if c["target"].get("path") == "rotation"}
    root = next((i for i, node in enumerate(gltf.get("nodes", []))
                 if node.get("name") == MIXAMO_PREFIX + "Hips"), None)
    root_moves = {c["sampler"] for c in animation["channels"]
                  if root is not None and c["target"].get("node") == root
                  and c["target"].get("path") == "translation"}
    inputs = {}
    samplers = []
    for index, sampler in enumerate(animation["samplers"]):
        times = tracks[sampler["input"]]
        all_values = read_floats(gltf, bin_bytes, sampler["output"])
        if sum(1 for t in times if t <= end) == 0:
            raise ValueError(f"--trim {new}: {end_ms:g} ms is before the first key")
        if not start_ms:
            kept = sum(1 for t in times if t <= end)
            span, values = times[:kept], all_values[:kept]
        else:
            after = [i for i, t in enumerate(times) if start + 1e-4 < t <= end]
            span = [start] + [times[i] for i in after]
            values = [sample_track(times, all_values, start, sampler.get("interpolation"),
                                   index in rotations, f"--trim {new}")]
            values += [all_values[i] for i in after]
            if index in root_moves:
                dx = all_values[0][0] - values[0][0]
                dz = all_values[0][2] - values[0][2]
                values = [(x + dx, y, z + dz) for x, y, z in values]
        if sampler["input"] not in inputs:
            shifted = [(t - start,) for t in span]
            inputs[sampler["input"]] = append_floats(
                gltf, bin_bytes, shifted, "SCALAR", bounds=True)
        output = gltf["accessors"][sampler["output"]]
        copy = {"input": inputs[sampler["input"]],
                "output": append_floats(gltf, bin_bytes, values, output["type"],
                                        bounds="min" in output)}
        if "interpolation" in sampler:
            copy["interpolation"] = sampler["interpolation"]
        samplers.append(copy)
    events = trim_events(animation, list(tracks.values()), start_ms, end_ms, f"--trim {new}")
    gltf["animations"].append({
        "name": new,
        "channels": [dict(c, target=dict(c["target"])) for c in animation["channels"]],
        "samplers": samplers,
    })
    set_clip_events(gltf["animations"][-1], events)


def gltf_bin_path(gltf_path, gltf):
    buffers = gltf.get("buffers", [])
    uri = buffers[0].get("uri", "") if len(buffers) == 1 else ""
    if not uri or uri.startswith("data:"):
        raise ValueError(f"{gltf_path}: expected one external buffer")
    return Path(gltf_path).parent / unquote(uri)


def postprocess_gltf(gltf_path, tag=True, clip_library=None, library=False,
                     tag_if_missing=False, in_place=(), trims=(), events=()):
    """Run the post-export pass on a `.gltf` and its `.bin`, in place.

    Normalization comes first so tagging sees canonical names; `tag` gates only
    tagging, so a binding survives --no-tag. `tag_if_missing` tags despite
    `tag=False` when names changed or the document has no engine tags.

    Clip edits run pins, then events, then trims, so a trim keeps the events
    inside its cut. Pinning moves no key in time.
    """
    gltf_path = Path(gltf_path)
    gltf = read_gltf(gltf_path)

    renamed = normalize_bone_prefixes(gltf)
    if renamed:
        print(f"\nNormalized {renamed} numbered bone prefix(es) to '{MIXAMO_PREFIX}'")
    if tag or (tag_if_missing and (renamed or not has_engine_tags(gltf))):
        print_tag_summary(gltf, tag_skeleton(gltf))

    if in_place or trims or events:
        if clip_library is not None:
            raise ValueError("--in-place, --trim and --event edit clips, and a bound "
                             "character keeps none; apply them to its library")
        bin_path = gltf_bin_path(gltf_path, gltf)
        bin_bytes = bytearray(bin_path.read_bytes())
        for clip in in_place:
            pin_root_in_place(gltf, bin_bytes, clip)
            print(f"\nPinned '{clip}' in place")
        if events:
            attach_events(gltf, document_track_times(gltf, bin_bytes), events)
        for source, name, ms in events:
            print(f"\nAdded event '{name}' at {ms:g} ms to '{source}'")
        for new, source, start_ms, end_ms in trims:
            trim_clip(gltf, bin_bytes, new, source, end_ms=end_ms, start_ms=start_ms)
            end = "its end" if end_ms is None else f"{end_ms:g} ms"
            span = f"at {end}" if not start_ms else f"from {start_ms:g} ms to {end}"
            print(f"\nTrimmed '{source}' {span} into '{new}'")
        bin_path.write_bytes(bytes(bin_bytes))

    if library or clip_library is not None:
        bin_path = gltf_bin_path(gltf_path, gltf)
        bin_bytes = bin_path.read_bytes()
        if library:
            check_library(gltf)
            bin_bytes = strip_skins(gltf, bin_bytes)
            print(f"\nWrote clip library: {len(gltf['animations'])} clip(s)")
        else:
            bin_bytes = strip_clips(gltf, bin_bytes)
            stamp_clip_library(gltf, clip_library)
            print(f"\nStripped clips; bound to clip library '{clip_library}'")
        bin_path.write_bytes(bin_bytes)

    write_gltf(gltf_path, gltf)


def bind_gltf_to_clip_library(gltf_path, clip_library):
    """Bind an already-exported character to a library, with no Blender run.

    A committed canonical model keeps the extras it has. A numbered or untagged
    rig is tagged, or it would lose its masks, sockets and zones without a word.
    """
    postprocess_gltf(gltf_path, tag=False, clip_library=clip_library, tag_if_missing=True)


def stamp_events_in_library(gltf_path, events, trims=()):
    """Stamp events into a committed library from Python, with no Blender run.

    A maintenance helper, not a converter flag: maintainers run it against a
    committed library to add events without re-exporting. It matches a full
    re-export only when the trimmed clips carry no events yet and no `--event`
    targets a trimmed clip; a rebuild rejects an event on a trim, since events
    attach before trims run.

    `events` are parse_event tuples and `trims` the parse_trim tuples the library
    was built with; mixamo_clip_events.stamp_library_events documents the result.
    """
    gltf_path = Path(gltf_path)
    gltf = read_gltf(gltf_path)
    bin_bytes = gltf_bin_path(gltf_path, gltf).read_bytes()
    stamp_library_events(gltf, document_track_times(gltf, bin_bytes), events, trims)
    write_gltf(gltf_path, gltf)
