/**
 * 世界状态。
 */

import { defineStore } from 'pinia'
import { ref } from 'vue'
import { api, type World, type MapNode, type JobStatus } from '@/api/client'

export const useWorldStore = defineStore('world', () => {
  const worlds = ref<World[]>([])
  const current = ref<World | null>(null)
  const maps = ref<MapNode[]>([])
  const currentMap = ref<MapNode | null>(null)

  const job = ref<JobStatus | null>(null)
  const generating = ref(false)
  const error = ref<string | null>(null)

  // 物理场数据
  const elevation = ref<Float32Array | null>(null)
  const biome = ref<Uint8Array | null>(null)
  const temperature = ref<Float32Array | null>(null)
  const precipitation = ref<Float32Array | null>(null)

  async function refreshWorlds() {
    try {
      worlds.value = await api.listWorlds()
    } catch (e) {
      error.value = String(e)
    }
  }

  async function createWorld(name: string, seed: number) {
    error.value = null
    const w = await api.createWorld(name, seed)
    current.value = w
    await refreshWorlds()
    return w
  }

  /** 触发生成并轮询到完成。 */
  async function generate(worldId: string, onProgress?: (j: JobStatus) => void) {
    generating.value = true
    error.value = null
    try {
      const { job_id, map_id } = await api.generateWorld(worldId)
      currentMap.value = { id: map_id } as MapNode

      // 轮询作业状态
      while (true) {
        await new Promise((r) => setTimeout(r, 800))
        const j = await api.getJob(job_id)
        job.value = j
        onProgress?.(j)

        if (j.state === 'done') break
        if (j.state === 'failed') throw new Error(j.error || '生成失败')
        if (j.state === 'cancelled') throw new Error('已取消')
      }

      // 拉取数据
      await loadMapData(map_id)
      maps.value = await api.listMaps(worldId)
      return map_id
    } catch (e) {
      error.value = String(e)
      throw e
    } finally {
      generating.value = false
    }
  }

  /** 加载地图的物理场。 */
  async function loadMapData(mapId: string) {
    const [elev, bio] = await Promise.all([
      api.getField(mapId, 'elevation'),
      api.getField(mapId, 'biome'),
    ])
    elevation.value = elev
    biome.value = Uint8Array.from(bio, (v) => Math.round(v))
    // 温度和降水按需加载
    try {
      temperature.value = await api.getField(mapId, 'temperature')
      precipitation.value = await api.getField(mapId, 'precipitation')
    } catch {
      // 可选通道
    }
  }

  return {
    worlds,
    current,
    maps,
    currentMap,
    job,
    generating,
    error,
    elevation,
    biome,
    temperature,
    precipitation,
    refreshWorlds,
    createWorld,
    generate,
    loadMapData,
  }
})
