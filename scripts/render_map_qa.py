"""Offscreen GPU check of the production ES 3.00 shaders and atlas mesh.

This is not browser or mobile UI automation. Optional local QA dependencies:
moderngl, numpy, Pillow, zstandard; also uses the web project's esbuild.
Only disposable API sessions are created; no credentials are saved.
"""
import argparse
import base64
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import time
import urllib.error
import urllib.request

import moderngl
import numpy as np
from PIL import Image
import zstandard


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--base', default='http://127.0.0.1:18798/terraforge')
    parser.add_argument('--project', required=True)
    parser.add_argument('--out', required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    out = Path(args.out).resolve(); out.mkdir(parents=True, exist_ok=True)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    token = None

    def call(path, method='GET', data=None, binary=False):
        headers = {'Content-Type': 'application/json'}
        if token: headers['Authorization'] = 'Bearer ' + token
        body = data if isinstance(data, bytes) else json.dumps(data).encode() if data is not None else None
        for attempt in range(10):
            req = urllib.request.Request(args.base.rstrip('/')+'/api/v1'+path, data=body, headers=headers, method=method)
            try:
                with opener.open(req, timeout=180) as response: raw = response.read()
                return raw if binary else json.loads(raw)
            except urllib.error.HTTPError as exc:
                if exc.code != 429 or attempt == 9: raise
                time.sleep(.3*(attempt+1))

    try:
        token = call('/session', 'POST')['token']
        project_bytes = Path(args.project).read_bytes()
        project = json.loads(project_bytes)
        imported = call('/projects/import', 'POST', project_bytes)
        maps = call('/worlds/'+imported['id']+'/maps')
        node = next(m for m in maps if m['parent'] is None)
        source_map = next(m for m in project['maps'] if m['node']['parent'] is None)
        surface = call('/maps/'+node['id']+'/surface', binary=True)
        (out/'surface.bin').write_bytes(surface)
        water = call('/maps/'+node['id']+'/hydrology')
        (out/'water.json').write_text(json.dumps(water), encoding='utf8')
        node_exe = shutil.which('node') or r'C:\Program Files\nodejs\node.exe'
        bundle="""import {createRequire} from 'node:module';
const [root,out]=process.argv.slice(1);const req=createRequire(root+'/apps/web/package.json');
const local=createRequire(req.resolve('vite/package.json'));const {build}=local('esbuild');
await build({entryPoints:[root+'/apps/web/src/renderer/surface.ts'],bundle:true,platform:'node',format:'esm',outfile:out+'/surface.mjs'});"""
        subprocess.run([node_exe,'--input-type=module','-e',bundle,str(root),str(out)],check=True)
        javascript = """
import fs from 'node:fs'; import {pathToFileURL} from 'node:url';
const [directory,spherical]=process.argv.slice(1);
const geometry=await import(pathToFileURL(directory+'/surface.mjs').href);
const bytes=fs.readFileSync(directory+'/surface.bin');
const view=new DataView(bytes.buffer,bytes.byteOffset,bytes.byteLength);
const n=view.getUint32(4,true),tri=view.getUint32(8,true);
const centers=new Float32Array(bytes.buffer.slice(bytes.byteOffset+12,bytes.byteOffset+12+12*n));
const indices=new Uint32Array(bytes.buffer.slice(bytes.byteOffset+12+12*n));
const atlas=spherical==='true'?geometry.atlasSurface(centers,indices):{positions:centers,indices,sourceIds:Uint32Array.from({length:n},(_,i)=>i)};
for(const name of ['positions','indices','sourceIds'])fs.writeFileSync(directory+'/'+name+'.bin',Buffer.from(atlas[name].buffer));
const water=JSON.parse(fs.readFileSync(directory+'/water.json','utf8'));
const lines=Float32Array.from(water.groups?.[0]||[]);
fs.writeFileSync(directory+'/rivers.bin',Buffer.from((spherical==='true'?geometry.atlasLines(lines):lines).buffer));
"""
        sphere = node['grid']['type']=='sphere'
        subprocess.run([node_exe,'--input-type=module','-e',javascript,str(out),str(sphere).lower()],check=True)
        positions = np.fromfile(out/'positions.bin',dtype='<f4').reshape(-1,3)
        source_ids = np.fromfile(out/'sourceIds.bin',dtype='<u4')
        indices = np.fromfile(out/'indices.bin',dtype='<u4')
        rivers = np.fromfile(out/'rivers.bin',dtype='<f4').reshape(-1,3)
        if not sphere:
            positions *= 2/node['grid']['width_m']; rivers *= 2/node['grid']['width_m']
        decoder = zstandard.ZstdDecompressor()
        def channel(name):
            field = source_map['channels'].get(name)
            if field is None: return np.zeros(len(source_ids),dtype='f4')
            data = decoder.decompress(base64.b64decode(field['data']),max_output_size=field['count']*4)
            return np.frombuffer(data,dtype='<f4')[source_ids].astype('f4')
        shader_source = (root/'apps/web/src/renderer/shaders.ts').read_text(encoding='utf8')
        shaders = {name:re.search(r'export const '+name+r' = `([\s\S]*?)`',shader_source).group(1)
                   for name in ['VERT','FRAG','LINE_VERT','LINE_FRAG']}
        ctx = moderngl.create_standalone_context(require=330)
        program = ctx.program(vertex_shader=shaders['VERT'],fragment_shader=shaders['FRAG'])
        line = ctx.program(vertex_shader=shaders['LINE_VERT'],fragment_shader=shaders['LINE_FRAG'])
        width,height=2048,(1024 if sphere else round(2048*node['grid']['rows']/node['grid']['size']*3**.5/2))
        frame = ctx.simple_framebuffer((width,height),components=3); frame.use()
        aspect=width/height
        matrix=np.diag([.5 if sphere else 1.,1. if sphere else aspect,1.,1.]).astype('f4')
        program['uViewProj'].write(matrix.tobytes())
        uniforms={'uRadius':1.,'uHeightScale':.00001,'uMapMetres':node['grid'].get('radius_m') if sphere else node['grid']['width_m']/2,
                  'uPlanarMode':True,'uWorldFlatMode':sphere,'uProjection':0,'uPreviewMode':False,'uStyle':2,'uGeographyActive':False,
                  'uLightDir':(-.45,.55,.7)}
        for name,value in uniforms.items():
            if name in program: program[name].value=value
        buffers=[]; content=[]
        attributes={'aCenter':positions,'aAtlasPosition':positions,'aHeight':channel('elevation'),'aBiome':channel('biome'),
                    'aPlate':channel('plate_id'),'aTemperature':channel('temperature'),'aPrecipitation':channel('precipitation'),
                    'aFlow':channel('flow_accumulation'),'aLake':channel('lake_depth'),
                    'aGeography':np.zeros(len(source_ids),dtype='f4'),'aPartition':np.zeros(len(source_ids),dtype='f4')}
        for name,values in attributes.items():
            if name not in program: continue
            buffer=ctx.buffer(values.astype('f4').tobytes());buffers.append(buffer)
            content.append((buffer,'3f' if values.ndim==2 else '1f',name))
        vao=ctx.vertex_array(program,content,index_buffer=ctx.buffer(indices.tobytes()))
        ctx.blend_func=(moderngl.SRC_ALPHA,moderngl.ONE_MINUS_SRC_ALPHA)
        river_vao=None
        if len(rivers):
            river_buffer=ctx.buffer(rivers.astype('f4').tobytes())
            river_vao=ctx.vertex_array(line,[(river_buffer,'3f','aPosition'),(river_buffer,'3f','aAtlasPosition')])
            line['uViewProj'].write(matrix.tobytes())
            for name,value in {'uSphere':False,'uWorldFlatMode':sphere,'uProjection':0,'uEye':(0.,0.,3.),'uRadius':1.,'uColor':(.19,.43,.49,.82)}.items():
                if name in line: line[name].value=value
        for layer,label in [(0,'terrain'),(4,'elevation'),(10,'game')]:
            frame.clear(.96,.95,.91)
            program['uLayer'].value=layer;vao.render(moderngl.TRIANGLES)
            if layer==10 and river_vao:
                ctx.enable(moderngl.BLEND);river_vao.render(moderngl.LINES);ctx.disable(moderngl.BLEND)
            image=Image.frombytes('RGB',(width,height),frame.read(components=3)).transpose(Image.Transpose.FLIP_TOP_BOTTOM)
            image.save(out/(label+'.png'))
        report={'result':'passed','exact_es300_sources':True,'browser_ui_checked':False,'renderer':ctx.info['GL_RENDERER'],
                'cells':node['cells'],'river_segments':len(rivers)//2,'shader_sha256':hashlib.sha256(shader_source.encode()).hexdigest(),
                'project_sha256':hashlib.sha256(project_bytes).hexdigest()}
        (out/'report.json').write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf8')
        print(json.dumps(report,ensure_ascii=False))
    finally:
        if token: call('/session','DELETE')


if __name__=='__main__':main()
