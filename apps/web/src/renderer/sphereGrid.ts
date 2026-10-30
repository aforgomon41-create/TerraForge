/** Lightweight globe used only while preview frames arrive. Final geometry comes from the server. */
export interface SphereGrid {
  indices: Uint32Array
  count: number
  /** 每格中心方向，xyz 交错 */
  centers: Float32Array
  /** 12 个五边形的索引 */
  pentagons: number[]
}

const PHI = (1 + Math.sqrt(5)) / 2

const ICO_VERTS: [number, number, number][] = [
  [-1, PHI, 0],
  [1, PHI, 0],
  [-1, -PHI, 0],
  [1, -PHI, 0],
  [0, -1, PHI],
  [0, 1, PHI],
  [0, -1, -PHI],
  [0, 1, -PHI],
  [PHI, 0, -1],
  [PHI, 0, 1],
  [-PHI, 0, -1],
  [-PHI, 0, 1],
].map(([x, y, z]) => {
  const l = Math.hypot(x, y, z)
  return [x / l, y / l, z / l] as [number, number, number]
})

const FACES: [number, number, number][] = [
  [0, 11, 5],
  [0, 5, 1],
  [0, 1, 7],
  [0, 7, 10],
  [0, 10, 11],
  [1, 5, 9],
  [5, 11, 4],
  [11, 10, 2],
  [10, 7, 6],
  [7, 1, 8],
  [3, 9, 4],
  [3, 4, 2],
  [3, 2, 6],
  [3, 6, 8],
  [3, 8, 9],
  [4, 9, 5],
  [2, 4, 11],
  [6, 2, 10],
  [8, 6, 7],
  [9, 8, 1],
]

/** 三角面内 (i,j) 点的线性索引。 */
function triIndex(freq: number, i: number, j: number): number {
  const before = i * (freq + 1) - Math.floor((i * (i - 1)) / 2)
  return before + j
}

/**
 * 构建网格。
 *
 * 拓扑键去重必须和 Rust 版一致：顶点、棱点、面内点分别用不同的键。
 */
export function buildSphereGrid(freq: number): SphereGrid {
  const keyToId = new Map<string, number>()
  const centers: number[] = []

  const perFace = ((freq + 1) * (freq + 2)) / 2
  const facePoints: number[][] = []

  for (let f = 0; f < 20; f++) {
    const vids = FACES[f]
    const a = ICO_VERTS[vids[0]]
    const b = ICO_VERTS[vids[1]]
    const c = ICO_VERTS[vids[2]]
    const ids = new Array(perFace).fill(0)

    for (let i = 0; i <= freq; i++) {
      for (let j = 0; j <= freq - i; j++) {
        const k = freq - i - j
        const idx = triIndex(freq, i, j)

        // 拓扑键：与 Rust 版 face_point_key 逻辑完全一致
        let key: string
        let pos: [number, number, number]

        if (k === freq) {
          key = 'v' + vids[0]
          pos = a
        } else if (i === freq) {
          key = 'v' + vids[1]
          pos = b
        } else if (j === freq) {
          key = 'v' + vids[2]
          pos = c
        } else if (j === 0) {
          // a-b 棱
          const [u, v, t] = vids[0] < vids[1] ? [vids[0], vids[1], i] : [vids[1], vids[0], freq - i]
          key = `e${u}_${v}_${t}`
          pos = slerp(a, b, i / freq)
        } else if (k === 0) {
          // b-c 棱
          const [u, v, t] = vids[1] < vids[2] ? [vids[1], vids[2], j] : [vids[2], vids[1], freq - j]
          key = `e${u}_${v}_${t}`
          pos = slerp(b, c, j / freq)
        } else if (i === 0) {
          // a-c 棱
          const [u, v, t] = vids[0] < vids[2] ? [vids[0], vids[2], j] : [vids[2], vids[0], freq - j]
          key = `e${u}_${v}_${t}`
          pos = slerp(a, c, j / freq)
        } else {
          key = `i${f}_${i}_${j}`
          // 面内点：线性插值后归一化
          const wa = k / freq
          const wb = i / freq
          const wc = j / freq
          pos = norm([
            a[0] * wa + b[0] * wb + c[0] * wc,
            a[1] * wa + b[1] * wb + c[1] * wc,
            a[2] * wa + b[2] * wb + c[2] * wc,
          ])
        }

        let id = keyToId.get(key)
        if (id === undefined) {
          id = centers.length / 3
          keyToId.set(key, id)
          centers.push(pos[0], pos[1], pos[2])
        }
        ids[idx] = id
      }
    }
    facePoints.push(ids)
  }

  const triangles: number[] = []
  for (const ids of facePoints) {
    const at = (i: number, j: number) => ids[triIndex(freq, i, j)]
    for (let i = 0; i < freq; i++) {
      for (let j = 0; j < freq - i; j++) {
        triangles.push(at(i, j), at(i + 1, j), at(i, j + 1))
        if (j < freq - i - 1) triangles.push(at(i + 1, j), at(i + 1, j + 1), at(i, j + 1))
      }
    }
  }
  // 五边形：二十面体顶点对应的格
  const pentagons: number[] = []
  for (let v = 0; v < 12; v++) {
    const id = keyToId.get('v' + v)
    if (id !== undefined) pentagons.push(id)
  }

  return {
    indices: new Uint32Array(triangles),
    count: centers.length / 3,
    centers: new Float32Array(centers),
    pentagons,
  }
}

function slerp(
  a: [number, number, number],
  b: [number, number, number],
  t: number,
): [number, number, number] {
  const d = Math.max(-1, Math.min(1, a[0] * b[0] + a[1] * b[1] + a[2] * b[2]))
  const theta = Math.acos(d)
  if (theta < 1e-9) return a
  const s = Math.sin(theta)
  const w0 = Math.sin((1 - t) * theta) / s
  const w1 = Math.sin(t * theta) / s
  return [a[0] * w0 + b[0] * w1, a[1] * w0 + b[1] * w1, a[2] * w0 + b[2] * w1]
}

function norm(v: [number, number, number]): [number, number, number] {
  const l = Math.hypot(v[0], v[1], v[2]) || 1
  return [v[0] / l, v[1] / l, v[2] / l]
}
