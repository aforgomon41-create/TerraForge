import { test } from 'node:test'
import assert from 'node:assert/strict'
import { decodeGeography, featureMask, featuresAt } from '../src/geography/catalog.ts'

function packet(corrupt = false) {
  const features = ['continent', 'region', 'site'].map((level, i) => ({
    id: i + 1,
    parent: i || null,
    level,
    kind: ['island', 'drainage_region', 'peak'][i],
    name: `地理对象${i}`,
    anchor_cell: 1,
    lat: 0,
    lon: 179.9,
    area_m2: 1000,
    cell_count: 2,
    radius_m: 50,
    partial: false,
    elevation_m: 500,
    prominence_m: 100,
    drainage_area_m2: null,
  }))
  const json = new TextEncoder().encode(
    JSON.stringify({ version: 1, cell_count: 3, resolution_m: 50, features, omitted_sites: 0 }),
  )
  const buffer = new ArrayBuffer(4 + json.length + 36),
    view = new DataView(buffer)
  view.setUint32(0, json.length, true)
  new Uint8Array(buffer, 4, json.length).set(json)
  for (const [i, id] of [0, 1, 1, 0, 2, 2, 0, 3, 0].entries())
    view.setUint32(4 + json.length + 4 * i, corrupt && i === 4 ? 100 : id, true)
  return buffer
}
test('unaligned geographic payload preserves ocean, parents and exact membership', () => {
  const g = decodeGeography(packet(), 3)
  assert.deepEqual(featuresAt(g, 0), [])
  assert.deepEqual(
    featuresAt(g, 1).map((f) => f.id),
    [1, 2, 3],
  )
  assert.deepEqual(
    featuresAt(g, 2).map((f) => f.id),
    [1, 2],
  )
  assert.deepEqual(Array.from(featureMask(g, g.features[0])), [0, 1, 1])
  assert.deepEqual(Array.from(featureMask(g, g.features[2])), [0, 1, 0])
})
test('bad dimensions and out-of-catalogue labels are rejected before rendering', () => {
  assert.throws(() => decodeGeography(packet(), 4), /尺寸/)
  assert.throws(() => decodeGeography(packet(true), 3), /编号/)
  assert.throws(() => decodeGeography(packet().slice(0, -1), 3), /尺寸/)
})
