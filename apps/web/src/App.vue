<script setup lang="ts">
import { ref, reactive, shallowRef, computed, watch, onMounted, onBeforeUnmount } from 'vue'
import { useWorldStore } from '@/stores/world'
import {
  api,
  type MapNode,
  type TectonicDiagnostic,
  type CirculationDiagnostic,
  type HydrologyDiagnostic,
  type RefinementPlan,
  type RefinementQuery,
} from '@/api/client'
import { HexSphereRenderer, MapStyle } from '@/renderer/HexSphereRenderer'
import { buildSphereGrid } from '@/renderer/sphereGrid'
import { direction, latLon, patchDirection, patchProject, nearestCell } from '@/renderer/geometry'
import type { Vec3 } from '@/renderer/math'
import { bindMapInput } from '@/renderer/interaction'
import type { DecodedFrameEvent } from '@/api/frame'
import { planarPreviewSurface } from '@/renderer/surface'
import { projections, type WorldProjection } from '@/renderer/projections'
import type { ToolContext } from '@/agent/tools'
import type { IceResponse } from '@/cryosphere/data'
import { flatDirection, flatProject } from '@/renderer/geometry'
import { parseSeed } from '@/generation/seed'
import {
  defaults,
  scopeDefaults,
  scopeParameters,
  extentFields,
  terrainFields,
  weatherFields,
  localFields,
  planetFields,
  type StartScope,
} from '@/generation/options'
import MapTree from '@/components/MapTree.vue'
import FeatureTree from '@/components/FeatureTree.vue'
import { featureKinds, featureMask, featuresAt } from '@/geography/catalog'
import {
  partitionModes,
  partitionLabels,
  partitionBoundaries,
  type PartitionMode,
} from '@/geography/view'
import AgentPanel from '@/components/AgentPanel.vue'
import EcosystemPanel from '@/components/EcosystemPanel.vue'
import RefinementPanel from '@/components/RefinementPanel.vue'
import { ecosystemAt, ecosystemModes } from '@/ecosystems/view'
import { evolutionClock } from '@/generation/progress'
const store = useWorldStore(),
  canvas = ref<HTMLCanvasElement | null>(null),
  fileInput = ref<HTMLInputElement | null>(null)
const scaleText = ref('')
const worldProjection = ref<'sphere' | WorldProjection>('sphere')
const liveFrame = shallowRef<DecodedFrameEvent | null>(null)
let previewContext = '',
  previewSphere: ReturnType<typeof buildSphereGrid> | undefined
const sphericalWorld = computed(
  () => (liveFrame.value?.grid || store.currentMap?.grid)?.type === 'sphere',
)
const projectionHint = computed(
  () => projections.find((p) => p.id === worldProjection.value)?.hint || '拖动旋转球面',
)
const liveLayer = computed(() => {
  if (!store.generating || !liveFrame.value) return layer.value
  const flags = liveFrame.value.meta?.available_channels ?? 15
  if (
    (layer.value === 1 && !(flags & 2)) ||
    (layer.value === 2 && !(flags & 4)) ||
    (layer.value === 5 && !(flags & 8)) ||
    layer.value === 3 ||
    layer.value === 6 ||
    layer.value === 7 ||
    layer.value === 8 ||
    layer.value === 9
  )
    return 0
  return layer.value
})
const liveCaption = computed(() => {
  const meta = liveFrame.value?.meta
  if (!meta) return '等待首个计算状态…'
  const clock =
    meta.elapsed_myr != null
      ? evolutionClock(meta.elapsed_myr, Number(store.current?.params?.tectonic_age_myr))
      : meta.year != null
        ? `求解第 ${meta.year} 年 / ${meta.month} 月`
        : `第 ${meta.step} 步`
  return `低精度实时预览 · ${clock} · ${meta.detail}`
})
const tab = ref<'tree' | 'create'>('create'),
  panel = ref<'project' | 'layers' | 'ai' | null>('project'),
  projectOpen = ref(true)
const name = ref('Aetheria'),
  seed = ref(''),
  land = ref(29),
  plates = ref(14),
  erosion = ref(1),
  evolutionMyr = ref(4600),
  evolutionStepMyr = ref(10),
  minPlateSpeed = ref(0.5),
  maxPlateSpeed = ref(12),
  worldShape = ref<'sphere' | 'plane'>('sphere'),
  allowLandAtEdge = ref(true)
const startScope = ref<StartScope>('world'),
  options = reactive({ ...defaults }),
  landscape = ref('hills'),
  monsoon = ref(true),
  glacier = ref(true)
const localScope = computed(() => startScope.value === 'region' || startScope.value === 'site')
watch(startScope, (scope) => {
  Object.assign(options, scopeDefaults(scope))
  land.value = scope === 'continent' ? 65 : 29
  erosion.value = 1
})
const style = ref(MapStyle.Parchment),
  layer = ref(10),
  notice = ref(''),
  ready = ref(false),
  busy = ref(false),
  loadLabel = ref('正在打开工作台…'),
  selection = ref<{ lat: number; lon: number; cell: number } | null>(null),
  pin = ref<{ x: number; y: number } | null>(null)
const partitionMode = ref<PartitionMode>('region'),
  partitionTruncated = ref(false)
const ecosystemMode = ref(1)
const selectedEcosystem = computed(() =>
  ecosystemAt(store.ecosystems?.available ? store.ecosystems : null, selection.value?.cell),
)
const partitionHint = computed(() => partitionModes.find((p) => p.id === partitionMode.value)!.hint)
const manualLat = ref('0'),
  manualLon = ref('0'),
  childWidth = ref(256),
  childName = ref(''),
  refine = ref(false),
  ended = ref(false)
const activeFeatureId = ref<number | null>(null)
const childScope = ref<'full' | 'custom'>('custom'),
  childPrecision = ref<'auto' | 'fixed'>('auto'),
  childSpacing = ref(1000),
  childPlan = shallowRef<RefinementPlan | null>(null),
  childPlanLoading = ref(false),
  childPlanError = ref('')
let childPlanKey = ''
let childPlanController: AbortController | undefined,
  childPlanTimer: ReturnType<typeof setTimeout> | undefined
const activeFeature = computed(() =>
  activeFeatureId.value ? store.geography?.features[activeFeatureId.value - 1] : null,
)
const selectedGeography = computed(() => featuresAt(store.geography, selection.value?.cell))
let renderer: HexSphereRenderer | null = null,
  heartbeat: ReturnType<typeof setInterval> | undefined,
  uiTimer: ReturnType<typeof setInterval> | undefined,
  interaction: AbortController | undefined
const kinds: Record<string, string> = {
    world: '世界',
    continent: '大陆',
    region: '区域',
    site: '地点',
  },
  nextKind: Record<string, string> = {
    world: 'continent',
    continent: 'region',
    region: 'site',
  }
