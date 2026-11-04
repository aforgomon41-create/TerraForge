import test from 'node:test'
import assert from 'node:assert/strict'
import {
  decodeEcosystems,
  ecosystemCatalogue,
  ecosystemChannels,
  environmentName,
  identificationName,
} from '../src/ecosystems/data.ts'
function packet(values = [25, 403, 1, 19, 21, 275]) {
  const header = new TextEncoder().encode(
    JSON.stringify({
      available: true,
      version: ecosystemCatalogue.version,
      count: 1,
      channels: ecosystemChannels,
    }),
  )
  const buffer = new ArrayBuffer(4 + header.length + 24),
    view = new DataView(buffer)
  view.setUint32(0, header.length, true)
  new Uint8Array(buffer, 4, header.length).set(header)
  values.forEach((v, i) => view.setFloat32(4 + header.length + i * 4, v, true))
  return buffer
}
test('shared 25/110 hierarchy preserves u16 ids above 255 and separate river habitats', () => {
  assert.equal(ecosystemCatalogue.biomes.length, 25)
  assert.equal(ecosystemCatalogue.groups.length, 110)
  const e = decodeEcosystems(packet(), 1)
  assert.equal(e.group[0], 403)
  assert.equal(e.habitatGroup[0], 19)
  assert.equal(e.biome[0], 25)
  assert.throws(() => decodeEcosystems(packet([19, 403, 1, 19, 21, 275]), 1))
  assert.throws(() => decodeEcosystems(packet([25, 403.5, 1, 19, 21, 275]), 1))
  assert.throws(() => decodeEcosystems(packet().slice(0, -1), 1))
  assert.throws(() => decodeEcosystems(packet(), 2))
})
test('local variants and unknown classes never masquerade as formal fine types', () => {
  assert.equal(identificationName(0, 275), '细类待定')
  assert.equal(identificationName(369, 512), '依据保存的冰雪状态')
  assert.match(environmentName(0, 7), /海底/)
  assert.doesNotMatch(environmentName(0, 7), /干旱/)
  assert.match(environmentName(22, 19), /冲积土估计/)
})
