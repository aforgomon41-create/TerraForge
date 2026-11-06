/** Same-origin API. Every tab gets a server-issued, temporary capability. */
import { decodeGeography } from '@/geography/catalog'
import { decodeIce, type IceResponse } from '@/cryosphere/data'
import { decodeEcosystems, type EcosystemResponse } from '@/ecosystems/data'
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
  | {
      type: 'plane'
      center_lat: number
      center_lon: number
      width_m: number
      size: number
      rows: number
      radius_m: number
      version: number
      reference_lat?: number | null
    }
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
export interface TectonicDiagnostic {
  available: boolean
  reason?: string
  groups?: number[][]
  plates?: { id: number; euler_pole: number[]; omega: number }[]
  planar_velocities?: number[][] | null
  planar_spin_rad_myr?: number[] | null
  plate_centers_m?: number[][]
  motion_grid?: GridDescriptor
  reference_frame?: string
  lifecycle?: {
    slot_uids: number[]
    identities: {
      uid: number
      born_myr: number
      ended_myr: number | null
      parent_uid: number | null
      absorbed_by: number | null
      status: string
      area_m2: number
      maximum_area_m2: number
    }[]
    events: { time_myr: number; kind: string; uid: number; related_uid: number | null }[]
    capacity_limited_events: number
  } | null
}
export interface CirculationDiagnostic {
  available: boolean
  reason?: string
  kind?: 'wind' | 'current'
  month?: number
  groups?: number[][]
  maximum_speed_m_s?: number
  heat_coupled?: boolean
  relative_solver_residual?: number[]
  current_velocity_scale?: number[]
  forcing_wind_rms_difference_m_s?: number[]
}
export interface HydrologyDiagnostic {
  available: boolean
  model?: string
  legacy_lakes?: boolean
  notice?: string
  reason?: string
  groups?: number[][]
  segments?: number
  seasonal_segments?: number
  perennial_segments?: number
  liquid_support?: boolean
  minimum_upstream_area_m2?: number
  limited?: boolean
}
export interface Surface {
  centers: Float32Array
  indices: Uint32Array
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
  cell_size_m?: number
  name?: string
}
export interface RefinementQuery {
  feature_id?: number
  center_lat?: number
  center_lon?: number
  width_m?: number
  cell_size_m?: number
}
export interface RefinementPlan {
  kind: string
  feature_id: number | null
  feature_name: string | null
  center_lat: number
  center_lon: number
  width_m: number
  cell_size_m: number
  side: number
  cells: number
  can_generate: boolean
  complete_object: boolean
  message: string
  maximum_width_m: number
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
    const error = new Error(body?.error || `请求未完成（${res.status}）`) as Error & {
      status: number
    }
    error.status = res.status
    throw error
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
async function geometryRequest(path: string, signal?: AbortSignal): Promise<Response> {
  for (let attempt = 0; ; attempt++) {
    try {
      return await authorizedFetch('/api/v1' + path, { signal })
    } catch (e) {
      if ((e as Error & { status?: number }).status !== 429 || attempt >= 10 || signal?.aborted)
        throw e
      await new Promise<void>((resolve, reject) => {
        const abort = () => {
          clearTimeout(timer)
          reject(new DOMException('已取消', 'AbortError'))
        }
        const timer = setTimeout(
          () => {
            signal?.removeEventListener('abort', abort)
            resolve()
          },
          150 * (attempt + 1),
        )
        signal?.addEventListener('abort', abort, { once: true })
      })
    }
  }
}
export const api = {
  async refinementPlan(
    id: string,
    query: RefinementQuery,
    signal?: AbortSignal,
  ): Promise<RefinementPlan> {
    const params = new URLSearchParams()
    for (const [key, value] of Object.entries(query))
      if (value !== undefined) params.set(key, String(value))
    const response = await geometryRequest(`/maps/${id}/refinement-plan?${params}`, signal)
    return response.json()
  },
  async hydrology(id: string, signal?: AbortSignal, month = 0): Promise<HydrologyDiagnostic> {
    const response = await geometryRequest(`/maps/${id}/hydrology?month=${month}`, signal)
    return response.json()
  },
  async cryosphere(
    id: string,
    count: number,
    month = 0,
    signal?: AbortSignal,
  ): Promise<IceResponse> {
    const response = await geometryRequest(`/maps/${id}/cryosphere?month=${month}`, signal)
    if (response.headers.get('content-type')?.includes('application/json')) return response.json()
    return decodeIce(await response.arrayBuffer(), count)
  },
  async ecosystems(id: string, count: number, signal?: AbortSignal): Promise<EcosystemResponse> {
    const response = await geometryRequest(`/maps/${id}/ecosystems`, signal)
    if (response.headers.get('content-type')?.includes('application/json')) return response.json()
    return decodeEcosystems(await response.arrayBuffer(), count)
  },
  async vectors(
    id: string,
    kind: 'wind' | 'current',
    month: number,
    signal?: AbortSignal,
  ): Promise<CirculationDiagnostic> {
    return request(`/maps/${id}/vectors/${kind}?month=${month}`, { signal })
  },
  async getGeography(id: string, cells: number, signal?: AbortSignal) {
    const response = await geometryRequest(`/maps/${id}/geography`, signal)
    return decodeGeography(await response.arrayBuffer(), cells)
  },
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
  createWorld: (name: string, seed: number | undefined, params?: Record<string, unknown>) =>
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
  async getSurface(id: string, signal?: AbortSignal): Promise<Surface> {
    const r = await geometryRequest(`/maps/${id}/surface`, signal)
    const bytes = await r.arrayBuffer()
    if (bytes.byteLength < 12 || bytes.byteLength % 4) throw new Error('地表几何长度无效')
    const head = new Uint32Array(bytes, 0, 3),
      count = head[1],
      len = head[2]
    if (head[0] !== 1 || len % 3 || bytes.byteLength !== 12 + count * 12 + len * 4)
      throw new Error('地表几何版本或尺寸无效')
    const indices = new Uint32Array(bytes, 12 + count * 12, len)
    if (indices.some((i) => i >= count)) throw new Error('地表顶点编号无效')
    return { centers: new Float32Array(bytes, 12, count * 3), indices }
  },
  async getOverlay(id: string, spacing: number, signal?: AbortSignal) {
    const r = await geometryRequest(`/maps/${id}/overlay?spacing_m=${spacing}`, signal)
    const data = await r.arrayBuffer()
    if (data.byteLength % 24) throw new Error('显示网格长度无效')
    return { lines: new Float32Array(data), spacing: Number(r.headers.get('x-tf-grid-spacing-m')) }
  },
  async tectonics(id: string, signal?: AbortSignal): Promise<TectonicDiagnostic> {
    const response = await geometryRequest(`/maps/${id}/tectonics`, signal)
    return response.json()
  },
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
  async download(
    id: string,
    format: 'png' | 'json',
    filename: string,
    ecosystem?: 'biome' | 'group' | 'variant',
  ) {
    const query = format === 'png' && ecosystem ? `?ecosystem=${ecosystem}` : ''
    const res = await authorizedFetch(`/api/v1/maps/${id}/export.${format}${query}`)
    const url = URL.createObjectURL(await res.blob())
    const a = document.createElement('a')
    a.href = url
    a.download = filename
    a.click()
    setTimeout(() => URL.revokeObjectURL(url), 10000)
  },
}
