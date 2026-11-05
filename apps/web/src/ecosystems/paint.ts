import palette from '../../../../data/ecosystems/paint.json' with { type: 'json' }
import { ecosystemBiomes, ecosystemGroups } from './data.ts'

/** Art direction only; categorical types and physical surface fields stay intact. */
export function ecosystemPaint(biome: number, group: number, variant: number): number[] {
  const b = ecosystemBiomes.get(biome),
    g = ecosystemGroups.get(group)
  const groups = palette.groups as unknown as Record<string, [number[], number]>,
    biomes = palette.biomes as unknown as Record<string, [number[], number]>
  const entry =
    (g?.biome_id === biome ? groups[g.code] : undefined) || (b ? biomes[b.code] : undefined)
  const [base, material] = entry || [[143, 150, 123], 0]
  const wet = variant & 3,
    relief = (variant >> 2) & 3,
    soil = (variant >> 4) & 3
  const rgb = base.map(
    (c, i) =>
      c +
      [(1.5 - wet) * 10, (wet - 1.5) * 7, (wet - 1.5) * 4][i] +
      (soil === 2 ? [-5, -3, -5][i] : 0) +
      (relief > 1 ? [3, 0, 2][i] : 0),
  )
  return [...rgb.map((c) => Math.min(1, Math.max(0, c / 255))), material]
}
