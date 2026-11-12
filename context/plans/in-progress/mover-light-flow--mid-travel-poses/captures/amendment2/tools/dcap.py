# Ruling B option (ii) capture driver: acap.py plus sweeper west-face views.
#   python dcap.py batch <jobs> <view> <bakes> <forms> <t-spec> [mask]
# Bakes: head (today), dark (POSTRETRO_PROTO_ENCLOSED_DARK=1), truth (dense,
# today's rule), dtruth (dense, dark rule), h2 (today, rebuilt binary).
import os, sys
HERE = os.path.dirname(os.path.abspath(__file__))
# Scratch root (ws/, cap/, darkb/): A2_ROOT, else the scratch layout <A>/darkb/tools.
A = os.environ.get('A2_ROOT') or os.path.dirname(os.path.dirname(HERE))
os.environ.setdefault('A2_ROOT', A)
sys.path.insert(0, os.path.join(A, 'tools'))
sys.path.insert(0, HERE)  # acap.py / aseries.py sit beside these in the repo copy
import acap  # noqa: E402

# Sweeper slab: x 1280..1312 at t 0, +256 t; y 72..440; z 8..88.
acap.VIEWS['sweepwest'] = lambda t: dict(map='enemy-movers', mover='em_sweeper',
                                         pose=((1280.0 + 256.0 * t - 150.0, 256.0, 48.0), 0.0, 0.0, 100.0))
# motion-harness fixed views (face plus floor and surroundings), for dark blotches.
acap.VIEWS['west'] = lambda t: dict(map='enemy-movers', mover='em_sweeper',
                                    pose=((1100.0, 256.0, 140.0), 0.0, -20.0, 90.0))
acap.VIEWS['east'] = lambda t: dict(map='enemy-movers', mover='em_sweeper',
                                    pose=((1700.0, 256.0, 140.0), 180.0, -20.0, 90.0))

if __name__ == '__main__':
    os.makedirs(acap.CAP, exist_ok=True)
    if sys.argv[1] == 'batch':
        jobs, view = int(sys.argv[2]), sys.argv[3]
        bakes, forms, ts = sys.argv[4].split(','), sys.argv[5].split(','), acap.parse_ts(sys.argv[6])
        mask = sys.argv[7] if len(sys.argv) > 7 and sys.argv[7] != '-' else None
        acap.batch(jobs, view, bakes, forms, ts, mask)