const biomes = [
  '海洋',
  '冰盖',
  '苔原',
  '针叶林',
  '温带阔叶林',
  '温带草原',
  '地中海灌丛',
  '沙漠',
  '稀树草原',
  '热带雨林',
  '热带季雨林',
  '高山草甸',
  '湿地',
  '内陆水体',
]
const tectonics = ref<TectonicDiagnostic | null>(null)
const circulation = ref<CirculationDiagnostic | null>(null)
const hydrology = ref<HydrologyDiagnostic | null>(null)
let hydrologyController: AbortController | undefined
let hydrologyCache: { key: string; data: HydrologyDiagnostic } | undefined
const iceMonth = ref(0)
const monthlyIce = shallowRef<{ key: string; data: IceResponse } | null>(null)
let iceController: AbortController | undefined
const visibleIce = computed(() => {
  if (iceMonth.value === 0) return store.cryosphere
  const key = `${store.currentMap?.id}:${iceMonth.value}`
  return monthlyIce.value?.key === key ? monthlyIce.value.data : null
})
const iceStatus = computed(() =>
  store.generating
    ? '冰雪将在最终气候完成后显示。'
    : visibleIce.value?.available
      ? '常年陆冰与季节海冰分别计算；灰紫色表示冻土估计，冻土表面夏季仍可解冻。'
      : visibleIce.value?.reason ||
        (store.cryosphere?.available === false && store.cryosphere.reason) ||
        '正在读取冰雪资料…',
)
const circulationMonth = ref(0)
let circulationController: AbortController | undefined
let circulationCache: { key: string; data: CirculationDiagnostic } | undefined
const circulationStatus = computed(() => {
  if (store.generating) return '生成完成并保存环流后显示；当前显示真实地形预览。'
  const data = circulation.value
  if (!data) return '正在读取环流资料…'
  if (!data.available) return data.reason || '此工程没有环流记录，请重新生成。'
  const current = layer.value === 9
  const peak = (data.maximum_speed_m_s || 0) * (current ? 100 : 1)
  const residual = Math.max(0, ...(data.relative_solver_residual || []))
  const scale = Math.min(1, ...(data.current_velocity_scale || []))
  return `箭头等长，表示流向；颜色由慢到快，最高约 ${peak.toFixed(1)} ${current ? 'cm/s' : 'm/s'}。${circulationMonth.value === 0 ? '年平均是矢量平均，季节反向流可能抵消。' : ''}${residual > 0.02 ? '环流尚未充分收敛，结果仅供参考。' : ''}${current ? '风驱表层流近似，采用一次滞后热量反馈。' : ''}${current && scale < 0.999 ? `部分月份速度整体缩放至 ${(scale * 100).toFixed(0)}%，残差对应缩放前方程。` : ''}`
})
let tectonicController: AbortController | undefined
let tectonicCache: { id: string; data: TectonicDiagnostic } | undefined
const selectedPlate = computed(() => {
  const cell = selection.value?.cell,
    data = tectonics.value,
    grid = store.currentMap?.grid
  if (cell === undefined || !data?.available || !grid || !store.plateIds) return null
  const id = Math.round(store.plateIds[cell]),
    plate = data.plates?.find((p) => p.id === id)
  if (!plate) return null
  let v = data.planar_velocities?.[id]
  if (v && data.motion_grid?.type === 'plane') {
    const root = data.motion_grid
    const point = flatProject(
      root.center_lat,
      root.center_lon,
      direction(selection.value!.lat, selection.value!.lon),
      root.radius_m,
      root.reference_lat ?? root.center_lat,
    )
    const center = data.plate_centers_m?.[id] ?? [0, 0, 0]
    const spin = data.planar_spin_rad_myr?.[id] ?? 0
    v = [v[0] - spin * (point[1] - center[1]), v[1] + spin * (point[0] - center[0]), 0]
  }
  if (!v) {
    const p = direction(selection.value!.lat, selection.value!.lon).map((x) => x * grid.radius_m)
    const a = plate.euler_pole
    v = [a[1] * p[2] - a[2] * p[1], a[2] * p[0] - a[0] * p[2], a[0] * p[1] - a[1] * p[0]].map(
      (x) => x * plate.omega,
    )
  }
  const uid = data.lifecycle?.slot_uids[id] ?? id
  const identity = data.lifecycle?.identities.find((item) => item.uid === uid)
  return { id: uid, speed: Math.hypot(...v) * 1e-4, identity }
})
const plateStatus = computed(() => {
  if (!tectonics.value?.available) return tectonics.value?.reason || '正在读取板块记录…'
  const life = tectonics.value.lifecycle
  if (!life?.identities.length)
    return '箭头等长，仅表示方向；点击地图查看 cm/年。旧工程没有板块生灭记录。'
  const active = life.identities.filter((p) => p.status === 'active').length
  const births = life.identities.filter((p) => p.parent_uid !== null).length
  const ended = life.identities.filter((p) => p.ended_myr !== null).length
  return `当前 ${active} 块；演化中新生 ${births} 块，终止 ${ended} 块。箭头等长；点击查看速度、身份与面积。${life.capacity_limited_events ? '板块计算容量达到上限，部分细小分裂未解析。' : ''}`
})
const layers = [
  {
    id: 10,
    name: '渲染地图',
    icon: '▰',
    unit: '自然彩绘 · 高度 · 生态 · 水系',
    ends: ['海洋', '低地', '山地'],
    gradient: 'linear-gradient(90deg,#315e70,#9ebdb0,#7d9966,#d7ccad)',
  },
  {
    id: 11,
    name: '冰雪与冻土',
    icon: '❄',
    unit: '常年陆冰 · 年均水面冰 · 冻土估计',
    ends: ['冻土', '水面冰', '陆冰'],
    gradient: 'linear-gradient(90deg,#a7a1b4,#b4d5dd,#f2f4ec)',
  },
  {
    id: 8,
    name: '风场',
    icon: '↝',
    unit: '箭头方向 · 颜色表示相对风速',
    ends: ['慢', '快'],
    gradient: 'linear-gradient(90deg,#447dab,#478b79,#b18d45,#ad4f40)',
  },
  {
    id: 9,
    name: '洋流',
    icon: '≈',
    unit: '表层海流 · 颜色表示相对流速',
    ends: ['慢', '快'],
    gradient: 'linear-gradient(90deg,#447dab,#478b79,#b18d45,#ad4f40)',
  },
  {
    id: 7,
    name: '自动分区',
    icon: '▧',
    unit: '自动地理分区 · 颜色代表类别',
    ends: [],
    gradient: '',
  },
  {
    id: 6,
    name: '板块与运动',
    icon: '↗',
    unit: '边界类型 · 运动方向；点击查看速度',
    ends: ['汇聚', '张裂', '走滑'],
    gradient: 'linear-gradient(90deg,#a63829,#19708c,#8c66a6)',
  },
  {
    id: 0,
    name: '地形地图',
    icon: '◈',
    unit: '海底深度 / 陆地地貌',
    ends: ['深海', '平原', '山地'],
    gradient: 'linear-gradient(90deg,#4d7b87,#9ebbb2,#9daa76,#b59d7d)',
  },
  {
    id: 4,
    name: '高程',
    icon: '≋',
    unit: '米 · 相对海平面',
    ends: ['−6,000', '0', '6,000'],
    gradient: 'linear-gradient(90deg,#4d7b87,#92b397,#c0ba80,#eeeadd)',
  },
  {
    id: 1,
    name: '温度',
    icon: '◌',
    unit: '℃ · 年均温',
    ends: ['−40', '0', '40'],
    gradient: 'linear-gradient(90deg,#597ab3,#c7d1a3,#c4593d)',
  },
  {
    id: 2,
    name: '降水',
    icon: '☂',
    unit: '毫米 / 年',
    ends: ['0', '1,500', '3,000+'],
    gradient: 'linear-gradient(90deg,#d4bd82,#8eada0,#367394)',
  },
  {
    id: 5,
    name: '水文',
    icon: '⌁',
    unit: '平方公里 · 上游汇水面积',
    ends: ['1,000', '30,000', '1,000,000+'],
    gradient: 'linear-gradient(90deg,#d6d6b5,#819fa1,#29668c)',
  },
  {
    id: 3,
    name: '生态',
    icon: '❋',
    unit: '分层生态 · 大类 / 细类 / 地方变体',
    ends: ['冰原', '森林', '荒漠'],
    gradient: 'linear-gradient(90deg,#e8ede5,#597b56,#cfb580)',
  },
]
const activeLayer = computed(() => layers.find((l) => l.id === liveLayer.value)!),
  modeText = computed(() =>
    store.session?.mode === 'desktop' ? '本地工程 · 自动保存' : '网页体验 · 临时会话',
  )
const locked = computed(() => busy.value || store.generating || store.loading || !ready.value),
  hasMap = computed(() => !!store.currentMap)
const next = computed(() => nextKind[store.currentMap?.kind || ''] || ''),
  spacing = computed(() => ({ continent: 1000, region: 100, site: 1 })[next.value] || 1)
const validSelection = computed(() => selection.value && store.currentMap?.status === 'done')
const refinementFeature = computed(() =>
  activeFeature.value?.level === next.value
    ? activeFeature.value
    : selectedGeography.value.find((f) => f.level === next.value) || null,
)
function childQuery(): RefinementQuery {
  const s = selection.value
  if (!s) throw new Error('请先选择位置')
  const precision = childPrecision.value === 'fixed' ? { cell_size_m: childSpacing.value } : {}
  if (refine.value && childScope.value === 'full' && refinementFeature.value)
    return { feature_id: refinementFeature.value.id, ...precision }
  return { center_lat: s.lat, center_lon: s.lon, width_m: childWidth.value * 1000, ...precision }
}
watch(
  () => [
    refine.value,
    store.currentMap?.id,
    selection.value?.lat,
    selection.value?.lon,
    refinementFeature.value?.id,
    childScope.value,
    childPrecision.value,
    childWidth.value,
    childSpacing.value,
  ],
  () => {
    clearTimeout(childPlanTimer)
    childPlanController?.abort()
    childPlan.value = null
    childPlanKey = ''
    childPlanError.value = ''
    childPlanLoading.value = false
    if (!refine.value || !selection.value || !next.value || !store.currentMap) return
    const id = store.currentMap.id,
      query = childQuery(),
      controller = new AbortController()
    childPlanController = controller
    if (Object.values(query).some((v) => !Number.isFinite(v))) {
      childPlanError.value = '请填写有效的覆盖宽度与计算格距'
      return
    }
    childPlanLoading.value = true
    childPlanTimer = setTimeout(async () => {
      try {
        const plan = await api.refinementPlan(id, query, controller.signal)
        if (!controller.signal.aborted) {
          childPlan.value = plan
          childPlanKey = `${id}:${JSON.stringify(query)}`
        }
      } catch (e) {
        if (!controller.signal.aborted)
          childPlanError.value = e instanceof Error ? e.message : String(e)
      } finally {
        if (!controller.signal.aborted) childPlanLoading.value = false
      }
    }, 250)
  },
  { flush: 'sync' },
)
function beginRefinement() {
  childWidth.value = (spacing.value * Math.min(256, store.session?.max_patch_side || 512)) / 1000
  childScope.value = refinementFeature.value ? 'full' : 'custom'
  childPrecision.value = 'auto'
  childSpacing.value = spacing.value
  childName.value = refinementFeature.value
    ? refinementFeature.value.name + ' · 概览'
    : '自定义精细图'
  refine.value = true
}
function changeChildScope(scope: 'full' | 'custom') {
  if (scope === 'custom' && childPlan.value) childWidth.value = childPlan.value.width_m / 1000
  childScope.value = scope
}
const selectedFacts = computed(() => {
  const i = selection.value?.cell
  if (i === undefined || !store.elevation) return null
  return {
    height: store.elevation[i],
    temp: store.temperature?.[i] || 0,
    rain: store.precipitation?.[i] || 0,
    biome: biomes[store.biome?.[i] || 0],
    flow: (store.flow?.[i] || 0) / 1e6,
  }
})
const selectedIce = computed(() => {
  const i = selection.value?.cell,
    ice = visibleIce.value
  return i !== undefined && ice?.available
    ? {
        land: ice.landIce[i],
        sea: ice.seaIce[i],
        shelf: ice.shelf[i],
        frost: ice.permafrost[i],
        active: ice.activeLayer[i],
      }
    : null
})
const progress = computed(() => Math.round((store.job?.progress || 0) * 100)),
  jobState = computed(
    () =>
      ({
        queued: '排队中',
        running: '正在演化',
        cancelling: '正在取消',
        done: '地图已完成',
        cancelled: '生成已取消',
        failed: '生成未完成',
      })[store.job?.state || ''] || '等待开始',
  )
