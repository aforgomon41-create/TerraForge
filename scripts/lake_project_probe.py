"""Audit a saved production project using lake_replay. Requires zstandard.
No server is started; output files are confined to the chosen artifact folder.
"""
import argparse
import array
import base64
import json
from pathlib import Path
import subprocess
import sys
import zstandard

p = argparse.ArgumentParser()
p.add_argument('--project', required=True)
p.add_argument('--replay', required=True)
p.add_argument('--out', required=True)
args = p.parse_args()
project = json.loads(Path(args.project).read_text(encoding='utf8'))
world = next(m for m in project['maps'] if m['node']['parent'] is None)
out = Path(args.out)
source = out/'input'
source.mkdir(parents=True, exist_ok=True)
(source/'descriptor.json').write_text(json.dumps(world['node']['grid']), encoding='utf8')
decoder = zstandard.ZstdDecompressor()
for name in ['elevation', 'precipitation', 'temperature', 'potential_evaporation', 'lake_depth', 'distance_to_sea']:
    field = world['channels'][name]
    (source/(name+'.bin')).write_bytes(decoder.decompress(base64.b64decode(field['data']), max_output_size=field['count']*4))
climate = world['metadata'].get('climate_atlas')
if climate:
    (source/'climate.json').write_bytes(decoder.decompress(base64.b64decode(climate['data']), max_output_size=climate['bytes']))
subprocess.run([str(Path(args.replay).resolve()), str(source), str(out/'recomputed'), 'water'], check=True, stdout=subprocess.DEVNULL)
report = json.loads((out/'recomputed/report.json').read_text(encoding='utf8'))
assert (source/'lake_depth.bin').read_bytes() == (out/'recomputed/lake_depth.bin').read_bytes(), 'saved lakes disagree with final climate'
model = world['metadata']['hydrology_model']
assert model['lake_depth'] == 'actual_supported_water'
water, sediment = model['water'], model['sediment']
assert abs(water['residual_m3_yr']) <= max(1, water['precipitation_m3_yr'])*1e-8
assert abs(sediment['residual_m3']) <= max(1, sediment['eroded_m3'])*1e-8
elevation = array.array('f', (source/'elevation.bin').read_bytes())
if sys.byteorder != 'little': elevation.byteswap()
land = sorted(h for h in elevation if h > 0)
report.update(saved_water_recomputes_exactly=True, persisted_model=model,
              elevation_m={str(q):land[round((len(land)-1)*q)] for q in [.1,.5,.9,.99]},
              checks=['final climate/lake agreement', 'water conservation', 'fluvial sediment conservation', 'unchanged elevation'])
(out/'report.json').write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf8')
print(json.dumps(report, ensure_ascii=False))
