import { test } from 'node:test'
import assert from 'node:assert/strict'
import { partitionBoundaries } from '../src/geography/view.ts'

const points = new Float32Array([0, 0, 0, 2, 0, 0, 0, 2, 0])
const triangle = new Uint32Array([0, 1, 2])
test('one partition emits no edge; two categories meet at the dual edge midpoints', () => {
  assert.equal(
    partitionBoundaries(points, triangle, new Uint32Array([1, 1, 1]), false).lines.length,
    0,
  )
  const result = partitionBoundaries(points, triangle, new Uint32Array([1, 2, 1]), false)
  assert.deepEqual(Array.from(result.lines), [1, 0, 0, 1, 1, 0])
  assert.equal(result.truncated, false)
})
test('three categories meet at one junction, with no fabricated rectangle', () => {
  const result = partitionBoundaries(points, triangle, new Uint32Array([1, 2, 3]), false)
  assert.equal(result.lines.length, 18)
  for (let i = 0; i < 3; i++) {
    assert.ok(Math.abs(result.lines[i * 6 + 3] - 2 / 3) < 1e-6)
    assert.ok(Math.abs(result.lines[i * 6 + 4] - 2 / 3) < 1e-6)
  }
})
test('sphere borders retain radius and split correctly when projected across the dateline', async () => {
  const { direction } = await import('../src/renderer/geometry.ts')
  const { atlasLines } = await import('../src/renderer/surface.ts')
  const sphere = Float32Array.from(
    [direction(0, 179), direction(0, -179), direction(2, 179)].flat(),
  )
  const result = partitionBoundaries(sphere, triangle, new Uint32Array([1, 2, 1]), true)
  for (let i = 0; i < result.lines.length; i += 3)
    assert.ok(Math.abs(Math.hypot(...result.lines.slice(i, i + 3)) - 1.0005) < 1e-6)
  const atlas = atlasLines(result.lines)
  assert.ok(atlas.length >= 12)
  for (let i = 0; i < atlas.length; i += 6) assert.ok(Math.abs(atlas[i] - atlas[i + 3]) < 0.1)
})
test('empty site membership stays empty; oversized edge rendering has an explicit limit', () => {
  assert.equal(
    partitionBoundaries(points, triangle, new Uint32Array([0, 0, 0]), false).lines.length,
    0,
  )
  assert.equal(
    partitionBoundaries(points, triangle, new Uint32Array([1, 2, 3]), false, 1, 1).truncated,
    true,
  )
  assert.throws(
    () => partitionBoundaries(points, new Uint32Array([0, 1, 9]), new Uint32Array(3), false),
    /编号/,
  )
})
