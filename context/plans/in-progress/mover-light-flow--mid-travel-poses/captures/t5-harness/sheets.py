# Verdict sheets from the captures and results.json: per view, stills
# (today | through-base | lerp | stripped, rows by t′, metric labels) and a
# block map (12c excess for the floor; as built minus stripped for the roof
# and sweeper faces), each cropped to the measured region.
import json, os, subprocess, sys
sys.path.insert(0, os.path.dirname(__file__))
import cap

S = cap.S
OUT = sys.argv[1]
TMP = os.path.join(S, 'sheet-tmp')
os.makedirs(TMP, exist_ok=True)
R = json.load(open(os.path.join(S, 'results.json')))
FONT = '/System/Library/Fonts/Supplemental/Arial Unicode.ttf'
NAMES = {'today': 'today', 'tb': 'through-base', 'lerp': 'lerp', 'strip-new': 'stripped'}


def bbox(view):
    make, ts = cap.VIEWS[view]
    xs, ys = [], []
    for t in ts:
        for bx, by in cap.crop_blocks(make(t)):
            xs += [bx * 8, bx * 8 + 8]
            ys += [by * 8, by * 8 + 8]
    m = 16
    x0, y0 = max(min(xs) - m, 0), max(min(ys) - m, 0)
    x1, y1 = min(max(xs) + m, cap.W), min(max(ys) + m, cap.H)
    return x0, y0, x1 - x0, y1 - y0


def run(*args):
    subprocess.run(['magick', *args], check=True)


def montage(tiles, labels, cols, geometry, title, out):
    args = ['montage', '-font', FONT, '-pointsize', '11', '-background', '#202020',
            '-fill', 'white']
    for tile, text in zip(tiles, labels):
        args += ['-label', text, tile]
    args += ['-tile', f'{cols}x', '-geometry', geometry, '-title', title, '-colors', '256',
             out]
    run(*args)


def d12(e):
    return f"{e['mean']:+.1f}, +{e['bright']:.1f}/{e['peaks']} -{e['dark']:.1f}/{e['pits']}"


def c12(e):
    return f"{e['bright']:.1f}/{e['peaks']}"


def block_map(view, t, column, values, scale, offset):
    """PGM at block resolution: 128 + scale * value (diff) or scale * value."""
    w, h = cap.W // 8, cap.H // 8
    pix = bytearray(h * w)
    for (bx, by), v in values.items():
        pix[by * w + bx] = max(0, min(255, int(round(offset + scale * v))))
    path = os.path.join(TMP, f'map-{view}-{t:.2f}-{column}.pgm')
    open(path, 'wb').write(b'P5 %d %d 255\n' % (w, h) + bytes(pix))
    return path


def sheet(view, width):
    make, ts = cap.VIEWS[view]
    x0, y0, cw, ch = bbox(view)
    crop = f'{cw}x{ch}+{x0}+{y0}'
    tiles, labels, maps, map_labels = [], [], [], []
    rows = {row['t']: row for row in R[view]}
    for t in ts:
        row = rows[t]
        spec = make(t)
        blocks = cap.crop_blocks(spec)
        lumas = {c: cap.read_luma(os.path.join(cap.CAP, cap.label(view, t, c) + '.png'))
                 for c in NAMES}
        means = {c: cap.block_means(l, blocks) for c, l in lumas.items()}
        for c, name in NAMES.items():
            src = os.path.join(cap.CAP, cap.label(view, t, c) + '.png')
            dst = os.path.join(TMP, f'{view}-{t:.2f}-{c}.png')
            run(src, '-crop', crop, '+repage', '-resize', f'{width}x', dst)
            tiles.append(dst)
            if view == 'floor':
                text = f"t' {t:.2f} {name}: 12c {c12(row['12c'][c])}, mean {row['crop_mean'][c]:.1f}"
            elif c == 'strip-new':
                text = f"t' {t:.2f} stripped (reference)"
            else:
                text = f"t' {t:.2f} {name}: {d12(row['12d'][c])}"
            labels.append(text)
            # Block maps.
            if view == 'floor':
                ex = cap.excess_metric(means[c])['excess']
                values = {b: max(v, 0.0) for b, v in ex.items()}
                pgm = block_map(view, t, c, values, 255 / 8, 0)  # black 0, white +8 levels
            else:
                if c == 'strip-new':
                    continue
                strip = 'strip-new'  # pixel-identical to strip-today
                values = {b: means[c][b] - means[strip][b] for b in blocks}
                pgm = block_map(view, t, c, values, 127 / 16, 128)  # grey 0, ±16 levels
            png = os.path.join(TMP, f'map-{view}-{t:.2f}-{c}.png')
            run(pgm, '-scale', '800%', '-crop', crop, '+repage', '-resize', f'{width}x', png)
            maps.append(png)
            map_labels.append(text)
    cols = 4
    montage(tiles, labels, cols, '+3+3',
            f"{view}: today | through-base | lerp | stripped ({'12c' if view == 'floor' else '12d'} labels)",
            os.path.join(OUT, f't5-{SHEET_NAMES[view]}-sheet.png'))
    mcols = 4 if view == 'floor' else 3
    legend = ('12c excess map: black 0, white +8 levels' if view == 'floor'
              else 'as built minus stripped per 8x8 block: mid-grey 0, black -16, white +16 levels')
    montage(maps, map_labels, mcols, '+3+3', f'{view}: {legend}',
            os.path.join(OUT, f't5-{SHEET_NAMES[view]}-map.png'))


SHEET_NAMES = {'roof': 'roof', 'west': 'sweeper-west', 'east': 'sweeper-east', 'floor': 'floor'}

if __name__ == '__main__':
    for view, width in (('roof', 300), ('west', 300), ('east', 300), ('floor', 300)):
        sheet(view, width)
        print('sheet', view, flush=True)
