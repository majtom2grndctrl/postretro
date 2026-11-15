# Side-by-side H.264 clips for Ruling B option (ii).
#   python dclips.py <out_dir>
# Panels (2 x 3): today | r40 alone | dark + r40 ;
#                 dense truth, today's rule + r40 | dense truth, dark rule + r40 | dark state + today's backface
import os, shutil, subprocess, sys
import numpy as np
from PIL import Image, ImageDraw, ImageFont
HERE = os.path.dirname(os.path.abspath(__file__))
# Scratch root (ws/, cap/, darkb/): A2_ROOT, else the scratch layout <A>/darkb/tools.
A = os.environ.get('A2_ROOT') or os.path.dirname(os.path.dirname(HERE))
os.environ.setdefault('A2_ROOT', A)
sys.path.insert(0, os.path.join(A, 'tools'))
sys.path.insert(0, HERE)  # acap.py / aseries.py sit beside these in the repo copy
sys.path.insert(0, HERE)
import dcap  # noqa: F401,E402
import acap  # noqa: E402

OUT = sys.argv[1]
try:
    FONT = ImageFont.truetype('arialbd.ttf', 20); SMALL = ImageFont.truetype('arial.ttf', 17)
except OSError:
    FONT = SMALL = ImageFont.load_default()
BG, INK = (16, 16, 16), (240, 240, 236)
PANELS = [('head', 'cur', 'today'), ('head', 'r40', 'r40 alone (ruling A)'), ('dark', 'r40', 'dark state + r40'),
          ('truth', 'r40', 'dense truth, today\'s rule, r40'), ('dtruth', 'r40', 'dense truth, dark rule, r40'),
          ('dark', 'cur', 'dark state, today\'s backface')]


def load(view, t, bake, form, crop, gain):
    p = os.path.join(acap.CAP, acap.label(view, t, bake, form) + '.png')
    a = np.asarray(Image.open(p).convert('RGB'), dtype=np.float32)
    if crop:
        a = a[crop[1]:crop[3], crop[0]:crop[2]]
    return a * gain


def clip(name, view, ts, crop, gain, pw, fps, title, note):
    tmp = os.path.join(A, 'darkb', 'cliptmp', name)
    shutil.rmtree(tmp, ignore_errors=True); os.makedirs(tmp)
    for i, t in enumerate(ts):
        imgs = []
        for p in PANELS:
            imgs.append(load(view, t, p[0], p[1], crop, gain))
        h0, w0 = imgs[0].shape[:2]
        ph = int(round(pw * h0 / w0)) // 2 * 2
        lab = 26
        W, H = pw * 3, 64 + (ph + lab) * 2
        H += H % 2
        S = Image.new('RGB', (W, H), BG); d = ImageDraw.Draw(S)
        d.text((8, 6), f'{title}   t′ {t:.4f}', font=FONT, fill=INK)
        d.text((8, 34), note, font=SMALL, fill=(190, 190, 186))
        labels = [p[2] for p in PANELS]
        for k, im in enumerate(imgs):
            r, c = divmod(k, 3)
            x, y = c * pw, 64 + r * (ph + lab)
            d.text((x + 6, y + 3), labels[k], font=SMALL, fill=INK)
            S.paste(Image.fromarray(np.clip(im, 0, 255).astype(np.uint8)).resize((pw - 2, ph)), (x + 1, y + lab))
        S.save(os.path.join(tmp, f'f{i:04d}.png'))
    out = os.path.join(OUT, name + '.mp4')
    subprocess.run(['ffmpeg', '-y', '-loglevel', 'error', '-framerate', str(fps), '-i', os.path.join(tmp, 'f%04d.png'),
                    '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-crf', '24', '-preset', 'slow', '-movflags', '+faststart', out],
                   check=True)
    shutil.rmtree(tmp, ignore_errors=True)
    print(name, os.path.getsize(out) // 1024, 'KiB')


if __name__ == '__main__':
    os.makedirs(OUT, exist_ok=True)
    clip('a2-ruling-b-dark-ceiling-band', 'door_ceiling', acap.parse_ts('0.1:0.6:135'), (0, 40, 960, 400), 2.0, 420, 30,
         'Ruling B (ii): rising-room ceiling, west-wall band', 'Camera rides the room, 0.66 u per frame, played at half speed. Frames brightened 2x.')
    clip('a2-ruling-b-dark-sweeper-east', 'sweepfix', acap.parse_ts('0:0.5:129'), (0, 120, 960, 420), 1.0, 420, 20,
         'Ruling B (ii): em_sweeper east face', 'Camera rides the slab, 1 u per frame (a third of real speed).')
    clip('a2-ruling-b-dark-sweeper-west', 'sweepwest', acap.parse_ts('0:0.5:129'), (0, 120, 960, 420), 1.0, 420, 20,
         'Ruling B (ii): em_sweeper west face', 'Camera rides the slab, 1 u per frame (a third of real speed).')
