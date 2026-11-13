# Side-by-side clip of a lintel exit window: final HEAD | ramp alone | prototype dark + r40.
#   python fclip.py <out_dir> [k0 k1]
import os, shutil, subprocess, sys
import numpy as np
from PIL import Image, ImageDraw, ImageFont
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import flib, fexit  # noqa: E402

OUT = sys.argv[1]
k0, k1 = (float(sys.argv[2]), float(sys.argv[3])) if len(sys.argv) > 3 else (0.96891, 0.97682)
try:
    FONT = ImageFont.truetype('arialbd.ttf', 20); SMALL = ImageFont.truetype('arial.ttf', 17)
except OSError:
    FONT = SMALL = ImageFont.load_default()
BG, INK = (16, 16, 16), (240, 240, 236)
PANELS = [('head', 'final build'), ('ohead', 'ramp alone'), ('odark', 'prototype dark + r40')]
CROP, GAIN, PW = (300, 40, 780, 330), 2.0, 420


def main():
    ts = fexit.window_ts(k0, k1)
    tmp = os.path.join(flib.F, 'cliptmp'); shutil.rmtree(tmp, ignore_errors=True); os.makedirs(tmp)
    for i, t in enumerate(ts):
        ims = []
        for col, _ in PANELS:
            a = np.asarray(Image.open(flib.path('door_ceiling', t, col)).convert('RGB'), dtype=np.float32)
            ims.append(a[CROP[1]:CROP[3], CROP[0]:CROP[2]] * GAIN)
        h0, w0 = ims[0].shape[:2]
        ph = int(round(PW * h0 / w0)) // 2 * 2
        W, H = PW * 3, 64 + 26 + ph
        H += H % 2
        S = Image.new('RGB', (W, H), BG); d = ImageDraw.Draw(S)
        frames = (t - k0) / fexit.FRAME
        d.text((8, 6), f"Lintel exit, above-door band   t′ {t:.5f}  ({frames:+.2f} frames from the inside knot)", font=FONT, fill=INK)
        d.text((8, 34), 'Camera rides the room. 8 samples per 60 fps frame, played at 30 fps (16x slow). Frames brightened 2x.',
               font=SMALL, fill=(190, 190, 186))
        for k, im in enumerate(ims):
            x = k * PW
            d.text((x + 6, 67), PANELS[k][1], font=SMALL, fill=INK)
            S.paste(Image.fromarray(np.clip(im, 0, 255).astype(np.uint8)).resize((PW - 2, ph)), (x + 1, 90))
        S.save(os.path.join(tmp, f'f{i:04d}.png'))
    out = os.path.join(OUT, f'a2-final-exit-{k0:.4f}.mp4')
    subprocess.run(['ffmpeg', '-y', '-loglevel', 'error', '-framerate', '30', '-i', os.path.join(tmp, 'f%04d.png'),
                    '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-crf', '24', '-preset', 'slow', '-movflags', '+faststart', out],
                   check=True)
    shutil.rmtree(tmp, ignore_errors=True)
    print(out, os.path.getsize(out) // 1024, 'KiB')


main()
