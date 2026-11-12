# Task 15b: tcap.py columns for each outside-distance variant (HEAD engine,
# variant PRLs), ceiling and east views, std and truth grids.
#   venv/bin/python -I vcap.py <jobs> <variant> [...]
import os, sys
SP = '/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/991d9ce0-b6e7-4e9e-8a81-c2d9f7c07b7d/scratchpad'
sys.path.insert(0, SP + '/t15/tools')
import tcap  # noqa: E402
T = SP + '/t15b'
for v in ('o44', 'o20', 'o10', 'o005'):
    tcap.COLS[v] = (SP + '/t15/bin/postretro', T + '/ws/content/dev/maps/' + v + '-{map}.prl')
if __name__ == '__main__':
    os.makedirs(tcap.CAP, exist_ok=True)
    jobs = int(sys.argv[1])
    for v in sys.argv[2:]:
        for grid in ('std', 'truth'):
            tcap.run_batch(jobs, v, grid, ['ceiling', 'east'])
