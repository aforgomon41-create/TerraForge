import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import {
  projectLonLat,
  unprojectPoint,
  projectionBounds,
  projections,
  type WorldProjection,
} from '../src/renderer/projections.ts'
import {
  direction,
  projectedWorldPoint,
  projectedWorldDirection,
} from '../src/renderer/geometry.ts'
import { atlasSurface, projectAtlasPositions } from '../src/renderer/surface.ts'
import { buildSphereGrid } from '../src/renderer/sphereGrid.ts'

const reference = JSON.parse(
  readFileSync(
    new URL('../../../third_party/proj/projection-reference.json', import.meta.url),
    'utf8',
  ),
)
test('four projections match coordinates from the independently executed PROJ library', () => {
  for (const c of reference.cases) {
    const xy = projectLonLat((c.lon * Math.PI) / 180, (c.lat * Math.PI) / 180, c.projection)
    assert.ok(Math.hypot(xy[0] - c.x, xy[1] - c.y) < 1e-7, JSON.stringify({ c, xy }))
    const restored = unprojectPoint(c.x, c.y, c.projection)!
    assert.ok(Math.abs((restored[0] * 180) / Math.PI - c.lon) < 0.00001)
    assert.ok(Math.abs((restored[1] * 180) / Math.PI - c.lat) < 0.00001)
  }
})
test('projection picking returns the original geographic position and rejects outside-map space', () => {
  for (const { id } of projections) {
    for (const lat of [-84, -45, 0, 45, 84])
      for (const lon of [-179.99, -100, 0, 100, 179.99]) {
        const d = direction(lat, lon),
          point = projectedWorldPoint(d, id)!,
          back = projectedWorldDirection(point, id)!
        assert.ok(Math.hypot(...d.map((v, i) => v - back[i])) < 1e-8, `${id},${lat},${lon}`)
      }
    assert.equal(unprojectPoint(2.1, 0, id), null)
    assert.equal(unprojectPoint(0, projectionBounds(id).height + 0.01, id), null)
  }
  assert.equal(projectedWorldPoint(direction(89, 0), 'mercator'), null)
  assert.equal(unprojectPoint(1.9, 0.9, 'mollweide'), null)
  assert.equal(unprojectPoint(1.9, 0.9, 'equal-earth'), null)
})
test('equal-area projections retain relative areas across latitude', () => {
  for (const id of ['mollweide', 'equal-earth'] as WorldProjection[]) {
    const scales = [0, 30, 60, 75].map((degrees) => {
      const lat = (degrees * Math.PI) / 180,
        delta = 1e-6
      const a = projectLonLat(1 - delta, lat, id),
        b = projectLonLat(1 + delta, lat, id)
      const c = projectLonLat(1, lat - delta, id),
        d = projectLonLat(1, lat + delta, id)
      return (
        Math.abs((b[0] - a[0]) * (d[1] - c[1]) - (b[1] - a[1]) * (d[0] - c[0])) /
        (4 * delta * delta * Math.cos(lat))
      )
    })
    assert.ok(
      scales.every((v) => Math.abs(v / scales[0] - 1) < 1e-6),
      `${id}: ${scales}`,
    )
  }
})
test('polar atlas caps have no missing meridian wedges and preserve the source field IDs', () => {
  const g = buildSphereGrid(8),
    mesh = atlasSurface(g.centers, g.indices)
  for (const y of [-0.995, 0.995])
    for (const x of [-1.99, -1.5, -0.5, 0.5, 1.5, 1.99]) {
      let found = false
      for (let i = 0; i < mesh.indices.length && !found; i += 3) {
        const [a, b, c] = Array.from(mesh.indices.subarray(i, i + 3), (id) => [
          mesh.positions[id * 3],
          mesh.positions[id * 3 + 1],
        ])
        const determinant = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1])
        if (Math.abs(determinant) < 1e-12) continue
        const u = ((b[1] - c[1]) * (x - c[0]) + (c[0] - b[0]) * (y - c[1])) / determinant
        const v = ((c[1] - a[1]) * (x - c[0]) + (a[0] - c[0]) * (y - c[1])) / determinant
        found = Math.min(u, v, 1 - u - v) >= -1e-6
      }
      assert.ok(found, `missing polar point ${x},${y}`)
    }
  for (const { id } of projections) {
    assert.ok(projectAtlasPositions(mesh.positions, id).every(Number.isFinite))
    assert.ok(mesh.sourceIds.every((source) => source < g.count))
  }
})
