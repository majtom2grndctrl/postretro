#!/usr/bin/env python3
"""
Generate emissive maps (_e.png) from diffuse textures for Postretro.

The output keeps the source's sRGB-encoded RGB values only for bright texels,
so it is an authoring starting point for the static, per-texel emissive slot.
"""

import argparse
import os
from pathlib import Path

from PIL import Image, ImageOps
from PIL.PngImagePlugin import PngInfo


# Bright-texture cutoff by material prefix. Neon keeps more of its colored
# source pattern; other prefixes conservatively retain only highlight texels.
DEFAULT_CUTOFFS = {
    "neon": 0.35,
    "default": 0.75,
}


def get_cutoff(filename):
    """Determine the bright-texel cutoff from the material prefix."""
    prefix = filename.split("_")[0].lower()
    return DEFAULT_CUTOFFS.get(prefix, DEFAULT_CUTOFFS["default"])


def emissive_from_diffuse(image, cutoff):
    """Keep source-color texels at or above the luminance cutoff, else black."""
    source = image.convert("RGBA")
    luminance = ImageOps.grayscale(source)
    cutoff_byte = round(cutoff * 255.0)
    mask = luminance.point(lambda value: 255 if value >= cutoff_byte else 0)
    return Image.composite(source, Image.new("RGBA", source.size, (0, 0, 0, 255)), mask)


def process_image(input_path, output_path, cutoff=None, force=False):
    """Generate an sRGB-content, untagged emissive sibling from one diffuse PNG."""
    if os.path.exists(output_path) and not force:
        print(f"Skipping {input_path} (output already exists)")
        return

    try:
        with Image.open(input_path) as image:
            if cutoff is None:
                cutoff = get_cutoff(os.path.basename(input_path))

            print(f"Processing {input_path} -> {output_path} (cutoff: {cutoff})")
            emissive = emissive_from_diffuse(image, cutoff)

            # `_e` carries sRGB-encoded color content, but current prl-build
            # deliberately accepts it regardless of PNG metadata. Strip all
            # color-management chunks so generated output matches the sibling
            # tools and remains an untagged, portable authoring asset.
            emissive.info.pop("srgb", None)
            emissive.info.pop("gamma", None)
            emissive.info.pop("icc_profile", None)
            emissive.save(output_path, "PNG", pnginfo=PngInfo(), icc_profile=b"")
    except Exception as error:
        print(f"Error processing {input_path}: {error}")


def main():
    parser = argparse.ArgumentParser(description="Generate emissive maps for Postretro.")
    parser.add_argument("--input", required=True, help="Input file or directory")
    parser.add_argument(
        "--cutoff",
        type=float,
        help="Keep texels at or above this luminance cutoff (0.0 - 1.0)",
    )
    parser.add_argument("--recursive", action="store_true", help="Process directories recursively")
    parser.add_argument("--force", action="store_true", help="Overwrite existing _e.png files")
    args = parser.parse_args()

    if args.cutoff is not None and not 0.0 <= args.cutoff <= 1.0:
        parser.error("--cutoff must be between 0.0 and 1.0")

    input_path = Path(args.input)
    if input_path.is_file():
        files = [input_path]
    elif input_path.is_dir():
        pattern = "**/*.[pj][np]g" if args.recursive else "*.[pj][np]g"
        files = list(input_path.glob(pattern))
    else:
        print(f"Invalid input path: {args.input}")
        return

    for source in files:
        if source.stem.endswith(("_s", "_n", "_e")):
            continue
        output_path = source.parent / f"{source.stem}_e.png"
        process_image(source, output_path, args.cutoff, args.force)


if __name__ == "__main__":
    main()
