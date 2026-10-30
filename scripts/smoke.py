"""Isolated API acceptance check. Uses disposable server-issued sessions only."""
import argparse,array,json,math,struct,threading,time,urllib.request,urllib.error
from pathlib import Path
parser=argparse.ArgumentParser();parser.add_argument('--base',default='http://127.0.0.1:18790/map');parser.add_argument('--out',default='artifacts/acceptance');args=parser.parse_args()
base=args.base.rstrip('/')+'/api/v1';out=Path(args.out);out.mkdir(parents=True,exist_ok=True)
report={'base':args.base,'maps':[],'frames':0};tokens=[]
def call(path,token=None,method='GET',data=None,binary=False,expected=200):
    payload=data if isinstance(data,bytes) else json.dumps(data).encode() if data is not None else None
    headers={'Content-Type':'application/json'}
    if token:headers['Authorization']='Bearer '+token
    request=urllib.request.Request(base+path,data=payload,headers=headers,method=method)
    opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        with opener.open(request,timeout=120) as r:status=r.status;body=r.read()
    except urllib.error.HTTPError as e:status=e.code;body=e.read()
    assert status==expected,(method,path,status,body[:200])
    if expected!=200:return None
    return body if binary else json.loads(body)
def field(map_id,ch,token):
    raw=call(f'/maps/{map_id}/field/{ch}',token,binary=True);data=array.array('f');data.frombytes(raw)
    import sys
    if sys.byteorder!='little':data.byteswap()
    assert all(math.isfinite(v) for v in data)
    return data

def follow(result,token):
    started=time.monotonic()
    while time.monotonic()-started<300:
        job=call('/jobs/'+result['job_id'],token)
        if job['state'] in ('done','failed','cancelled'):
            assert job['state']=='done',job
            return call('/maps/'+result['map_id'],token),round(time.monotonic()-started,3)
        time.sleep(.2)
    raise AssertionError('generation timeout')
def monitor(job_id,token):
    try:
        request=urllib.request.Request(base+'/events?job_id='+job_id,headers={'Authorization':'Bearer '+token})
        opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
        with opener.open(request,timeout=120) as response:
            for line in response:
                if line.startswith(b'event: frame'):report['frames']+=1
                if line.startswith(b'event: done') or line.startswith(b'event: failed'):return
    except Exception as e:report['stream_note']=str(e)
