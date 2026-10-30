import { test } from 'node:test'
import assert from 'node:assert/strict'
import { atlasSurface, atlasLines } from '../src/renderer/surface.ts'
import { direction, flatDirection, flatProject } from '../src/renderer/geometry.ts'
import { buildSphereGrid } from '../src/renderer/sphereGrid.ts'
test('shared preview mesh is closed, without duplicated faces or unused vertices', () => {
  const g = buildSphereGrid(8),
    edges = new Map<string, number>(),
    used = new Set<number>()
  assert.equal(g.indices.length / 3, 20 * 8 * 8)
  for (let i = 0; i < g.indices.length; i += 3) {
    const [a, b, c] = Array.from(g.indices.subarray(i, i + 3))
    ;[a, b, c].forEach((v) => used.add(v))
    for (const [x, y] of [
      [a, b],
      [b, c],
      [c, a],
    ]) {
      const key = `${Math.min(x, y)}:${Math.max(x, y)}`
      edges.set(key, (edges.get(key) || 0) + 1)
    }
  }
  assert.equal(used.size, g.count)
  assert.ok([...edges.values()].every((n) => n === 2))
  assert.equal(g.count - edges.size + g.indices.length / 3, 2)
})
test('atlas seam triangles retain field IDs and are drawn on both sides', () => {
  const centers = Float32Array.from(
    [direction(5, 179), direction(8, -179), direction(0, -178)].flat(),
  )
  const mesh = atlasSurface(centers, new Uint32Array([0, 1, 2]))
  assert.equal(mesh.indices.length, 6)
  const spans = []
  for (let i = 0; i < 6; i += 3) {
    const x = Array.from(mesh.indices.subarray(i, i + 3), (id) => mesh.positions[id * 3])
    assert.ok(Math.max(...x) - Math.min(...x) < 0.05)
    spans.push(x.reduce((a, b) => a + b) / 3)
    assert.deepEqual(
      Array.from(mesh.indices.subarray(i, i + 3), (id) => mesh.sourceIds[id]),
      [0, 1, 2],
    )
  }
  assert.ok(spans.some((x) => x > 1.9) && spans.some((x) => x < -1.9))
  const line = atlasLines(Float32Array.from([direction(5, 179), direction(5, -179)].flat()))
  assert.equal(line.length, 12)
  for (let i = 0; i < 12; i += 6) assert.ok(Math.abs(line[i] - line[i + 3]) < 0.05)
})
test('planar descendants preserve the root geographic reference across the dateline', () => {
  const radius = 6371000,
    reference = 30
  const dir = flatDirection(31, 179.9, 15000, -1000, radius, reference)
  const restored = flatProject(31, 179.9, dir, radius, reference)
  assert.ok(Math.abs(restored[0] - 15000) < 1e-6)
  assert.ok(Math.abs(restored[1] + 1000) < 1e-6)
})
