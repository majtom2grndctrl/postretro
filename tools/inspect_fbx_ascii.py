#!/usr/bin/env python3
"""
Inspect an ASCII FBX file and extract structural information relevant to
the Mixamo-to-glTF pipeline: armature/model transforms, bone hierarchy,
rest poses, animation curve metadata, and root bone keyframe samples.

Usage:
    python tools/inspect_fbx_ascii.py path/to/file.fbx [--anim-samples N]

Output is plain text, designed to be read by a human or piped into context.
Keeps output compact — samples keyframes rather than dumping all of them.
"""

import sys
import re
import argparse
from pathlib import Path
from collections import defaultdict


def parse_args():
    parser = argparse.ArgumentParser(description="Inspect ASCII FBX structure")
    parser.add_argument("fbx_path", help="Path to an ASCII .fbx file")
    parser.add_argument(
        "--anim-samples", type=int, default=8,
        help="Number of animation keyframe values to sample per curve (default: 8)"
    )
    parser.add_argument(
        "--bone-filter", default=None,
        help="Only show bones matching this substring (case-insensitive)"
    )
    parser.add_argument(
        "--full-hierarchy", action="store_true",
        help="Print the full bone parent-child tree"
    )
    return parser.parse_args()


class FbxNode:
    """Minimal representation of an FBX node for structural inspection."""
    __slots__ = ("name", "props", "children", "line_num")

    def __init__(self, name, props=None, line_num=0):
        self.name = name
        self.props = props or []
        self.children = []
        self.line_num = line_num

    def find(self, name):
        """Find first direct child with given name."""
        for c in self.children:
            if c.name == name:
                return c
        return None

    def find_all(self, name):
        """Find all direct children with given name."""
        return [c for c in self.children if c.name == name]

    def find_recursive(self, name):
        """Find all descendants with given name."""
        result = []
        for c in self.children:
            if c.name == name:
                result.append(c)
            result.extend(c.find_recursive(name))
        return result

    def prop_string(self):
        return ", ".join(str(p) for p in self.props)


def tokenize_props(prop_str):
    """Parse the property string after a node name into typed values."""
    props = []
    if not prop_str or not prop_str.strip():
        return props
    # Split on commas, but respect quoted strings
    current = ""
    in_quote = False
    for ch in prop_str:
        if ch == '"' and not in_quote:
            in_quote = True
            current += ch
        elif ch == '"' and in_quote:
            in_quote = False
            current += ch
        elif ch == ',' and not in_quote:
            props.append(parse_value(current.strip()))
            current = ""
        else:
            current += ch
    if current.strip():
        props.append(parse_value(current.strip()))
    return props


def parse_value(s):
    """Parse a single FBX property value."""
    if not s:
        return s
    if s.startswith('"') and s.endswith('"'):
        return s[1:-1]
    # Try numeric
    try:
        if '.' in s or 'e' in s or 'E' in s:
            return float(s)
        return int(s)
    except ValueError:
        return s


def parse_fbx_ascii(filepath):
    """
    Parse an ASCII FBX file into a tree of FbxNodes.
    Handles the large-file case by streaming line by line.
    """
    root = FbxNode("__root__")
    stack = [root]

    # Pattern: "  NodeName: prop1, prop2, ... {"  or "  NodeName: {"  or just "}"
    node_re = re.compile(r'^(\s*)(\w+)\s*:\s*(.*?)\s*(\{)?\s*$')
    # Some nodes span: "NodeName: {" on one line
    close_re = re.compile(r'^\s*\}\s*$')
    # Continuation of properties (arrays of numbers)
    array_re = re.compile(r'^\s*(-?[\d.eE+\-]+\s*,?\s*)+$')

    with open(filepath, 'r', errors='replace') as f:
        line_num = 0
        pending_array = None

        for line in f:
            line_num += 1
            stripped = line.rstrip()

            if not stripped:
                continue

            # Close brace
            if close_re.match(stripped):
                if len(stack) > 1:
                    stack.pop()
                continue

            m = node_re.match(stripped)
            if m:
                name = m.group(2)
                prop_str = m.group(3)
                has_brace = m.group(4) is not None

                # Check if the prop_str ends with { (no space before it)
                if not has_brace and prop_str.endswith('{'):
                    prop_str = prop_str[:-1].rstrip()
                    if prop_str.endswith(','):
                        prop_str = prop_str[:-1].rstrip()
                    has_brace = True

                props = tokenize_props(prop_str)
                node = FbxNode(name, props, line_num)
                stack[-1].children.append(node)

                if has_brace:
                    stack.append(node)
                continue

            # Lines that are just data (array continuations, etc.) —
            # attach to current parent as a raw-data child
            if array_re.match(stripped) and len(stack) > 1:
                # Store numeric arrays as property data on a special node
                data_node = FbxNode("__data__", line_num=line_num)
                nums = re.findall(r'-?[\d.]+(?:[eE][+\-]?\d+)?', stripped)
                data_node.props = [float(n) if '.' in n or 'e' in n.lower() else int(n) for n in nums]
                stack[-1].children.append(data_node)

    return root


