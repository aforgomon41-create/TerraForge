import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  direction,
  latLon,
  patchDirection,
  patchProject,
  nearestCell,
  worldMapPoint,
  worldMapDirection,
} from '../src/renderer/geometry.ts'
test('geographic round trip at dateline and near poles', () => {
  for (const lat of [-89, -45, 0, 45, 89])
    for (const lon of [-179, 0, 179]) {
      const p = latLon(direction(lat, lon))
      assert.ok(Math.abs(p.lat - lat) < 1e-9)
      assert.ok(Math.abs(p.lon - lon) < 1e-9)
    }
})
test('local east increases longitude; local north increases latitude', () => {
  const east = latLon(patchDirection(30, 100, 1000, 0, 6371000))
  const north = latLon(patchDirection(30, 100, 0, 1000, 6371000))
  assert.ok(east.lon > 100)
  assert.ok(north.lat > 30)
})
test('cell picking uses received geometry rather than regenerated coordinates', () => {
  const points = new Float32Array([1, 0, 0, 0, 0, 1, -1, 0, 0])
  assert.equal(nearestCell(points, [0, 0, 1], true), 1)
  assert.equal(nearestCell(new Float32Array([10, 20, 0, 20, 30, 0]), [19, 28, 0], false), 1)
})

test('local projection round-trips at the exact poles and dateline', () => {
  for (const [lat, lon] of [
    [90, 120],
    [-90, -120],
    [89.99, 179.99],
    [0, -179.99],
  ]) {
    const d = patchDirection(lat, lon, 1234, -5678, 3000000),
      p = patchProject(lat, lon, d, 3000000)!
    assert.ok(Math.abs(p[0] - 1234) < 1e-6)
    assert.ok(Math.abs(p[1] + 5678) < 1e-6)
  }
})

test('world atlas and globe select identical geographic cells, including the seam', () => {
  const points = new Float32Array([1, 0, 0, 0, 1, 0, -1, 0, 0])
  for (const [lat, lon] of [
    [0, 0],
    [60, 179.99],
    [-60, -179.99],
    [90, 0],
    [-90, 0],
  ]) {
    const d = direction(lat, lon),
      p = worldMapPoint(d),
      restored = worldMapDirection(p)
    assert.ok(Math.hypot(...d.map((v, i) => v - restored[i])) < 1e-10)
    assert.equal(nearestCell(points, d, true), nearestCell(points, restored, true))
  }
  assert.deepEqual(worldMapPoint(direction(0, 180)), [2, 0, 0])
})
