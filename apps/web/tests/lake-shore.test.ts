import { test } from 'node:test'
import assert from 'node:assert/strict'
import { lakeShoreDepths } from '../src/renderer/lakeShore.ts'

test('lake surface clips against the actual dry bank instead of filling its triangle', () => {
  const height = new Float32Array([1, 5, 5])
  const depth = new Float32Array([2, 0, 0]) // A real surface at 3 m.
  const signed = lakeShoreDepths(height, depth, new Uint32Array([0, 1, 2]))
  assert.deepEqual(Array.from(signed), [2, -2, -2])
  assert.equal((signed[0] + signed[1]) / 2, 0) // Shore meets terrain at 3 m.
  assert.deepEqual(Array.from(depth), [2, 0, 0])
})
test('no lakes or missing legacy depths cannot create water', () => {
  const height = new Float32Array([1, 3, 5])
  const faces = new Uint32Array([0, 1, 2])
  assert.ok(lakeShoreDepths(height, undefined, faces).every((v) => v === 0))
  assert.ok(lakeShoreDepths(height, new Float32Array(3), faces).every((v) => v === 0))
})
