#!/usr/bin/env python3
"""
Generate normal maps (_n.png) from diffuse textures for Postretro.
Uses Sobel filtering to derive tangent-space normals from luminance gradients.
"""

import os
import argparse
from pathlib import Path
from PIL import Image, ImageOps
from PIL.PngImagePlugin import PngInfo

try:
    import numpy as np
    HAS_NUMPY = True
except ImportError:
    HAS_NUMPY = False

# Strength controls how pronounced the surface detail appears.
# Higher values = steeper normals from the same gradient.
MATERIAL_STRENGTH = {
    "metal":    1.5,
    "concrete": 1.0,
    "plaster":  0.8,
    "stone":    1.2,
    "wood":     1.0,
    "default":  1.0,
}


def get_strength(filename):
    prefix = filename.split("_")[0].lower()
    return MATERIAL_STRENGTH.get(prefix, MATERIAL_STRENGTH["default"])


def sobel_normal_map(gray_array, strength):
    """
    Derive a tangent-space normal map from a grayscale height field using
    Sobel operators. Returns an RGBA uint8 array ready for PNG export.
    """
    h = gray_array.astype(np.float32) / 255.0

    # Sobel kernels — sample 3x3 neighbourhood
    # Wrap at edges so tiling textures stay seamless.
    def shift(arr, dy, dx):
        return np.roll(np.roll(arr, dy, axis=0), dx, axis=1)

    gx = (
        -1 * shift(h, -1, -1) + 1 * shift(h, -1, 1)
        - 2 * shift(h,  0, -1) + 2 * shift(h,  0, 1)
        - 1 * shift(h,  1, -1) + 1 * shift(h,  1, 1)
    ) * strength

    gy = (
        -1 * shift(h, -1, -1) - 2 * shift(h, -1, 0) - 1 * shift(h, -1, 1)
        + 1 * shift(h,  1, -1) + 2 * shift(h,  1, 0) + 1 * shift(h,  1, 1)
    ) * strength

    # Tangent-space normal: (-gx, -gy, 1), normalised.
    nx = -gx
    ny = -gy
    nz = np.ones_like(h)
    length = np.sqrt(nx * nx + ny * ny + nz * nz)
    nx /= length
    ny /= length
    nz /= length

    # Encode to [0, 255]: n * 0.5 + 0.5, then * 255.
    r = np.clip(nx * 0.5 + 0.5, 0.0, 1.0) * 255.0
    g = np.clip(ny * 0.5 + 0.5, 0.0, 1.0) * 255.0
    b = np.clip(nz * 0.5 + 0.5, 0.0, 1.0) * 255.0
    a = np.full_like(r, 255.0)

    return np.stack([r, g, b, a], axis=-1).astype(np.uint8)


def neutral_normal_map(size):
    """
    Return a flat (0,0,1) tangent-space normal map as a Pillow Image.
    Used as fallback when numpy is absent; produces no surface detail.
    Encodes to (127, 127, 255) — same as the engine's placeholder.
    """
    return Image.new("RGBA", size, (127, 127, 255, 255))


def process_image(input_path, output_path, strength=None, force=False):
    if os.path.exists(output_path) and not force:
        print(f"Skipping {input_path} (output already exists)")
        return

    try:
        with Image.open(input_path) as img:
            if strength is None:
                strength = get_strength(os.path.basename(input_path))

            if HAS_NUMPY:
                print(f"Processing {input_path} -> {output_path} (strength: {strength})")
                gray = np.array(ImageOps.grayscale(img))
                rgba_array = sobel_normal_map(gray, strength)
                normal = Image.fromarray(rgba_array, "RGBA")
            else:
                print(
                    f"Processing {input_path} -> {output_path} "
                    f"(flat fallback — install numpy for Sobel filtering)"
                )
                normal = neutral_normal_map(img.size)

            # Normal maps must be linear PNGs — no sRGB / gAMA / iCCP chunks.
            # prl-build rejects non-linear _n.png siblings at compile time.
            # Strip every color-management chunk Pillow might carry forward.
            # See context/lib/resource_management.md §4.3.
            normal.info.pop("srgb", None)
            normal.info.pop("gamma", None)
            normal.info.pop("icc_profile", None)
            empty_meta = PngInfo()
            normal.save(output_path, "PNG", pnginfo=empty_meta, icc_profile=b"")

    except Exception as e:
        print(f"Error processing {input_path}: {e}")


def main():
    parser = argparse.ArgumentParser(description="Generate normal maps for Postretro.")
    parser.add_argument("--input", required=True, help="Input file or directory")
    parser.add_argument(
        "--strength",
        type=float,
        help="Override normal-map strength (default: material heuristic, typically 1.0)",
    )
    parser.add_argument("--recursive", action="store_true", help="Process directories recursively")
    parser.add_argument("--force", action="store_true", help="Overwrite existing _n.png files")

    args = parser.parse_args()

    input_path = Path(args.input)

    if input_path.is_file():
        files = [input_path]
    elif input_path.is_dir():
        pattern = "**/*.[pj][np]g" if args.recursive else "*.[pj][np]g"
        files = list(input_path.glob(pattern))
    else:
        print(f"Invalid input path: {args.input}")
        return

    for f in files:
        # Skip files that are already sibling maps
        if f.stem.endswith("_s") or f.stem.endswith("_n"):
            continue

        output_path = f.parent / (f.stem + "_n.png")
        process_image(f, output_path, args.strength, args.force)


if __name__ == "__main__":
    main()
