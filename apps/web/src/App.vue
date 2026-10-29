<script setup lang="ts">
import { ref, onMounted, onBeforeUnmount, watch, computed } from 'vue'
import { useWorldStore } from '@/stores/world'
import { HexSphereRenderer, MapStyle } from '@/renderer/HexSphereRenderer'
import { buildSphereGrid } from '@/renderer/sphereGrid'
import { openEventStream, decodeBase64, type FrameEvent } from '@/api/sse'
import MapTree from '@/components/MapTree.vue'
import AgentPanel from '@/components/AgentPanel.vue'
import type { ToolContext } from '@/agent/tools'
import { api, type MapNode } from '@/api/client'

const store = useWorldStore()
const canvasRef = ref<HTMLCanvasElement | null>(null)
let renderer: HexSphereRenderer | null = null

// 界面状态
const showNewWorld = ref(true)
const worldName = ref('Aetheria')
const seed = ref(42)
const progress = ref(0)
const currentStage = ref('')
const fps = ref(0)
const hasMap = ref(false)
const errorMsg = ref('')
const styleName = ref<MapStyle>(MapStyle.Realistic)
const layerName = ref(0)
const exporting = ref(false)

/** 导出 PNG。用浏览器下载。 */
function exportPng() {
  if (!currentMapId.value) return
  const url = api.exportPng(currentMapId.value, 4096)
  const link = document.createElement('a')
  link.href = url
  link.download = 'terraforge-map.png'
  link.target = '_blank'
  document.body.appendChild(link)
  link.click()
  document.body.removeChild(link)
}

/** 导出 JSON 世界数据。 */
function exportJson() {
  if (!currentMapId.value) return
  const url = api.exportJson(currentMapId.value)
  const link = document.createElement('a')
  link.href = url
  link.download = 'terraforge-world.json'
  link.target = '_blank'
  document.body.appendChild(link)
  link.click()
  document.body.removeChild(link)
}

const LAYERS = [
  { id: 0, label: '地形' },
  { id: 1, label: '温度' },
  { id: 2, label: '降水' },
  { id: 3, label: '生态' },
  { id: 4, label: '高程' },
]

const STYLES: { id: MapStyle; label: string }[] = [
  { id: MapStyle.Realistic, label: '写实' },
  { id: MapStyle.Watercolor, label: '水彩' },
  { id: MapStyle.Parchment, label: '羊皮纸' },
  { id: MapStyle.Contour, label: '等高线' },
]

onMounted(() => {
  if (!canvasRef.value) return
  try {
    renderer = new HexSphereRenderer(canvasRef.value)
    renderer.start()

    // 先画一个空的六角球，让用户看到背景
    const preview = buildSphereGrid(96)
    renderer.setCells({
      centers: preview.centers,
      heights: new Float32Array(preview.count),
      biomes: new Uint8Array(preview.count),
      count: preview.count,
    })

    // 鼠标交互
    setupInteraction(canvasRef.value)
  } catch (e) {
    errorMsg.value = String(e)
  }

  setInterval(() => {
    if (renderer) fps.value = renderer.getFps()
  }, 500)
})

onBeforeUnmount(() => {
  renderer?.dispose()
})

function setupInteraction(el: HTMLCanvasElement) {
  let dragging = false
  let lastX = 0
  let lastY = 0

  el.addEventListener('pointerdown', (e) => {
    dragging = true
    lastX = e.clientX
    lastY = e.clientY
    el.setPointerCapture(e.pointerId)
  })

  el.addEventListener('pointermove', (e) => {
    if (!dragging) return
    renderer?.rotate(e.clientX - lastX, e.clientY - lastY)
    lastX = e.clientX
    lastY = e.clientY
  })

  el.addEventListener('pointerup', (e) => {
    dragging = false
    el.releasePointerCapture(e.pointerId)
  })

  el.addEventListener(
    'wheel',
    (e) => {
      e.preventDefault()
      renderer?.zoom(-e.deltaY)
    },
    { passive: false }
  )

  // 触摸双指缩放
  let lastPinch = 0
  el.addEventListener('touchmove', (e) => {
    if (e.touches.length === 2) {
      const d = Math.hypot(
        e.touches[0].clientX - e.touches[1].clientX,
        e.touches[0].clientY - e.touches[1].clientY
      )
      if (lastPinch > 0) renderer?.zoom((d - lastPinch) * 3)
      lastPinch = d
      e.preventDefault()
    }
  })
  el.addEventListener('touchend', () => {
    lastPinch = 0
  })
}

