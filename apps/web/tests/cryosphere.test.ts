import test from 'node:test'
import assert from 'node:assert/strict'
import { decodeIce, iceAttributes, type IceData } from '../src/cryosphere/data.ts'
function packet() {
  const header = new TextEncoder().encode(
    JSON.stringify({
      available: true,
      count: 2,
      month: 1,
      model: 'fixture',
      channels: [
        'land_ice_m',
        'snow_cover',
        'sea_ice_cover',
        'ice_shelf_m',
        'permafrost',
        'active_layer_m',
        'liquid_water_mm',
        'lake_ice_cover',
      ],
    }),
  )
  const bytes = new ArrayBuffer(4 + header.length + 2 * 32),
    view = new DataView(bytes)
  view.setUint32(0, header.length, true)
  new Uint8Array(bytes, 4, header.length).set(header)
  view.setFloat32(4 + header.length, 1000, true)
  view.setFloat32(4 + header.length + 2 * 2 * 4 + 4, 0.7, true)
  return bytes
}
test('unaligned binary ice data preserves physical values and rejects damaged bounds', () => {
  const bytes = packet(),
    ice = decodeIce(bytes, 2)
  assert.equal(ice.landIce[0], 1000)
  assert.ok(Math.abs(ice.seaIce[1] - 0.7) < 1e-6)
  assert.throws(() => decodeIce(bytes, 3))
  assert.throws(() => decodeIce(bytes.slice(0, -1), 2))
  const bad = packet(),
    v = new DataView(bad),
    start = 4 + v.getUint32(0, true)
  v.setFloat32(start + 2 * 2 * 4, 2, true)
  assert.throws(() => decodeIce(bad, 2))
})

test('thin ice has a gradual appearance without changing physical thickness', () => {
  const ice = decodeIce(packet(), 2)
  ice.landIce[0] = 1
  ice.landIce[1] = 10
  const a = iceAttributes(ice, 2)
  assert.ok(a.cover[0] > 0.25 && a.cover[0] < 0.3)
  assert.ok(a.cover[4] > 0.95 && a.cover[4] < 1)
  assert.deepEqual(ice.landIce, new Float32Array([1, 10]))
})
test('projection seam remapping keeps ice, water and ground categories independent', () => {
  const ice: IceData = decodeIce(packet(), 2),
    before = ice.landIce.slice()
  const attributes = iceAttributes(ice, 2, new Uint32Array([1, 0, 1]))
  assert.equal(attributes.cover[0], 0)
  assert.ok(attributes.cover[1] > 0.69)
  assert.equal(attributes.cover[4], 1)
  assert.equal(attributes.cover[5], 0)
  assert.deepEqual(ice.landIce, before)
  assert.ok(iceAttributes(undefined, 2).cover.every((v) => v === 0))
})
