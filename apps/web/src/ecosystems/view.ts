import {
  ecosystemBiomes,
  ecosystemGroups,
  ecosystemColor,
  environmentName,
  identificationName,
  type EcosystemData,
} from './data.ts'

export const ecosystemModes = [
  { id: 0, name: '大类' },
  { id: 1, name: '细类' },
  { id: 2, name: '地方变体' },
] as const
export const supportLabels: Record<string, string> = {
  environment_estimate: '可按环境估计',
  additional_process: '需补充过程',
  subsurface: '需地下系统',
  human_activity: '需人类活动',
}
export interface EcosystemLegendRow {
  id: number
  name: string
  code: string
  environment?: string
  count: number
  percent: number
  color: string
  pending: boolean
}

/** Percentages describe sampling cells, not physical area or overlapping totals. */
export function ecosystemDistribution(
  data: EcosystemData,
  mode: number,
  habitats = false,
): EcosystemLegendRow[] {
  const n = data.biome.length,
    counts = new Map<number, { b: number; g: number; v: number; count: number }>()
  for (let i = 0; i < n; i++) {
    const b = (habitats ? data.habitatBiome : data.biome)[i]
    if (habitats && !b) continue
    const g = mode === 0 ? 0 : (habitats ? data.habitatGroup : data.group)[i],
      v = mode === 2 ? data.variant[i] : 0,
      key = (b * 512 + g) * 64 + v
    const row = counts.get(key)
    if (row) row.count++
    else counts.set(key, { b, g, v, count: 1 })
  }
  return [...counts]
    .map(([id, r]) => {
      const b = ecosystemBiomes.get(r.b),
        g = ecosystemGroups.get(r.g),
        pending = mode !== 0 && !g
      return {
        id,
        name: g?.name_zh || (b?.name_zh || '未识别大类') + (pending ? ' · 细类待定' : ''),
        code: g?.code || b?.code || '—',
        environment: mode === 2 ? environmentName(r.v, r.b) : undefined,
        count: r.count,
        percent: Number(((r.count / n) * 100).toPrecision(4)),
        color: 'rgb(' + ecosystemColor(r.b, r.g, r.v, mode).join(',') + ')',
        pending,
      }
    })
    .sort((a, b) => b.count - a.count || a.id - b.id)
}
export function ecosystemAt(data: EcosystemData | null | undefined, index: number | undefined) {
  if (!data || index === undefined || index < 0 || index >= data.biome.length) return null
  const b = ecosystemBiomes.get(data.biome[index]),
    g = ecosystemGroups.get(data.group[index]),
    h = ecosystemBiomes.get(data.habitatBiome[index]),
    hg = ecosystemGroups.get(data.habitatGroup[index]),
    evidence = data.evidence[index]
  return {
    biome: b?.name_zh || '未识别大类',
    biome_code: b?.code || null,
    group: g?.name_zh || '细类待定',
    group_code: g?.code || null,
    environment: environmentName(data.variant[index], data.biome[index]),
    identification: identificationName(data.group[index], evidence),
    habitat: h
      ? { biome: h.name_zh, group: hg?.name_zh || '细类待定', code: hg?.code || h.code }
      : null,
    basis: [
      [1, '月温度、雨季与干湿程度'],
      [2, '高程、坡度与起伏'],
      [4, '实际湖面或液态支持河流'],
      [8, '保存的冰雪状态'],
      [16, '土壤背景估计'],
      [32, '邻海位置'],
    ]
      .filter(([bit]) => evidence & (bit as number))
      .map(([, label]) => label as string),
  }
}
export function ecosystemSummary(data: EcosystemData) {
  let resolved = 0
  for (const g of data.group) if (g) resolved++
  return {
    version: data.version,
    basis: '采样点占比；不是面积加权统计。局部生境可以与背景重叠，不能相加。',
    catalogue: { biomes: ecosystemBiomes.size, groups: ecosystemGroups.size },
    resolved_ground_cells: resolved,
    pending_ground_cells: data.biome.length - resolved,
    biomes: ecosystemDistribution(data, 0),
    groups: ecosystemDistribution(data, 1),
    habitats: ecosystemDistribution(data, 1, true),
    notice: '只识别环境支持的类型；地方变体是项目环境描述，不是新增IUCN细类。',
  }
}

/** RGB values are classified before interpolation, including projection seams. */
export function ecosystemAttributes(
  data: EcosystemData | undefined,
  count: number,
  sourceIds?: Uint32Array,
  mode = 1,
) {
  if (
    ![0, 1, 2].includes(mode) ||
    (data &&
      [
        data.biome,
        data.group,
        data.habitatBiome,
        data.habitatGroup,
        data.variant,
        data.evidence,
      ].some((values) => values.length !== count))
  )
    throw Error('生态绘制资料尺寸或模式无效')
  const n = sourceIds?.length || count,
    colors = new Float32Array(n * 3),
    paint = new Float32Array(n * 3),
    cache = new Map<number, number[]>()
  if (!data) return { colors, paint }
  const rgb = (b: number, g: number, v: number, m: number) => {
    const key = m * 1_000_000 + (b * 512 + (m ? g : 0)) * 64 + (m === 2 ? v : 0)
    let value = cache.get(key)
    if (!value) {
      value = ecosystemColor(b, g, v, m).map((c) => c / 255)
      cache.set(key, value)
    }
    return value
  }
  for (let i = 0; i < n; i++) {
    const id = sourceIds?.[i] ?? i
    if (id >= count) throw Error('生态投影编号越界')
    const b = data.biome[id],
      g = data.group[id],
      v = data.variant[id]
    colors.set(rgb(b, g, v, mode), i * 3)
    paint.set(rgb(b, g, v, 2), i * 3)
  }
  return { colors, paint }
}