async function handleGenerate() {
  if (!renderer) return
  errorMsg.value = ''
  progress.value = 0
  currentStage.value = '准备中'

  try {
    const w = await store.createWorld(worldName.value, Number(seed.value))
    const mapId = await store.generate(w.id, (j) => {
      progress.value = j.progress
      currentStage.value = j.stage || '生成中'
    })

    // 用真实数据重建网格
    const elev = store.elevation!
    const biome = store.biome!

    // 推断网格频率：格子数满足 10f²+2
    const n = elev.length
    const freq = Math.round(Math.sqrt((n - 2) / 10))
    const grid = buildSphereGrid(freq)

    if (grid.count !== n) {
      throw new Error(`网格不匹配: 前端 ${grid.count} vs 后端 ${n}（freq=${freq}）`)
    }

    renderer.setCells({
      centers: grid.centers,
      heights: elev,
      biomes: biome,
      count: n,
    })

    // 按高程范围设置缩放
    let maxElev = 0
    for (let i = 0; i < n; i++) if (elev[i] > maxElev) maxElev = elev[i]
    renderer.heightScale = 0.12 / Math.max(maxElev, 1)

    // 切回真实数据渲染：预览帧只有 384x192，细节远不如完整数据
    renderer.exitPreview()

    // 刷新地图树，并把当前图设为选中
    if (store.current) await refreshMaps(store.current.id)
    currentMapId.value = mapId
    hasMap.value = true
    currentStage.value = '完成'
    progress.value = 1

    // 稍等一下让用户看到 100%，然后关掉面板显示地图
    setTimeout(() => {
      showNewWorld.value = false
    }, 600)
  } catch (e) {
    errorMsg.value = String(e)
  }
}

watch(styleName, (v) => {
  if (renderer) renderer.style = v
})

watch(layerName, (v) => {
  if (renderer) renderer.layer = v
})

// SSE 事件流
let closeStream: (() => void) | null = null

function startStream() {
  closeStream?.()
  // SSE 流也走 /map/api 前缀
  const base = import.meta.env.DEV ? '' : import.meta.env.BASE_URL.replace(/\/$/, '')
  closeStream = openEventStream(base, {
    onFrame: (f: FrameEvent) => {
      try {
        const bytes = decodeBase64(f.data)
        renderer?.uploadFrame(bytes, f.w, f.h, f.stats)
        liveStats.value = f.stats
      } catch (e) {
        console.warn('帧处理失败', e)
      }
    },
    onProgress: (p) => {
      progress.value = p.progress
      if (p.stage) currentStage.value = p.stage
    },
    onStage: (s) => {
      currentStage.value = s.to
    },
    onDone: () => {
      currentStage.value = '完成'
    },
    onFailed: (f) => {
      errorMsg.value = f.error
    },
  })
}

function stopStream() {
  closeStream?.()
  closeStream = null
}

const liveStats = ref<any>(null)

// 地图树
const maps = ref<MapNode[]>([])
const currentMapId = ref<string | null>(null)
const showTree = ref(true)
const showAgent = ref(false)

