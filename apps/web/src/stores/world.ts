import { defineStore } from 'pinia'
import { ref, shallowRef } from 'vue'
import {
  api,
  type World,
  type MapNode,
  type JobStatus,
  type Session,
  type ChildRequest,
} from '@/api/client'
import { openEventStream, type DecodedFrameEvent } from '@/api/sse'
import type { Geography } from '@/geography/catalog'
import type { IceResponse } from '@/cryosphere/data'
export const useWorldStore = defineStore('world', () => {
  const session = ref<Session | null>(null),
    worlds = ref<World[]>([]),
    current = ref<World | null>(null),
    maps = ref<MapNode[]>([]),
    currentMap = ref<MapNode | null>(null)
  const job = ref<JobStatus | null>(null),
    generating = ref(false),
    loading = ref(false),
    error = ref('')
  const centers = shallowRef<Float32Array | null>(null),
    elevation = shallowRef<Float32Array | null>(null),
    biome = shallowRef<Uint8Array | null>(null),
    temperature = shallowRef<Float32Array | null>(null),
    precipitation = shallowRef<Float32Array | null>(null),
    flow = shallowRef<Float32Array | null>(null),
    lakeDepth = shallowRef<Float32Array | null>(null),
    plateIds = shallowRef<Float32Array | null>(null)
  const indices = shallowRef<Uint32Array | null>(null)
  const cryosphere = shallowRef<IceResponse | null>(null)
  const geography = shallowRef<Geography | null>(null),
    geographyLoading = ref(false),
    geographyError = ref('')
  let loadController: AbortController | null = null,
    closed = false,
    stopStream: (() => void) | null = null
  let frameHandler: ((f: DecodedFrameEvent) => void) | undefined
  async function init() {
    session.value = await api.openSession()
    await refreshWorlds()
    if (worlds.value[0]) await selectWorld(worlds.value[0].id)
  }
  async function refreshWorlds() {
    worlds.value = await api.listWorlds()
  }
  async function refreshMaps() {
    if (current.value) maps.value = await api.listMaps(current.value.id)
  }
  function clearMap() {
    loadController?.abort()
    currentMap.value = null
    centers.value = elevation.value = temperature.value = precipitation.value = flow.value = null
    plateIds.value = null
    lakeDepth.value = null
    cryosphere.value = null
    biome.value = null
    indices.value = null
    geography.value = null
    geographyLoading.value = false
    geographyError.value = ''
  }
  async function selectWorld(id: string) {
    if (generating.value) throw new Error('请先等待或取消当前生成')
    clearMap()
    current.value = await api.getWorld(id)
    await refreshMaps()
    const candidates = maps.value
      .filter((m) => m.status === 'done' && m.grid)
      .sort((a, b) => Number(b.parent === null) - Number(a.parent === null))
    for (const map of candidates) {
      try {
        await loadMapData(map.id)
        return
      } catch (e) {
        error.value = (e as Error).message
      }
    }
    if (maps.value.some((m) => m.status === 'done' && !m.grid))
      error.value = '旧地图缺少空间信息，可以在此工程重新生成。'
  }
  async function createWorld(
    name: string,
    seed: number | undefined,
    params?: Record<string, unknown>,
  ) {
    const w = await api.createWorld(name, seed, params)
    await refreshWorlds()
    clearMap()
    current.value = await api.getWorld(w.id)
    maps.value = []
    return w
  }
  async function loadMapData(id: string) {
    loadController?.abort()
    const controller = new AbortController()
    loadController = controller
    loading.value = true
    geography.value = null
    geographyLoading.value = false
    geographyError.value = ''
    try {
      const node = await api.getMap(id)
      if (node.status !== 'done') throw new Error('这张地图尚未完成')
      if (!node.grid) throw new Error('旧地图缺少坐标数据，请重新生成')
      const surface = await api.getSurface(id, controller.signal)
      const xyz = surface.centers
      const [e, b, t, p] = await Promise.all(
        ['elevation', 'biome', 'temperature', 'precipitation'].map((ch) =>
          api.getField(id, ch, controller.signal),
        ),
      )
      const [f, ids, lakes] = await Promise.all([
        api.getField(id, 'flow_accumulation', controller.signal),
        api.getField(id, 'plate_id', controller.signal).catch((e: Error & { status?: number }) => {
          if (e.status === 404) return null
          throw e
        }),
        api
          .getField(id, 'lake_depth', controller.signal)
          .catch((e: Error & { status?: number }) => {
            if (e.status === 404) return null
            throw e
          }),
      ])
      if (controller.signal.aborted) return
      if (
        xyz.length !== node.cells * 3 ||
        [e, b, t, p, f].some((a) => a.length !== node.cells) ||
        (ids && ids.length !== node.cells) ||
        (lakes && lakes.length !== node.cells)
      )
        throw new Error('地图数据与坐标尺寸不一致')
      centers.value = xyz
      indices.value = surface.indices
      elevation.value = e
      biome.value = Uint8Array.from(b, (v) => Math.round(v))
      temperature.value = t
      precipitation.value = p
      flow.value = f
      lakeDepth.value = lakes
      plateIds.value = ids
      currentMap.value = node
      cryosphere.value = null
      void api
        .cryosphere(id, node.cells, 0, controller.signal)
        .then((value) => {
          if (!controller.signal.aborted && currentMap.value?.id === id) cryosphere.value = value
        })
        .catch((e: Error) => {
          if (!controller.signal.aborted && currentMap.value?.id === id)
            cryosphere.value = { available: false, reason: e.message }
        })
      // Geography is derived independently: a failed detection cannot hide the map.
      geographyLoading.value = true
      void api
        .getGeography(id, node.cells, controller.signal)
        .then((g) => {
          if (!controller.signal.aborted && currentMap.value?.id === id) geography.value = g
        })
        .catch((e: Error) => {
          if (!controller.signal.aborted && currentMap.value?.id === id)
            geographyError.value = e.message
        })
        .finally(() => {
          if (!controller.signal.aborted && currentMap.value?.id === id)
            geographyLoading.value = false
        })
    } catch (e) {
      if ((e as Error).name !== 'AbortError') throw e
    } finally {
      if (loadController === controller) loading.value = false
    }
  }
  async function follow(result: { job_id: string; map_id: string }) {
    job.value = {
      id: result.job_id,
      map_id: result.map_id,
      state: 'queued',
      stage: '排队',
      progress: 0,
      error: null,
    }
    await refreshMaps()
    stopStream?.()
    stopStream = openEventStream(result.job_id, {
      onFrame: (f) => {
        if (closed || job.value?.id !== f.job_id || !generating.value) return
        if (f.meta) {
          job.value.progress = Math.max(job.value.progress, f.meta.progress)
          job.value.stage = f.meta.stage
        }
        frameHandler?.(f)
      },
      onResync: () => {
        void api
          .getJob(result.job_id)
          .then((j) => {
            if (job.value?.id === result.job_id) job.value = j
          })
          .catch(() => {})
      },
      onProgress: (p) => {
        if (job.value) {
          job.value.progress = Math.max(job.value.progress, p.progress)
          job.value.stage = p.stage
        }
      },
      onStage: (s) => {
        if (job.value) job.value.stage = s.to
      },
    })
    try {
      const deadline = Date.now() + 30 * 60 * 1000
      while (!closed) {
        const j = await api.getJob(result.job_id)
        if (job.value?.id === j.id) j.progress = Math.max(j.progress, job.value.progress)
        job.value = j
        if (j.state === 'done') {
          await refreshMaps()
          await loadMapData(result.map_id)
          return result.map_id
        }
        if (j.state === 'failed' || j.state === 'cancelled')
          throw new Error(j.error || (j.state === 'cancelled' ? '生成已取消' : '生成未完成'))
        if (Date.now() > deadline) throw new Error('生成时间较长，请重新打开工程检查任务状态')
        await new Promise((r) => setTimeout(r, 1000))
      }
      throw new Error('页面已关闭')
    } finally {
      stopStream?.()
      stopStream = null
      await refreshMaps().catch(() => {})
    }
  }
  async function generate(worldId: string) {
    if (generating.value) throw new Error('已有生成任务')
    generating.value = true
    try {
      return await follow(await api.generateWorld(worldId))
    } finally {
      generating.value = false
    }
  }
  async function child(parentId: string, body: ChildRequest) {
    if (generating.value) throw new Error('已有生成任务')
    generating.value = true
    try {
      return await follow(await api.generateChild(parentId, body))
    } finally {
      generating.value = false
    }
  }
  async function cancel() {
    if (job.value && generating.value) {
      const r = await api.cancelJob(job.value.id)
      if (r.ok) job.value.state = 'cancelling'
    }
  }
  function dispose() {
    closed = true
    loadController?.abort()
    stopStream?.()
  }
  function onFrame(fn: (f: DecodedFrameEvent) => void) {
    frameHandler = fn
  }
  return {
    session,
    worlds,
    current,
    maps,
    currentMap,
    job,
    generating,
    loading,
    error,
    centers,
    indices,
    geography,
    geographyLoading,
    geographyError,
    elevation,
    biome,
    temperature,
    precipitation,
    flow,
    lakeDepth,
    plateIds,
    cryosphere,
    init,
    refreshWorlds,
    refreshMaps,
    selectWorld,
    createWorld,
    generate,
    child,
    loadMapData,
    cancel,
    dispose,
    onFrame,
    clearMap,
  }
})
