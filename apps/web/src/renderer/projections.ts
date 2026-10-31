/** Spherical map formulae. Coordinates use a uniform scale, never separate x/y stretching. */
export type WorldProjection = 'equirectangular' | 'mercator' | 'mollweide' | 'equal-earth'
export const projections: { id: WorldProjection; label: string; hint: string }[] = [
  { id: 'equirectangular', label: '等距圆柱', hint: '经纬网直观；高纬地区会拉宽' },
  { id: 'mercator', label: '墨卡托', hint: '保留局部角度；高纬面积放大，截去约85.05°以外极区' },
  { id: 'mollweide', label: '莫尔威德', hint: '椭圆形等积总图；适合比较陆地面积' },
  { id: 'equal-earth', label: 'Equal Earth', hint: '等积世界地图；兼顾面积比较与整体观感' },
]
const A1 = 1.340264,
  A2 = -0.081106,
  A3 = 0.000893,
  A4 = 0.003796
const M = Math.sqrt(3) / 2,
  EE_SCALE = (2 * M * A1) / Math.PI
export const MERCATOR_MAX_LAT = 2 * Math.atan(Math.exp(Math.PI)) - Math.PI / 2
const clamp = (v: number, low: number, high: number) => Math.max(low, Math.min(high, v))
export function projectionIndex(projection: WorldProjection) {
  return projections.findIndex((p) => p.id === projection)
}
export function projectionBounds(projection: WorldProjection) {
  return {
    width: 2,
    height:
      projection === 'mercator' ? 2 : projection === 'equal-earth' ? 1.3173627591574 * EE_SCALE : 1,
  }
}
/** Longitude may be unwrapped across the seam for individual mesh triangles. */
export function projectLonLat(
  lon: number,
  lat: number,
  projection: WorldProjection,
): [number, number] {
  const x = (lon * 2) / Math.PI
  if (projection === 'equirectangular') return [x, (lat * 2) / Math.PI]
  if (projection === 'mercator')
    return [
      x,
      (Math.asinh(Math.tan(clamp(lat, -MERCATOR_MAX_LAT, MERCATOR_MAX_LAT))) * 2) / Math.PI,
    ]
  if (projection === 'mollweide') {
    let theta = lat
    if (Math.abs(lat) > Math.PI / 2 - 1e-10) theta = (Math.sign(lat) * Math.PI) / 2
    else
      for (let i = 0; i < 16; i++) {
        const delta =
          (2 * theta + Math.sin(2 * theta) - Math.PI * Math.sin(lat)) /
          Math.max(1e-12, 2 + 2 * Math.cos(2 * theta))
        theta -= delta
        if (Math.abs(delta) < 1e-12) break
      }
    return [x * Math.cos(theta), Math.sin(theta)]
  }
  const theta = Math.asin(M * Math.sin(lat)),
    t2 = theta * theta,
    t6 = t2 * t2 * t2
  return [
    (EE_SCALE * lon * Math.cos(theta)) / (M * (A1 + 3 * A2 * t2 + t6 * (7 * A3 + 9 * A4 * t2))),
    EE_SCALE * theta * (A1 + A2 * t2 + t6 * (A3 + A4 * t2)),
  ]
}
export function unprojectPoint(
  x: number,
  y: number,
  projection: WorldProjection,
): [number, number] | null {
  if (!Number.isFinite(x) || !Number.isFinite(y)) return null
  const bounds = projectionBounds(projection),
    tolerance = 1e-8
  if (Math.abs(y) > bounds.height + tolerance) return null
  let lon: number, lat: number
  if (projection === 'equirectangular') {
    lon = (x * Math.PI) / 2
    lat = (y * Math.PI) / 2
  } else if (projection === 'mercator') {
    lon = (x * Math.PI) / 2
    lat = Math.atan(Math.sinh((y * Math.PI) / 2))
  } else if (projection === 'mollweide') {
    const theta = Math.asin(clamp(y, -1, 1)),
      c = Math.cos(theta)
    if (c < 1e-9) return Math.abs(x) < tolerance ? [0, (Math.sign(y) * Math.PI) / 2] : null
    lon = (x * Math.PI) / (2 * c)
    lat = Math.asin(clamp((2 * theta + Math.sin(2 * theta)) / Math.PI, -1, 1))
  } else {
    const yy = y / EE_SCALE
    let theta = yy
    for (let i = 0; i < 12; i++) {
      const t2 = theta * theta,
        t6 = t2 * t2 * t2
      const delta =
        (theta * (A1 + A2 * t2 + t6 * (A3 + A4 * t2)) - yy) /
        (A1 + 3 * A2 * t2 + t6 * (7 * A3 + 9 * A4 * t2))
      theta -= delta
      if (Math.abs(delta) < 1e-12) break
    }
    const t2 = theta * theta,
      t6 = t2 * t2 * t2
    lon = ((x / EE_SCALE) * M * (A1 + 3 * A2 * t2 + t6 * (7 * A3 + 9 * A4 * t2))) / Math.cos(theta)
    lat = Math.asin(clamp(Math.sin(theta) / M, -1, 1))
  }
  if (Math.abs(lon) > Math.PI + tolerance) return null
  return [clamp(lon, -Math.PI, Math.PI), clamp(lat, -Math.PI / 2, Math.PI / 2)]
}
