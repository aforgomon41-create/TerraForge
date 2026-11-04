// Exercise actual AI tool definitions with an isolated context; no model/network calls.
import {createRequire} from 'node:module'
import {mkdirSync,writeFileSync} from 'node:fs'
import {resolve,dirname} from 'node:path'
import {fileURLToPath} from 'node:url'
import assert from 'node:assert/strict'
const root=resolve(dirname(fileURLToPath(import.meta.url)),'..'),web=resolve(root,'apps/web'),req=createRequire(resolve(web,'package.json')),local=createRequire(req.resolve('vite/package.json'))
const {build}=local('esbuild'),out=resolve(root,'artifacts/agent-ecosystem-qa');mkdirSync(out,{recursive:true})
await build({entryPoints:[resolve(web,'src/agent/tools.ts')],bundle:true,platform:'node',format:'cjs',outfile:resolve(out,'tools.cjs'),alias:{'@':resolve(web,'src')},plugins:[{name:'isolated-api',setup(b){b.onResolve({filter:/^@\/api\/client$/},()=>({path:'api',namespace:'qa'}));b.onLoad({filter:/.*/,namespace:'qa'},()=>({contents:'export const api={sample:async()=>({cell:0}),metadata:async()=>({})}'}))}}]})
const {buildTools,buildSystemPrompt}=req(resolve(out,'tools.cjs'))
let current={available:true,version:'iucn-get-v2.1-tf1',biome:new Uint8Array([19,25,7]),group:new Uint16Array([306,403,0]),habitatBiome:new Uint8Array([1,0,6]),habitatGroup:new Uint16Array([19,0,97]),variant:new Uint8Array([2,3,3]),evidence:new Uint16Array([275,279,259])}
const modes=[],ctx={worldId:()=> 'qa-world',mapId:()=> 'qa-map',maps:()=>[],elevation:()=>new Float32Array([100,200,-1000]),biome:()=>new Uint8Array([4,12,0]),ecosystems:()=>current,setEcosystemMode:m=>modes.push(m),selection:()=>({lat:0,lon:0,cell:1}),geography:()=>null,selectFeature:()=>{},generate:async()=>{},drillDown:async()=>{},selectMap:async()=>{},setLayer:()=>{},setStyle:()=>{}}
const tools=buildTools(ctx),tool=n=>tools.find(t=>t.name===n)
const stats=await tool('terrain_stats').execute({})
assert.equal(stats.ecosystems.catalogue.groups,110);assert.equal(stats.ecosystems.pending_ground_cells,1);assert.equal(stats.ecosystems.habitats.length,2)
const selected=await tool('selected_location').execute({});assert.equal(selected.ecosystem.group_code,'TF1.3')
await tool('set_ecosystem_view').execute({mode:'variant'});assert.deepEqual(modes,[2])
await assert.rejects(()=>tool('set_ecosystem_view').execute({mode:'fake'}))
current={available:false,reason:'旧工程未保存细分类'}
assert.equal((await tool('terrain_stats').execute({})).ecosystems.available,false)
await assert.rejects(()=>tool('set_ecosystem_view').execute({mode:'group'}))
assert.match(buildSystemPrompt(),/不得把名录110种说成已全部模拟/)
const report={result:'passed',checks:['real terrain_stats hierarchy','selected u16 fine type','valid/invalid mode selection','legacy fallback','prompt scope'],network_calls:false,model_calls:false}
writeFileSync(resolve(out,'report.json'),JSON.stringify(report,null,2));console.log(JSON.stringify(report))
