import type { Geography } from './catalog'

export type PartitionMode = 'landmass' | 'region' | 'site'
export const partitionModes: { id: PartitionMode; label: string; hint: string }[] = [
  {
    id: 'landmass',
    label: '大陆与岛屿',
    hint: '不同颜色代表不同陆块，边缘截断处仅代表当前可见部分。',
  },
  {
    id: 'region',
    label: '汇水区域',
    hint: '区域由地形和水流关系划分；点击查看所属陆块与自然地点。',
  },
  { id: 'site', label: '自然地点', hint: '只显示识别到的点位及湖盆采样范围，其余陆地留空。' },
]
export function partitionLabels(g: Geography, mode: PartitionMode): Uint32Array {
  return g[mode]
}

/** Categorical borders on the dual of the actual terrain triangles. A border
 * passes through an edge midpoint, never along an arbitrary bounding rectangle.
 * Three different labels meet at the triangle centre. Uniform triangles emit none.
 */
export function partitionBoundaries(
  centers: Float32Array,
  indices: Uint32Array,
  labels: Uint32Array,
  sphere: boolean,
  scale = 1,
  maxSegments = 500_000,
): { lines: Float32Array; truncated: boolean } {
  if (centers.length !== labels.length * 3 || indices.length % 3)
    throw new Error('分区边界与地图尺寸不一致')
  const out: number[] = []
  const point = (i: number) => [
    centers[3 * i] * scale,
    centers[3 * i + 1] * scale,
    centers[3 * i + 2] * scale,
  ]
  const middle = (a: number[], b: number[]) => a.map((v, i) => (v + b[i]) * 0.5)
  const emit = (a: number[], b: number[]) => {
    for (const p of [a, b]) {
      const factor = sphere ? 1.0005 / Math.hypot(...p) : 1
      out.push(p[0] * factor, p[1] * factor, p[2] * factor)
    }
  }
  for (let t = 0; t < indices.length; t += 3) {
    const a = indices[t],
      b = indices[t + 1],
      c = indices[t + 2]
    if (a >= labels.length || b >= labels.length || c >= labels.length)
      throw new Error('分区三角形编号无效')
    const x = labels[a],
      y = labels[b],
      z = labels[c]
    if (x === y && y === z) continue
    const needed = x !== y && y !== z && z !== x ? 3 : 1
    if (out.length / 6 + needed > maxSegments)
      return { lines: Float32Array.from(out), truncated: true }
    const pa = point(a),
      pb = point(b),
      pc = point(c)
    const edges: number[][] = []
    if (x !== y) edges.push(middle(pa, pb))
    if (y !== z) edges.push(middle(pb, pc))
    if (z !== x) edges.push(middle(pc, pa))
    if (edges.length === 2) emit(edges[0], edges[1])
    else {
      const junction = pa.map((v, i) => (v + pb[i] + pc[i]) / 3)
      for (const edge of edges) emit(edge, junction)
    }
  }
  return { lines: Float32Array.from(out), truncated: false }
}
