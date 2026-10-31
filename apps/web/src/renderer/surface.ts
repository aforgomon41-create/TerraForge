import { worldMapPoint } from './geometry.ts'
import { projectLonLat, type WorldProjection } from './projections.ts'

/** Project the already-unwrapped atlas once, avoiding a nonlinear solve per GPU vertex per frame. */
export function projectAtlasPositions(positions: Float32Array, projection: WorldProjection) {
  const projected = new Float32Array(positions.length)
  for (let i = 0; i < positions.length; i += 3) {
    const xy = projectLonLat(
      (positions[i] * Math.PI) / 2,
      (positions[i + 1] * Math.PI) / 2,
      projection,
    )
    projected[i] = xy[0]
    projected[i + 1] = xy[1]
  }
  return projected
}

export function planarPreviewSurface(aspect: number, columns = 128) {
  const rows = Math.max(4, Math.round(columns * aspect)),
    centers = new Float32Array((columns + 1) * (rows + 1) * 3)
  const indices = new Uint32Array(columns * rows * 6)
  for (let r = 0; r <= rows; r++)
    for (let c = 0; c <= columns; c++) {
      const i = (r * (columns + 1) + c) * 3
      centers[i] = (c / columns) * 2 - 1
      centers[i + 1] = (1 - (r / rows) * 2) * aspect
    }
  for (let r = 0; r < rows; r++)
    for (let c = 0; c < columns; c++) {
      const a = r * (columns + 1) + c,
        i = (r * columns + c) * 6
      indices.set([a, a + columns + 1, a + 1, a + 1, a + columns + 1, a + columns + 2], i)
    }
  return { centers, indices, count: centers.length / 3 }
}

/** Duplicate only triangles crossing the atlas seam, retaining original field IDs. */
export function atlasSurface(centers: Float32Array, indices: Uint32Array) {
  const count = centers.length / 3,
    base = new Float32Array(centers.length)
  for (let i = 0; i < count; i++)
    base.set(worldMapPoint([centers[i * 3], centers[i * 3 + 1], centers[i * 3 + 2]]), i * 3)
  const extra: number[] = [],
    sources: number[] = [],
    triangles: number[] = [],
    cache = new Map<string, number>()
  const vertex = (id: number, longitude: number) => {
    if (longitude === base[id * 3]) return id
    const key = `${id}:${longitude}`
    let result = cache.get(key)
    if (result === undefined) {
      result = count + sources.length
      cache.set(key, result)
      sources.push(id)
      extra.push(longitude, base[id * 3 + 1], 0)
    }
    return result
  }
  for (let i = 0; i < indices.length; i += 3) {
    const ids = [indices[i], indices[i + 1], indices[i + 2]],
      x = ids.map((id) => base[id * 3])
    const pole = ids.findIndex((id) => Math.abs(base[id * 3 + 1]) > 1 - 1e-7)
    const nonpolar = pole < 0 ? x : x.filter((_, k) => k !== pole)
    const seam = Math.max(...nonpolar) - Math.min(...nonpolar) > 2
    const copies = seam ? [1, -1] : [0]
    for (const copy of copies) {
      const longitude = x.map(
        (value) => value + (copy === 1 && value < 0 ? 4 : copy === -1 && value > 0 ? -4 : 0),
      )
      if (pole < 0) triangles.push(...ids.map((id, k) => vertex(id, longitude[k])))
      else {
        // A pole is a point on the globe but a line in some atlases. A polar
        // spherical triangle becomes a quad; one pole longitude would leave gaps.
        const a = (pole + 1) % 3,
          b = (pole + 2) % 3
        const pa = vertex(ids[pole], longitude[a]),
          pb = vertex(ids[pole], longitude[b])
        const va = vertex(ids[a], longitude[a]),
          vb = vertex(ids[b], longitude[b])
        triangles.push(pa, va, vb, pa, vb, pb)
      }
    }
  }
  const positions = new Float32Array(base.length + extra.length)
  positions.set(base)
  positions.set(extra, base.length)
  const sourceIds = new Uint32Array(count + sources.length)
  for (let i = 0; i < count; i++) sourceIds[i] = i
  sourceIds.set(sources, count)
  return { positions, indices: Uint32Array.from(triangles), sourceIds }
}

export function atlasLines(lines: Float32Array) {
  const out: number[] = []
  for (let i = 0; i < lines.length; i += 6) {
    const a = worldMapPoint([lines[i], lines[i + 1], lines[i + 2]])
    const b = worldMapPoint([lines[i + 3], lines[i + 4], lines[i + 5]])
    if (Math.abs(a[1]) > 1 - 1e-7) a[0] = b[0]
    if (Math.abs(b[1]) > 1 - 1e-7) b[0] = a[0]
    if (Math.abs(a[0] - b[0]) <= 2) out.push(...a, ...b)
    else {
      out.push(a[0] < 0 ? a[0] + 4 : a[0], a[1], 0, b[0] < 0 ? b[0] + 4 : b[0], b[1], 0)
      out.push(a[0] > 0 ? a[0] - 4 : a[0], a[1], 0, b[0] > 0 ? b[0] - 4 : b[0], b[1], 0)
    }
  }
  return new Float32Array(out)
}