// Agent 工具上下文：把 App 的能力暴露给 Agent
const toolCtx: ToolContext = {
  worldId: () => store.current?.id ?? null,
  mapId: () => currentMapId.value,
  maps: () => maps.value,
  elevation: () => store.elevation,
  biome: () => store.biome,
  generate: async () => {
    if (!store.current) throw new Error('还没有世界')
    await store.generate(store.current.id)
    await refreshMaps(store.current.id)
  },
  drillDown: async (parentId, kind) => {
    const parent = maps.value.find((m) => m.id === parentId)
    if (!parent) throw new Error('找不到父图')
    await handleGenerateChild(parent, kind)
  },
  selectMap: async (id) => {
    await handleSelectMap(id)
  },
  setLayer: (layer) => {
    const map: Record<string, number> = {
      terrain: 0,
      temperature: 1,
      precipitation: 2,
      biome: 3,
      elevation: 4,
    }
    if (map[layer] !== undefined) layerName.value = map[layer]
  },
  setStyle: (style) => {
    const map: Record<string, MapStyle> = {
      realistic: MapStyle.Realistic,
      watercolor: MapStyle.Watercolor,
      parchment: MapStyle.Parchment,
      contour: MapStyle.Contour,
    }
    if (map[style] !== undefined) styleName.value = map[style]
  },
}

async function refreshMaps(worldId: string) {
  try {
    maps.value = await api.listMaps(worldId)
    if (maps.value.length > 0 && !currentMapId.value) {
      currentMapId.value = maps.value[0].id
    }
  } catch (e) {
    console.warn('地图列表加载失败', e)
  }
}

/** 生成子图（下钻到更细层级）。 */
async function handleGenerateChild(parent: MapNode, kind: string) {
  try {
    // 子图参数：以父图中心为中心，覆盖宽度按层级递减
    const widthByKind: Record<string, number> = {
      continent: 4_000_000,   // 4000 km
      region: 400_000,        // 400 km
      site: 20_000,           // 20 km
    }
    const res = await api.generateChild(parent.id, {
      kind,
      center_lon: 0,
      center_lat: 0,
      width_m: widthByKind[kind] ?? 400_000,
    })
    currentMapId.value = res.map_id
    if (store.current) refreshMaps(store.current.id)

    // 轮询到完成
    while (true) {
      await new Promise((r) => setTimeout(r, 700))
      const j = await api.getJob(res.job_id)
      if (j.state === 'done') break
      if (j.state === 'failed') throw new Error(j.error || '生成失败')
    }
    if (store.current) await refreshMaps(store.current.id)
  } catch (e) {
    errorMsg.value = String(e)
  }
}

async function handleSelectMap(id: string) {
  currentMapId.value = id
  const node = maps.value.find((m) => m.id === id)
  if (!node || node.status !== 'done') return

  try {
    const elev = await api.getField(id, 'elevation')
    const biome = await api.getField(id, 'biome')
    const biomes = Uint8Array.from(biome, (v) => Math.round(v))

    let maxE = 0
    for (let i = 0; i < elev.length; i++) if (elev[i] > maxE) maxE = elev[i]

    if (node.kind === 'world') {
      renderer?.setSphereMode()
      const freq = Math.round(Math.sqrt((elev.length - 2) / 10))
      const grid = buildSphereGrid(freq)
      // 加载温度和降水（图层切换要用）
      let temps: Float32Array | undefined
      let precs: Float32Array | undefined
      try {
        temps = await api.getField(id, 'temperature')
        precs = await api.getField(id, 'precipitation')
      } catch {
        // 可选通道，失败就跳过
      }

      renderer?.setCells({
        centers: grid.centers,
        heights: elev,
        biomes,
        temperatures: temps,
        precipitations: precs,
        count: elev.length,
      })
    } else {
      // 子图：平面网格。
      // 格子中心铺在一个平面的球冠上，六角格半径必须与格间距匹配。
      const size = Math.round(Math.sqrt(node.cells))
      renderer?.setPlanarMode(size)

      // span 控制平面网格的物理跨度。相机在 z 轴上方，距离 2.0，
      // 视野 45 度，可见范围约 2*tan(22.5°)*2 = 1.66 个单位
      // 六角格网的正确排布：
      // - 行内相邻格水平间距 = cellStep
      // - 行间垂直间距 = cellStep * sqrt(3)/2（约 0.866 倍）
      // - 奇数行水平偏移半格
      //
      // 之前行间距用了 cellStep，导致行与行之间空隙过大，
      // 渲染出来就是一条条斜纹。
      const cellStep = 1.9 / size
      const rowSpacing = cellStep * 0.866
      const totalW = size * cellStep
      const totalH = size * rowSpacing
      // 六边形外接圆半径 = 对边距 / sqrt(3)，略放大消除接缝
      const hexR = cellStep * 0.60

      // 相机半高：让整块网格刚好铺满画面
      if (renderer) renderer.planarHalfHeight = (totalH / 2) * 1.05

      const centers = new Float32Array(elev.length * 3)
      for (let r = 0; r < size; r++) {
        const rowShift = r % 2 === 0 ? 0 : 0.5 * cellStep
        for (let c = 0; c < size; c++) {
          const idx2 = r * size + c
          const px = c * cellStep + rowShift - totalW / 2
          const py = r * rowSpacing - totalH / 2
          centers[idx2 * 3] = px / 1.0
          centers[idx2 * 3 + 1] = -py
          centers[idx2 * 3 + 2] = 0
        }
      }
      renderer?.setCells(
        { centers, heights: elev, biomes, count: elev.length },
        hexR * 1.02
      )
    }

    if (renderer) {
      renderer.heightScale = 0.12 / Math.max(maxE, 1)
      renderer.layer = layerName.value
    }
    hasMap.value = true
  } catch (e) {
    errorMsg.value = String(e)
  }
}


