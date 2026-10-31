"""Decode PRL section 34 (OctahedralShVolume v11) into per-stored-tile mean irradiance.

Throwaway reader mirroring crates/level-format/src/{container,sh_volume,sh_reconstruct,octahedral}.rs
and the BC6H mode-11 decoder in crates/level-compiler/src/bc6h.rs.
"""
import struct, sys
import numpy as np

AF = 4


def read_section(path, want):
    with open(path, "rb") as f:
        hdr = f.read(8)
        assert hdr[:4] == b"PRL\0", hdr
        version, count = struct.unpack_from("<HH", hdr, 4)
        table = f.read(22 * count)
        for i in range(count):
            sid, off, size, ver = struct.unpack_from("<IQQH", table, i * 22)
            if sid == want:
                f.seek(off)
                return f.read(size)
    raise SystemExit(f"section {want} missing in {path}")


def parse34(data):
    o = 0
    (ver,) = struct.unpack_from("<I", data, 0)
    assert ver == 11, ver
    origin = struct.unpack_from("<3f", data, 4)
    cell = struct.unpack_from("<3f", data, 16)
    dims = struct.unpack_from("<3I", data, 28)
    (stride, nanim, tile_dim, border, aw, ah, tpr, layers, tpl, fmt, alen) = struct.unpack_from("<11I", data, 40)
    o = 84
    n = dims[0] * dims[1] * dims[2]
    rec = np.frombuffer(data, dtype=np.uint8, count=n * 8, offset=o).reshape(n, 8)
    o += n * 8
    atlas = np.frombuffer(data, dtype=np.uint8, count=alen, offset=o)
    return dict(origin=np.array(origin), cell=np.array(cell), dims=dims, tile_dim=tile_dim, border=border,
                aw=aw, ah=ah, tpr=tpr, layers=layers, tpl=tpl, fmt=fmt, validity=rec[:, 0].copy(),
                level=rec[:, 5].copy(), scale=rec[:, 6].copy(), atlas=atlas)


def corner_locals():
    out = []
    for cz in (0, AF - 1):
        for cy in (0, AF - 1):
            for cx in (0, AF - 1):
                out.append((cx, cy, cz))
    return out


def stored_tiles(v):
    """List of (kind, probe_coord_float[3]) per stored slot, in payload order.
    kind: 'L0', 'L1', 'L2'. coord is the probe-grid coordinate of the stored probe (L0, L1) or of
    the node origin (L2 brick/node mean)."""
    dx, dy, dz = v["dims"]
    valid = v["validity"].reshape(dz, dy, dx) != 0
    level = v["level"].reshape(dz, dy, dx)
    scale = v["scale"].reshape(dz, dy, dx)
    bx_n, by_n, bz_n = (dx + 3) // 4, (dy + 3) // 4, (dz + 3) // 4
    out = []
    for bz in range(bz_n):
        for by in range(by_n):
            for bx in range(bx_n):
                x0, y0, z0 = bx * 4, by * 4, bz * 4
                sub = valid[z0:z0 + 4, y0:y0 + 4, x0:x0 + 4]
                if not sub.any():
                    continue
                lv = int(level[z0, y0, x0]); sc = int(scale[z0, y0, x0])
                if sc == 0:
                    if lv == 0:
                        zz, yy, xx = np.nonzero(sub)
                        # x-fastest local order == sort by (z, y, x)
                        for z, y, x in zip(zz, yy, xx):
                            out.append(("L0", (x0 + x, y0 + y, z0 + z)))
                    elif lv == 1:
                        for (cx, cy, cz) in corner_locals():
                            out.append(("L1", (x0 + cx, y0 + cy, z0 + cz)))
                    else:
                        out.append(("L2", (x0, y0, z0)))
                else:
                    edge = 1 << sc
                    if bx % edge or by % edge or bz % edge:
                        continue
                    span = AF * edge - 1
                    if lv == 1:
                        for c in range(8):
                            out.append(("L1", (x0 + (span if c & 1 else 0), y0 + (span if c & 2 else 0),
                                               z0 + (span if c & 4 else 0))))
                    else:
                        out.append(("L2", (x0, y0, z0)))
    return out


W4 = np.array([0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64], dtype=np.int64)


def bits(lo, hi, start, count):
    """Extract `count` bits at `start` from 128-bit (lo, hi) uint64 arrays."""
    if start + count <= 64:
        return (lo >> np.uint64(start)) & np.uint64((1 << count) - 1)
    if start >= 64:
        return (hi >> np.uint64(start - 64)) & np.uint64((1 << count) - 1)
    lo_n = 64 - start
    low = lo >> np.uint64(start)
    high = hi & np.uint64((1 << (count - lo_n)) - 1)
    return low | (high << np.uint64(lo_n))


def unq(x):
    x = x.astype(np.int64)
    return np.where(x == 0, 0, np.where(x == 0x3FF, 0xFFFF, 64 * x + 32))


def f16_to_f32(h):
    return h.astype(np.uint16).view(np.float16).astype(np.float32)


