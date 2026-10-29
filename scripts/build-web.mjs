import {spawnSync} from 'node:child_process'
import {fileURLToPath} from 'node:url'
const cwd=fileURLToPath(new URL('../apps/web',import.meta.url))
for(const args of [['node_modules/vue-tsc/bin/vue-tsc.js','--noEmit'],['node_modules/vite/bin/vite.js','build']]){const result=spawnSync(process.execPath,args,{cwd,stdio:'inherit'});if(result.status!==0)process.exit(result.status||1)}
