#!/usr/bin/env python3
"""Check the generated SDK ComponentKind export with the official luau-analyze.

Usage: python3 sdk/type-tests/luau/check_anonymous_particles.py /path/to/luau-analyze
The stock CLI cannot load declaration files. Import the exact exported alias in a
minimal module, without maintaining a second union or rewriting its members.
"""

from pathlib import Path
import re
import subprocess
import sys
import tempfile

if len(sys.argv) != 2:
    raise SystemExit(__doc__)

fixtures = Path(__file__).resolve().parent
sdk = fixtures.parents[1] / "types" / "postretro.d.luau"
declarations = [
    line for line in sdk.read_text().splitlines()
    if line.startswith("export type ComponentKind = ")
]
if len(declarations) != 1:
    raise SystemExit("expected exactly one generated SDK ComponentKind export")

with tempfile.TemporaryDirectory(prefix="postretro-luau-particle-types-") as temporary:
    root = Path(temporary)
    (root / "component-kind.luau").write_text(declarations[0] + "\nreturn {}\n")
    for case in ["positive", "negative"]:
        name = f"anonymous-particles-{case}.luau"
        (root / name).write_bytes((fixtures / name).read_bytes())
        result = subprocess.run(
            [sys.argv[1], "--mode=strict", "--formatter=gnu", str(root / name)],
            capture_output=True, text=True,
        )
        diagnostics = result.stdout + result.stderr
        if case == "positive":
            if result.returncode != 0 or diagnostics.strip():
                raise SystemExit(f"retained emitter kind must pass without diagnostics:\n{diagnostics}")
        else:
            blocks = re.split(
                r"(?m)^(?=[^\n]+:\d+\.\d+-\d+\.\d+: TypeError:)", diagnostics
            )
            errors = [block for block in blocks if "TypeError:" in block]
            if result.returncode != 1 or len(errors) != 2:
                raise SystemExit(f"both removed names must produce type errors:\n{diagnostics}")
            for error, name in zip(errors, ["particle_state", "particle"]):
                if not re.search(rf'got\s+\'"{name}"\'', error) or "Expected this to be" not in error:
                    raise SystemExit(f"unexpected removed-kind diagnostic:\n{diagnostics}")
        print(f"{case}: exit {result.returncode}")
        if diagnostics.strip():
            print(diagnostics.strip())
