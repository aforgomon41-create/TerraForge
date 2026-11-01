export interface GeographicFeature {
  id: number
  parent: number | null
  level: 'continent' | 'region' | 'site'
  kind: string
  name: string
  anchor_cell: number
  lat: number
  lon: number
  area_m2: number
  cell_count: number
  radius_m: number
  partial: boolean
  elevation_m: number
  prominence_m: number | null
  drainage_area_m2: number | null
}
export interface Geography {
  version: number
  cell_count: number
  resolution_m: number
  features: GeographicFeature[]
  omitted_sites: number
  landmass: Uint32Array
  region: Uint32Array
  site: Uint32Array
}
export const featureKinds: Record<string, string> = {
  continent: '大陆',
  island: '岛屿',
  land_fragment: '陆地片区',
  drainage_region: '汇水区',
  peak: '山峰',
  pass: '山口',
  lake_basin: '湖盆',
  river_mouth: '河口',
  map_outlet: '边界出水口',
}
export function decodeGeography(buffer: ArrayBuffer, cells: number): Geography {
  if (buffer.byteLength < 4) throw new Error('地理资料不完整')
  const view = new DataView(buffer),
    length = view.getUint32(0, true)
  if (length > buffer.byteLength - 4 || buffer.byteLength !== 4 + length + cells * 12)
    throw new Error('地理归属尺寸不匹配')
  const g = JSON.parse(new TextDecoder().decode(new Uint8Array(buffer, 4, length))) as Geography
  if (
    g.version !== 1 ||
    g.cell_count !== cells ||
    !Array.isArray(g.features) ||
    g.features.length > 100000
  )
    throw new Error('地理资料版本或尺寸无效')
  for (const [i, f] of g.features.entries()) {
    if (
      f.id !== i + 1 ||
      !['continent', 'region', 'site'].includes(f.level) ||
      !Number.isInteger(f.anchor_cell) ||
      f.anchor_cell < 0 ||
      f.anchor_cell >= cells ||
      (f.parent !== null && (!Number.isInteger(f.parent) || f.parent <= 0 || f.parent >= f.id)) ||
      ![f.lat, f.lon, f.area_m2, f.radius_m].every(Number.isFinite)
    )
      throw new Error('地理对象属性无效')
  }
  // JSON header need not be aligned. Read explicitly as little endian on every device.
  const channels = ['landmass', 'region', 'site'] as const
  for (const [channel, name] of channels.entries()) {
    const values = new Uint32Array(cells),
      level = ['continent', 'region', 'site'][channel]
    for (let i = 0; i < cells; i++) {
      const id = view.getUint32(4 + length + 4 * (channel * cells + i), true)
      if (id && g.features[id - 1]?.level !== level) throw new Error('地理归属编号无效')
      values[i] = id
    }
    g[name] = values
  }
  return g
}
export function featureMask(g: Geography, feature: GeographicFeature): Float32Array {
  const labels =
    feature.level === 'continent' ? g.landmass : feature.level === 'region' ? g.region : g.site
  const mask = Float32Array.from(labels, (id) => Number(id === feature.id))
  mask[feature.anchor_cell] = 1
  return mask
}
export function featuresAt(g: Geography | null, cell: number | undefined): GeographicFeature[] {
  if (!g || cell === undefined || cell < 0 || cell >= g.cell_count) return []
  return [g.landmass[cell], g.region[cell], g.site[cell]]
    .filter(Boolean)
    .map((id) => g.features[id - 1])
}
