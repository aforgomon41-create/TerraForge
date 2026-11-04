/** Shared taxonomy and portable ecosystem packet; independent of UI layout. */
import catalogue from '../../../../data/ecosystems/catalogue.json' with { type: 'json' }
export const ecosystemCatalogue = catalogue
export const ecosystemBiomes = new Map(catalogue.biomes.map((b) => [b.id, b]))
export const ecosystemGroups = new Map(catalogue.groups.map((g) => [g.id, g]))
export const ecosystemChannels = [
  'ecosystem_biome',
  'ecosystem_group',
  'ecosystem_habitat_biome',
  'ecosystem_habitat_group',
  'ecosystem_variant',
  'ecosystem_evidence',
] as const
export interface EcosystemData {
  available: true
  version: string
  biome: Uint8Array
  group: Uint16Array
  habitatBiome: Uint8Array
  habitatGroup: Uint16Array
  variant: Uint8Array
  evidence: Uint16Array
}
export type EcosystemResponse = EcosystemData | { available: false; reason: string }
export function decodeEcosystems(buffer: ArrayBuffer, count: number): EcosystemData {
  if (buffer.byteLength < 4 || !Number.isInteger(count) || count < 1 || count > 4_000_000)
    throw Error('生态资料尺寸无效')
  const view = new DataView(buffer),
    size = view.getUint32(0, true)
  if (size > 65536 || size + 4 > buffer.byteLength || buffer.byteLength !== 4 + size + count * 24)
    throw Error('生态资料长度不匹配')
  const header = JSON.parse(new TextDecoder().decode(new Uint8Array(buffer, 4, size)))
  if (
    !header.available ||
    header.version !== catalogue.version ||
    header.count !== count ||
    JSON.stringify(header.channels) !== JSON.stringify(ecosystemChannels)
  )
    throw Error('生态资料版本不匹配')
  const columns = Array.from({ length: 6 }, (_, c) => {
    const values = new Uint16Array(count)
    for (let i = 0; i < count; i++) {
      const value = view.getFloat32(4 + size + (c * count + i) * 4, true)
      if (!Number.isInteger(value) || value < 0 || value > 65535) throw Error('生态类别编号无效')
      values[i] = value
    }
    return values
  })
  for (let i = 0; i < count; i++) {
    for (const [parent, child] of [
      [0, 1],
      [2, 3],
    ]) {
      const b = columns[parent][i],
        g = columns[child][i]
      if (
        (b !== 0 && !ecosystemBiomes.has(b)) ||
        (g !== 0 && ecosystemGroups.get(g)?.biome_id !== b)
      )
        throw Error('生态分类层级不一致')
    }
    if (columns[4][i] > 63 || (columns[5][i] & ~831) !== 0) throw Error('生态环境编号无效')
  }
  return {
    available: true,
    version: header.version,
    biome: Uint8Array.from(columns[0]),
    group: columns[1],
    habitatBiome: Uint8Array.from(columns[2]),
    habitatGroup: columns[3],
    variant: Uint8Array.from(columns[4]),
    evidence: columns[5],
  }
}
export function environmentName(value: number, biome: number): string {
  const relief = ['缓坡', '丘陵', '山地', '陡坡'][(value >> 2) & 3]
  if ([1, 2, 4, 5, 6, 7].includes(biome))
    return `${biome === 5 || biome === 7 ? '海底' : '水域'} · ${relief} · 底质待定`
  return [
    ['干旱', '季节干湿', '湿润', '积水'][value & 3],
    relief,
    ['土壤未细分', '冲积土估计', '有机土估计', '薄层岩质土估计'][(value >> 4) & 3],
  ].join(' · ')
}
export function identificationName(group: number, evidence: number): string {
  return group === 0
    ? '细类待定'
    : evidence & 512 && group === 369
      ? '依据保存的冰雪状态'
      : '环境估计'
}
export function ecosystemColor(
  biome: number,
  group: number,
  variant: number,
  mode: number,
): number[] {
  const hex = ecosystemBiomes.get(biome)?.color || '#90938b',
    rgb = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16))
  if (mode > 0 && group > 0) {
    const shift = (group % 16) - 3
    rgb[0] += shift * 2.5
    rgb[1] += shift * 1.5
    rgb[2] -= shift
  }
  if (mode === 2) {
    const wet = variant & 3,
      relief = (variant >> 2) & 3
    rgb[0] += (1.5 - wet) * 8 + relief * 2
    rgb[1] += (wet - 1.5) * 5
    rgb[2] += (wet - 1.5) * 4 + relief * 3
  }
  return rgb.map((c) => Math.trunc(Math.min(255, Math.max(0, c))))
}
