"""Real HTTP geography acceptance; uses only disposable sessions, no page automation."""
import argparse
import array
import collections
import hashlib
import json
from pathlib import Path
import struct
import sys
import time
import urllib.error
import urllib.request


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--base', default='http://127.0.0.1:18795/terraforge')
    parser.add_argument('--shape', choices=['sphere', 'plane'], default='sphere')
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
        world = call('/worlds', token, 'POST', {
            'name': 'Geographic recognition acceptance', 'seed': 42,
            'params': {'world_shape': args.shape},
        })
        started = time.monotonic()
        job = call('/worlds/' + world['id'] + '/generate', token, 'POST')
        for _ in range(900):
            state = call('/jobs/' + job['job_id'], token)
            assert state['state'] not in ['failed', 'cancelled'], state
            if state['state'] == 'done':
                break
            time.sleep(1)
        else:
            raise TimeoutError('world did not complete')
        report['generation_secs'] = time.monotonic()-started
        map_id = job['map_id']
        node = call('/maps/' + map_id, token)
        catalog, labels, size, elapsed = geography(map_id, token)
        features = catalog['features']
        assert catalog['cell_count'] == node['cells']
        elevation_bytes = call('/maps/' + map_id + '/field/elevation', token, binary=True)
        height = struct.unpack('<' + 'f'*node['cells'], elevation_bytes)
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
        assert len(call('/worlds/' + world['id'] + '/maps', token)) == 1
        project = call('/maps/' + map_id + '/export.json', token, binary=True)
        imported = call('/projects/import', token, 'POST', project)
        imported_maps = call('/worlds/' + imported['id'] + '/maps', token)
        assert len(imported_maps) == 1
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
