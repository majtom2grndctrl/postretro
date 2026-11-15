# Print the probe lattice (id 34 header) of a PRL in engine and map units.
#   python m9lattice.py <prl>
import struct, sys
sys.path.insert(0, r'C:\Users\danhi\Projects\Personal\postretro\context\plans\in-progress\mover-light-flow--mid-travel-poses\captures\amendment2\tools')
import knots

def lattice(prl):
    s = knots.sections(prl)[34]
    ver = struct.unpack_from('<I', s, 0)[0]
    o = struct.unpack_from('<3f', s, 4); c = struct.unpack_from('<3f', s, 16); d = struct.unpack_from('<3I', s, 28)
    return ver, o, c, d

if __name__ == '__main__':
    ver, o, c, d = lattice(sys.argv[1])
    U = 0.0254
    print('version', ver, 'origin(engine m)', o, 'cell', c, 'dims', d)
    # engine (ex, ey, ez) -> map (x=-ez/U, y=-ex/U, z=ey/U)
    for axis, name, f in ((2, 'map x', lambda v: -v / U), (0, 'map y', lambda v: -v / U), (1, 'map z', lambda v: v / U)):
        vals = [f(o[axis] + i * c[axis]) for i in range(d[axis])]
        print(name, ' '.join(f'{v:.2f}' for v in sorted(vals)))
