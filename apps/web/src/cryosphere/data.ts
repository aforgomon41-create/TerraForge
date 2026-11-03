export interface IceData {
  available: true
  model: string
  month: number
  count: number
  landIce: Float32Array
  snow: Float32Array
  seaIce: Float32Array
  shelf: Float32Array
  permafrost: Float32Array
  activeLayer: Float32Array
  liquidWater: Float32Array
  lakeIce: Float32Array
}
export type IceResponse = IceData | { available: false; reason: string }
const channels = [
  'land_ice_m',
  'snow_cover',
  'sea_ice_cover',
  'ice_shelf_m',
  'permafrost',
  'active_layer_m',
  'liquid_water_mm',
  'lake_ice_cover',
]
export function decodeIce(bytes: ArrayBuffer, count: number): IceData {
  if (bytes.byteLength < 4) throw new Error('冰雪资料长度无效')
  const length = new DataView(bytes).getUint32(0, true)
  if (length > 65536 || 4 + length > bytes.byteLength) throw new Error('冰雪资料头无效')
  const header = JSON.parse(new TextDecoder().decode(new Uint8Array(bytes, 4, length)))
  if (
    header.available !== true ||
    header.count !== count ||
    !Number.isInteger(header.month) ||
    header.month < 0 ||
    header.month > 12 ||
    JSON.stringify(header.channels) !== JSON.stringify(channels) ||
    bytes.byteLength !== 4 + length + count * 32
  )
    throw new Error('冰雪资料与地图尺寸不一致')
  const data = channels.map(
    (_, i) =>
      new Float32Array(bytes.slice(4 + length + i * count * 4, 4 + length + (i + 1) * count * 4)),
  )
  if (
    data.some((v) => v.some((x) => !Number.isFinite(x) || x < 0)) ||
    [1, 2, 4, 7].some((i) => data[i].some((x) => x > 1))
  )
    throw new Error('冰雪数值无效')
  return {
    available: true,
    model: header.model,
    month: header.month,
    count,
    landIce: data[0],
    snow: data[1],
    seaIce: data[2],
    shelf: data[3],
    permafrost: data[4],
    activeLayer: data[5],
    liquidWater: data[6],
    lakeIce: data[7],
  }
}

/** Pack appearance attributes; physical thickness and runoff remain unchanged. */
export function iceAttributes(ice: IceData | undefined, count: number, sourceIds?: Uint32Array) {
  const size = sourceIds?.length || count,
    cover = new Float32Array(size * 4),
    ground = new Float32Array(size * 2)
  if (!ice) return { cover, ground }
  if (ice.count !== count) throw new Error('冰雪字段长度与地表不一致')
  for (let j = 0; j < size; j++) {
    const i = sourceIds?.[j] ?? j
    cover[j * 4] = Math.min(1, ice.landIce[i])
    cover[j * 4 + 1] = ice.seaIce[i]
    cover[j * 4 + 2] = 1 - Math.exp(-ice.shelf[i] / 30)
    cover[j * 4 + 3] = ice.snow[i]
    ground[j * 2] = ice.permafrost[i]
    ground[j * 2 + 1] = ice.lakeIce[i]
  }
  return { cover, ground }
}
