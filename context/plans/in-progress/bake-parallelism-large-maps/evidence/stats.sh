#!/bin/bash
F="${1:-$(dirname "$0")/cpu-samples.tsv}"
awk -F'\t' 'NR>1 && $2!="EXITED" && $5!="" {st=$5; sub(/^[0-9.]+s +/,"",st); sub(/\.\.\.$/,"",st); print st"\t"$2"\t"$4"\t"$3}' $F | sort -t$'\t' -k1,1 -k2,2n | awk -F'\t' '
{ st=$1; v[st,++n[st]]=$2; s[st]+=$2; th[st]=$3; if($4>rss[st])rss[st]=$4 }
END { printf "%-28s %5s %7s %6s %6s %6s %5s %8s\n","stage","n","mean%","p10","p50","p90","thr","maxRSSMB";
 for (st in n) { c=n[st]; p10=v[st,int(c*0.1)+1]; p50=v[st,int(c*0.5)+1]; p90=v[st,(int(c*0.9)+1>c?c:int(c*0.9)+1)];
 printf "%-28s %5d %7.1f %6.1f %6.1f %6.1f %5s %8.0f\n", st, c, s[st]/c, p10, p50, p90, th[st], rss[st]/1024 } }'