def decode_bc6h_blocks(blocks):
    """blocks: (N,16) uint8 -> (N,16,3) float32 (texel k at (k%4, k//4))."""
    b = np.ascontiguousarray(blocks).view("<u8").reshape(-1, 2)
    lo, hi = b[:, 0], b[:, 1]
    mode = bits(lo, hi, 0, 5)
    assert np.all(mode == 0b00011), "non mode-11 block"
    ep0 = [unq(bits(lo, hi, 5 + 10 * c, 10)) for c in range(3)]
    ep1 = [unq(bits(lo, hi, 35 + 10 * c, 10)) for c in range(3)]
    idx = [bits(lo, hi, 65, 3)]
    for k in range(1, 16):
        idx.append(bits(lo, hi, 68 + 4 * (k - 1), 4))
    idx = np.stack(idx, axis=1).astype(np.int64)  # (N,16)
    w = W4[idx]
    out = np.empty((blocks.shape[0], 16, 3), dtype=np.float32)
    for c in range(3):
        interp = ((64 - w) * ep0[c][:, None] + w * ep1[c][:, None] + 32) >> 6
        out[:, :, c] = f16_to_f32((interp * 31) >> 6)
    return out


def decode_layer(v, layer):
    aw, ah = v["aw"], v["ah"]
    if v["fmt"] == 1:
        bw, bh = (aw + 3) // 4, (ah + 3) // 4
        per = bw * bh * 16
        raw = v["atlas"][layer * per:(layer + 1) * per]
        dec = decode_bc6h_blocks(raw.reshape(-1, 16)).reshape(bh, bw, 4, 4, 3)
        return dec.transpose(0, 2, 1, 3, 4).reshape(bh * 4, bw * 4, 3)[:ah, :aw]
    per = aw * ah * 8
    raw = v["atlas"][layer * per:(layer + 1) * per]
    return f16_to_f32(np.frombuffer(raw.tobytes(), dtype="<u2").reshape(ah, aw, 4)[:, :, :3].copy())


def tile_means(v, n_tiles):
    """Mean RGB over each stored tile's interior texels -> (n_tiles, 3)."""
    td, bd, tpl, tpr = v["tile_dim"], v["border"], v["tpl"], v["tpr"]
    out = np.zeros((n_tiles, 3), dtype=np.float64)
    for layer in range(v["layers"]):
        img = decode_layer(v, layer)
        for t in range(layer * tpl, min(n_tiles, (layer + 1) * tpl)):
            local = t - layer * tpl
            ty, tx = divmod(local, tpr)
            out[t] = img[ty * td + bd:(ty + 1) * td - bd, tx * td + bd:(tx + 1) * td - bd].reshape(-1, 3).mean(axis=0)
    return out


def per_probe(v, st, means):
    """Reconstructed mean RGB per valid probe (NaN for invalid), shape (dz,dy,dx,3)."""
    dx, dy, dz = v["dims"]
    valid = v["validity"].reshape(dz, dy, dx) != 0
    level = v["level"].reshape(dz, dy, dx)
    scale = v["scale"].reshape(dz, dy, dx)
    out = np.full((dz, dy, dx, 3), np.nan)
    slot = {(kind, tuple(int(a) for a in c)): i for i, (kind, c) in enumerate(st)}
    zz, yy, xx = np.nonzero(valid)
    for z, y, x in zip(zz, yy, xx):
        lv = int(level[z, y, x]); sc = int(scale[z, y, x])
        if lv == 0:
            out[z, y, x] = means[slot[("L0", (x, y, z))]]
            continue
        edge = AF << sc
        ox, oy, oz = x // edge * edge, y // edge * edge, z // edge * edge
        if lv == 2:
            out[z, y, x] = means[slot[("L2", (ox, oy, oz))]]
            continue
        # L1: scale-aware trilinear over the node's valid corners, renormalized
        span = edge - 1
        acc = np.zeros(3); wsum = 0.0
        fx, fy, fz = (x - ox) / span, (y - oy) / span, (z - oz) / span
        for c in range(8):
            cx = ox + (span if c & 1 else 0); cy = oy + (span if c & 2 else 0); cz = oz + (span if c & 4 else 0)
            w = (fx if c & 1 else 1 - fx) * (fy if c & 2 else 1 - fy) * (fz if c & 4 else 1 - fz)
            if valid[cz, cy, cx] and w > 0:
                acc += w * means[slot[("L1", (cx, cy, cz))]]; wsum += w
        out[z, y, x] = acc / wsum if wsum > 0 else np.nan
    return out


def load(path):
    v = parse34(read_section(path, 34))
    st = stored_tiles(v)
    means = tile_means(v, len(st))
    return v, st, means, per_probe(v, st, means)


if __name__ == "__main__":
    v = parse34(read_section(sys.argv[1], 34))
    print({k: (val if not isinstance(val, np.ndarray) else val.shape) for k, val in v.items()})
    st = stored_tiles(v)
    print("stored tiles", len(st), "levels", {k: sum(1 for s in st if s[0] == k) for k in ("L0", "L1", "L2")})