const mapStats = computed(() => {
  const f = store.elevation
  if (!f) return null
  let land = 0,
    min = Infinity,
    max = -Infinity
  for (const x of f) {
    if (x > 0) land++
    min = Math.min(min, x)
    max = Math.max(max, x)
  }
  return {
    land: Math.round((land / f.length) * 100),
    min: Math.round(min),
    max: Math.round(max),
  }
})
async function guard(fn: () => Promise<unknown>) {
  notice.value = ''
  try {
    await fn()
  } catch (e) {
    if ((e as Error)?.name !== 'AbortError')
      notice.value = e instanceof Error ? e.message : String(e)
  }
}
function startNew() {
  tab.value = 'create'
  projectOpen.value = true
  panel.value = 'project'
}
function collapseProject() {
  projectOpen.value = false
  panel.value = null
}
function openProject() {
  projectOpen.value = true
  panel.value = 'project'
}
function toggleProject() {
  panel.value = panel.value === 'project' ? null : 'project'
  projectOpen.value = true
}
function reload() {
  window.location.reload()
}
function randomSeed() {
  seed.value = String(crypto.getRandomValues(new Uint32Array(1))[0])
}
async function generate() {
  await guard(async () => {
    const n = parseSeed(seed.value)
    if (!name.value.trim()) throw new Error('请填写地图名称')
    const parameters = scopeParameters(
      startScope.value,
      worldShape.value,
      options,
      landscape.value,
      monsoon.value,
      glacier.value,
    )
    Object.assign(parameters, { erosion_strength: erosion.value })
    if (!localScope.value)
      Object.assign(parameters, {
        land_fraction: land.value / 100,
        plate_count: plates.value,
        erosion_strength: erosion.value,
        tectonic_age_myr: evolutionMyr.value,
        tectonic_step_myr: evolutionStepMyr.value,
        plate_speed_min_cm_yr: minPlateSpeed.value,
        plate_speed_max_cm_yr: maxPlateSpeed.value,
        allow_land_at_edge: allowLandAtEdge.value,
      })
    await store.createWorld(name.value.trim(), n, parameters)
    tab.value = 'tree'
    await store.generate(store.current!.id)
  })
}
async function regenerate() {
  if (!store.current) return
  await guard(() => store.generate(store.current!.id))
}
const displaySpacingKm = ref(100)
const actualDisplaySpacing = ref(0)
watch(
  () => store.current?.id,
  () => {
    const preferred = Number(store.current?.params?.display_grid_m ?? 100000)
    displaySpacingKm.value = Number.isFinite(preferred) ? Math.max(0, preferred / 1000) : 100
  },
)
let overlayController: AbortController | undefined
let overlayDebounce: ReturnType<typeof setTimeout> | undefined
let overlayCache: { key: string; lines: Float32Array; spacing: number } | undefined
async function refreshOverlay() {
  overlayController?.abort()
  const node = store.currentMap
  if (!node?.grid || !renderer) return
  const controller = new AbortController()
  overlayController = controller
  const key = `${node.id}:${displaySpacingKm.value}`
  try {
    const result =
      overlayCache?.key === key
        ? overlayCache
        : await api.getOverlay(node.id, displaySpacingKm.value * 1000, controller.signal)
    if (controller.signal.aborted || store.currentMap?.id !== node.id) return
    overlayCache = { key, ...result }
    actualDisplaySpacing.value = result.spacing
    const width = node.grid.type === 'sphere' ? 0 : node.grid.width_m
    const lines = width ? result.lines.map((v) => (v * 2) / width) : result.lines
    renderer.setOverlay(lines)
  } catch (e) {
    if ((e as Error).name !== 'AbortError') notice.value = '显示网格未加载：' + (e as Error).message
  }
}
watch(displaySpacingKm, () => {
  clearTimeout(overlayDebounce)
  overlayDebounce = setTimeout(() => {
    void refreshOverlay()
  }, 200)
})
async function refreshTectonics() {
  tectonicController?.abort()
  const node = store.currentMap
  if (!node || !renderer) return
  const controller = new AbortController()
  tectonicController = controller
  try {
    const data =
      tectonicCache?.id === node.id
        ? tectonicCache.data
        : await api.tectonics(node.id, controller.signal)
    if (
      controller.signal.aborted ||
      node.id !== store.currentMap?.id ||
      layer.value !== 6 ||
      store.generating
    )
      return
    tectonicCache = { id: node.id, data }
    tectonics.value = data
    const scale = node.grid?.type === 'sphere' ? 1 : 2 / (node.grid?.width_m || 2)
    renderer.setDiagnostics((data.groups || []).map((g) => Float32Array.from(g, (v) => v * scale)))
  } catch (e) {
    if (
      !controller.signal.aborted &&
      node.id === store.currentMap?.id &&
      (e as Error).name !== 'AbortError'
    )
      tectonics.value = { available: false, reason: (e as Error).message }
  }
}
async function refreshHydrology() {
  hydrologyController?.abort()
  const node = store.currentMap
  if (!node || !renderer) return
  const controller = new AbortController()
  hydrologyController = controller
  const key = `${node.id}:${iceMonth.value}`
  try {
    const data =
      hydrologyCache?.key === key
        ? hydrologyCache.data
        : await api.hydrology(node.id, controller.signal, iceMonth.value)
    if (
      controller.signal.aborted ||
      store.generating ||
      layer.value !== 10 ||
      store.currentMap?.id !== node.id ||
      key !== `${node.id}:${iceMonth.value}`
    )
      return
    hydrologyCache = { key, data }
    hydrology.value = data
    const scale = node.grid?.type === 'sphere' ? 1 : 2 / (node.grid?.width_m || 2)
    renderer.setDiagnostics((data.groups || []).map((g) => Float32Array.from(g, (v) => v * scale)))
  } catch (e) {
    if (!controller.signal.aborted && (e as Error).name !== 'AbortError')
      hydrology.value = { available: false, reason: (e as Error).message }
  }
}
async function refreshIce() {
  iceController?.abort()
  const node = store.currentMap
  if (!node || !renderer || store.generating) return
  if (iceMonth.value === 0 || store.cryosphere?.available === false) {
    const data = store.cryosphere
    renderer.setCryosphere(data?.available ? data : undefined)
    return
  }
  const key = `${node.id}:${iceMonth.value}`,
    controller = new AbortController()
  iceController = controller
  monthlyIce.value = null
  renderer.setCryosphere()
  try {
    const data = await api.cryosphere(node.id, node.cells, iceMonth.value, controller.signal)
    if (
      controller.signal.aborted ||
      store.generating ||
      store.currentMap?.id !== node.id ||
      key !== `${node.id}:${iceMonth.value}`
    )
      return
    monthlyIce.value = { key, data }
    renderer?.setCryosphere(data.available ? data : undefined)
  } catch (e) {
    if (!controller.signal.aborted && store.currentMap?.id === node.id)
      monthlyIce.value = { key, data: { available: false, reason: (e as Error).message } }
  }
}
async function refreshCirculation() {
  circulationController?.abort()
  const node = store.currentMap
  if (!node || !renderer || store.generating || ![8, 9].includes(layer.value)) return
  const requestedLayer = layer.value
  const key = `${node.id}:${requestedLayer}:${circulationMonth.value}`
  const controller = new AbortController()
  circulationController = controller
  circulation.value = null
  renderer.setDiagnostics([])
  try {
    const data =
      circulationCache?.key === key
        ? circulationCache.data
        : await api.vectors(
            node.id,
            requestedLayer === 8 ? 'wind' : 'current',
            circulationMonth.value,
            controller.signal,
          )
    if (
      controller.signal.aborted ||
      node.id !== store.currentMap?.id ||
      requestedLayer !== layer.value ||
      store.generating
    )
      return
    circulationCache = { key, data }
    circulation.value = data
    const scale = node.grid?.type === 'sphere' ? 1 : 2 / (node.grid?.width_m || 2)
    renderer.setDiagnostics((data.groups || []).map((g) => Float32Array.from(g, (v) => v * scale)))
  } catch (e) {
    if (
      !controller.signal.aborted &&
      node.id === store.currentMap?.id &&
      requestedLayer === layer.value
    )
      circulation.value = { available: false, reason: (e as Error).message }
  }
}
watch(circulationMonth, () => {
  void refreshCirculation()
})
let partitionCache: { key: string; data: ReturnType<typeof partitionBoundaries> } | undefined
function refreshPartitions() {
  if (!renderer || layer.value !== 7 || store.generating) return
  const node = store.currentMap,
    g = store.geography
  renderer.setDiagnostics([])
  partitionTruncated.value = false
  if (!node?.grid || !g || !store.centers || !store.indices) {
    renderer.setPartitionLabels(null)
    return
  }
  const labels = partitionLabels(g, partitionMode.value),
    key = `${node.id}:${partitionMode.value}`
  if (partitionCache?.key !== key)
    partitionCache = {
      key,
      data: partitionBoundaries(
        store.centers,
        store.indices,
        labels,
        node.grid.type === 'sphere',
        node.grid.type === 'sphere' ? 1 : 2 / node.grid.width_m,
      ),
    }
  renderer.setPartitionLabels(labels)
  renderer.setDiagnostics([partitionCache.data.lines])
  partitionTruncated.value = partitionCache.data.truncated
}
watch([() => store.geography, partitionMode], refreshPartitions)
function applyMap() {
  const node = store.currentMap
  if (!renderer) return
  activeFeatureId.value = null
  if (store.generating && liveFrame.value) {
    renderLiveFrame()
    return
  }
  if (!node || !store.centers || !store.indices || !store.elevation || !store.biome) {
    renderer.clear()
    selection.value = null
    return
  }
  let centers = store.centers
  if (node.grid && node.grid.type !== 'sphere') {
    const g = node.grid
    renderer.setPlanarMode()
    centers = centers.map((v) => (v * 2) / g.width_m)
    renderer.planarHalfHeight =
      1.08 *
      Math.max(
        (Math.sqrt(3) / 2) * (g.type === 'plane' ? g.rows / g.size : 1),
        canvas.value!.clientHeight / canvas.value!.clientWidth,
      )
  } else if (worldProjection.value !== 'sphere') renderer.setWorldFlatMode(worldProjection.value)
  else renderer.setSphereMode()
  renderer.setCells({
    centers,
    indices: store.indices,
    heights: store.elevation,
    biomes: store.biome,
    temperatures: store.temperature || undefined,
    precipitations: store.precipitation || undefined,
    flows: store.flow || undefined,
    lakeDepths: store.lakeDepth || undefined,
    cryosphere: visibleIce.value?.available ? visibleIce.value : undefined,
    ecosystems: store.ecosystems?.available ? store.ecosystems : undefined,
    ecosystemMode: ecosystemMode.value,
    plateIds: store.plateIds || undefined,
    count: node.cells,
  })
  renderer.mapMetres =
    node.grid?.type === 'sphere' ? node.grid.radius_m : (node.grid?.width_m || 2) / 2
  renderer.setOverlay(new Float32Array())
  renderer.setDiagnostics([])
  tectonics.value = null
  hydrology.value = null
  void refreshIce()
  if (layer.value === 10) void refreshHydrology()
  if (layer.value === 6) void refreshTectonics()
  if (layer.value === 8 || layer.value === 9) void refreshCirculation()
  else if (layer.value === 7) {
    refreshPartitions()
    void refreshOverlay()
  } else void refreshOverlay()
  renderer.heightScale = 0.025 / Math.max(mapStats.value?.max || 1, 1)
  renderer.layer = layer.value
  renderer.style = style.value
  renderer.exitPreview()
  selection.value = null
  pin.value = null
  refine.value = false
}
watch(() => store.currentMap, applyMap)
watch([() => store.ecosystems, ecosystemMode], () => {
  if (!store.generating)
    renderer?.setEcosystems(
      store.ecosystems?.available ? store.ecosystems : undefined,
      ecosystemMode.value,
    )
})
watch(worldProjection, () => {
  const selected = selection.value
  const feature = activeFeatureId.value
  applyMap()
  if (!store.generating && selected) {
    selection.value = selected
    if (feature) selectGeographic(feature)
    updatePin()
  }
})
watch(
  () => store.generating,
  (active) => {
    if (!active) {
      liveFrame.value = null
      previewContext = ''
      applyMap()
    } else {
      liveFrame.value = null
      previewContext = ''
    }
  },
)
watch(
  () => store.error,
  (message) => {
    if (message) notice.value = message
  },
)
watch(layer, (v) => {
  if (store.generating && liveFrame.value) {
    if (renderer) renderer.layer = liveLayer.value
    return
  }
  tectonicController?.abort()
  circulationController?.abort()
  hydrologyController?.abort()
  renderer?.setDiagnostics([])
  if (v === 6) void refreshTectonics()
  else if (v === 10) void refreshHydrology()
  else if (v === 8 || v === 9) void refreshCirculation()
  else if (v === 7) {
    refreshPartitions()
    void refreshOverlay()
  } else void refreshOverlay()
  if (renderer) renderer.layer = v
})
watch(
  () => store.cryosphere,
  () => void refreshIce(),
)
watch(iceMonth, () => {
  void refreshIce()
  if (layer.value === 10) void refreshHydrology()
})
watch(style, (v) => {
  if (renderer) renderer.style = v
})
watch(next, () => {
  childWidth.value = (spacing.value * Math.min(256, store.session?.max_patch_side || 512)) / 1000
  childName.value = next.value ? '自定义精细图' : ''
})
function chooseAt(p: Vec3) {
  const node = store.currentMap
  if (!node?.grid || !store.centers || !renderer || store.generating) return
  const g = node.grid
  let geo, point: Vec3
  if (g.type !== 'sphere') {
    point = [(p[0] * g.width_m) / 2, (p[1] * g.width_m) / 2, 0]
    const half = g.width_m / 2
    if (
      Math.abs(point[0]) > half ||
      Math.abs(point[1]) > ((half * Math.sqrt(3)) / 2) * (g.type === 'plane' ? g.rows / g.size : 1)
    )
      return
    geo = latLon(
      g.type === 'plane'
        ? flatDirection(
            g.center_lat,
            g.center_lon,
            point[0],
            point[1],
            g.radius_m,
            g.reference_lat ?? g.center_lat,
          )
        : patchDirection(g.center_lat, g.center_lon, point[0], point[1], g.radius_m),
    )
  } else {
    point = p
    geo = latLon(p)
  }
  const i = nearestCell(store.centers, point, g.type === 'sphere')
  if (i < 0) return
  activeFeatureId.value = null
  renderer.setGeographyMask(null)
  selection.value = { ...geo, cell: i }
  manualLat.value = geo.lat.toFixed(5)
  manualLon.value = geo.lon.toFixed(5)
  refine.value = false
  if (innerWidth < 760) panel.value = 'layers'
  updatePin()
}
function selectGeographic(id: number) {
  const geo = store.geography,
    grid = store.currentMap?.grid
  if (!geo || !grid || !renderer || !store.centers || locked.value) return
  const feature = geo.features[id - 1]
  if (!feature) return
  activeFeatureId.value = id
  selection.value = { lat: feature.lat, lon: feature.lon, cell: feature.anchor_cell }
  manualLat.value = feature.lat.toFixed(5)
  manualLon.value = feature.lon.toFixed(5)
  renderer.setGeographyMask(featureMask(geo, feature))
  const i = feature.anchor_cell * 3
  const point: Vec3 =
    grid.type === 'sphere'
      ? direction(feature.lat, feature.lon)
      : [(store.centers[i] * 2) / grid.width_m, (store.centers[i + 1] * 2) / grid.width_m, 0]
  renderer.centerAt(point)
  if (innerWidth < 760) panel.value = 'layers'
  refine.value = false
  updatePin()
}
function manualSelect() {
  const lat = Number(manualLat.value),
    lon = Number(manualLon.value)
  if (!Number.isFinite(lat) || Math.abs(lat) > 90 || !Number.isFinite(lon) || Math.abs(lon) > 180) {
    notice.value = '经度范围 −180～180，纬度范围 −90～90'
    return
  }
  const node = store.currentMap
  if (!node?.grid) return
  const d = direction(lat, lon)
  if (node.grid.type === 'sphere') {
    chooseAt(d)
    renderer?.centerAt(d)
  } else {
    const g = node.grid,
      p =
        g.type === 'plane'
          ? flatProject(g.center_lat, g.center_lon, d, g.radius_m, g.reference_lat ?? g.center_lat)
          : patchProject(g.center_lat, g.center_lon, d, g.radius_m)
    if (!p) {
      notice.value = '坐标超出当前地图'
      return
    }
    const renderPoint: Vec3 = [(p[0] * 2) / g.width_m, (p[1] * 2) / g.width_m, 0]
    chooseAt(renderPoint)
    renderer?.centerAt(renderPoint)
  }
}
function updatePin() {
  const node = store.currentMap,
    s = selection.value
  if (!node?.grid || !s || !renderer) {
    pin.value = null
    return
  }
  const g = node.grid
  const p: Vec3 =
    g.type === 'sphere'
      ? direction(s.lat, s.lon)
      : [
          (store.centers![s.cell * 3] * 2) / g.width_m,
          (store.centers![s.cell * 3 + 1] * 2) / g.width_m,
          0,
        ]
  pin.value = renderer.project(p)
}
async function generateChild(parentId = store.currentMap?.id, kind = next.value) {
  if (!parentId || !kind || !selection.value) throw new Error('请先在父图上选择位置')
  if (store.currentMap?.id !== parentId) throw new Error('请先切换到父图并选择位置')
  if (kind !== next.value) throw new Error('只能生成下一层地图')
  if (refine.value && (childPlanLoading.value || childPlanError.value))
    throw new Error('请先完成范围与精度核验')
  if (refine.value && childPlanKey !== `${parentId}:${JSON.stringify(childQuery())}`)
    throw new Error('范围或精度已变更，请等待重新核验')
  const plan = refine.value ? childPlan.value : await api.refinementPlan(parentId, childQuery())
  if (!plan?.can_generate) throw new Error(plan?.message || '请先核验范围与精度')
  if (store.currentMap?.id !== parentId) throw new Error('父图已切换，请重新选择范围')
  await store.child(parentId, {
    kind,
    center_lat: plan.center_lat,
    center_lon: plan.center_lon,
    width_m: plan.width_m,
    cell_size_m: plan.cell_size_m,
    name: childName.value.trim() || '自定义精细图',
  })
  tab.value = 'tree'
  refine.value = false
}
async function exportFile(format: 'png' | 'json') {
  if (!store.currentMap) return
  busy.value = true
  loadLabel.value = format === 'png' ? '正在绘制地图…' : '正在保存完整工程…'
  try {
    if (format === 'png' && layer.value === 10 && renderer) {
      if (!hydrology.value) await refreshHydrology()
      const blob = await renderer.capturePng(),
        url = URL.createObjectURL(blob)
      const link = document.createElement('a')
      link.href = url
      link.download = (store.current?.name || 'TerraForge') + '-渲染视图.png'
      link.click()
      setTimeout(() => URL.revokeObjectURL(url), 1000)
      return
    }
    await api.download(
      store.currentMap.id,
      format,
      (store.current?.name || 'TerraForge') + (format === 'png' ? '.png' : '.tfworld'),
      layer.value === 3 && store.ecosystems?.available
        ? (['biome', 'group', 'variant'] as const)[ecosystemMode.value]
        : undefined,
    )
  } finally {
    busy.value = false
  }
}
async function importFile(event: Event) {
  const input = event.target as HTMLInputElement,
    file = input.files?.[0]
  if (!file) return
  busy.value = true
  loadLabel.value = '正在还原工程…'
  await guard(async () => {
    const max = store.session?.mode === 'desktop' ? 512 : 64
    if (file.size > max * 1024 * 1024) throw new Error(`当前模式最多导入 ${max} MB 工程`)
    const w = await api.importProject(file)
    await store.refreshWorlds()
    await store.selectWorld(w.id)
    tab.value = 'tree'
  })
  busy.value = false
  input.value = ''
}
async function deleteCurrent() {
  const world = store.current
  if (!world || !confirm(`删除「${world.name}」及其全部地图？请先保存需要的工程。`)) return
  await guard(async () => {
    await api.deleteWorld(world.id)
    store.clearMap()
    store.current = null
    store.maps = []
    await store.refreshWorlds()
    if (store.worlds[0]) await store.selectWorld(store.worlds[0].id)
    else tab.value = 'create'
  })
}
async function endSession() {
  await guard(async () => {
    await api.closeSession()
    ended.value = true
    store.dispose()
    if (heartbeat) clearInterval(heartbeat)
    renderer?.clear()
  })
}
const toolCtx: ToolContext = {
  worldId: () => store.current?.id || null,
  mapId: () => store.currentMap?.id || null,
  maps: () => store.maps,
  elevation: () => store.elevation,
  biome: () => store.biome,
  ecosystems: () => store.ecosystems,
  setEcosystemMode: (mode) => {
    ecosystemMode.value = mode
    layer.value = 3
  },
  selection: () => selection.value,
  geography: () => store.geography,
  selectFeature: selectGeographic,
  generate: async () => {
    if (!store.current) throw new Error('请先创建世界')
    await store.generate(store.current.id)
  },
  drillDown: async (id, kind) => generateChild(id, kind),
  selectMap: async (id) => {
    if (!store.maps.some((m) => m.id === id)) throw new Error('地图不属于当前工程')
    await store.loadMapData(id)
  },
  setLayer: (key) => {
    const v: Record<string, number> = {
      terrain: 0,
      temperature: 1,
      precipitation: 2,
      biome: 3,
      elevation: 4,
      hydrology: 5,
      geography: 7,
      tectonics: 6,
      wind: 8,
      current: 9,
      rendered: 10,
    }
    if (v[key] === undefined) throw new Error('未知图层')
    layer.value = v[key]
  },
  setStyle: (key) => {
    const v: Record<string, MapStyle> = {
      realistic: MapStyle.Realistic,
      watercolor: MapStyle.Watercolor,
      parchment: MapStyle.Parchment,
      contour: MapStyle.Contour,
    }
    if (v[key] === undefined) throw new Error('未知风格')
    style.value = v[key]
    layer.value = 0
  },
}
function leave(e: PageTransitionEvent) {
  if (!e.persisted && store.session?.mode === 'web' && !ended.value) api.leave()
}
function renderLiveFrame() {
  const f = liveFrame.value
  if (!renderer || !canvas.value || !f || !store.generating) return
  const grid = f.grid || store.maps.find((m) => m.id === store.job?.map_id)?.grid
  if (!grid) return
  const context = `${f.job_id}:${grid.type}:${grid.type === 'sphere' ? worldProjection.value : 'local'}`
  if (previewContext !== context) {
    renderer.clear()
    let geometry
    if (grid.type === 'sphere') {
      geometry = previewSphere ||= buildSphereGrid(32)
      if (worldProjection.value === 'sphere') renderer.setSphereMode()
      else renderer.setWorldFlatMode(worldProjection.value)
      renderer.previewHalfHeight = 1
      renderer.mapMetres = grid.radius_m
    } else {
      const aspect = (Math.sqrt(3) / 2) * (grid.type === 'plane' ? grid.rows / grid.size : 1)
      geometry = planarPreviewSurface(aspect)
      renderer.setPlanarMode()
      renderer.previewHalfHeight = aspect
      renderer.planarHalfHeight =
        1.08 * Math.max(aspect, canvas.value.clientHeight / canvas.value.clientWidth)
      renderer.mapMetres = grid.width_m / 2
    }
    renderer.setCells({
      ...geometry,
      heights: new Float32Array(geometry.count),
      biomes: new Uint8Array(geometry.count),
    })
    selection.value = null
    pin.value = null
    previewContext = context
  }
  renderer.heightScale = 0.025 / Math.max(f.stats.max_elevation, 1)
  renderer.layer = liveLayer.value
  renderer.style = style.value
  renderer.uploadFrame(f.pixels, f.w, f.h, f.stats)
}
onMounted(async () => {
  try {
    if (!canvas.value) return
    renderer = new HexSphereRenderer(canvas.value)
    renderer.start()
    interaction = bindMapInput(canvas.value, renderer, chooseAt, updatePin)
    store.onFrame((f) => {
      if (!renderer || !store.generating || store.job?.id !== f.job_id) return
      liveFrame.value = f
      renderLiveFrame()
    })
    await store.init()
    ready.value = true
    if (store.current) tab.value = 'tree'
    heartbeat = setInterval(() => {
      void api.heartbeat().catch((e) => {
        notice.value = '会话连接中断：' + e.message
      })
    }, 30000)
    uiTimer = setInterval(() => {
      updatePin()
      const g = store.currentMap?.grid
      const value = renderer?.measureScale()
      if (!g || value == null) {
        scaleText.value = ''
        return
      }
      const metres = value * (g.type === 'sphere' ? g.radius_m : g.width_m / 2)
      scaleText.value =
        metres >= 1000
          ? (metres / 1000).toLocaleString(undefined, { maximumFractionDigits: 1 }) + ' km'
          : metres.toFixed(1) + ' m'
    }, 100)
    window.addEventListener('pagehide', leave)
  } catch (e) {
    notice.value = e instanceof Error ? e.message : String(e)
    loadLabel.value = '工作台尚未连接；请刷新重试'
  }
})
onBeforeUnmount(() => {
  clearTimeout(childPlanTimer)
  childPlanController?.abort()
  iceController?.abort()
  hydrologyController?.abort()
  tectonicController?.abort()
  circulationController?.abort()
  store.dispose()
  renderer?.dispose()
  interaction?.abort()
  clearInterval(heartbeat)
  clearInterval(uiTimer)
  clearTimeout(overlayDebounce)
  overlayController?.abort()
  window.removeEventListener('pagehide', leave)
})
</script>

