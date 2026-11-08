#!/bin/bash
# Untraced short captures: check the [pdd] log lines and image identity.
# validate.sh [config ...]   default: base off t4 t6 t8
# Configs: base; off, t4, t6, t8 (bin-pdd); opt-off, opt-all, opt-sparse (bin-opt).
# A `-dark` suffix sets POSTRETRO_PDD_DARK (check mode: half the texels fail).
S=/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/8a65a298-34f6-4aca-a32c-6780069d4aa6/scratchpad/pdd
O=$S/validate
mkdir -p $O
cd /Users/dhiester/Projects/Personal/postretro
CONFIGS="${*:-base off t4 t6 t8}"
for cfg in $CONFIGS; do
  for view in a b c; do
    case $view in a) yaw=-135;; b) yaw=45;; c) yaw=-45;; esac
    cat > $O/$cfg-$view.json <<EOF
{"map": "/Users/dhiester/Projects/Personal/postretro/content/dev/maps/stress-warren-hallway-inspection.prl",
 "camera": {"position": [32.1056, 22.86, 53.2384], "yaw_deg": $yaw, "pitch_deg": -12, "fov_deg": 100},
 "resolution": [1920, 1080], "output": "$O/$cfg-$view.png",
 "measurement": {"report": "$O/$cfg-$view.report.json", "warmup_frames": 10, "sample_frames": 10}}
EOF
    base=${cfg%-dark}
    unset POSTRETRO_PDD_T POSTRETRO_PDD_MODE POSTRETRO_PDD_DARK POSTRETRO_PDD_PAIRED
    [ "$base" != "$cfg" ] && export POSTRETRO_PDD_DARK=1
    case $base in
      base) bin=$S/bin-base/postretro;;
      opt-*) bin=$S/bin-opt/postretro; export POSTRETRO_PDD_MODE=${base#opt-};;
      t*) bin=$S/bin-pdd/postretro; export POSTRETRO_PDD_T=${base:1};;
      *) bin=$S/bin-pdd/postretro;;
    esac
    POSTRETRO_SH_STREAMING=sync-proof RUST_LOG=warn $bin --capture $O/$cfg-$view.json > $O/$cfg-$view.log 2>&1
    echo "$cfg-$view rc=$? $(grep -h '\[pdd\]' $O/$cfg-$view.log | sort -u | tr '\n' ' ')"
  done
done
