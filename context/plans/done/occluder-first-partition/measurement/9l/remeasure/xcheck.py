"""Task 9l re-measure (9k's xcheck.py, re-rooted to target/occluder-9l/): cross-check the uncapped scratch walk against the harness sweep record.
For every pose the capped walk did not trip, uncapped portal tests must equal the
sweep's `considered`; every tripped pose must cost over 5,000 uncapped.
usage: python3 xcheck.py <side>...   (before, or a 9l config: cap26 cap19)
"""
import re, sys
G9G = "/Users/dhiester/Projects/Personal/postretro/target/occluder-9g"
G9L = "/Users/dhiester/Projects/Personal/postretro/target/occluder-9l"
for side in sys.argv[1:]:
    d = f"{G9G if side == 'before' else G9L}/{side}/stress-street"
    for unc, rec in (("uncapped-street.txt", f"stress-street.{'before' if side == 'before' else 'after'}.sweep"),
                     ("uncapped-rooftop.txt", "rooftop.sweep")):
        u = {}
        for l in open(f"{d}/{unc}"):
            if l.startswith("pose "):
                p = l.split(); u[p[1]] = int(p[2].split("=")[1])
        ok = bad = trip_ok = trip_bad = 0
        names = set()
        for l in open(f"{d}/{rec}"):
            if not l.startswith("pose "):
                continue
            name = l.split()[1]; names.add(name)
            c = int(re.search(r"considered=(\d+)", l).group(1))
            if " path=step_limit " in l:
                trip_ok += u[name] > 5000; trip_bad += u[name] <= 5000
            else:
                ok += c == u[name]; bad += c != u[name]
        print(f"{side} {unc}: {len(u)} uncapped poses, {len(names)} record poses; untripped equal {ok}, differ {bad}; "
              f"tripped and over 5,000 uncapped {trip_ok}, tripped but under {trip_bad}")
