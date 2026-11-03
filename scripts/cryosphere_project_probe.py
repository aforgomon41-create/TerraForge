"""Audit a real saved project and matching month 0/1/7 API packets. No network."""
import argparse
import base64
import hashlib
import json
from pathlib import Path
import struct

import numpy as np
import zstandard


def audit(project_path, artifacts):
    project_path, artifacts = Path(project_path), Path(artifacts)
    project_bytes = project_path.read_bytes()
    project = json.loads(project_bytes)
    world = next(m for m in project['maps'] if m['node']['parent'] is None)
    count = world['node']['cells']
    decoder = zstandard.ZstdDecompressor()

    def channel(name):
        item = world['channels'][name]
        assert item['count'] == count
        raw = decoder.decompress(base64.b64decode(item['data']), max_output_size=count*4)
        values = np.frombuffer(raw, dtype='<f4')
        assert values.size == count and np.isfinite(values).all(), name
        return values

    names = ['land_ice_m', 'snow_cover', 'sea_ice_cover', 'ice_shelf_m',
             'permafrost', 'active_layer_m', 'liquid_water_mm', 'lake_ice_cover']
    fields = {name: channel(name) for name in names}
    elevation, discharge = channel('elevation'), channel('liquid_discharge_m3_s')
    land = elevation > 0
    assert (discharge >= 0).all()
    assert (fields['land_ice_m'][~land] == 0).all()
    assert (fields['sea_ice_cover'][land] == 0).all()
    assert (fields['ice_shelf_m'][land] == 0).all()
    for name, values in fields.items():
        assert (values >= 0).all(), name
        if name in ['snow_cover', 'sea_ice_cover', 'permafrost', 'lake_ice_cover']:
            assert (values <= 1).all(), name

    packed = world['metadata']['cryosphere_atlas']
    atlas = json.loads(decoder.decompress(base64.b64decode(packed['data']), max_output_size=packed['bytes']))
    budget = atlas['ice_budget']
    assert abs(budget['residual_m3']) <= max(1, budget['snowfall_m3'])*1e-8
    monthly = {}
    for month in [0, 1, 7]:
        raw = (artifacts/f'ice-{month}.bin').read_bytes()
        size = struct.unpack_from('<I', raw)[0]
        header = json.loads(raw[4:4+size])
        assert header['available'] and header['count'] == count and header['month'] == month
        assert header['channels'] == names
        assert len(raw) == 4+size+count*32
        values = np.frombuffer(raw, dtype='<f4', offset=4+size).reshape(8, count)
        assert np.isfinite(values).all() and (values >= 0).all()
        if month == 0:
            for i, name in enumerate(names):
                assert np.array_equal(fields[name], values[i]), name
        hydro = json.loads((artifacts/(f'hydrology-{month}.json' if month else 'hydrology.json')).read_bytes())
        assert hydro['available'] and hydro['liquid_support']
        assert hydro['segments'] == sum(len(g)//6 for g in hydro['groups'])
        if month:
            assert hydro['monthly_liquid'], 'monthly rivers must use monthly liquid forcing'
        monthly[str(month)] = {
            'sea_ice_mean_cover': float(values[2, ~land].mean()),
            'snow_mean_land_cover': float(values[1, land].mean()),
            'lake_ice_cells_above_half': int((values[7] > .5).sum()),
            'liquid_water_mean_mm_year': float(values[6, land].mean()),
            'river_segments': hydro['segments'],
            'seasonal_segments': hydro['seasonal_segments'],
            'monthly_liquid': hydro['monthly_liquid'],
        }
    stats = {name: {'positive_cells': int((v > 0).sum()), 'maximum': float(v.max())}
             for name, v in fields.items()}
    return {'result': 'passed', 'cells': count, 'shape': world['node']['grid']['type'],
            'project_sha256': hashlib.sha256(project_bytes).hexdigest(),
            'project_bytes': len(project_bytes), 'model': atlas['model'],
            'ice_budget': budget, 'fields': stats, 'months': monthly,
            'checks': ['finite/bounded fields', 'land/sea ice masks', 'solid ice volume ledger',
                       'canonical packet matches saved channels', 'monthly liquid-supported rivers']}


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--project', required=True)
    parser.add_argument('--artifacts', required=True)
    parser.add_argument('--out', required=True)
    args = parser.parse_args()
    result = audit(args.project, args.artifacts)
    Path(args.out).write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf8')
    print(json.dumps(result, ensure_ascii=False))
