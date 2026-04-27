#!/usr/bin/env python3
"""
Generate specular maps (_s.png) from diffuse textures for Postretro.
Uses material prefixes and lightweight visual processing heuristics.
"""

import os
import argparse
from pathlib import Path
from PIL import Image, ImageOps
from PIL.PngImagePlugin import PngInfo

# Heuristics based on material prefix
# (intensity, gamma)
DEFAULTS = {
    "metal": (1.0, 1.0),
    "concrete": (0.3, 2.0),
    "plaster": (0.3, 2.0),
    "stone": (0.3, 2.0),
    "wood": (0.5, 1.5),
    "default": (0.5, 1.5),
}

def get_heuristics(filename):
    """Determine intensity and gamma based on filename prefix."""
    prefix = filename.split('_')[0].lower()
    return DEFAULTS.get(prefix, DEFAULTS["default"])

def process_image(input_path, output_path, intensity=None, gamma=None, force=False):
    """
    Generate a specular map from a diffuse texture.
    out = (in / 255.0)^gamma * intensity * 255.0
    """
    if os.path.exists(output_path) and not force:
        print(f"Skipping {input_path} (output already exists)")
        return

    try:
        with Image.open(input_path) as img:
            # Determine heuristics if not provided
            if intensity is None or gamma is None:
                h_int, h_gam = get_heuristics(os.path.basename(input_path))
                intensity = intensity if intensity is not None else h_int
                gamma = gamma if gamma is not None else h_gam

            print(f"Processing {input_path} -> {output_path} (Int: {intensity}, Gamma: {gamma})")

            # Convert to grayscale
            gray = ImageOps.grayscale(img)
            
            # Apply gamma and intensity
            # We use point() for efficient pixel-wise transformation
            if gamma == 1.0 and intensity == 1.0:
                spec = gray
            else:
                # out = ((pix / 255.0) ^ gamma) * intensity * 255.0
                lut = [
                    int(min(255, max(0, ((i / 255.0) ** gamma) * intensity * 255.0)))
                    for i in range(256)
                ]
                spec = gray.point(lut)

            # Specular maps must be linear PNGs — no sRGB / gAMA / iCCP
            # chunks. `prl-build` rejects non-linear `_s.png` siblings at
            # compile time. Strip every color-management chunk Pillow might
            # otherwise carry forward from the diffuse source: drop `info`
            # entries (PIL maps `srgb`/`gamma`/`icc_profile` from there) and
            # pass an empty `PngInfo` plus `icc_profile=b""` so no chunk
            # is written. See context/lib/resource_management.md §4.2.
            spec.info.pop("srgb", None)
            spec.info.pop("gamma", None)
            spec.info.pop("icc_profile", None)
            empty_meta = PngInfo()
            spec.save(output_path, "PNG", pnginfo=empty_meta, icc_profile=b"")
            
    except Exception as e:
        print(f"Error processing {input_path}: {e}")

def main():
    parser = argparse.ArgumentParser(description="Generate specular maps for Postretro.")
    parser.add_argument("--input", required=True, help="Input file or directory")
    parser.add_argument("--intensity", type=float, help="Override specular intensity (0.0 - 1.0)")
    parser.add_argument("--gamma", type=float, help="Override gamma exponent (>= 1.0)")
    parser.add_argument("--recursive", action="store_true", help="Process directories recursively")
    parser.add_argument("--force", action="store_true", help="Overwrite existing _s.png files")
    
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
        # Skip files that are already specular maps or normal maps
        if f.name.endswith("_s.png") or f.name.endswith("_n.png"):
            continue
            
        output_name = f.stem + "_s.png"
        output_path = f.parent / output_name
        
        process_image(f, output_path, args.intensity, args.gamma, args.force)

if __name__ == "__main__":
    main()