const stageLabel = computed(() => {
  if (progress.value >= 1) return '世界已成形'
  return currentStage.value || '等待开始'
})
</script>

<template>
  <div class="relative w-full h-full overflow-hidden">
    <!-- 全屏地图画布 -->
    <canvas ref="canvasRef" class="absolute inset-0 w-full h-full" />

    <!-- 暗角 -->
    <div class="absolute inset-0 pointer-events-none"
         style="background: radial-gradient(circle at 50% 45%, transparent 30%, rgba(0,0,0,0.55) 100%)" />

    <!-- 左上：标题 -->
    <div class="absolute top-6 left-6 animate-fade-up pointer-events-none">
      <h1 class="text-2xl font-serif tracking-wide text-ink/95">
        TerraForge
      </h1>
      <p class="text-xs text-dim/60 mt-1 tracking-wider">世界生成器</p>
    </div>

    <!-- 左侧：地图树 -->
    <div
      v-if="hasMap"
      class="absolute top-24 left-6 w-64 panel p-3 animate-fade-up max-h-[60vh] overflow-y-auto z-40"
    >
      <div class="flex items-center justify-between mb-2 px-1">
        <span class="label-xs">地图层级</span>
        <button
          class="text-[10px] text-dim/50 hover:text-ink transition-colors"
          @click="showTree = !showTree"
        >
          {{ showTree ? '收起' : '展开' }}
        </button>
      </div>
      <MapTree
        v-if="showTree"
        :maps="maps"
        :current-id="currentMapId"
        @select="handleSelectMap"
        @generate="handleGenerateChild"
      />
    </div>

    <!-- 右上：图层切换 -->
    <div v-if="hasMap" class="absolute top-24 right-6 panel px-3 py-2 animate-fade-up z-40">
      <div class="label-xs mb-1.5 px-1">图层</div>
      <div class="flex flex-col gap-0.5">
        <button
          v-for="l in LAYERS"
          :key="l.id"
          class="px-3 py-1 text-xs rounded text-left transition-colors"
          :class="layerName === l.id
            ? 'bg-cool/20 text-cool border border-cool/40'
            : 'text-dim/70 hover:text-ink hover:bg-white/5 border border-transparent'"
          @click="layerName = l.id"
        >
          {{ l.label }}
        </button>
      </div>
    </div>

    <!-- 右上：风格切换 -->
    <div v-if="hasMap" class="absolute top-6 right-6 panel px-3 py-2 animate-fade-up">
      <div class="flex gap-1">
        <button
          v-for="s in STYLES"
          :key="s.id"
          class="px-3 py-1.5 text-xs rounded-lg transition-all"
          :class="styleName === s.id
            ? 'bg-accent/20 text-accent border border-accent/40'
            : 'text-dim/70 hover:text-ink hover:bg-white/5 border border-transparent'"
          @click="styleName = s.id"
        >
          {{ s.label }}
        </button>
      </div>
    </div>

    <!-- 左下：状态 -->
    <div class="absolute bottom-6 left-6 text-xs text-dim/50 font-mono pointer-events-none">
      <div v-if="fps > 0">{{ fps }} fps</div>
      <div v-if="hasMap">{{ store.elevation?.length.toLocaleString() }} 格</div>
    </div>

    <!-- 创建面板 -->
    <div
      v-if="showNewWorld"
      class="absolute inset-0 flex items-center justify-center bg-black/40 backdrop-blur-sm z-50"
    >
      <div class="panel w-[420px] p-8 animate-fade-up">
        <h2 class="text-xl font-serif mb-1">创造一颗星球</h2>
        <p class="text-sm text-dim/70 mb-6">
          用真实的地球物理过程，从板块运动开始演化
        </p>

        <div class="space-y-5">
          <div>
            <label class="label-xs block mb-2">世界名称</label>
            <input
              v-model="worldName"
              class="w-full px-3 py-2.5 rounded-lg bg-white/5 border border-white/10
                     text-sm focus:outline-none focus:border-accent/50 transition-colors"
            />
          </div>

          <div>
            <label class="label-xs block mb-2">随机种子</label>
            <div class="flex gap-2">
              <input
                v-model.number="seed"
                type="number"
                class="flex-1 px-3 py-2.5 rounded-lg bg-white/5 border border-white/10
                       text-sm font-mono focus:outline-none focus:border-accent/50"
              />
              <button class="btn-ghost px-3" @click="seed = Math.floor(Math.random() * 99999)">
                随机
              </button>
            </div>
          </div>

          <div v-if="store.generating" class="pt-2">
            <div class="flex justify-between text-xs text-dim/70 mb-2">
              <span>{{ stageLabel }}</span>
              <span class="font-mono">{{ Math.round(progress * 100) }}%</span>
            </div>
            <div class="h-1 rounded-full bg-white/10 overflow-hidden">
              <div
                class="h-full bg-accent transition-all duration-300 rounded-full"
                :style="{ width: (progress * 100) + '%' }"
              />
            </div>
          </div>

          <div v-if="errorMsg" class="text-xs text-danger/90 p-3 rounded-lg bg-danger/10 border border-danger/20">
            {{ errorMsg }}
          </div>

          <button
            class="btn-primary w-full py-3 text-sm"
            :disabled="store.generating"
            @click="handleGenerate"
          >
            <span v-if="!store.generating">开始生成</span>
            <span v-else class="flex items-center justify-center gap-2">
              <span class="spinner" />
              演化中...
            </span>
          </button>
        </div>
      </div>
    </div>

    <!-- 导出 -->
    <div v-if="hasMap" class="absolute bottom-6 right-[21rem] flex gap-1.5 z-40">
      <button class="btn-ghost text-xs" @click="exportPng">导出 PNG</button>
      <button class="btn-ghost text-xs" @click="exportJson">导出 JSON</button>
    </div>

    <!-- AI 助手 -->
    <button
      v-if="hasMap"
      class="absolute bottom-6 right-6 btn-primary text-xs z-40"
      :class="{ 'mr-40': showAgent }"
      @click="showAgent = !showAgent"
    >
      {{ showAgent ? '关闭助手' : 'AI 助手' }}
    </button>

    <div
      v-if="showAgent"
      class="absolute bottom-20 right-6 w-96 h-[520px] panel overflow-hidden animate-fade-up z-40"
    >
      <AgentPanel :tool-ctx="toolCtx" />
    </div>

    <!-- 重新生成按钮 -->
    <button
      v-if="hasMap"
      class="absolute bottom-6 right-6 btn-ghost text-xs"
      @click="showNewWorld = true"
    >
      创造新的世界
    </button>
  </div>
</template>