<template>
  <main class="workbench">
    <canvas
      ref="canvas"
      class="map-canvas"
      tabindex="0"
      aria-label="交互地图：拖动旋转或平移，滚轮缩放，点击选择位置；方向键移动，Enter 选择中心，Home 复位"
    />
    <div class="paper-texture" aria-hidden="true" />
    <div v-if="!hasMap && !store.generating" class="blank-atlas" aria-hidden="true">
      <svg viewBox="0 0 700 700">
        <defs>
          <clipPath id="globeClip">
            <circle cx="350" cy="350" r="252" />
          </clipPath>
        </defs>
        <g fill="none" stroke="currentColor" stroke-width=".8">
          <circle cx="350" cy="350" r="252" />
          <circle cx="350" cy="350" r="270" stroke-dasharray="2 7" />
          <g clip-path="url(#globeClip)">
            <ellipse cx="350" cy="350" rx="80" ry="252" />
            <ellipse cx="350" cy="350" rx="178" ry="252" />
            <path d="M98 350H602M350 98V602" />
            <ellipse cx="350" cy="350" rx="252" ry="84" />
            <ellipse cx="350" cy="350" rx="252" ry="176" />
          </g>
          <path d="M350 55V80M350 620V645M55 350H80M620 350H645" />
        </g>
      </svg>
      <div>
        <span class="eyebrow">A WORLD, WAITING TO UNFOLD</span>
        <h1>绘一颗世界。</h1>
        <p>
          从板块的相遇，到河流的去向。
          <br />
          创建你的第一份地理手记。
        </p>
      </div>
    </div>
    <header class="paper-panel topbar">
      <a class="brand" href="#" @click.prevent="renderer?.resetView()">
        <svg viewBox="0 0 36 36" aria-hidden="true">
          <circle cx="18" cy="18" r="15" />
          <path d="m18 5 5 13-5 13-5-13Zm-13 13h26M18 3v30" />
        </svg>
        <span>
          TerraForge
          <small>世界地理工作台</small>
        </span>
      </a>
      <div class="top-divider" />
      <div class="project-title">
        <strong>{{ store.current?.name || '未命名的探索' }}</strong>
        <span>
          <i class="status-dot" />
          {{ modeText }}
        </span>
      </div>
      <nav aria-label="工程工具">
        <select
          v-if="sphericalWorld"
          v-model="worldProjection"
          aria-label="世界投影"
          :disabled="store.loading"
          :title="projectionHint"
          class="projection-select"
        >
          <option value="sphere">球面视图</option>
          <option v-for="projection in projections" :key="projection.id" :value="projection.id">
            {{ projection.label }}
          </option>
        </select>
        <button :disabled="!ready || store.generating" @click="startNew">＋ 新建</button>
        <button :disabled="locked" @click="fileInput?.click()">打开工程</button>
        <button :disabled="locked || !hasMap" @click="guard(() => exportFile('json'))">
          保存工程
        </button>
        <button :disabled="locked || !hasMap" @click="guard(() => exportFile('png'))">
          {{ layer === 10 ? '导出渲染视图 ↗' : '导出地图 ↗' }}
        </button>
        <button
          class="ai-button"
          :aria-expanded="panel === 'ai'"
          @click="panel = panel === 'ai' ? 'layers' : 'ai'"
        >
          ✧ 地理助手
        </button>
      </nav>
      <input
        ref="fileInput"
        type="file"
        accept=".tfworld,.json,application/json"
        hidden
        @change="importFile"
      />
    </header>
    <div v-if="notice" class="notice paper-panel" role="alert">
      <span>{{ notice }}</span>
      <button aria-label="关闭提示" @click="notice = ''">×</button>
    </div>
    <div v-if="hasMap" class="map-caption">
      <span class="eyebrow">{{ kinds[store.currentMap!.kind] }} · {{ activeLayer.name }}</span>
      <h1>{{ store.currentMap?.name || store.current?.name }}</h1>
      <span class="mono">
        {{ store.currentMap?.cells.toLocaleString() }} 采样点 ·
        {{
          store.currentMap!.cell_size_m >= 1000
            ? (store.currentMap!.cell_size_m / 1000).toFixed(1) + ' km'
            : store.currentMap!.cell_size_m.toFixed(0) + ' m'
        }}
        / 格
      </span>
    </div>
    <aside
      v-if="projectOpen"
      class="paper-panel project-panel"
      :class="{ 'mobile-hidden': panel !== 'project' }"
      aria-label="工程与创建"
    >
      <header class="panel-heading">
        <span class="eyebrow">FIELD NOTES / 地理手记</span>
        <button class="icon-button" aria-label="收起工程面板" @click="collapseProject">−</button>
      </header>
      <div class="tabs" role="tablist" aria-label="工程面板">
        <button role="tab" :aria-selected="tab === 'tree'" @click="tab = 'tree'">
          地图册
          <small>{{ store.maps.length || '—' }}</small>
        </button>
        <button role="tab" :aria-selected="tab === 'create'" @click="tab = 'create'">
          创建世界
        </button>
      </div>
      <div class="panel-body">
        <template v-if="tab === 'tree'">
          <label class="world-switch">
            当前工程
            <select
              :value="store.current?.id || ''"
              :disabled="locked"
              @change="guard(() => store.selectWorld(($event.target as HTMLSelectElement).value))"
            >
              <option value="" disabled>还没有工程</option>
              <option v-for="w in store.worlds" :key="w.id" :value="w.id">
                {{ w.name }}
              </option>
            </select>
          </label>
          <MapTree
            :maps="store.maps"
            :current-id="store.currentMap?.id || null"
            :disabled="locked"
            @select="(id) => guard(() => store.loadMapData(id))"
          />
          <FeatureTree
            v-if="hasMap && !store.generating"
            :geography="store.geography"
            :loading="store.geographyLoading"
            :error="store.geographyError"
            :selected="activeFeatureId"
            :disabled="locked"
            @select="selectGeographic"
          />
          <div v-if="mapStats" class="world-facts">
            <span>
              陆地
              <b>{{ mapStats.land }}%</b>
            </span>
            <span>
              最高
              <b>{{ mapStats.max.toLocaleString() }} m</b>
            </span>
            <span>
              最深
              <b>{{ mapStats.min.toLocaleString() }} m</b>
            </span>
          </div>
          <div v-if="store.current" class="project-actions">
            <button :disabled="locked" @click="regenerate">重新生成总图</button>
            <button :disabled="locked" class="danger-link" @click="deleteCurrent">删除工程</button>
          </div>
        </template>
        <form id="world-create" v-else class="create-form" @submit.prevent="generate">
          <div class="intro">
            <span class="eyebrow">01 / BEGIN A WORLD</span>
            <h2>山海，由此开始。</h2>
            <p>设定起点，观察地形与气候如何成形。</p>
          </div>
          <label>
            起始范围
            <select v-model="startScope">
              <option value="world">世界</option>
              <option value="continent">独立大陆</option>
              <option value="region">独立区域</option>
              <option value="site">独立地点</option>
            </select>
            <small>
              {{
                localScope
                  ? '直接生成局部地貌，使用指定的温度和降雨背景。'
                  : startScope === 'continent'
                    ? '以一块主要大陆为目标，设置海岸、内陆与构造。'
                    : '生成完整星球或独立平面世界。'
              }}
            </small>
          </label>
          <label>
            地图名称
            <input v-model="name" maxlength="80" required placeholder="给这颗世界起个名字" />
          </label>
          <label>
            世界种子
            <span class="input-action">
              <input
                v-model="seed"
                inputmode="numeric"
                pattern="[0-9]*"
                placeholder="留空，每次自动随机"
              />
              <button type="button" aria-label="随机选择种子" @click="randomSeed">随机</button>
            </span>
            <small>
              留空会自动随机；填入固定值可复现地图。
              <template v-if="store.current">当前工程种子：{{ store.current.seed }}</template>
            </small>
          </label>
          <label v-if="!localScope" class="range-label">
            目标陆地占比
            <b>{{ land }}%</b>
            <input v-model.number="land" type="range" min="5" max="90" step="1" />
            <span>
              <small>群岛星球</small>
              <small>辽阔大陆</small>
            </span>
          </label>
          <details :open="localScope">
            <summary>更多生成参数</summary>
            <label v-if="startScope === 'world'">
              世界形态
              <select v-model="worldShape">
                <option value="sphere">球形世界</option>
                <option value="plane">平面世界</option>
              </select>
            </label>
            <template v-if="!localScope && (startScope === 'continent' || worldShape === 'plane')">
              <label class="check-label">
                <input v-model="allowLandAtEdge" type="checkbox" />
                允许陆地被地图边缘截断
              </label>
              <p class="muted">关闭后会保留海洋边缘，让大陆海岸完整留在图内。</p>
            </template>
            <template v-if="startScope !== 'world' || worldShape === 'plane'">
              <label v-for="field in extentFields" :key="field.key">
                {{ field.label }}
                <input
                  v-model.number="options[field.key]"
                  type="number"
                  :min="field.min"
                  :max="field.max"
                  :step="field.step"
                  required
                />
              </label>
            </template>
            <template v-if="localScope">
              <label>
                主要地貌
                <select v-model="landscape">
                  <option value="hills">丘陵</option>
                  <option value="mountains">山地</option>
                  <option value="plains">平原</option>
                  <option value="valley">河谷</option>
                  <option value="coast">海岸</option>
                </select>
              </label>
              <label v-for="field in localFields" :key="field.key">
                {{ field.label }}
                <input
                  v-model.number="options[field.key]"
                  type="number"
                  :min="field.min"
                  :max="field.max"
                  :step="field.step"
                  required
                />
              </label>
              <p class="muted">
                {{
                  startScope === 'site'
                    ? '地点采用更小的默认范围与起伏；适合河谷、营地等局部场景。'
                    : '区域会形成局部山谷与流域；更改基准温度不会生成另一颗星球。'
                }}
              </p>
            </template>
            <template v-if="!localScope">
              <label>
                初始板块数量
                <input v-model.number="plates" type="number" min="8" max="30" required />
              </label>
              <label>
                总演化时间 · 百万年
                <input
                  v-model.number="evolutionMyr"
                  type="number"
                  min="0"
                  max="4600"
                  step="1"
                  required
                />
                <small>4600 百万年 = 46 亿年；长期块体历史与近期精细计算分别记录。</small>
              </label>
              <label>
                历史推进步长 · 百万年
                <input
                  v-model.number="evolutionStepMyr"
                  type="number"
                  min="0.1"
                  max="50"
                  step="0.1"
                  required
                />
                <small>
                  约
                  {{ Math.ceil(evolutionMyr / Math.max(0.1, evolutionStepMyr)) }}
                  个历史间隔；内部自动细分，实时预览保留实际计算时刻。
                </small>
              </label>
              <label>
                初始板速下限 · 厘米/年
                <input
                  v-model.number="minPlateSpeed"
                  type="number"
                  min="0"
                  :max="maxPlateSpeed"
                  step="0.1"
                  required
                />
              </label>
              <label>
                板速上限 · 厘米/年
                <input
                  v-model.number="maxPlateSpeed"
                  type="number"
                  :min="Math.max(0.1, minPlateSpeed)"
                  max="20"
                  step="0.1"
                  required
                />
                <small>相对生成器的固定地幔参考系；旋转板块不同位置的速度不同。</small>
              </label>
              <label v-for="field in terrainFields" :key="field.key">
                {{ field.label }}
                <input
                  v-model.number="options[field.key]"
                  type="number"
                  :min="field.min"
                  :max="field.max"
                  :step="field.step"
                  required
                />
              </label>
              <label v-for="field in weatherFields" :key="field.key">
                {{ field.label }}
                <input
                  v-model.number="options[field.key]"
                  type="number"
                  :min="field.min"
                  :max="field.max"
                  :step="field.step"
                  required
                />
              </label>
              <label class="check-label">
                <input v-model="monsoon" type="checkbox" />
                季风与季节差异
              </label>
            </template>
            <label>
              侵蚀强度
              <input v-model.number="erosion" type="number" min="0" max="2" step="0.1" required />
            </label>
            <label class="check-label">
              <input v-model="glacier" type="checkbox" />
              冰川侵蚀
            </label>
            <label v-if="startScope === 'world'" v-for="field in planetFields" :key="field.key">
              {{ field.label }}
              <input
                v-model.number="options[field.key]"
                type="number"
                :min="field.min"
                :max="field.max"
                :step="field.step"
                required
              />
            </label>
            <label>
              显示格距 · km
              <input
                v-model.number="options.grid_km"
                type="number"
                min="0"
                max="10000"
                step="0.001"
                required
              />
              <small>只影响叠加格网；0为隐藏，不改变地形或计算精度。</small>
            </label>
          </details>
          <div class="quality-note">
            <span class="status-dot" />
            <div>
              {{
                store.session?.quality === 'economy'
                  ? '经济精度'
                  : store.session?.quality === 'high'
                    ? '高精度'
                    : '标准精度'
              }}
              <small>
                {{
                  store.session?.mode === 'desktop'
                    ? '保存在本机，可离线打开'
                    : '网页版按资源预算生成，局部图最多 ' +
                      (store.session?.max_patch_side || 512) +
                      ' × ' +
                      (store.session?.max_patch_side || 512) +
                      ' 格'
                }}
              </small>
            </div>
          </div>
        </form>
      </div>
      <footer class="panel-footer">
        <button
          class="primary start-button"
          type="submit"
          form="world-create"
          v-if="tab === 'create'"
          :disabled="locked"
        >
          {{ store.generating ? '世界正在成形…' : '开始演化' }}
          <span>→</span>
        </button>

        <template v-if="store.session?.mode === 'web'">
          体验数据在离开后清理。保存工程，留住这次探索。
          <button :disabled="locked" class="text-button" @click="endSession">结束本次体验</button>
        </template>
        <template v-else>工程保存在本机。也可以导出 .tfworld 文件备份。</template>
      </footer>
    </aside>
    <button v-else class="paper-panel reopen-project" @click="openProject">☷ 工程与创建</button>
    <div class="north-mark" aria-hidden="true">
      <span>N</span>
      <svg viewBox="0 0 32 48">
        <path d="m16 2 10 38-10-9L6 40Z" />
        <path d="M16 2v29" />
      </svg>
    </div>
    <div
      v-if="pin"
      class="selection-pin"
      :style="{ left: pin.x + 'px', top: pin.y + 'px' }"
      aria-hidden="true"
    >
      <i />
      <span>{{ selection?.lat.toFixed(2) }}°, {{ selection?.lon.toFixed(2) }}°</span>
    </div>
    <div
      v-if="panel !== 'ai'"
      class="right-stack"
      :class="{ 'mobile-hidden': panel !== 'layers', 'ecosystem-open': layer === 3 }"
    >
      <aside class="paper-panel layers-panel" aria-label="地图图层">
        <span class="eyebrow">MAP LAYERS / 图层</span>
        <div role="radiogroup" aria-label="地图图层">
          <button
            v-for="l in layers"
            :key="l.id"
            role="radio"
            :aria-checked="layer === l.id"
            :class="{ active: layer === l.id }"
            :disabled="!hasMap && !store.generating"
            @click="layer = l.id"
          >
            <span>{{ l.icon }}</span>
            {{ l.name }}
            <i v-if="layer === l.id">●</i>
          </button>
        </div>
        <EcosystemPanel
          v-if="layer === 3"
          v-model:mode="ecosystemMode"
          :data="store.ecosystems"
          :generating="store.generating"
          :has-map="hasMap"
        />
        <p v-if="layer === 10" class="layer-help" role="status">
          {{
            store.generating
              ? '实时预览使用当前已计算的地形与气候；最终生态和河道会在完成后显示。'
              : hydrology?.available
                ? `地形明暗、生态植被与真实河道；${hydrology.segments || 0} 段河流，湖泊随地形保存。`
                : hydrology?.reason || '正在读取水系…'
          }}
        </p>
        <p v-if="layer === 10 && hydrology?.notice" class="layer-help" role="status">
          {{ hydrology.notice }}
        </p>
        <template v-if="layer === 10 || layer === 11">
          <label class="style-select">
            冰雪时段
            <select
              v-model.number="iceMonth"
              :disabled="store.generating || !hasMap"
              aria-label="冰雪月份"
            >
              <option :value="0">常年陆冰 / 年均水面冰</option>
              <option v-for="month in 12" :key="month" :value="month">{{ month }} 月</option>
            </select>
          </label>
          <p class="layer-help" role="status">{{ iceStatus }}</p>
          <p v-if="layer === 10 && hydrology?.liquid_support" class="layer-help">
            浅青河段表示季节冻结或融水河道；冰盖核心不绘制普通地表河。
          </p>
        </template>
        <p v-if="layer === 6" class="layer-help" role="status">
          {{ plateStatus }}
        </p>
        <template v-if="layer === 7">
          <label class="style-select">
            分区内容
            <select v-model="partitionMode" aria-label="自动分区内容">
              <option v-for="mode in partitionModes" :key="mode.id" :value="mode.id">
                {{ mode.label }}
              </option>
            </select>
          </label>
          <p class="layer-help" role="status">
            {{
              store.generating
                ? '生成完成后自动识别分区；当前显示真实地形预览。'
                : store.geographyError ||
                  (store.geographyLoading ? '正在读取自动分区…' : partitionHint)
            }}
          </p>
          <p v-if="partitionTruncated" class="layer-help">
            边界较多，仅显示部分细线；分区颜色完整保留。
          </p>
        </template>
        <template v-if="layer === 8 || layer === 9">
          <label class="style-select">
            时段
            <select v-model.number="circulationMonth" aria-label="环流月份">
              <option :value="0">年平均</option>
              <option v-for="month in 12" :key="month" :value="month">{{ month }} 月</option>
            </select>
          </label>
          <p class="layer-help" role="status">{{ circulationStatus }}</p>
          <p v-if="layer === 9 && circulation?.available" class="layer-help">
            风驱表层流近似；{{
              circulation.heat_coupled ? '已参与海洋热量搬运。' : '局部气候仍以预设值为准。'
            }}
          </p>
        </template>
        <label class="style-select">
          显示网格
          <select
            v-model.number="displaySpacingKm"
            :disabled="!hasMap"
            aria-label="六角显示网格间距"
          >
            <option
              v-if="![0, 0.001, 0.01, 0.1, 1, 10, 50, 100, 250, 500].includes(displaySpacingKm)"
              :value="displaySpacingKm"
            >
              自定义 · {{ displaySpacingKm }} km
            </option>
            <option :value="0">隐藏网格</option>
            <option :value="0.001">1 m / 格</option>
            <option :value="0.01">10 m / 格</option>
            <option :value="0.1">100 m / 格</option>
            <option :value="1">1 km / 格</option>
            <option :value="10">10 km / 格</option>
            <option :value="50">50 km / 格</option>
            <option :value="100">100 km / 格</option>
            <option :value="250">250 km / 格</option>
            <option :value="500">500 km / 格</option>
          </select>
        </label>
        <small v-if="actualDisplaySpacing && displaySpacingKm">
          实际显示格距约
          {{
            actualDisplaySpacing >= 1000
              ? (actualDisplaySpacing / 1000).toFixed(1) + ' km'
              : actualDisplaySpacing.toFixed(1) + ' m'
          }}
        </small>
        <label v-if="layer === 0" class="style-select">
          地形图笔触
          <select v-model.number="style" aria-label="地图视觉风格">
            <option :value="MapStyle.Parchment">纸张地形</option>
            <option :value="MapStyle.Realistic">自然设色</option>
            <option :value="MapStyle.Watercolor">淡彩</option>
            <option :value="MapStyle.Contour">等高线</option>
          </select>
        </label>
      </aside>
      <aside class="paper-panel selection-panel" aria-label="选区信息">
        <span class="eyebrow">SELECTED LANDSCAPE</span>
        <h2>{{ activeFeature?.name || (selection ? '此处的地理' : '为探索，选一个落点。') }}</h2>
        <template v-if="selectedFacts && selection">
          <p class="coordinates mono">
            {{ Math.abs(selection.lat).toFixed(3) }}° {{ selection.lat >= 0 ? 'N' : 'S' }} ·
            {{ Math.abs(selection.lon).toFixed(3) }}°
            {{ selection.lon >= 0 ? 'E' : 'W' }}
          </p>
          <p v-if="store.geographyLoading" class="empty-note">正在读取地理归属…</p>
          <div v-if="selectedGeography.length" class="geography-breadcrumb">
            <button
              v-for="f in selectedGeography"
              :key="f.id"
              :disabled="locked"
              @click="selectGeographic(f.id)"
            >
              {{ f.name }} ›
            </button>
          </div>
          <p v-if="activeFeature" class="empty-note">
            {{ featureKinds[activeFeature.kind] }} ·
            {{
              (activeFeature.area_m2 / 1e6).toLocaleString(undefined, { maximumFractionDigits: 1 })
            }}
            km²
            {{ activeFeature.partial ? ' · 当前地图内的部分范围' : '' }}
            <span v-if="activeFeature.prominence_m != null">
              · 相对高差 {{ Math.round(activeFeature.prominence_m) }} m
            </span>
            <span v-if="activeFeature.drainage_area_m2 != null">
              · 汇水面积
              {{
                (activeFeature.drainage_area_m2 / 1e6).toLocaleString(undefined, {
                  maximumFractionDigits: 1,
                })
              }}
              km²
            </span>
          </p>
          <dl class="selection-facts">
            <div>
              <dt>高程</dt>
              <dd>
                {{ Math.round(selectedFacts.height).toLocaleString() }}
                <small>m</small>
              </dd>
            </div>
            <div>
              <dt>年均温</dt>
              <dd>
                {{ selectedFacts.temp.toFixed(1) }}
                <small>℃</small>
              </dd>
            </div>
            <div>
              <dt>年降水</dt>
              <dd>
                {{ Math.round(selectedFacts.rain).toLocaleString() }}
                <small>mm</small>
              </dd>
            </div>
            <div v-if="!selectedEcosystem">
              <dt>生态</dt>
              <dd>{{ selectedFacts.biome }}</dd>
            </div>
            <template v-if="selectedEcosystem">
              <div class="ecosystem-fact">
                <dt>群系大类</dt>
                <dd>{{ selectedEcosystem.biome }}</dd>
              </div>
              <div class="ecosystem-fact">
                <dt>详细类型</dt>
                <dd>
                  {{ selectedEcosystem.group }}
                  <small v-if="selectedEcosystem.group_code">
                    {{ selectedEcosystem.group_code }}
                  </small>
                </dd>
              </div>
              <div class="ecosystem-fact">
                <dt>地方环境</dt>
                <dd>{{ selectedEcosystem.environment }}</dd>
              </div>
              <div class="ecosystem-fact">
                <dt>识别程度</dt>
                <dd>{{ selectedEcosystem.identification }}</dd>
              </div>
              <div v-if="selectedEcosystem.habitat" class="ecosystem-fact">
                <dt>局部生境</dt>
                <dd>
                  {{ selectedEcosystem.habitat.biome }} · {{ selectedEcosystem.habitat.group }}
                  <small>{{ selectedEcosystem.habitat.code }}</small>
                </dd>
              </div>
              <div class="ecosystem-fact">
                <dt>可用依据</dt>
                <dd>{{ selectedEcosystem.basis.join('；') }}</dd>
              </div>
            </template>
            <template v-if="selectedIce">
              <div v-if="selectedIce.land > 0.1">
                <dt>陆冰厚度估计</dt>
                <dd>
                  {{ Math.round(selectedIce.land).toLocaleString() }}
                  <small>m</small>
                </dd>
              </div>
              <div v-if="selectedFacts.height <= 0">
                <dt>{{ iceMonth ? '海冰覆盖' : '年均海冰' }}</dt>
                <dd>
                  {{ Math.round(selectedIce.sea * 100) }}
                  <small>%</small>
                </dd>
              </div>
              <div v-if="selectedIce.shelf > 0.1">
                <dt>浮冰架厚度估计</dt>
                <dd>
                  {{ Math.round(selectedIce.shelf).toLocaleString() }}
                  <small>m</small>
                </dd>
              </div>
              <div v-if="selectedFacts.height > 0 && selectedIce.land < 1">
                <dt>冻土估计</dt>
                <dd>
                  {{
                    selectedIce.frost > 0.5
                      ? '多年冻土'
                      : selectedIce.frost > 0
                        ? '冻土边缘'
                        : '未估计有多年冻土'
                  }}
                </dd>
              </div>
              <div v-if="selectedIce.frost > 0.1">
                <dt>夏季解冻层</dt>
                <dd>
                  {{ selectedIce.active.toFixed(2) }}
                  <small>m</small>
                </dd>
              </div>
            </template>
            <div v-if="selectedPlate && layer === 6">
              <dt>板块 {{ selectedPlate.id + 1 }}</dt>
              <dd>
                {{ selectedPlate.speed.toFixed(2) }}
                <small>cm/年</small>
              </dd>
            </div>
            <div v-if="selectedPlate?.identity && layer === 6">
              <dt>独立运动开始</dt>
              <dd>
                {{ selectedPlate.identity.born_myr.toFixed(0) }}
                <small>百万年 / 自演化起点</small>
              </dd>
            </div>
            <div v-if="selectedPlate?.identity && layer === 6">
              <dt>当前 / 历史最大面积</dt>
              <dd>
                {{ (selectedPlate.identity.area_m2 / 1e12).toFixed(2) }} /
                {{ (selectedPlate.identity.maximum_area_m2 / 1e12).toFixed(2) }}
                <small>百万 km²</small>
              </dd>
            </div>
          </dl>
          <button
            v-if="next && !refine"
            class="primary"
            :disabled="locked"
            @click="beginRefinement"
          >
            提高此处精度 →
          </button>
          <RefinementPanel
            v-if="refine && next"
            :source="store.currentMap?.name || kinds[store.currentMap?.kind || '']"
            :kind="kinds[next]"
            :feature="refinementFeature?.name || null"
            :scope="childScope"
            @update:scope="changeChildScope"
            v-model:precision="childPrecision"
            v-model:width="childWidth"
            v-model:spacing="childSpacing"
            v-model:name="childName"
            :plan="childPlan"
            :loading="childPlanLoading"
            :error="childPlanError"
            :locked="locked"
            @submit="guard(() => generateChild())"
          />
          <p v-else-if="!next" class="hint">已到地点层，可继续查看各项数据。</p>
        </template>
        <p v-else class="muted">
          点击地图查看海拔、温度与降水。
          <br />
          从这里，进入更细的地图。
        </p>
        <details v-if="hasMap">
          <summary>按经纬度选择</summary>
          <form class="coordinate-form" @submit.prevent="manualSelect">
            <label>
              纬度
              <input v-model="manualLat" type="number" min="-90" max="90" step="any" required />
            </label>
            <label>
              经度
              <input v-model="manualLon" type="number" min="-180" max="180" step="any" required />
            </label>
            <button>选择</button>
          </form>
        </details>
      </aside>
    </div>
    <AgentPanel v-if="panel === 'ai'" :tool-ctx="toolCtx" @close="panel = 'layers'" />
    <div class="map-controls paper-panel">
      <button aria-label="放大地图" @click="renderer?.zoom(180)">＋</button>
      <button aria-label="缩小地图" @click="renderer?.zoom(-180)">−</button>
      <button aria-label="复位地图视图" @click="renderer?.resetView()">⌖</button>
    </div>
    <div v-if="hasMap || store.generating" class="map-legend">
      <div v-if="scaleText" class="distance-scale">
        <i />
        {{ scaleText }}
      </div>
      <span>
        {{
          store.generating &&
          liveFrame?.meta?.month &&
          liveFrame.meta.detail.includes('本月累计') &&
          layer === 2
            ? '本月累计降水（mm；求解中）'
            : liveLayer === 11 && iceMonth
              ? `${iceMonth}月冰雪 · 冻土估计`
              : liveLayer === 3
                ? store.ecosystems?.available
                  ? `${ecosystemModes[ecosystemMode].name}生态 · 图例见图层面板`
                  : '原14类兼容生态'
                : activeLayer.unit
        }}
      </span>
      <p v-if="liveLayer === 7" class="partition-key">
        <i />
        相邻区域分色 · 细线为边界
      </p>
      <i
        v-else-if="liveLayer !== 3 || !store.ecosystems?.available"
        :style="{ background: activeLayer.gradient }"
      />
      <div v-if="liveLayer !== 7 && (liveLayer !== 3 || !store.ecosystems?.available)">
        <span v-for="end in activeLayer.ends" :key="end">{{ end }}</span>
      </div>
      <small>
        {{
          store.currentMap?.grid?.type === 'patch'
            ? '局部切平面 · 北向上'
            : !sphericalWorld &&
                (store.currentMap?.grid?.type === 'plane' || liveFrame?.grid?.type === 'plane')
              ? '平面世界 · 北向上'
              : worldProjection !== 'sphere'
                ? `世界展开 · ${projections.find((p) => p.id === worldProjection)?.label}`
                : '球面视图 · 拖动旋转'
        }}
        · {{ layer === 10 ? '自然彩绘' : layer === 0 && style === 2 ? '纸张地形' : '地理地图' }}
      </small>
    </div>
    <section
      v-if="store.generating || store.job"
      class="paper-panel generation-bar"
      :class="{ settled: !store.generating }"
      aria-label="生成进度"
    >
      <div>
        <div class="generation-label">
          <strong>{{ jobState }}</strong>
          <span>{{ store.job?.stage || '等待任务启动' }} · {{ progress }}%</span>
        </div>
        <progress :value="store.job?.progress || 0" max="1" :aria-label="jobState" />
        <div v-if="store.generating" class="generation-steps live-clock">
          <span>{{ liveCaption }}</span>
          <span v-if="liveFrame && layer !== liveLayer">当前图层尚未就绪，显示地形预览</span>
        </div>
      </div>
      <button
        v-if="store.generating"
        :disabled="store.job?.state === 'cancelling'"
        @click="guard(store.cancel)"
      >
        {{ store.job?.state === 'cancelling' ? '取消中…' : '取消' }}
      </button>
      <button v-else class="icon-button" aria-label="收起生成结果" @click="store.job = null">
        ×
      </button>
    </section>
    <div v-if="busy || store.loading || !ready" class="loading-chip paper-panel" role="status">
      <i class="spinner" />
      {{ store.loading ? '正在展开地图…' : loadLabel }}
    </div>
    <nav class="mobile-nav paper-panel" aria-label="移动工作台面板">
      <button @click="toggleProject">☷ 工程</button>
      <button @click="panel = panel === 'layers' ? null : 'layers'">◈ 图层与选区</button>
      <button @click="panel = panel === 'ai' ? null : 'ai'">✧ AI</button>
    </nav>
    <div v-if="ended" class="ended-screen">
      <div class="paper-panel">
        <span class="eyebrow">FIELD NOTES / 已结束</span>
        <h1>这次探索已收笔。</h1>
        <p>
          临时世界与地图已清理。
          <br />
          保存的工程文件仍可在下一次探索中打开。
        </p>
        <button class="primary" @click="reload">开始新的探索 →</button>
      </div>
    </div>
  </main>
</template>
