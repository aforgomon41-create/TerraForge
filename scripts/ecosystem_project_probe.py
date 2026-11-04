"""Check real ecosystem channels, hierarchy, support and API/save agreement."""
import argparse
import base64
import hashlib
import json
from pathlib import Path
import struct
import numpy as np
import zstandard

def audit(project_path, packet_path, source_path=None):
    raw=Path(project_path).read_bytes();project=json.loads(raw)
    root=next(m for m in project['maps'] if m['node']['parent'] is None)
    catalog=json.loads((Path(__file__).resolve().parent.parent/'data/ecosystems/catalogue.json').read_text(encoding='utf8'))
    biomes={x['id']:x for x in catalog['biomes']};groups={x['id']:x for x in catalog['groups']}
    names=['ecosystem_biome','ecosystem_group','ecosystem_habitat_biome','ecosystem_habitat_group','ecosystem_variant','ecosystem_evidence']
    n=root['node']['cells'];decoder=zstandard.ZstdDecompressor();fields=[]
    for name in names:
        v=root['channels'][name];assert v['count']==n
        data=np.frombuffer(decoder.decompress(base64.b64decode(v['data']),max_output_size=n*4),dtype='<f4')
        assert data.size==n and np.isfinite(data).all() and (data>=0).all() and (data==np.floor(data)).all()
        fields.append(data.astype('u2'))
    for parent,child in [(fields[0],fields[1]),(fields[2],fields[3])]:
        assert set(np.unique(parent))<=set(biomes)|{0}
        for g in np.unique(child):
            if not g:continue
            assert g in groups and (parent[child==g]==groups[g]['biome_id']).all()
            assert groups[g]['support']=='environment_estimate',groups[g]['code']
    assert (fields[4]<=63).all() and ((fields[5]&~np.uint16(831))==0).all()
    assert root['metadata']['ecosystem_model']['version']==catalog['version']
    if packet_path:
        packet=Path(packet_path).read_bytes();size=struct.unpack_from('<I',packet)[0];header=json.loads(packet[4:4+size])
        assert header['count']==n and header['channels']==names and header['version']==catalog['version']
        assert len(packet)==4+size+24*n
        saved=np.frombuffer(packet,dtype='<f4',offset=4+size).reshape(6,n)
        assert np.array_equal(saved,np.array(fields,dtype='<f4'))
    result={'result':'passed','cells':n,'shape':root['node']['grid']['type'],'catalogue':[len(biomes),len(groups)],
        'project_bytes':len(raw),'sha256':hashlib.sha256(raw).hexdigest(),
        'resolved_ground_cells':int((fields[1]>0).sum()),'unresolved_ground_cells':int((fields[1]==0).sum()),
        'ground_types':{groups[int(g)]['code']:int((fields[1]==g).sum()) for g in np.unique(fields[1]) if g},
        'habitat_types':{groups[int(g)]['code']:int((fields[3]==g).sum()) for g in np.unique(fields[3]) if g},
        'biomes':{biomes[int(b)]['code']:int((fields[0]==b).sum()) for b in np.unique(fields[0]) if b},
        'distinct_local_variants':len(np.unique(fields[4])),'u16_ids_preserved':bool((fields[1]>255).any())}
    if source_path:
        source=json.loads(Path(source_path).read_bytes());before=next(m for m in source['maps'] if m['node']['parent'] is None)
        physical=[name for name in before['channels'] if name not in names]
        assert all(before['channels'][name]==root['channels'][name] for name in physical),'ecology replay changed physical channels'
        assert before['metadata']['climate_atlas']==root['metadata']['climate_atlas']
        assert before['metadata']['cryosphere_atlas']==root['metadata']['cryosphere_atlas']
        result['unchanged_physical_channels']=physical
    return result

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--project',required=True);p.add_argument('--packet');p.add_argument('--source-project');p.add_argument('--out',required=True);args=p.parse_args()
    report=audit(args.project,args.packet,args.source_project);Path(args.out).write_text(json.dumps(report,indent=2),encoding='utf8');print(json.dumps(report))
