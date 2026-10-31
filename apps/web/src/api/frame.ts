import type { GridDescriptor } from './client'
export interface FrameStats {
  land_fraction: number
  max_elevation: number
  min_elevation: number
  max_temperature: number
  min_temperature: number
}
export interface FrameMeta {
  stage: string
  detail: string
  progress: number
  step: number
  elapsed_myr: number | null
  year: number | null
  month: number | null
  channels: number
  available_channels: number
}
export interface FrameEvent {
  job_id: string
  tick: number
  w: number
  h: number
  fmt: string
  stats: FrameStats
  data: string
  grid?: GridDescriptor
  meta?: FrameMeta
}
export type DecodedFrameEvent = Omit<FrameEvent, 'data'> & { pixels: Uint8Array }

export async function decodeFrame(frame: FrameEvent): Promise<DecodedFrameEvent> {
  const { w, h, data, fmt, stats, ...rest } = frame
  if (!Number.isInteger(w) || !Number.isInteger(h) || w < 1 || h < 1 || w > 512 || h > 512)
    throw new Error('预览帧尺寸无效')
  if (
    !Number.isFinite(stats.min_elevation) ||
    !Number.isFinite(stats.max_elevation) ||
    stats.min_elevation > stats.max_elevation
  )
    throw new Error('预览帧高程范围无效')
  const expected = w * h * 5
  if (data.length > 2_000_000) throw new Error('预览帧过大')
  const pixels = Uint8Array.from(atob(data), (c) => c.charCodeAt(0))
  if (fmt === 'raw-u16le+u8x3') {
    if (pixels.length !== expected) throw new Error('预览帧长度无效')
    return { ...rest, w, h, fmt, stats, pixels }
  }
  if (fmt !== 'gzip-u16le+u8x3') throw new Error('未知的预览帧格式')
  const reader = new Blob([pixels.buffer])
    .stream()
    .pipeThrough(new DecompressionStream('gzip'))
    .getReader()
  const output = new Uint8Array(expected)
  let used = 0
  try {
    while (true) {
      const { value, done } = await reader.read()
      if (done) break
      if (used + value.length > expected) throw new Error('预览解压超出尺寸')
      output.set(value, used)
      used += value.length
    }
    if (used !== expected) throw new Error('预览帧长度无效')
    return { ...rest, w, h, fmt, stats, pixels: output }
  } finally {
    await reader.cancel().catch(() => {})
  }
}

/** One decode in flight and one latest pending frame; never build an animation backlog. */
export function latestFrameQueue<T, R>(
  decode: (value: T) => Promise<R>,
  deliver: (value: R) => void,
  onError: () => void = () => {},
) {
  let pending: { value: T } | null = null,
    busy = false,
    stopped = false
  async function pump() {
    if (busy || stopped) return
    busy = true
    try {
      while (pending && !stopped) {
        const next = pending.value
        pending = null
        try {
          const result = await decode(next)
          if (!stopped) deliver(result)
        } catch {
          if (!stopped) onError()
        }
      }
    } finally {
      busy = false
    }
  }
  return {
    push(value: T) {
      if (stopped) return
      pending = { value }
      void pump()
    },
    stop() {
      stopped = true
      pending = null
    },
  }
}
