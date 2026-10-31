import { test } from 'node:test'
import assert from 'node:assert/strict'
import { gzipSync } from 'node:zlib'
import { decodeFrame, latestFrameQueue, type FrameEvent } from '../src/api/frame.ts'
import { frameTextureChannels } from '../src/renderer/frameTexture.ts'
const raw = Uint8Array.from({ length: 5 * 8 * 4 }, (_, i) => i % 256)
const makeFrame = (data: Uint8Array, fmt = 'gzip-u16le+u8x3'): FrameEvent => ({
  job_id: 'job-a',
  tick: 1,
  w: 8,
  h: 4,
  fmt,
  data: Buffer.from(data).toString('base64'),
  stats: {
    min_elevation: -6000,
    max_elevation: 8000,
    land_fraction: 0.3,
    min_temperature: 0,
    max_temperature: 20,
  },
})

test('preview textures preserve metre-scale changes across elevation byte boundaries', () => {
  const heights = [0, 254, 255, 256, 257, 511, 512, 65535]
  const data = Uint8Array.from(heights.flatMap((h) => [h & 255, h >> 8, 123, 42, 99]))
  const { rgba, flow } = frameTextureChannels(data, heights.length, 1)
  const restored = heights.map((_, i) => rgba[i * 4] + 256 * rgba[i * 4 + 1])
  assert.deepEqual(restored, heights)
  assert.equal((restored[2] + restored[3]) / 2, 255.5) // Bilinear interpolation stays continuous.
  assert.ok(flow.every((v) => v === 99))
  assert.ok(heights.every((_, i) => rgba[i * 4 + 2] === 123 && rgba[i * 4 + 3] === 42))
})
test('raw and compressed frame formats produce identical bounded channel bytes', async () => {
  const a = await decodeFrame(makeFrame(raw, 'raw-u16le+u8x3')),
    b = await decodeFrame(makeFrame(gzipSync(raw)))
  assert.deepEqual(a.pixels, raw)
  assert.deepEqual(b.pixels, raw)
  assert.equal('data' in b, false)
})
test('invalid frame dimensions, truncation and decompression expansion are rejected', async () => {
  await assert.rejects(decodeFrame({ ...makeFrame(raw), w: 4096 }))
  await assert.rejects(decodeFrame(makeFrame(raw.slice(0, -1), 'raw-u16le+u8x3')))
  await assert.rejects(decodeFrame(makeFrame(gzipSync(new Uint8Array(raw.length + 1)))))
  await assert.rejects(decodeFrame(makeFrame(raw, 'unknown')))
})
test('slow decoding keeps only the latest pending frame and stops late delivery', async () => {
  const waiting: ((n: number) => void)[] = [],
    delivered: number[] = []
  const queue = latestFrameQueue<number, number>(
    (n) => new Promise((resolve) => waiting.push(() => resolve(n))),
    (n) => delivered.push(n),
  )
  queue.push(1)
  queue.push(2)
  queue.push(3)
  assert.equal(waiting.length, 1)
  waiting[0](1)
  await new Promise((resolve) => setTimeout(resolve, 0))
  assert.equal(waiting.length, 2)
  waiting[1](3)
  await new Promise((resolve) => setTimeout(resolve, 0))
  assert.deepEqual(delivered, [1, 3])
  queue.push(4)
  queue.stop()
  waiting[2](4)
  await new Promise((resolve) => setTimeout(resolve, 0))
  assert.deepEqual(delivered, [1, 3])
})
