import type { Vec3 } from './math'
import {
  projectLonLat,
  unprojectPoint,
  MERCATOR_MAX_LAT,
  type WorldProjection,
} from './projections.ts'
export function latLon(dir: ArrayLike<number>) {
  const length = Math.hypot(dir[0], dir[1], dir[2])
  return {
    lat: (Math.asin(Math.max(-1, Math.min(1, dir[1] / length))) * 180) / Math.PI,
    lon: (Math.atan2(dir[2], dir[0]) * 180) / Math.PI,
  }
}
export function direction(lat: number, lon: number): Vec3 {
  const a = (lat * Math.PI) / 180,
    b = (lon * Math.PI) / 180
  return [Math.cos(a) * Math.cos(b), Math.sin(a), Math.cos(a) * Math.sin(b)]
}
export function patchFrame(lat: number, lon: number) {
  const c = direction(lat, lon),
    horizontal = Math.hypot(c[0], c[2])
  const east: Vec3 = horizontal > 1e-12 ? [-c[2] / horizontal, 0, c[0] / horizontal] : [0, 0, 1]
  const north: Vec3 = [
    east[1] * c[2] - east[2] * c[1],
    east[2] * c[0] - east[0] * c[2],
    east[0] * c[1] - east[1] * c[0],
  ]
  return { c, east, north }
}
export function patchDirection(
  lat: number,
  lon: number,
  x: number,
  y: number,
  radius: number,
): Vec3 {
  const { c, east, north } = patchFrame(lat, lon),
    ex = Math.tan(x / radius),
    ny = Math.tan(y / radius)
  const p = c.map((v, i) => v + east[i] * ex + north[i] * ny) as Vec3
  const length = Math.hypot(...p)
  return p.map((v) => v / length) as Vec3
}
export function patchProject(lat: number, lon: number, dir: Vec3, radius: number): Vec3 | null {
  const { c, east, north } = patchFrame(lat, lon),
    dot = (a: Vec3, b: Vec3) => a.reduce((sum, v, i) => sum + v * b[i], 0),
    front = dot(dir, c)
  if (front <= 0) return null
  return [
    Math.atan(dot(dir, east) / front) * radius,
    Math.atan(dot(dir, north) / front) * radius,
    0,
  ]
}
export function nearestCell(centers: Float32Array, p: Vec3, sphere: boolean) {
  let best = -1,
    score = Infinity
  for (let i = 0; i < centers.length; i += 3) {
    const d = sphere
      ? -(centers[i] * p[0] + centers[i + 1] * p[1] + centers[i + 2] * p[2])
      : (centers[i] - p[0]) ** 2 + (centers[i + 1] - p[1]) ** 2
    if (d < score) {
      score = d
      best = i / 3
    }
  }
  return best
}

/** Equal-angle atlas: longitude ±180 maps to x ±2, latitude ±90 to y ±1. */
export function worldMapPoint(p: Vec3): Vec3 {
  const geo = latLon(p)
  return [geo.lon / 90, geo.lat / 90, 0]
}
export function worldMapDirection(p: Vec3): Vec3 {
  return direction(p[1] * 90, p[0] * 90)
}
export function projectedWorldPoint(p: Vec3, projection: WorldProjection): Vec3 | null {
  const geo = latLon(p),
    lat = (geo.lat * Math.PI) / 180
  if (projection === 'mercator' && Math.abs(lat) > MERCATOR_MAX_LAT + 1e-9) return null
  const xy = projectLonLat((geo.lon * Math.PI) / 180, lat, projection)
  return [xy[0], xy[1], 0]
}
export function projectedWorldDirection(p: Vec3, projection: WorldProjection): Vec3 | null {
  const geo = unprojectPoint(p[0], p[1], projection)
  return geo ? direction((geo[1] * 180) / Math.PI, (geo[0] * 180) / Math.PI) : null
}

export function flatDirection(
  lat: number,
  lon: number,
  x: number,
  y: number,
  radius: number,
  referenceLat = lat,
): Vec3 {
  return direction(
    lat + ((y / radius) * 180) / Math.PI,
    lon +
      ((x / (radius * Math.max(0.05, Math.cos((referenceLat * Math.PI) / 180)))) * 180) / Math.PI,
  )
}
export function flatProject(
  lat: number,
  lon: number,
  dir: Vec3,
  radius: number,
  referenceLat = lat,
): Vec3 {
  const geo = latLon(dir),
    dl = ((geo.lon - lon + 540) % 360) - 180
  return [
    ((dl * Math.PI) / 180) * radius * Math.max(0.05, Math.cos((referenceLat * Math.PI) / 180)),
    (((geo.lat - lat) * Math.PI) / 180) * radius,
    0,
  ]
}
