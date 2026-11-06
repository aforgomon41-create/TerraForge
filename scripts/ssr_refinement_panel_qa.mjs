// Verify real Vue markup and start-button gating without a browser or listener.
import {createRequire} from 'node:module'
import {readFileSync,mkdirSync,writeFileSync} from 'node:fs'
import {dirname,resolve} from 'node:path'
import {fileURLToPath} from 'node:url'
import assert from 'node:assert/strict'
const root=resolve(dirname(fileURLToPath(import.meta.url)),'..'),web=resolve(root,'apps/web')
const req=createRequire(resolve(web,'package.json')),local=createRequire(req.resolve('vite/package.json'))
const {build}=local('esbuild'),{parse,compileScript}=req('vue/compiler-sfc')
const out=resolve(root,'artifacts/refinement-panel-ssr');mkdirSync(out,{recursive:true})
await build({stdin:{contents:"import Panel from './src/components/RefinementPanel.vue';import {createSSRApp} from 'vue';import {renderToString} from 'vue/server-renderer';export async function render(props){return renderToString(createSSRApp(Panel,props))}",resolveDir:web},bundle:true,platform:'node',format:'cjs',outfile:resolve(out,'panel.cjs'),alias:{'@':resolve(web,'src')},define:{'process.env.NODE_ENV':'"production"'},plugins:[{name:'vue-ssr',setup(builder){builder.onLoad({filter:/\.vue$/},args=>{const {descriptor}=parse(readFileSync(args.path,'utf8'),{filename:args.path});return {contents:compileScript(descriptor,{id:'refinement-qa',inlineTemplate:true,templateOptions:{ssr:true}}).content,loader:'ts',resolveDir:dirname(args.path)}})}}]})
const {render}=req(resolve(out,'panel.cjs'))
const plan={kind:'continent',feature_id:2,feature_name:'大陆 2',center_lat:-30,center_lon:179.9,width_m:6000000,cell_size_m:11718.75,side:512,cells:262144,can_generate:true,complete_object:true,message:'完整概览，后续可选择局部继续细化。',maximum_width_m:14000000}
const base={source:'世界总图',kind:'大陆',feature:'大陆 2',scope:'full',precision:'auto',width:256,spacing:1000,name:'大陆 2 · 概览',plan,loading:false,error:'',locked:false}
for(const [name,changes,enabled] of [['full',{},true],['custom',{scope:'custom',feature:null,width:80},true],['loading',{plan:null,loading:true},false],['invalid',{plan:{...plan,can_generate:false,message:'指定精度超过预算'}},false],['error',{plan:null,error:'读取失败'},false],['generating',{locked:true},false],['fixed',{precision:'fixed'},true]]){
 const html=await render({...base,...changes});writeFileSync(resolve(out,name+'.html'),html)
 const start=html.match(/<button class="primary"([^>]*)>/)?.[1];assert.notEqual(start,undefined);assert.equal(!start.includes('disabled'),enabled,name)
 assert.match(html,/完整对象/);assert.match(html,/自定范围/);assert.match(html,/计算精度/)
 if(name==='full'){assert.match(html,/6000/);assert.match(html,/11\.72 km/);assert.match(html,/readonly/)}
 if(name==='custom')assert.match(html,/此处没有对应的地理对象/)
 if(name==='fixed')assert.match(html,/指定计算格距（m）/)
 if(name==='invalid')assert.match(html,/指定精度超过预算/)
 if(name==='loading')assert.match(html,/正在核验/)
}
const report={result:'passed',states:7,browser_ui_checked:false,network_listener_started:false};writeFileSync(resolve(out,'report.json'),JSON.stringify(report,null,2));console.log(JSON.stringify(report))
