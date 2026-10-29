/** Same-origin API. Every tab gets a server-issued, temporary capability. */
export const API_BASE = import.meta.env.DEV
  ? ''
  : new URL(import.meta.env.BASE_URL, window.location.href).pathname.replace(/\/$/, '')
let token = sessionStorage.getItem('tf-session-v2') || ''
export interface Session {
  token: string
  mode: 'web' | 'desktop'
  quality: string
  max_patch_side: number
  expires_in_secs: number
}
export interface World {
  id: string
  name: string
  seed: number
  created_at: number
  params?: Record<string, unknown>
}
export type GridDescriptor =
  | { type: 'sphere'; frequency: number; radius_m: number; version: number }
  | {
      type: 'patch'
      center_lat: number
      center_lon: number
      width_m: number
      size: number
      radius_m: number
      version: number
    }
export interface MapNode {
  id: string
  world_id: string
  kind: string
  name: string
  parent: string | null
  cells: number
  cell_size_m: number
  status: string
  grid: GridDescriptor | null
  seed: number
  generator_version: string
}
export interface JobStatus {
  id: string
  map_id: string
  state: string
  stage: string
  progress: number
  error: string | null
}
export interface ChildRequest {
  kind: string
  center_lon: number
  center_lat: number
  width_m: number
  name?: string
}
export function authHeaders(): Record<string, string> {
  return token ? { Authorization: `Bearer ${token}` } : {}
}
export async function authorizedFetch(path: string, init: RequestInit = {}): Promise<Response> {
  const res = await fetch(API_BASE + path, {
    ...init,
    credentials: 'omit',
    headers: { ...authHeaders(), ...init.headers },
  })
  if (!res.ok) {
    const body = await res.json().catch(() => null)
    throw new Error(body?.error || `请求未完成（${res.status}）`)
  }
  return res
}
async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  const r = await authorizedFetch('/api/v1' + path, {
    ...init,
    headers: { 'Content-Type': 'application/json', ...init.headers },
  })
  return r.json()
}
async function binary(path: string, signal?: AbortSignal) {
  const r = await authorizedFetch('/api/v1' + path, { signal })
  const bytes = await r.arrayBuffer()
  if (bytes.byteLength % 4) throw new Error('地图数据长度无效')
  return new Float32Array(bytes)
}
export const api = {
  async openSession() {
    const session = await request<Session>('/session', { method: 'POST' })
    token = session.token
    sessionStorage.setItem('tf-session-v2', token)
    return session
  },
  heartbeat: () => request('/session/heartbeat', { method: 'POST' }),
  async closeSession() {
    await request('/session', { method: 'DELETE', keepalive: true })
    token = ''
    sessionStorage.removeItem('tf-session-v2')
  },
  leave() {
    void fetch(API_BASE + '/api/v1/session', {
      method: 'DELETE',
      headers: authHeaders(),
      credentials: 'omit',
      keepalive: true,
    }).catch(() => {})
    sessionStorage.removeItem('tf-session-v2')
  },
  listWorlds: () => request<World[]>('/worlds'),
  getWorld: (id: string) => request<World>(`/worlds/${id}`),
  createWorld: (name: string, seed: number, params?: Record<string, unknown>) =>
    request<World>('/worlds', {
      method: 'POST',
      body: JSON.stringify({ name, seed, params }),
    }),
  deleteWorld: (id: string) => request(`/worlds/${id}`, { method: 'DELETE' }),
  listMaps: (id: string) => request<MapNode[]>(`/worlds/${id}/maps`),
  getMap: (id: string) => request<MapNode>(`/maps/${id}`),
  getJob: (id: string) => request<JobStatus>(`/jobs/${id}`),
  generateWorld: (id: string) =>
    request<{ job_id: string; map_id: string }>(`/worlds/${id}/generate`, {
      method: 'POST',
    }),
  generateChild: (id: string, body: ChildRequest) =>
    request<{ job_id: string; map_id: string }>(`/maps/${id}/children`, {
      method: 'POST',
      body: JSON.stringify(body),
    }),
  cancelJob: (id: string) => request<{ ok: boolean }>(`/jobs/${id}/cancel`, { method: 'POST' }),
  getField: (id: string, ch: string, signal?: AbortSignal) =>
    binary(`/maps/${id}/field/${ch}`, signal),
  getGrid: (id: string, signal?: AbortSignal) => binary(`/maps/${id}/grid`, signal),
  metadata: (id: string, cell?: number) =>
    request<Record<string, unknown>>(
      `/maps/${id}/metadata${cell === undefined ? '' : '?cell=' + cell}`,
    ),
  sample: (id: string, lat: number, lon: number) =>
    request(`/maps/${id}/sample?lat=${lat}&lon=${lon}`),
  async importProject(file: File) {
    const r = await authorizedFetch('/api/v1/projects/import', {
      method: 'POST',
      body: file,
      headers: { 'Content-Type': 'application/json' },
    })
    return r.json() as Promise<World>
  },
  async download(id: string, format: 'png' | 'json', filename: string) {
    const res = await authorizedFetch(`/api/v1/maps/${id}/export.${format}`)
    const url = URL.createObjectURL(await res.blob())
    const a = document.createElement('a')
    a.href = url
    a.download = filename
    a.click()
    setTimeout(() => URL.revokeObjectURL(url), 10000)
  },
}
