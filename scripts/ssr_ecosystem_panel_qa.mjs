// Compile and render the actual Vue component without a browser or TCP listener.
import {createRequire} from 'node:module'
import {readFileSync,mkdirSync,writeFileSync} from 'node:fs'
import {dirname,resolve} from 'node:path'
import {fileURLToPath} from 'node:url'
import assert from 'node:assert/strict'
const root=resolve(dirname(fileURLToPath(import.meta.url)),'..'),web=resolve(root,'apps/web')
const req=createRequire(resolve(web,'package.json')),local=createRequire(req.resolve('vite/package.json'))
const {build}=local('esbuild'),{parse,compileScript}=req('vue/compiler-sfc')
const out=resolve(root,'artifacts/ecosystem-panel-ssr');mkdirSync(out,{recursive:true})
await build({stdin:{contents:"import Panel from './src/components/EcosystemPanel.vue';import {createSSRApp} from 'vue';import {renderToString} from 'vue/server-renderer';export async function render(props){return renderToString(createSSRApp(Panel,props))}",resolveDir:web},
 bundle:true,platform:'node',format:'cjs',outfile:resolve(out,'panel.cjs'),alias:{'@':resolve(web,'src')},define:{'process.env.NODE_ENV':'"production"'},
 plugins:[{name:'vue-ssr',setup(builder){builder.onLoad({filter:/\.vue$/},args=>{
  const {descriptor}=parse(readFileSync(args.path,'utf8'),{filename:args.path})
  return {contents:compileScript(descriptor,{id:'ecosystem-panel-qa',inlineTemplate:true,templateOptions:{ssr:true}}).content,loader:'ts',resolveDir:dirname(args.path)}
 })}}]})
const {render}=req(resolve(out,'panel.cjs'))
const data={available:true,version:'iucn-get-v2.1-tf1',biome:new Uint8Array([19,19,25]),group:new Uint16Array([306,0,403]),habitatBiome:new Uint8Array([1,0,0]),habitatGroup:new Uint16Array([19,0,0]),variant:new Uint8Array([2,6,3]),evidence:new Uint16Array([275,275,279])}
const cases=[['current',{data,mode:1,generating:false,hasMap:true}],['legacy',{data:{available:false,reason:'旧工程未保存生态细分类'},mode:1,generating:false,hasMap:true}],['generating',{data:null,mode:1,generating:true,hasMap:false}]]
for(const [name,props] of cases){
 const html=await render(props);writeFileSync(resolve(out,name+'.html'),html)
 assert.equal((html.match(/class="eco-entry"/g)||[]).length,110)
 assert.equal((html.match(/aria-pressed=/g)||[]).length,3)
 if(name==='current'){assert.equal((html.match(/class="eco-row"/g)||[]).length,3);assert.match(html,/细类待定/);assert.match(html,/33\.3%/)}
 else {assert.equal((html.match(/class="eco-row"/g)||[]).length,0);assert.equal((html.match(/disabled/g)||[]).length,3)}
}
const report={result:'passed',checks:['real Vue SSR compilation','25/110 disclosure structure','current/legacy/generating states','unknown details stay explicit'],browser_ui_checked:false,network_listener_started:false}
writeFileSync(resolve(out,'report.json'),JSON.stringify(report,null,2));console.log(JSON.stringify(report))
