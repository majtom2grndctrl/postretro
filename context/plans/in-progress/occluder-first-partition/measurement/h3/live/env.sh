# Shared paths and helpers for the live H3 scripts. Sourced (POSIX sh), never run.
W=/Users/dhiester/Projects/Personal/postretro
L=$W/context/plans/in-progress/occluder-first-partition/measurement/h3/live
B=$W/target/occluder-h3live/bin          # frozen postretro (dev, dev-tools), scripts-build, walktime-capped
RUNS=$W/target/occluder-h3live/runs      # one directory per batch
SIDES="before cap26 cap19"
SENTINEL_POSE=-17.78,1.0,-13.8176,90,0   # occlusion-test fc:P1, before build

prl_of() { # side map
  case $1 in
    before) echo "$W/target/occluder-9g/before/$2/$2.prl" ;;
    cap26|cap19) echo "$W/target/occluder-9l/$1/$2/$2.prl" ;;
    *) echo "/nonexistent/unknown-side-$1" ;;
  esac
}

baked_of() { # side
  case $1 in
    before) echo "$W/target/occluder-baselines/baked" ;;
    *) echo "$W/target/occluder-9l/baked" ;;
  esac
}

safe() { # pose label -> file-safe tag part
  echo "$1" | tr ':/@' '___'
}

stamp() { date '+%F %T'; }

# true when the console session is locked (key present and true). An unlocked
# session usually has no CGSSessionScreenIsLocked key at all.
screen_locked() {
  v=$(ioreg -n Root -d1 -a 2>/dev/null | grep -A1 CGSSessionScreenIsLocked | tail -1 | tr -d '\t<>/ ')
  [ "$v" = true ]
}

amd_temp() { # current AMD GPU temperature, empty if unreadable
  "$L/amdstat.sh" | grep -oE 'temp=[0-9]+' | cut -d= -f2
}

free_gb() { df -g "$W" | tail -1 | awk '{print $4}'; }

clean_ktrace() { # xctrace leaves ~1 GB instruments*.ktrace per recording
  for d in "${TMPDIR:-/tmp}" "$(getconf DARWIN_USER_TEMP_DIR 2>/dev/null)"; do
    [ -n "$d" ] && rm -f "$d"/instruments*.ktrace
  done
}
