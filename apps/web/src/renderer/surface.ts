import { worldMapPoint } from './geometry.ts'

/** Duplicate only triangles crossing the atlas seam, retaining original field IDs. */
export function atlasSurface(centers: Float32Array, indices: Uint32Array) {
  const count = centers.length / 3
  const base = new Float32Array(centers.length)
  for (let i = 0; i < count; i++)
    base.set(worldMapPoint([centers[i * 3], centers[i * 3 + 1], centers[i * 3 + 2]]), i * 3)
  const extra: number[] = [],
    sources: number[] = [],
    extraIndices: number[] = []
  const out = indices.slice(),
    cache = new Map<string, number>()
  const vertex = (id: number, shift: number) => {
    if (!shift) return id
    const key = `${id}:${shift}`
    let v = cache.get(key)
    if (v === undefined) {
      v = count + sources.length
      cache.set(key, v)
      sources.push(id)
      extra.push(base[id * 3] + shift, base[id * 3 + 1], 0)
    }
    return v
  }
  for (let i = 0; i < indices.length; i += 3) {
    const ids = [indices[i], indices[i + 1], indices[i + 2]]
    const x = ids.map((id) => base[id * 3])
    if (Math.max(...x) - Math.min(...x) <= 2) continue
    for (let k = 0; k < 3; k++) out[i + k] = vertex(ids[k], x[k] < 0 ? 4 : 0)
    extraIndices.push(...ids.map((id, k) => vertex(id, x[k] > 0 ? -4 : 0)))
  }
  const positions = new Float32Array(base.length + extra.length)
  positions.set(base)
  positions.set(extra, base.length)
  const triangles = new Uint32Array(out.length + extraIndices.length)
  triangles.set(out)
  triangles.set(extraIndices, out.length)
  const sourceIds = new Uint32Array(count + sources.length)
  for (let i = 0; i < count; i++) sourceIds[i] = i
  sourceIds.set(sources, count)
  return { positions, indices: triangles, sourceIds }
}

export function atlasLines(lines: Float32Array) {
  const out: number[] = []
  for (let i = 0; i < lines.length; i += 6) {
    const a = worldMapPoint([lines[i], lines[i + 1], lines[i + 2]])
    const b = worldMapPoint([lines[i + 3], lines[i + 4], lines[i + 5]])
    if (Math.abs(a[0] - b[0]) <= 2) out.push(...a, ...b)
    else {
      out.push(a[0] < 0 ? a[0] + 4 : a[0], a[1], 0, b[0] < 0 ? b[0] + 4 : b[0], b[1], 0)
      out.push(a[0] > 0 ? a[0] - 4 : a[0], a[1], 0, b[0] > 0 ? b[0] - 4 : b[0], b[1], 0)
    }
  }
  return new Float32Array(out)
}