def extract_objects(root):
    """Extract Objects section — the main content."""
    objects = root.find("Objects")
    if not objects:
        # Try under a top-level node
        for c in root.children:
            objects = c.find("Objects")
            if objects:
                break
    return objects


def extract_model_nodes(objects):
    """Find all Model nodes (armatures, meshes, bones)."""
    if not objects:
        return []
    return objects.find_all("Model")


def extract_properties(node):
    """Extract P (property) entries from a Properties70 block."""
    props = {}
    p70 = node.find("Properties70")
    if not p70:
        return props
    for p in p70.find_all("P"):
        if p.props and len(p.props) >= 5:
            name = p.props[0]
            values = p.props[4:]
            if len(values) == 1:
                props[name] = values[0]
            else:
                props[name] = values
        elif p.props:
            props[p.props[0]] = p.props[1:]
    return props


def format_vec(vals):
    """Format a list of numbers compactly."""
    if isinstance(vals, (int, float)):
        return f"{vals}"
    return f"[{', '.join(f'{v:.6g}' for v in vals if isinstance(v, (int, float)))}]"


def extract_anim_curves(objects):
    """Extract AnimationCurve nodes with their keyframe data."""
    if not objects:
        return []
    curves = []
    for node in objects.find_all("AnimationCurve"):
        curve_id = node.props[0] if node.props else None
        name = node.props[1] if len(node.props) > 1 else ""

        key_count_node = node.find("KeyCount")
        key_count = key_count_node.props[0] if key_count_node and key_count_node.props else 0

        # Collect KeyValueFloat data
        kvf = node.find("KeyValueFloat")
        values = []
        if kvf:
            for data in kvf.find_all("__data__"):
                values.extend(data.props)

        # Also try direct "a" property (alternate format)
        a_node = node.find("a")
        if a_node and a_node.props:
            values.extend(a_node.props)

        curves.append({
            "id": curve_id,
            "name": name,
            "key_count": key_count,
            "values": values,
        })
    return curves


def extract_connections(root):
    """Extract connection (parent-child) relationships."""
    connections_node = root.find("Connections")
    if not connections_node:
        for c in root.children:
            connections_node = c.find("Connections")
            if connections_node:
                break
    if not connections_node:
        return []

    conns = []
    for c in connections_node.find_all("C"):
        if c.props and len(c.props) >= 3:
            conns.append({
                "type": c.props[0],
                "child": c.props[1],
                "parent": c.props[2],
                "prop": c.props[3] if len(c.props) > 3 else None,
            })
    return conns


