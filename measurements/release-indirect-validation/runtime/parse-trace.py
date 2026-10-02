"""Count every labelled world/shadow pass; never filter out cache-refresh frames."""
import sys, json, hashlib
import xml.etree.ElementTree as E
from pathlib import Path
from collections import defaultdict
src=Path(sys.argv[1]); dest=Path(sys.argv[2])
r=E.parse(src).getroot(); ids={e.attrib['id']:e for e in r.iter() if 'id' in e.attrib}
def resolve(e):return ids[e.attrib['ref']] if 'ref' in e.attrib else e
labels={'Depth Pre-Pass','Textured Pass','Promoted Spot World Depth Cache Pass','Dynamic Spot World Depth Cache Pass','Spot Shadow Depth Pass','Promoted Cube World Depth Cache Pass','Dynamic Cube World Depth Cache Pass','Cube Shadow Depth Pass','Promoted Spot Entity Shadow Depth Pass','Dynamic Spot Entity Shadow Depth Pass','Promoted Cube Entity Shadow Depth Pass','Dynamic Cube Entity Shadow Depth Pass'}
encoders=defaultdict(set)
for row in r.iter('row'):
 c=list(row)
 if 'postretro (' not in resolve(c[10]).attrib.get('fmt',''):continue
 label=resolve(c[6]).attrib.get('fmt','').split(':')[0]
 if label in labels:encoders[label].add((resolve(c[15]).text,resolve(c[16]).text))
counts={k:len(encoders[k]) for k in sorted(labels)}
camera_frames=min(counts['Depth Pre-Pass'],counts['Textured Pass'])
refreshes=sum(n for k,n in counts.items() if 'World Depth Cache Pass' in k)
result={'source':src.name,'sha256':hashlib.sha256(src.read_bytes()).hexdigest(),'unique_encoders_by_label':counts,'camera_frame_equivalents':camera_frames,'uncached_spot_world_passes':counts['Spot Shadow Depth Pass'],'uncached_cube_face_world_passes':counts['Cube Shadow Depth Pass'],'cached_world_refresh_passes':refreshes,'cached_refreshes_per_camera_frame_estimate':refreshes/camera_frames if camera_frames else None,'deduplication':'Metal command-buffer ID + encoder ID, process=postretro; every observed labelled pass retained. Frame normalization uses the smaller camera-pass count; trace boundaries can include partial frames. No temporal grouping or cache-refresh-frame exclusion.'}
dest.write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result,indent=2))
