"""Real HTTP geography acceptance; uses only disposable sessions, no page automation."""
import argparse
import array
import base64
import collections
import gzip
import hashlib
import json
import math
from pathlib import Path
import struct
import sys
import threading
import time
import urllib.error
import urllib.request


def float_field(raw, offset=0, count=None):
    values = array.array('f')
    end = None if count is None else offset + 4*count
    values.frombytes(memoryview(raw)[offset:end])
    if sys.byteorder != 'little':
        values.byteswap()
    return values


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--base', default='http://127.0.0.1:18795/terraforge')
    parser.add_argument('--shape', choices=['sphere', 'plane'], default='sphere')
    parser.add_argument('--tectonic-myr', type=float)
    parser.add_argument('--vectors', action='store_true', help='Verify saved monthly wind/current fields and project round trip')
    parser.add_argument('--live', action='store_true', help='Verify changing solver frames in the same generation')
    parser.add_argument('--map-png', action='store_true', help='Keep the API map export for terrain inspection')
    parser.add_argument('--out', default='artifacts/geography-acceptance')
    args = parser.parse_args()
    base = args.base.rstrip('/') + '/api/v1'
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    tokens = []
    report = {'shape': args.shape, 'result': 'incomplete'}

    def call(path, token=None, method='GET', data=None, binary=False, expected=200):
        headers = {'Content-Type': 'application/json'}
        if token:
            headers['Authorization'] = 'Bearer ' + token
        body = data if isinstance(data, bytes) else json.dumps(data).encode() if data is not None else None
        for attempt in range(8):
            try:
                request = urllib.request.Request(base + path, data=body, method=method, headers=headers)
                with opener.open(request, timeout=180) as response:
                    status, content = response.status, response.read()
            except urllib.error.HTTPError as exc:
                status, content = exc.code, exc.read()
            if status != 429 or expected == 429 or attempt == 7:
                break
            time.sleep(.25 * (attempt + 1))
        assert status == expected, (path, status, content[:200])
        return content if binary else json.loads(content)

    def geography(map_id, token):
        begin = time.monotonic()
        raw = call('/maps/' + map_id + '/geography', token, binary=True)
        size = struct.unpack_from('<I', raw)[0]
        catalog = json.loads(raw[4:4+size])
        n = catalog['cell_count']
        assert len(raw) == 4 + size + 12*n
        labels = array.array('I')
        labels.frombytes(raw[4+size:])
        if sys.byteorder != 'little':
            labels.byteswap()
        return catalog, [labels[i*n:(i+1)*n] for i in range(3)], len(raw), time.monotonic()-begin

    try:
        tokens.extend(call('/session', method='POST')['token'] for _ in range(2))
        token, other = tokens
        params = {'world_shape': args.shape}
        if args.tectonic_myr is not None:
            params['tectonic_age_myr'] = args.tectonic_myr
        world = call('/worlds', token, 'POST', {
            'name': 'Geographic recognition acceptance', 'seed': 42,
            'params': params,
        })
        started = time.monotonic()
        job = call('/worlds/' + world['id'] + '/generate', token, 'POST')
        live_frames, live_errors = [], []
        def monitor():
            try:
                req=urllib.request.Request(base+'/events?job_id='+job['job_id']+'&frame_encoding=gzip',headers={'Authorization':'Bearer '+token})
                with opener.open(req,timeout=180) as stream:
                    kind,data='message',[]
                    for raw in stream:
                        line=raw.decode().strip()
                        if line.startswith('event:'): kind=line[6:].strip()
                        elif line.startswith('data:'): data.append(line[5:].strip())
                        elif not line and data:
                            value=json.loads('\n'.join(data)); data=[]
                            if kind=='frame':
                                pixels=base64.b64decode(value['data'],validate=True)
                                if value['fmt'].startswith('gzip-'): pixels=gzip.decompress(pixels)
                                assert len(pixels)==value['w']*value['h']*5
                                if not live_frames or value['tick']>live_frames[-1]['tick']:
                                    live_frames.append({'seconds':time.monotonic()-started,'tick':value['tick'],'meta':value['meta'],
                                        'height_hash':hashlib.sha256(pixels[0::5]+pixels[1::5]).hexdigest()})
                            elif kind in ('done','failed','cancelled'): break
            except Exception as exc: live_errors.append(repr(exc))
        stream_thread=threading.Thread(target=monitor,daemon=True) if args.live else None
        if stream_thread: stream_thread.start()
        for poll in range(900):
            state = call('/jobs/' + job['job_id'], token)
            if poll%30==0: print(json.dumps({'shape':args.shape,'seconds':round(time.monotonic()-started,1),'stage':state.get('stage'),'progress':state.get('progress')},ensure_ascii=False),flush=True)
            assert state['state'] not in ['failed', 'cancelled'], state
            if state['state'] == 'done':
                break
            time.sleep(1)
        else:
            raise TimeoutError('world did not complete')
        report['generation_secs'] = time.monotonic()-started
        if stream_thread:
            stream_thread.join(5)
            assert not stream_thread.is_alive() and not live_errors,(live_errors,'stream did not close')
            tectonic=[frame for frame in live_frames if frame['meta']['elapsed_myr'] is not None]
            assert len(live_frames)>=8 and len({f['height_hash'] for f in tectonic})>=3
            assert all(a['meta']['progress']<=b['meta']['progress']+1e-6 for a,b in zip(live_frames,live_frames[1:]))
            gaps=[b['seconds']-a['seconds'] for a,b in zip(live_frames,live_frames[1:])]
            report['live']={'frames':len(live_frames),'tectonic_frames':len(tectonic),'largest_gap_seconds':max(gaps)}
            live_out=Path(args.out); live_out.mkdir(parents=True,exist_ok=True)
            (live_out/(args.shape+'-frames.json')).write_text(json.dumps(live_frames,ensure_ascii=False),encoding='utf-8')
        map_id = job['map_id']
        node = call('/maps/' + map_id, token)
        catalog, labels, size, elapsed = geography(map_id, token)
        features = catalog['features']
        assert catalog['cell_count'] == node['cells']
        elevation_bytes = call('/maps/' + map_id + '/field/elevation', token, binary=True)
        height = float_field(elevation_bytes)
        assert len(height) == node['cells']
        land_heights = sorted(h for h in height if h > 0)
        if land_heights:
            report['land_elevation_m'] = {str(q): land_heights[round((len(land_heights)-1)*q)] for q in [.1,.5,.9,.99]}
            report['land_cell_share_below_1000m'] = sum(h < 1000 for h in land_heights)/len(land_heights)
            report['land_cell_share_above_3000m'] = sum(h > 3000 for h in land_heights)/len(land_heights)
        if args.tectonic_myr == 0:
            assert report['land_elevation_m']['0.5'] < 800, report
        del land_heights
        biome_bytes = call('/maps/' + map_id + '/field/biome', token, binary=True)
        biomes = float_field(biome_bytes)
        assert len(biomes) == node['cells']
        report['land_biome_counts'] = dict(collections.Counter(int(b) for b,h in zip(biomes,height) if h>0))
        if node['grid']['type'] == 'sphere':
            surface = call('/maps/' + map_id + '/surface', token, binary=True)
            version, cells, _ = struct.unpack_from('<III', surface)
            assert version == 1 and cells == node['cells']
            xyz = float_field(surface, 12, 3*cells)
            distance = float_field(call('/maps/' + map_id + '/field/distance_to_sea', token, binary=True))
            temperature = float_field(call('/maps/' + map_id + '/field/temperature', token, binary=True))
            assert len(xyz) == 3*cells and len(distance) == cells and len(temperature) == cells
            inland = [i for i,h in enumerate(height) if h>0 and distance[i]>300_000 and abs(math.degrees(math.asin(max(-1,min(1,xyz[3*i+1])))))<50]
            if inland:
                heights = sorted(height[i] for i in inland)
                report['nonpolar_inland'] = {'definition': 'latitude within 50 degrees; more than 300 km from coast',
                    'cells': len(inland), 'median_height_m': heights[len(heights)//2],
                    'mean_temperature_c': sum(temperature[i] for i in inland)/len(inland),
                    'biome_counts': dict(collections.Counter(int(biomes[i]) for i in inland))}
            del surface, xyz, distance, temperature, inland
        for i, h in enumerate(height):
            assert bool(labels[0][i]) == (h > 0)
            assert bool(labels[1][i]) == (h > 0)
            if h <= 0:
                assert labels[2][i] == 0
            else:
                region = features[labels[1][i]-1]
                assert region['level'] == 'region' and region['parent'] == labels[0][i]
        for i, feature in enumerate(features):
            assert feature['id'] == i+1 and 0 <= feature['anchor_cell'] < node['cells']
            assert height[feature['anchor_cell']] > 0
            if feature['level'] == 'continent':
                assert labels[0][feature['anchor_cell']] == feature['id']
            elif feature['level'] == 'region':
                assert labels[1][feature['anchor_cell']] == feature['id']
        call('/maps/' + map_id + '/geography', other, expected=404)
        cached, repeated, _, cached_secs = geography(map_id, token)
        assert cached == catalog and repeated == labels
        metadata = call('/maps/' + map_id + '/metadata', token)
        assert 'geography_v1' not in metadata
        vector_fields = {}
        if args.vectors:
            assert 'climate_atlas' not in metadata
            for kind, month in [('wind', 1), ('current', 7), ('wind', 0)]:
                suffix = f'/vectors/{kind}?month={month}'
                data = call('/maps/' + map_id + suffix, token)
                assert data['available'] and data['units'] == 'm/s', data
                assert 0 < len(data['samples']) <= 700
                assert len(data['groups']) == 4
                assert all(len(g) % 18 == 0 for g in data['groups'])
                assert all(math.isfinite(v) for g in data['groups'] for v in g)
                assert data['maximum_speed_m_s'] > 0
                call('/maps/' + map_id + suffix, other, expected=404)
                vector_fields[suffix] = data
            report['circulation'] = {suffix: {'peak_m_s': data['maximum_speed_m_s'],
                'samples': len(data['samples']), 'heat_coupled': data['heat_coupled'],
                'relative_solver_residual': data['relative_solver_residual']}
                for suffix, data in vector_fields.items()}
            history = metadata.get('tectonic_history', {})
            report['history'] = {key: history.get(key) for key in [
                'duration_myr', 'output_step_myr', 'ancient_duration_myr', 'detailed_duration_myr',
                'ancient_steps', 'steps', 'relative_budget_error']}
            saved_water=call('/maps/'+map_id+'/hydrology',token)
            assert saved_water['available'] and saved_water['source']=='saved_priority_flood_receivers'
            assert saved_water['segments']<=60000 and all(len(g)%6==0 for g in saved_water['groups'])
            call('/maps/'+map_id+'/hydrology',other,expected=404)
            report['river_segments']=saved_water['segments']
            receiver_bytes=call('/maps/'+map_id+'/field/flow_receiver',token,binary=True)
            life = history.get('lifecycle')
            if life and life.get('identities'):
                identities = life['identities']
                assert all(item['uid'] == i for i,item in enumerate(identities))
                assert len(set(life['slot_uids'])) == len(life['slot_uids'])
                assert all(0 <= uid < len(identities) for uid in life['slot_uids'])
                assert all(item['parent_uid'] is None or item['parent_uid'] < item['uid'] for item in identities)
                assert all(item['ended_myr'] is None or item['ended_myr'] >= item['born_myr'] for item in identities)
                assert all(item['ended_myr'] is None for item in identities if item['status'] in ('outside_window','below_resolution','unresolved_initial'))
                assert abs(life['area_history'][-1]['time_myr'] - history['duration_myr']) < 1e-6
                assert history['relative_budget_error'] < 1e-6
                diagnostic = call('/maps/'+map_id+'/tectonics',token)
                assert diagnostic['available'] and diagnostic['lifecycle'] == life
                assert all(a['plate_uid'] == life['slot_uids'][a['plate_id']] for a in diagnostic['arrows'])
                report['lifecycle'] = {'identities':len(identities),'status_counts':dict(collections.Counter(item['status'] for item in identities)),
                    'event_counts':dict(collections.Counter(event['kind'] for event in life['events'])),
                    'area_samples':len(life['area_history']),'subducted_surface_samples':life['subducted_surface_samples'],
                    'ridge_surface_samples':life['ridge_surface_samples'],'capacity_limited_events':life['capacity_limited_events']}
        assert len(call('/worlds/' + world['id'] + '/maps', token)) == 1
        project = call('/maps/' + map_id + '/export.json', token, binary=True)
        if args.map_png:
            image=call('/maps/'+map_id+'/export.png?size=2048',token,binary=True)
            assert image.startswith(b'\x89PNG\r\n\x1a\n')
            image_out=Path(args.out); image_out.mkdir(parents=True,exist_ok=True)
            (image_out/(args.shape+'.png')).write_bytes(image)
        if args.vectors:
            saved = Path(args.out)
            saved.mkdir(parents=True, exist_ok=True)
            (saved / (args.shape + '.terraforge.json')).write_bytes(project)
        imported = call('/projects/import', token, 'POST', project)
        imported_maps = call('/worlds/' + imported['id'] + '/maps', token)
        assert len(imported_maps) == 1
        for suffix, data in vector_fields.items():
            assert call('/maps/' + imported_maps[0]['id'] + suffix, token) == data
        if args.vectors and metadata.get('tectonic_history',{}).get('lifecycle',{}).get('identities'):
            restored = call('/maps/'+imported_maps[0]['id']+'/tectonics',token)
            assert restored['lifecycle'] == life
        if args.vectors:
            assert call('/maps/'+imported_maps[0]['id']+'/hydrology',token)==saved_water
            assert call('/maps/'+imported_maps[0]['id']+'/field/flow_receiver',token,binary=True)==receiver_bytes
        rebuilt, rebuilt_labels, _, rebuild_secs = geography(imported_maps[0]['id'], token)
        assert rebuilt_labels == labels
        assert [(f['id'], f['kind'], f['parent'], f['anchor_cell']) for f in rebuilt['features']] == [
            (f['id'], f['kind'], f['parent'], f['anchor_cell']) for f in features]
        report.update(result='passed', cells=node['cells'], feature_counts=dict(collections.Counter(f['kind'] for f in features)),
                      geography_bytes=size, first_read_secs=elapsed, cached_read_secs=cached_secs,
                      import_rebuild_secs=rebuild_secs, project_bytes=len(project),
                      elevation_sha256=hashlib.sha256(elevation_bytes).hexdigest(),
                      checks=['land/sea membership','parent hierarchy','feature anchors','cached repeat',
                              'session isolation','public metadata bounded','no extra sampling maps','project round trip'])
        print(json.dumps(report, ensure_ascii=False), flush=True)
    finally:
        for token in tokens:
            try:
                call('/session', token, 'DELETE')
            except Exception:
                pass
        out = Path(args.out)
        out.mkdir(parents=True, exist_ok=True)
        (out / (args.shape + '.json')).write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')


if __name__ == '__main__':
    main()
