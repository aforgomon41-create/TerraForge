import { test } from 'node:test'
import assert from 'node:assert/strict'
import { heightGradients } from '../src/renderer/relief.ts'
import { buildSphereGrid } from '../src/renderer/sphereGrid.ts'

test('a linear slope keeps its exact gradient across shared triangle edges', () => {
  const points = new Float32Array([0, 0, 0, 1, 0, 0, 0, 1, 0, 1, 1, 0])
  const triangles = new Uint32Array([0, 1, 2, 2, 1, 3])
  const heights = new Float32Array([10, 12, 13, 15]) // h = 10 + 2x + 3y
  const gradient = heightGradients(points, triangles, heights)
  for (let i = 0; i < 4; i++)
    assert.deepEqual(Array.from(gradient.slice(i * 3, i * 3 + 3)), [2, 3, 0])
  assert.deepEqual(heights, new Float32Array([10, 12, 13, 15]))
  // Reversing face winding does not reverse the lighting slope.
  assert.deepEqual(gradient, heightGradients(points, new Uint32Array([2, 1, 0, 3, 1, 2]), heights))
})

test('a constant-height globe has no invented slopes, including poles and dateline', () => {
  const mesh = buildSphereGrid(8)
  const result = heightGradients(mesh.centers, mesh.indices, new Float32Array(mesh.count).fill(500))
  assert.ok(result.every((v) => Number.isFinite(v) && v === 0))
})

test('degenerate faces and unused vertices remain finite', () => {
  const result = heightGradients(
    new Float32Array(12),
    new Uint32Array([0, 1, 2]),
    new Float32Array(4),
  )
  assert.ok(result.every((v) => Number.isFinite(v) && v === 0))
})
