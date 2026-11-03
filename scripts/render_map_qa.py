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
import sys
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
    parser.add_argument('--view', default='equirectangular', choices=['globe','equirectangular','mercator','mollweide','equal-earth'])
    parser.add_argument('--width', type=int, default=2048)
    parser.add_argument('--zoom', type=float, default=1.)
    parser.add_argument('--center', nargs=2, type=float, default=[0.,0.])
    parser.add_argument('--material-fallback', action='store_true')
    parser.add_argument('--ice-month',type=int,choices=range(13),default=0)
    parser.add_argument('--offline-artifacts', help='Folder containing surface.bin and hydrology.json from offline_world_qa; no network access')
    args = parser.parse_args()
    if not 64 <= args.width <= 8192 or not 0.5 <= args.zoom <= 30:
        parser.error('width must be 64..8192 and zoom must be 0.5..30')
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
        project_bytes = Path(args.project).read_bytes()
        project = json.loads(project_bytes)
        source_map = next(m for m in project['maps'] if m['node']['parent'] is None)
        if args.offline_artifacts:
            node = source_map['node']
        else:
            token = call('/session', 'POST')['token']
            imported = call('/projects/import', 'POST', project_bytes)
            maps = call('/worlds/'+imported['id']+'/maps')
            node = next(m for m in maps if m['parent'] is None)
        decoder = zstandard.ZstdDecompressor()
        field = source_map['channels']['elevation']
        elevation_bytes = decoder.decompress(base64.b64decode(field['data']),max_output_size=field['count']*4)
        (out/'elevation.bin').write_bytes(elevation_bytes)
        lake_field = source_map['channels'].get('lake_depth')
        lake_bytes = decoder.decompress(base64.b64decode(lake_field['data']),max_output_size=lake_field['count']*4) if lake_field else bytes(field['count']*4)
        (out/'lake.bin').write_bytes(lake_bytes)
        surface = (Path(args.offline_artifacts)/'surface.bin').read_bytes() if args.offline_artifacts else call('/maps/'+node['id']+'/surface', binary=True)
        (out/'surface.bin').write_bytes(surface)
        water_file=f'hydrology-{args.ice_month}.json' if args.ice_month else 'hydrology.json'
        water = json.loads((Path(args.offline_artifacts)/water_file).read_text(encoding='utf8')) if args.offline_artifacts else call('/maps/'+node['id']+f'/hydrology?month={args.ice_month}')
        (out/'water.json').write_text(json.dumps(water), encoding='utf8')
        has_ice='land_ice_m' in source_map['channels'] and 'cryosphere_atlas' in source_map.get('metadata',{})
        ice_bytes=((Path(args.offline_artifacts)/f'ice-{args.ice_month}.bin').read_bytes() if args.offline_artifacts else call('/maps/'+node['id']+f'/cryosphere?month={args.ice_month}',binary=True)) if has_ice else b''
        (out/'ice.bin').write_bytes(ice_bytes)
        node_exe = shutil.which('node') or r'C:\Program Files\nodejs\node.exe'
        bundle="""import {createRequire} from 'node:module';
const [root,out]=process.argv.slice(1);const req=createRequire(root+'/apps/web/package.json');
const local=createRequire(req.resolve('vite/package.json'));const {build}=local('esbuild');
await build({stdin:{contents:"export * from './surface.ts'; export * from './math.ts'; export * from './projections.ts'; export * from './relief.ts'; export * from './lakeShore.ts'; export * from '../cryosphere/data.ts';",resolveDir:root+'/apps/web/src/renderer'},bundle:true,platform:'node',format:'esm',outfile:out+'/surface.mjs'});"""
        subprocess.run([node_exe,'--input-type=module','-e',bundle,str(root),str(out)],check=True)
        javascript = """
import fs from 'node:fs'; import {pathToFileURL} from 'node:url';
const [directory,spherical,view,normalization]=process.argv.slice(1);
const geometry=await import(pathToFileURL(directory+'/surface.mjs').href);
const bytes=fs.readFileSync(directory+'/surface.bin');
const surfaceView=new DataView(bytes.buffer,bytes.byteOffset,bytes.byteLength);
const n=surfaceView.getUint32(4,true);
const centers=new Float32Array(bytes.buffer.slice(bytes.byteOffset+12,bytes.byteOffset+12+12*n));
const indices=new Uint32Array(bytes.buffer.slice(bytes.byteOffset+12+12*n));
const flat=spherical==='true'&&view!=='globe';
const atlas=flat?geometry.atlasSurface(centers,indices):{positions:centers,indices,sourceIds:Uint32Array.from({length:n},(_,i)=>i)};
const h=fs.readFileSync(directory+'/elevation.bin');
const heights=new Float32Array(h.buffer.slice(h.byteOffset,h.byteOffset+h.byteLength));
const gradient=geometry.heightGradients(Float32Array.from(centers,v=>v*Number(normalization)),indices,heights);
const lakeBytes=fs.readFileSync(directory+'/lake.bin');
const lakeDepth=new Float32Array(lakeBytes.buffer.slice(lakeBytes.byteOffset,lakeBytes.byteOffset+lakeBytes.byteLength));
const shore=geometry.lakeShoreDepths(heights,lakeDepth,indices);
const mappedShore=Float32Array.from(atlas.sourceIds,id=>shore[id]);
fs.writeFileSync(directory+'/shore.bin',Buffer.from(mappedShore.buffer));
const iceBytes=fs.readFileSync(directory+'/ice.bin');
const ice=iceBytes.length?geometry.decodeIce(iceBytes.buffer.slice(iceBytes.byteOffset,iceBytes.byteOffset+iceBytes.byteLength),n):undefined;
const iceAttributes=geometry.iceAttributes(ice,n,atlas.sourceIds);
fs.writeFileSync(directory+'/cryocover.bin',Buffer.from(iceAttributes.cover.buffer));
fs.writeFileSync(directory+'/cryoground.bin',Buffer.from(iceAttributes.ground.buffer));
const mappedGradient=Float32Array.from({length:atlas.sourceIds.length*3},(_,i)=>gradient[atlas.sourceIds[Math.floor(i/3)]*3+i%3]);
fs.writeFileSync(directory+'/gradients.bin',Buffer.from(mappedGradient.buffer));
fs.writeFileSync(directory+'/geoPositions.bin',Buffer.from(atlas.positions.buffer));
if(flat)atlas.positions=geometry.projectAtlasPositions(atlas.positions,view);
for(const name of ['positions','indices','sourceIds'])fs.writeFileSync(directory+'/'+name+'.bin',Buffer.from(atlas[name].buffer));
const water=JSON.parse(fs.readFileSync(directory+'/water.json','utf8'));
let first=0;const ranges=[];
const groups=(water.groups||[]).map((group,index)=>{const lines=Float32Array.from(group);const atlas=flat?geometry.atlasLines(lines):lines;ranges.push({index,first,count:atlas.length/3});first+=atlas.length/3;return atlas;});
const riverGeo=new Float32Array(first*3);let offset=0;for(const group of groups){riverGeo.set(group,offset);offset+=group.length;}
fs.writeFileSync(directory+'/riverGroups.json',JSON.stringify(ranges));
fs.writeFileSync(directory+'/riverGeo.bin',Buffer.from(riverGeo.buffer));
fs.writeFileSync(directory+'/rivers.bin',Buffer.from((flat?geometry.projectAtlasPositions(riverGeo,view):riverGeo).buffer));
const eye=[0,Math.sin(.35)*3.2,Math.cos(.35)*3.2];
fs.writeFileSync(directory+'/camera.json',JSON.stringify({eye,bounds:geometry.projectionBounds(view),projection:geometry.projectionIndex(view),matrix:Array.from(geometry.mat4Mul(geometry.perspective(Math.PI/4,1,.01,100),geometry.lookAt(eye,[0,0,0],[0,1,0])))}));
"""
        sphere = node['grid']['type']=='sphere'
        globe = sphere and args.view == 'globe'
        atlas = sphere and not globe
        subprocess.run([node_exe,'--input-type=module','-e',javascript,str(out),str(sphere).lower(),args.view,str(1 if sphere else 2/node['grid']['width_m'])],check=True)
        positions = np.fromfile(out/'positions.bin',dtype='<f4').reshape(-1,3)
        source_ids = np.fromfile(out/'sourceIds.bin',dtype='<u4')
        indices = np.fromfile(out/'indices.bin',dtype='<u4')
        rivers = np.fromfile(out/'rivers.bin',dtype='<f4').reshape(-1,3)
        river_geo = np.fromfile(out/'riverGeo.bin',dtype='<f4').reshape(-1,3)
        geo_positions = np.fromfile(out/'geoPositions.bin',dtype='<f4').reshape(-1,3)
        camera = json.loads((out/'camera.json').read_text())
        river_groups=json.loads((out/'riverGroups.json').read_text())
        if not sphere:
            positions *= 2/node['grid']['width_m']; rivers *= 2/node['grid']['width_m']
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
        pigment_path = root/'apps/web/src/assets/atlas-pigment.png'
        pigment_image = Image.new('RGB',(1,1),(141,141,141)) if args.material_fallback else Image.open(pigment_path).convert('RGB')
        pigment = ctx.texture(pigment_image.size,3,pigment_image.tobytes(),alignment=1)
        pigment.build_mipmaps(); pigment.filter=(moderngl.LINEAR_MIPMAP_LINEAR,moderngl.LINEAR)
        # Desktop GL sampler mirrored repeat, matching WebGL2's material loader.
        # ModernGL's public repeat flags expose REPEAT only; use ctypes for MIRRORED_REPEAT.
        import ctypes
        import ctypes.util
        native_gl=ctypes.WinDLL('opengl32') if sys.platform=='win32' else ctypes.CDLL(ctypes.util.find_library('GL'))
        native_gl.glBindTexture(0x0DE1,pigment.glo)
        native_gl.glTexParameteri(0x0DE1,0x2802,0x8370);native_gl.glTexParameteri(0x0DE1,0x2803,0x8370)
        pigment.use(2);program['uPigmentTex'].value=2
        width=args.width
        height=width if globe else (round(width*camera['bounds']['height']/2) if atlas else round(width*node['grid']['rows']/node['grid']['size']*3**.5/2))
        frame = ctx.simple_framebuffer((width,height),components=3); frame.use()
        aspect=width/height
        matrix=np.array(camera['matrix'],dtype='f4') if globe else np.diag([.5 if atlas else 1.,1./camera['bounds']['height'] if atlas else aspect,1.,1.]).astype('f4')
        if not globe:
            matrix[0,0]*=args.zoom;matrix[1,1]*=args.zoom
            matrix[3,0]=-args.center[0]*matrix[0,0];matrix[3,1]=-args.center[1]*matrix[1,1]
        program['uViewProj'].write(matrix.tobytes())
        uniforms={'uRadius':1.,'uHeightScale':.00001,'uMapMetres':node['grid'].get('radius_m') if sphere else node['grid']['width_m']/2,
                  'uPlanarMode':not globe,'uWorldFlatMode':atlas,'uProjection':camera['projection'],'uPreviewMode':False,'uHasCryosphere':has_ice,'uStyle':2,'uGeographyActive':False,
                  'uLightDir':(-.45,.55,.7)}
        for name,value in uniforms.items():
            if name in program: program[name].value=value
        buffers=[]; content=[]
        attributes={'aCenter':positions,'aAtlasPosition':geo_positions,'aHeight':channel('elevation'),'aBiome':channel('biome'),
                    'aHeightGradient':np.fromfile(out/'gradients.bin',dtype='<f4').reshape(-1,3),
                    'aPlate':channel('plate_id'),'aTemperature':channel('temperature'),'aPrecipitation':channel('precipitation'),
                    'aFlow':channel('flow_accumulation'),'aLake':np.fromfile(out/'shore.bin',dtype='<f4'),
                    'aCryosphere':np.fromfile(out/'cryocover.bin',dtype='<f4').reshape(-1,4),'aFrozen':np.fromfile(out/'cryoground.bin',dtype='<f4').reshape(-1,2),
                    'aGeography':np.zeros(len(source_ids),dtype='f4'),'aPartition':np.zeros(len(source_ids),dtype='f4')}
        program['uHeightScale'].value=.025/max(float(attributes['aHeight'].max()),1.)
        for name,values in attributes.items():
            if name not in program: continue
            buffer=ctx.buffer(values.astype('f4').tobytes());buffers.append(buffer)
            content.append((buffer,str(values.shape[1])+'f' if values.ndim==2 else '1f',name))
        vao=ctx.vertex_array(program,content,index_buffer=ctx.buffer(indices.tobytes()))
        ctx.blend_func=(moderngl.SRC_ALPHA,moderngl.ONE_MINUS_SRC_ALPHA)
        river_vao=None
        if len(rivers):
            river_buffer=ctx.buffer(rivers.astype('f4').tobytes())
            river_geo_buffer=ctx.buffer(river_geo.astype('f4').tobytes())
            river_vao=ctx.vertex_array(line,[(river_buffer,'3f','aPosition'),(river_geo_buffer,'3f','aAtlasPosition')])
            line['uViewProj'].write(matrix.tobytes())
            for name,value in {'uSphere':globe,'uWorldFlatMode':atlas,'uProjection':camera['projection'],'uEye':tuple(camera['eye']),'uRadius':1.,'uColor':(.18,.46,.57,.84)}.items():
                if name in line: line[name].value=value
        for layer,label in [(0,'terrain'),(4,'elevation'),(10,'game'),(11,'cryosphere')]:
            frame.clear(.96,.95,.91)
            ctx.enable(moderngl.DEPTH_TEST)
            program['uLayer'].value=layer;vao.render(moderngl.TRIANGLES)
            if layer==10 and river_vao:
                ctx.disable(moderngl.DEPTH_TEST)
                ctx.enable(moderngl.BLEND)
                for group in river_groups:
                    line['uColor'].value=(.34,.62,.68,.74) if group['index']==1 else (.18,.46,.57,.84)
                    river_vao.render(moderngl.LINES,vertices=group['count'],first=group['first'])
                ctx.disable(moderngl.BLEND)
            image=Image.frombytes('RGB',(width,height),frame.read(components=3)).transpose(Image.Transpose.FLIP_TOP_BOTTOM)
            image.save(out/(label+'.png'))
        gl_error = ctx.error
        if gl_error != 'GL_NO_ERROR': raise RuntimeError(gl_error)
        report={'result':'passed','exact_es300_sources':True,'browser_ui_checked':False,'renderer':ctx.info['GL_RENDERER'],
                'offline_artifacts':bool(args.offline_artifacts),
                'cryosphere_available':has_ice,'ice_month':args.ice_month,
                'view':args.view if sphere else 'plane','pixels':[width,height],'material_fallback':args.material_fallback,
                'zoom':args.zoom,'center':args.center,
                'material_sha256':hashlib.sha256(pigment_path.read_bytes()).hexdigest(),'gl_error':gl_error,
                'cells':node['cells'],'river_segments':len(rivers)//2,'shader_sha256':hashlib.sha256(shader_source.encode()).hexdigest(),
                'project_sha256':hashlib.sha256(project_bytes).hexdigest()}
        (out/'report.json').write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf8')
        print(json.dumps(report,ensure_ascii=False))
    finally:
        if token: call('/session','DELETE')


if __name__=='__main__':main()