def sample_values(values, n):
    """Return up to n evenly-spaced samples from a list."""
    if not values:
        return []
    if len(values) <= n:
        return values
    step = max(1, len(values) // n)
    return [values[i] for i in range(0, len(values), step)][:n]


def main():
    args = parse_args()
    fbx_path = Path(args.fbx_path)

    if not fbx_path.exists():
        print(f"ERROR: File not found: {fbx_path}")
        sys.exit(1)

    # Quick binary check
    with open(fbx_path, 'rb') as f:
        header = f.read(32)
    if header.startswith(b'Kaydara FBX Binary'):
        print(f"ERROR: {fbx_path.name} is a BINARY FBX, not ASCII.")
        print("This script only handles ASCII FBX files.")
        print("Try re-downloading from Mixamo with 'ASCII' format selected,")
        print("or use Blender to convert: import the binary FBX, then")
        print("File > Export > FBX with 'ASCII' checked.")
        sys.exit(1)

    file_size = fbx_path.stat().st_size
    print(f"{'=' * 60}")
    print(f"FBX ASCII Inspector — {fbx_path.name}")
    print(f"{'=' * 60}")
    print(f"File size: {file_size / 1024 / 1024:.1f} MB")
    print()

    print("Parsing FBX structure...")
    root = parse_fbx_ascii(fbx_path)

    # -- Header info --
    header_ext = root.find("FBXHeaderExtension")
    if header_ext:
        version = header_ext.find("FBXVersion")
        if version and version.props:
            print(f"FBX Version: {version.props[0]}")
        creator = header_ext.find("Creator")
        if creator and creator.props:
            print(f"Creator: {creator.prop_string()}")

    # -- Global settings --
    global_settings = root.find("GlobalSettings")
    if global_settings:
        gprops = extract_properties(global_settings)
        print(f"\n--- Global Settings ---")
        for key in ("UpAxis", "UpAxisSign", "FrontAxis", "FrontAxisSign",
                     "CoordAxis", "CoordAxisSign", "UnitScaleFactor",
                     "OriginalUnitScaleFactor"):
            if key in gprops:
                print(f"  {key}: {format_vec(gprops[key])}")

    # -- Objects --
    objects = extract_objects(root)
    models = extract_model_nodes(objects)

    if not models:
        print("\nWARNING: No Model nodes found. The FBX structure may be non-standard.")
        print(f"Top-level nodes: {[c.name for c in root.children]}")
        sys.exit(0)

    # Categorize models
    armatures = []
    meshes = []
    bones = []
    other = []

    for m in models:
        # Model type is usually in the props or a child
        type_str = ""
        if len(m.props) >= 3:
            type_str = str(m.props[2]) if m.props[2] else ""
        elif len(m.props) >= 2:
            type_str = str(m.props[1]) if m.props[1] else ""

        name = m.props[1] if len(m.props) >= 2 else str(m.props[0]) if m.props else "?"
        # Clean the "Model::" prefix
        if isinstance(name, str):
            name = name.replace("Model::", "").replace("\\x00\\x01", "").strip()

        model_id = m.props[0] if m.props else None

        model_info = {
            "node": m,
            "name": name,
            "type": type_str,
            "id": model_id,
            "properties": extract_properties(m),
        }

        type_lower = type_str.lower()
        if "limb" in type_lower or "skeleton" in type_lower or "null" in type_lower:
            bones.append(model_info)
        elif "mesh" in type_lower:
            meshes.append(model_info)
        else:
            # Check name heuristics
            if "mixamorig:" in name or "Hips" in name:
                bones.append(model_info)
            elif "Armature" in name:
                armatures.append(model_info)
            else:
                other.append(model_info)

    print(f"\n--- Object Summary ---")
    print(f"  Models total: {len(models)}")
    print(f"  Armatures: {len(armatures)}")
    print(f"  Meshes: {len(meshes)}")
    print(f"  Bones/Limbs: {len(bones)}")
    print(f"  Other: {len(other)}")

    # -- Armature details --
    if armatures:
        print(f"\n--- Armature(s) ---")
        for arm in armatures:
            props = arm["properties"]
            print(f"  {arm['name']} (id={arm['id']}, type={arm['type']})")
            for key in ("Lcl Translation", "Lcl Rotation", "Lcl Scaling",
                         "PreRotation", "PostRotation", "RotationOffset",
                         "ScalingOffset", "GeometricTranslation",
                         "GeometricRotation", "GeometricScaling"):
                if key in props:
                    print(f"    {key}: {format_vec(props[key])}")

    # -- Mesh details --
    if meshes:
        print(f"\n--- Mesh Object(s) ---")
        for mesh in meshes:
            props = mesh["properties"]
            print(f"  {mesh['name']} (id={mesh['id']}, type={mesh['type']})")
            for key in ("Lcl Translation", "Lcl Rotation", "Lcl Scaling",
                         "GeometricTranslation", "GeometricRotation",
                         "GeometricScaling"):
                if key in props:
                    print(f"    {key}: {format_vec(props[key])}")

    # -- Bone details --
    if bones:
        print(f"\n--- Bones ({len(bones)}) ---")
        bone_filter = args.bone_filter.lower() if args.bone_filter else None
        shown = 0
        key_bones = {"Hips", "Spine", "Head", "LeftFoot", "RightFoot",
                     "LeftHand", "RightHand", "LeftUpLeg", "RightUpLeg"}

        for bone in bones:
            name = bone["name"]
            short = name.replace("mixamorig:", "")

            if bone_filter and bone_filter not in name.lower():
                continue

            # Show key bones always, others only with --full-hierarchy or filter
            is_key = short in key_bones
            if not is_key and not args.full_hierarchy and not bone_filter:
                continue

            props = bone["properties"]
            print(f"  {name} (id={bone['id']})")
            for key in ("Lcl Translation", "Lcl Rotation", "Lcl Scaling",
                         "PreRotation", "PostRotation"):
                if key in props:
                    print(f"    {key}: {format_vec(props[key])}")
            shown += 1

        if not args.full_hierarchy and not bone_filter:
            print(f"  ... showing {shown} key bones of {len(bones)} total")
            print(f"  Use --full-hierarchy to see all, or --bone-filter <name>")

    # -- Connections (build parent-child map) --
    connections = extract_connections(root)
    id_to_name = {}
    for m in models:
        mid = m.props[0] if m.props else None
        name = m.props[1] if len(m.props) >= 2 else "?"
        if isinstance(name, str):
            name = name.replace("Model::", "").strip()
        if mid is not None:
            id_to_name[mid] = name

    parent_map = {}
    children_map = defaultdict(list)
    for conn in connections:
        if conn["type"] == "OO":
            child_name = id_to_name.get(conn["child"], f"id:{conn['child']}")
            parent_name = id_to_name.get(conn["parent"], f"id:{conn['parent']}")
            parent_map[child_name] = parent_name
            children_map[parent_name].append(child_name)

    # Print skeleton hierarchy (just top levels)
    print(f"\n--- Skeleton Hierarchy (top levels) ---")
    hips_name = None
    for name in id_to_name.values():
        if "Hips" in name:
            hips_name = name
            break

    def print_tree(name, depth=0, max_depth=3):
        prefix = "  " * (depth + 1)
        print(f"{prefix}{name}")
        if depth < max_depth:
            for child in children_map.get(name, []):
                print_tree(child, depth + 1, max_depth)
        elif children_map.get(name):
            print(f"{prefix}  ... ({len(children_map[name])} children)")

    if hips_name:
        print_tree(hips_name, max_depth=2)
    else:
        print("  (Could not locate Hips bone)")

    # -- Animation curves --
    anim_curves = extract_anim_curves(objects)
    print(f"\n--- Animation Data ---")
    print(f"  AnimationCurve nodes: {len(anim_curves)}")

    if anim_curves:
        total_keys = sum(c["key_count"] for c in anim_curves)
        total_values = sum(len(c["values"]) for c in anim_curves)
        print(f"  Total declared keyframes: {total_keys}")
        print(f"  Total extracted values: {total_values}")

        # Find curves connected to key bones
        curve_id_to_curve = {c["id"]: c for c in anim_curves if c["id"]}

        # AnimationCurveNode connections tell us what bone each curve targets
        anim_curve_nodes = []
        if objects:
            anim_curve_nodes = objects.find_all("AnimationCurveNode")

        acn_id_to_info = {}
        for acn in anim_curve_nodes:
            acn_id = acn.props[0] if acn.props else None
            acn_name = acn.props[1] if len(acn.props) >= 2 else ""
            if isinstance(acn_name, str):
                acn_name = acn_name.replace("AnimCurveNode::", "").strip()
            acn_id_to_info[acn_id] = acn_name

        # Map: curve_node_id -> bone_name, curve_node_id -> channel_name
        acn_to_bone = {}
        acn_to_curves = defaultdict(list)
        for conn in connections:
            if conn["type"] == "OO":
                child_name = id_to_name.get(conn["child"])
                parent_name = id_to_name.get(conn["parent"])
                # ACN -> bone connection
                if conn["child"] in acn_id_to_info and conn["parent"] in id_to_name:
                    acn_to_bone[conn["child"]] = id_to_name[conn["parent"]]
            if conn["type"] in ("OO", "OP"):
                # Curve -> ACN connection
                if conn["child"] in curve_id_to_curve and conn["parent"] in acn_id_to_info:
                    channel = conn.get("prop", "?")
                    acn_to_curves[conn["parent"]].append((conn["child"], channel))

        # Show root bone (Hips) animation curves
        print(f"\n--- Root Bone Animation Samples ---")
        shown_any = False
        for acn_id, bone_name in acn_to_bone.items():
            if "Hips" not in bone_name:
                continue
            channel = acn_id_to_info.get(acn_id, "?")
            print(f"\n  Bone: {bone_name}, Channel: {channel}")
            for curve_id, prop in acn_to_curves.get(acn_id, []):
                curve = curve_id_to_curve.get(curve_id)
                if not curve:
                    continue
                samples = sample_values(curve["values"], args.anim_samples)
                samples_str = ", ".join(f"{v:.4f}" if isinstance(v, float) else str(v)
                                       for v in samples)
                print(f"    {prop}: {curve['key_count']} keys,"
                      f" samples=[{samples_str}]")
                if curve["values"]:
                    vals = [v for v in curve["values"] if isinstance(v, (int, float))]
                    if vals:
                        print(f"      range: [{min(vals):.4f}, {max(vals):.4f}]")
                shown_any = True

        if not shown_any:
            print("  (Could not resolve Hips animation curves from connections)")
            # Fallback: show first few curves with data
            print(f"\n  First curves with data:")
            for curve in anim_curves[:5]:
                if curve["values"]:
                    samples = sample_values(curve["values"], args.anim_samples)
                    samples_str = ", ".join(f"{v:.4f}" if isinstance(v, float) else str(v)
                                           for v in samples)
                    print(f"    id={curve['id']}, name={curve['name']},"
                          f" keys={curve['key_count']}, samples=[{samples_str}]")

    print(f"\n{'=' * 60}")
    print("Done.")


if __name__ == "__main__":
    main()