try:
    for _ in range(2):tokens.append(call('/session',method='POST')['token'])
    a,b=tokens
    world=call('/worlds',a,'POST',{'name':'Acceptance world','seed':42});wid=world['id']
    result=call(f'/worlds/{wid}/generate',a,'POST');thread=threading.Thread(target=monitor,args=(result['job_id'],a),daemon=True);thread.start()
    root,elapsed=follow(result,a);report['maps'].append({'kind':'world','cells':root['cells'],'seconds':elapsed})
    assert root['cells']==655362 and root['grid']['frequency']==256
    elevation=field(root['id'],'elevation',a);temperature=field(root['id'],'temperature',a);rain=field(root['id'],'precipitation',a)
    land=sorted(v for v in elevation if v>0);report['land_fraction']=len(land)/len(elevation);report['land_median_m']=land[len(land)//2]
    assert report['land_median_m']<3000
    centers=array.array('f');centers.frombytes(call(f"/maps/{root['id']}/grid",a,binary=True))
    index=next((i for i,h in enumerate(elevation) if 100<h<1500 and 10<temperature[i]<22 and 400<rain[i]<2200),next(i for i,h in enumerate(elevation) if h>0))
    x,y,z=centers[index*3:index*3+3];lat=math.degrees(math.asin(y/math.sqrt(x*x+y*y+z*z)));lon=math.degrees(math.atan2(z,x));report['selection']={'lat':lat,'lon':lon,'elevation_m':elevation[index]}
    monthly=call(f"/maps/{root['id']}/climate?cell={index}",a)
    assert monthly['available'] and len(monthly['temperature_c'])==12
    assert abs(sum(monthly['precipitation_mm'])-rain[index])<max(0.1,rain[index]*1e-5)
    assert abs(sum(monthly['temperature_c'])/12-temperature[index])<0.001
    tectonics=call(f"/maps/{root['id']}/tectonics",a)
    assert tectonics['available'] and all(len(g)%6==0 for g in tectonics['groups']) and tectonics['arrows']
    assert all(math.isfinite(v) for g in tectonics['groups'] for v in g)
    report['science']={'monthly_model':monthly['model'],'diagnostic_cells':tectonics['diagnostic_cells'],'arrows':len(tectonics['arrows'])}
    parent=root;first_child=None
    for kind,width,spacing in [('continent',128000,1000),('region',12800,100),('site',128,1)]:
        result=call(f"/maps/{parent['id']}/children",a,'POST',{'kind':kind,'center_lat':lat,'center_lon':lon,'width_m':width,'name':kind+' acceptance'})
        node,seconds=follow(result,a);height=field(node['id'],'elevation',a);assert len(height)==16384==node['cells'];assert node['cell_size_m']==spacing;assert node['parent']==parent['id'];assert node['grid']['width_m']==width
        monthly_child=call(f"/maps/{node['id']}/climate?cell=0",a)
        assert monthly_child['available'] and len(monthly_child['temperature_c'])==12
        diagnostic=call(f"/maps/{node['id']}/tectonics",a)
        assert diagnostic['available'] and diagnostic['diagnostic_cells']<=32768
        assert all(abs(v)<width*2 for g in diagnostic['groups'] for v in g)
        report['maps'].append({'kind':kind,'cells':node['cells'],'spacing_m':spacing,'seconds':seconds});parent=node
        if first_child is None:first_child=node;first_height=height
    # The same geometric selection derives the same seed and field, independent of object IDs.
    repeated=call(f"/maps/{root['id']}/children",a,'POST',{'kind':'continent','center_lat':lat,'center_lon':lon,'width_m':128000})
    repeated,_=follow(repeated,a);assert repeated['seed']==first_child['seed'];assert field(repeated['id'],'elevation',a)==first_height
    for path in [f'/worlds/{wid}',f"/maps/{root['id']}",f"/maps/{root['id']}/field/elevation",f"/maps/{root['id']}/climate?cell=0",f"/maps/{root['id']}/tectonics",'/jobs/'+result['job_id'],'/events?job_id='+result['job_id']]:call(path,b,expected=404)
    call('/system/concurrency',b,'PATCH',{'max_concurrent':8},expected=403)
    project=call(f"/maps/{parent['id']}/export.json",a,binary=True);(out/'roundtrip.tfworld').write_bytes(project);assert len(json.loads(project)['maps'])==5
    restored=call('/projects/import',b,'POST',project);restored_maps=call('/worlds/'+restored['id']+'/maps',b);assert len(restored_maps)==5;restored_site=next(m for m in restored_maps if m['kind']=='site');assert field(restored_site['id'],'elevation',b)==field(parent['id'],'elevation',a)
    assert call(f"/maps/{restored_site['id']}/climate?cell=0",b)['temperature_c']==call(f"/maps/{parent['id']}/climate?cell=0",a)['temperature_c']
    png=call(f"/maps/{root['id']}/export.png?size=1024",a,binary=True);assert png[:8]==b'\x89PNG\r\n\x1a\n';assert struct.unpack('>II',png[16:24])==(1024,512);(out/'atlas.png').write_bytes(png);report['project_bytes']=len(project);report['png_bytes']=len(png)
    # One running and one queued cancellation must never report completion.
    pending=[call(f'/worlds/{wid}/generate',a,'POST') for _ in range(2)]
    for job in reversed(pending):assert call('/jobs/'+job['job_id']+'/cancel',a,'POST')['ok']
    for job in pending:
        deadline=time.monotonic()+30
        while time.monotonic()<deadline:
            status=call('/jobs/'+job['job_id'],a)
            if status['state']=='cancelled':break
            assert status['state']!='done',status;time.sleep(.1)
        assert status['state']=='cancelled',status
        assert call('/maps/'+job['map_id'],a)['status']=='cancelled'
    thread.join(timeout=2);assert report['frames']>=1,report
    call('/worlds/'+wid,a,'DELETE');call('/maps/'+root['id'],a,expected=404)
    report['result']='passed';print(json.dumps(report,ensure_ascii=False,indent=2))
finally:
    for token in tokens:
        try:call('/session',token,'DELETE')
        except Exception:pass
    (out/'report.json').write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')
