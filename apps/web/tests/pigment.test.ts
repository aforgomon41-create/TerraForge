import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createPigmentTexture } from '../src/renderer/pigmentTexture.ts'

function harness() {
  let active = 101
  const bindings = new Map<number, unknown>([
    [100, 'preview'],
    [101, 'flow'],
  ])
  const uploads: unknown[][] = []
  let deleted = 0
  const image = { onload: null, onerror: null, src: '' } as unknown as HTMLImageElement
  const texture = {}
  const gl = {
    ACTIVE_TEXTURE: 1,
    TEXTURE2: 102,
    TEXTURE_2D: 2,
    TEXTURE_MIN_FILTER: 3,
    TEXTURE_MAG_FILTER: 4,
    TEXTURE_WRAP_S: 5,
    TEXTURE_WRAP_T: 6,
    LINEAR_MIPMAP_LINEAR: 7,
    LINEAR: 8,
    MIRRORED_REPEAT: 9,
    RGBA: 10,
    UNSIGNED_BYTE: 11,
    createTexture: () => texture,
    activeTexture: (unit: number) => (active = unit),
    getParameter: () => active,
    bindTexture: (_target: number, value: unknown) => bindings.set(active, value),
    texParameteri: () => {},
    texImage2D: (...args: unknown[]) => uploads.push(args),
    generateMipmap: () => {},
    deleteTexture: () => deleted++,
  } as unknown as WebGL2RenderingContext
  const material = createPigmentTexture(gl, '/atlas-pigment.png', () => image)
  return { material, image, uploads, bindings, active: () => active, deleted: () => deleted }
}

test('asynchronous material loading preserves both live-preview texture units', () => {
  const h = harness()
  assert.equal(h.uploads.length, 1) // A complete neutral fallback exists before IO.
  assert.equal(h.image.src, '/atlas-pigment.png')
  h.image.onload!.call(h.image, new Event('load'))
  assert.equal(h.uploads.length, 2)
  assert.equal(h.bindings.get(100), 'preview')
  assert.equal(h.bindings.get(101), 'flow')
  assert.equal(h.bindings.get(102), h.material.texture)
  assert.equal(h.active(), 101)
  h.material.dispose()
})

test('failed optional material leaves a complete fallback and can be disposed', () => {
  const h = harness()
  h.image.onerror!.call(h.image, new Event('error'))
  assert.equal(h.uploads.length, 1)
  assert.equal(h.image.onload, null)
  h.material.dispose()
  h.material.dispose()
  assert.equal(h.deleted(), 1)
})

test('late image completion cannot resurrect a disposed GPU material', () => {
  const h = harness()
  const queuedLoad = h.image.onload!
  h.material.dispose()
  queuedLoad.call(h.image, new Event('load'))
  assert.equal(h.uploads.length, 1)
  assert.equal(h.deleted(), 1)
  assert.equal(h.image.onload, null)
})
