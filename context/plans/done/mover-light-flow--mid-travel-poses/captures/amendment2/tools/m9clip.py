# M9 sightings: HEAD | dense truth | (HEAD - truth) x4 around 128; mask 02 unless M9_MASK (hex, or 'none').
#   python m9clip.py clip  <view> <t-spec> <out.mp4> <title> [crop x0,y0,x1,y1] [gain] [fps]
#   python m9clip.py sheet <view> <t,t,..> <out.png> <title> [crop] [gain]
import os, shutil, subprocess, sys
import numpy as np
from PIL import Image, ImageDraw, ImageFont
import m9cap

try:
    FONT = ImageFont.truetype('arialbd.ttf', 20); SMALL = ImageFont.truetype('arial.ttf', 16)
except OSError:
    FONT = SMALL = ImageFont.load_default()
BG, INK = (16, 16, 16), (240, 240, 236)
PW = 420


MASK = None if os.environ.get('M9_MASK', '02') == 'none' else os.environ.get('M9_MASK', '02')


def panel_row(view, t, crop, gain, mask=None):
    mask = MASK if mask is None else mask
    h = np.asarray(Image.open(m9cap.path(view, t, 'head', mask)).convert('RGB'), dtype=np.float32)
    tr = np.asarray(Image.open(m9cap.path(view, t, 'truth', mask)).convert('RGB'), dtype=np.float32)
    x0, y0, x1, y1 = crop
    h, tr = h[y0:y1, x0:x1], tr[y0:y1, x0:x1]
    lum = lambda a: a @ np.array([0.2126, 0.7152, 0.0722], np.float32)
    d = (lum(h) - lum(tr)) * 4.0
    dr = np.stack([128 + np.clip(d, 0, 127), 128 - np.abs(d) * 0.5, 128 + np.clip(-d, 0, 127)], -1)
    return [np.clip(h * gain, 0, 255), np.clip(tr * gain, 0, 255), np.clip(dr, 0, 255)]


def compose(view, t, crop, gain, title, sub):
    ims = panel_row(view, t, crop, gain)
    h0, w0 = ims[0].shape[:2]
    ph = int(round(PW * h0 / w0)) // 2 * 2
    W, H = PW * 3, 64 + 26 + ph
    H += H % 2
    S = Image.new('RGB', (W, H), BG); d = ImageDraw.Draw(S)
    d.text((8, 6), f'{title}   t′ {t:.4f}', font=FONT, fill=INK)
    d.text((8, 36), sub, font=SMALL, fill=(190, 190, 186))
    names = ['HEAD', 'dense same-rule truth', 'HEAD − truth ×4 (red brighter, blue darker)']
    for k, im in enumerate(ims):
        d.text((k * PW + 6, 67), names[k], font=SMALL, fill=INK)
        S.paste(Image.fromarray(im.astype(np.uint8)).resize((PW - 2, ph)), (k * PW + 1, 90))
    return S


if __name__ == '__main__':
    mode, view, tspec, out, title = sys.argv[1:6]
    crop = tuple(int(v) for v in sys.argv[6].split(',')) if len(sys.argv) > 6 else (0, 0, 960, 540)
    gain = float(sys.argv[7]) if len(sys.argv) > 7 else 2.0
    fps = int(sys.argv[8]) if len(sys.argv) > 8 else 30
    ts = m9cap.parse_ts(tspec)
    sub = (f'Static indirect only (light_term_mask 0x02), frames brightened {gain:g}x.' if MASK == '02'
           else f'All light terms, frames brightened {gain:g}x.' if MASK is None else f'light_term_mask 0x{MASK}, frames brightened {gain:g}x.')
    if mode == 'sheet':
        rows = [np.asarray(compose(view, t, crop, gain, title, sub)) for t in ts]
        Image.fromarray(np.concatenate(rows, 0)).save(out); print(out); sys.exit()
    tmp = out + '.tmp'; shutil.rmtree(tmp, ignore_errors=True); os.makedirs(tmp)
    for i, t in enumerate(ts):
        compose(view, t, crop, gain, title, sub + f' Played at {fps} fps.').save(os.path.join(tmp, f'f{i:04d}.png'))
    subprocess.run(['ffmpeg', '-y', '-loglevel', 'error', '-framerate', str(fps), '-i', os.path.join(tmp, 'f%04d.png'),
                    '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-crf', '24', '-preset', 'slow', '-movflags', '+faststart', out],
                   check=True)
    shutil.rmtree(tmp, ignore_errors=True)
    print(out, os.path.getsize(out) // 1024, 'KiB')
