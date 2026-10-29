/**
 * 后端 API 客户端。
 *
 * 所有请求自动带上客户端令牌。令牌首次访问时生成，存在 localStorage。
 */

// 部署在 /map/ 子路径下时，API 也在 /map/api/
// 开发环境用 Vite 代理，生产环境用相对路径
// API_BASE 只到站点根（含子路径），request() 里会拼 /api/v1/...
export const API_BASE = import.meta.env.DEV
  ? ''
  : import.meta.env.BASE_URL.replace(/\/$/, '')

/** 客户端身份令牌。用来区分「我的世界」。 */
export function getClientToken(): string {
  const KEY = 'tf-client-token'
  let token = localStorage.getItem(KEY)
  if (!token) {
    token = 'ct_' + crypto.randomUUID().replace(/-/g, '').slice(0, 24)
    localStorage.setItem(KEY, token)
  }
  return token
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(API_BASE + path, {
    ...init,
    headers: {
      'Content-Type': 'application/json',
      'x-tf-client': getClientToken(),
      ...init?.headers,
    },
  })
  if (!res.ok) {
    const text = await res.text().catch(() => '')
    throw new Error(`请求失败 ${res.status}: ${text.slice(0, 200)}`)
  }
  return res.json() as Promise<T>
}

export interface World {
  id: string
  name: string
  seed: number
  created_at: number
  share_token: string | null
}

export interface MapNode {
  id: string
  world_id: string
  kind: string
  parent: string | null
  cells: number
  cell_size_m: number
  status: string
  created_at: number
  completed_at: number | null
}

export interface JobStatus {
  id: string
  map_id: string
  state: string
  stage: string
  progress: number
  error: string | null
}

export const api = {
  health: () => request<{ status: string; version: string }>('/api/v1/health'),

  createWorld: (name: string, seed: number) =>
    request<World>('/api/v1/worlds', {
      method: 'POST',
      body: JSON.stringify({ name, seed }),
    }),

  listWorlds: () => request<World[]>('/api/v1/worlds'),

  getWorld: (id: string) => request<any>(`/api/v1/worlds/${id}`),

  generateWorld: (worldId: string) =>
    request<{ job_id: string; map_id: string }>(`/api/v1/worlds/${worldId}/generate`, {
      method: 'POST',
    }),

  listMaps: (worldId: string) => request<MapNode[]>(`/api/v1/worlds/${worldId}/maps`),

  getMap: (id: string) => request<MapNode>(`/api/v1/maps/${id}`),

  getJob: (id: string) => request<JobStatus>(`/api/v1/jobs/${id}`),

  cancelJob: (id: string) =>
    request<{ ok: boolean }>(`/api/v1/jobs/${id}/cancel`, { method: 'POST' }),

  systemStatus: () => request<any>('/api/v1/system/status'),

  /** 导出 PNG 地图（服务端渲染）。 */
  exportPng: (mapId: string, size = 2048) =>
    `${API_BASE}/api/v1/maps/${mapId}/export.png?size=${size}`,

  /** 导出 JSON 世界数据。 */
  exportJson: (mapId: string) => `${API_BASE}/api/v1/maps/${mapId}/export.json`,

  /** 生成子图（大陆/区域/地点）。 */
  generateChild: (
    parentId: string,
    body: { kind: string; center_lon: number; center_lat: number; width_m: number; name?: string }
  ) =>
    request<{ job_id: string; map_id: string; kind: string; cells: number; cell_size_m: number }>(
      `/api/v1/maps/${parentId}/children`,
      { method: 'POST', body: JSON.stringify(body) }
    ),

  /** 取某个通道的原始 f32 数据。 */
  async getField(mapId: string, channel: string): Promise<Float32Array> {
    const res = await fetch(API_BASE + `/api/v1/maps/${mapId}/field/${channel}`, {
      headers: { 'x-tf-client': getClientToken() },
    })
    if (!res.ok) throw new Error(`取字段失败: ${res.status}`)
    const buf = await res.arrayBuffer()
    return new Float32Array(buf)
  },
}
